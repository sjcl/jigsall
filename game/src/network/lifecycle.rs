//! Bounded resource policy for establishment and join; origins are abuse keys,
//! never authenticated player identities. All clocks are supplied by the caller.
use super::transport::{DisconnectReason, Origin};
use std::{
    collections::{hash_map::RandomState, BTreeMap, BTreeSet, VecDeque},
    hash::{BuildHasher, Hash},
    time::{Duration, Instant},
};

pub const MAX_CONNECTIONS: usize = 64;
const _: () = assert!(crate::multiplayer::catch_up::MAX_PENDING_JOIN_SYNCS < MAX_CONNECTIONS);
pub const MAX_CONNECTING: usize = 16;
pub const MAX_PENDING_CONNECTIONS: usize = 32;
pub const MAX_PENDING_PER_ORIGIN: usize = 4;
pub const CONNECTING_TIMEOUT: Duration = Duration::from_secs(10);
pub const AUTHENTICATED_HANDOFF_TIMEOUT: Duration = Duration::from_secs(5);
pub const MAX_RELIABLE_QUEUE_BYTES: u64 = 512 * 1024;
// Leave room for a maximum-size Control record under the native 512 KiB limit.
pub const MAX_BULK_QUEUE_BYTES: u64 = 240 * 1024;
pub const BULK_FRAME_BYTES: u64 = 128 * 1024;
pub const BULK_BYTES_PER_SECOND: u64 = 4 * 1024 * 1024;
pub const MAX_ORIGINS: usize = 256;
pub const ORIGIN_TTL: Duration = Duration::from_secs(120);
pub const ORIGIN_COOLDOWN: Duration = Duration::from_secs(30);
const MAX_BACKOFF: Duration = Duration::from_secs(240);
const RETIRED_ORIGINS: usize = 512;
pub const CONNECTION_START_BURST: u64 = 8;
pub const CONNECTION_START_INTERVAL: Duration = Duration::from_secs(1);
pub const JOIN_ADMISSION_BURST: u64 = 12;
pub const JOIN_ADMISSION_INTERVAL: Duration = Duration::from_secs(2);
pub const ORIGIN_JOIN_BURST: u64 = 4;
pub const ORIGIN_JOIN_INTERVAL: Duration = Duration::from_secs(5);
pub const COOLDOWN_FAILURES: u8 = 3;

#[derive(Clone, Copy, Debug)]
pub struct SyncPolicy {
    pub control_response: Duration,
    pub bulk_stall: Duration,
    pub throughput_grace: Duration,
    pub minimum_bytes_per_second: u64,
    pub sync_hard_limit: Duration,
}
impl Default for SyncPolicy {
    fn default() -> Self {
        Self {
            control_response: Duration::from_secs(12),
            bulk_stall: Duration::from_secs(30),
            throughput_grace: Duration::from_secs(30),
            minimum_bytes_per_second: 128 * 1024,
            sync_hard_limit: Duration::from_secs(300),
        }
    }
}
impl SyncPolicy {
    pub fn transfer_budget(self, size: u64) -> Duration {
        self.throughput_grace
            + Duration::from_secs(size.div_ceil(self.minimum_bytes_per_second.max(1)))
    }
}

#[derive(Clone)]
pub(crate) struct Bucket {
    credit: u128,
    updated: Instant,
    burst: u64,
    interval: Duration,
    rate: u64,
}
impl Bucket {
    pub fn new(burst: u64, interval: Duration, now: Instant) -> Self {
        Self {
            credit: burst as u128 * interval.as_nanos(),
            updated: now,
            burst,
            interval,
            rate: 1,
        }
    }
    pub fn per_second(burst: u64, rate: u64, now: Instant) -> Self {
        Self {
            rate,
            ..Self::new(burst, Duration::from_secs(1), now)
        }
    }
    pub fn available(&mut self, amount: u64, now: Instant) -> bool {
        self.credit = self
            .credit
            .saturating_add(
                now.saturating_duration_since(self.updated)
                    .as_nanos()
                    .saturating_mul(self.rate as u128),
            )
            .min(self.burst as u128 * self.interval.as_nanos());
        self.updated = self.updated.max(now);
        self.credit >= amount as u128 * self.interval.as_nanos()
    }
    pub fn take(&mut self, amount: u64, now: Instant) -> bool {
        if !self.available(amount, now) {
            return false;
        }
        self.credit -= amount as u128 * self.interval.as_nanos();
        true
    }
}
#[derive(Clone)]
struct OriginState {
    bucket: Bucket,
    touched: Instant,
    failures: u8,
    cooldown: Option<Instant>,
}
// Fingerprints accelerate lookup; only full key equality establishes ownership.
// Never replace debt with fresh credit merely to make room for a new owner.
pub(crate) struct HistoryCache<K, V> {
    records: Vec<(u64, K, V)>,
    capacity: usize,
    hash: RandomState,
    #[cfg(test)]
    collide: bool,
}
impl<K: Copy + Eq + Hash, V> HistoryCache<K, V> {
    pub fn new(capacity: usize) -> Self {
        Self {
            records: Vec::with_capacity(capacity),
            capacity,
            hash: RandomState::new(),
            #[cfg(test)]
            collide: false,
        }
    }
    #[cfg(test)]
    pub fn force_collisions(&mut self) {
        assert!(self.records.is_empty());
        self.collide = true;
    }
    #[cfg(all(test, feature = "gns"))]
    pub fn len(&self) -> usize {
        self.records.len()
    }
    fn fingerprint(&self, key: K) -> u64 {
        #[cfg(test)]
        if self.collide {
            return 0;
        }
        self.hash.hash_one(key)
    }
    pub fn get_mut(&mut self, key: K) -> Option<&mut V> {
        let hash = self.fingerprint(key);
        self.records
            .iter_mut()
            .find(|(h, k, _)| *h == hash && *k == key)
            .map(|(_, _, v)| v)
    }
    pub fn take(&mut self, key: K) -> Option<V> {
        let hash = self.fingerprint(key);
        let index = self
            .records
            .iter()
            .position(|(h, k, _)| *h == hash && *k == key)?;
        Some(self.records.swap_remove(index).2)
    }
    pub fn prune(&mut self, mut live: impl FnMut(K, &V) -> bool) {
        self.records.retain(|(_, k, v)| live(*k, v));
    }
    #[cfg(feature = "gns")]
    pub fn has_room(&mut self, mut reclaimable: impl FnMut(K, &mut V) -> Option<Instant>) -> bool {
        self.records.len() < self.capacity
            || self
                .records
                .iter_mut()
                .any(|(_, k, v)| reclaimable(*k, v).is_some())
    }
    pub fn remember(
        &mut self,
        key: K,
        mut value: V,
        mut reclaimable: impl FnMut(K, &mut V) -> Option<Instant>,
    ) -> Result<(), V> {
        if self.records.len() == self.capacity {
            let victim = self
                .records
                .iter_mut()
                .enumerate()
                .filter_map(|(i, (_, k, v))| reclaimable(*k, v).map(|at| (i, at)))
                .min_by_key(|(_, at)| *at)
                .map(|(i, _)| i);
            let Some(victim) = victim else {
                return if reclaimable(key, &mut value).is_some() {
                    Ok(())
                } else {
                    Err(value)
                };
            };
            self.records.swap_remove(victim);
        }
        let hash = self.fingerprint(key);
        self.records.push((hash, key, value));
        Ok(())
    }
}
pub(crate) struct Admission {
    global: Option<Bucket>,
    global_policy: Option<(u64, Duration)>,
    origins: BTreeMap<Origin, OriginState>,
    active: BTreeSet<Origin>,
    retired: HistoryCache<Origin, OriginState>,
    untracked: Option<Bucket>,
    burst: u64,
    interval: Duration,
}
impl Admission {
    #[cfg(any(feature = "gns", test))]
    pub fn connections() -> Self {
        Self::new(CONNECTION_START_BURST, CONNECTION_START_INTERVAL, None)
    }
    pub fn joins() -> Self {
        Self::new(
            ORIGIN_JOIN_BURST,
            ORIGIN_JOIN_INTERVAL,
            Some((JOIN_ADMISSION_BURST, JOIN_ADMISSION_INTERVAL)),
        )
    }
    pub fn authentication() -> Self {
        Self::new(ORIGIN_JOIN_BURST, ORIGIN_JOIN_INTERVAL, None)
    }
    fn new(burst: u64, interval: Duration, global: Option<(u64, Duration)>) -> Self {
        Self {
            global: None,
            global_policy: global,
            origins: BTreeMap::new(),
            active: BTreeSet::new(),
            retired: HistoryCache::new(RETIRED_ORIGINS),
            untracked: None,
            burst,
            interval,
        }
    }
    fn entry(&mut self, origin: Origin, now: Instant) -> Option<&mut OriginState> {
        self.origins
            .retain(|o, s| self.active.contains(o) || Self::live(s, now));
        self.retired
            .prune(|o, s| self.active.contains(&o) || Self::live(s, now));
        if self.origins.contains_key(&origin) {
            return self.origins.get_mut(&origin);
        }
        let mut inherited = None;
        if self.origins.len() >= MAX_ORIGINS {
            let victim = self
                .origins
                .iter()
                .filter(|(o, _)| !self.active.contains(o))
                .min_by_key(|(_, s)| s.touched)
                .map(|(&o, _)| o);
            let Some(victim) = victim else {
                return self.retired.get_mut(origin);
            };
            inherited = self.retired.take(origin);
            let state = self.origins.remove(&victim)?;
            if let Err(state) = self.retired.remember(victim, state, |o, s| {
                if self.active.contains(&o) {
                    None
                } else {
                    Self::reclaimable(s, now)
                }
            }) {
                self.origins.insert(victim, state);
                return None;
            }
        }
        let state = inherited
            .or_else(|| self.retired.take(origin))
            .unwrap_or_else(|| OriginState {
                bucket: Bucket::new(self.burst, self.interval, now),
                touched: now,
                failures: 0,
                cooldown: None,
            });
        Some(self.origins.entry(origin).or_insert(state))
    }
    fn live(s: &OriginState, now: Instant) -> bool {
        now.saturating_duration_since(s.touched) < ORIGIN_TTL
            || s.cooldown
                .is_some_and(|until| now.saturating_duration_since(until) < ORIGIN_TTL)
    }
    fn reclaimable(s: &mut OriginState, now: Instant) -> Option<Instant> {
        (s.failures == 0 && s.cooldown.is_none() && s.bucket.available(s.bucket.burst, now))
            .then_some(s.touched)
    }
    pub fn protect(&mut self, origins: impl IntoIterator<Item = Option<Origin>>) {
        self.active = origins
            .into_iter()
            .flatten()
            .map(Origin::normalized)
            .collect();
    }
    pub fn admit(
        &mut self,
        origin: Option<Origin>,
        pending: usize,
        now: Instant,
    ) -> Result<(), DisconnectReason> {
        let origin = origin.map(Origin::normalized);
        let mut untracked = false;
        if let Some(origin) = origin {
            if pending >= MAX_PENDING_PER_ORIGIN {
                return Err(DisconnectReason::JoinCapacity);
            }
            if let Some(s) = self.entry(origin, now) {
                s.touched = now;
                if s.cooldown.is_some_and(|until| now < until) || !s.bucket.available(1, now) {
                    return Err(DisconnectReason::RateLimited);
                }
            } else {
                // Only cache misses share this budget. Remembered origins retain
                // their independent limits even when a many-source flood fills it.
                untracked = true;
                if !self
                    .untracked
                    .get_or_insert_with(|| Bucket::per_second(32, 4, now))
                    .available(1, now)
                {
                    return Err(DisconnectReason::RateLimited);
                }
            }
        }
        if let Some((burst, interval)) = self.global_policy {
            if !self
                .global
                .get_or_insert_with(|| Bucket::new(burst, interval, now))
                .take(1, now)
            {
                return Err(DisconnectReason::RateLimited);
            }
        }
        // A global refusal must not spend origin or fallback credit.
        if untracked {
            self.untracked.as_mut().unwrap().take(1, now);
        } else if let Some(origin) = origin {
            if let Some(s) = self
                .origins
                .get_mut(&origin)
                .or_else(|| self.retired.get_mut(origin))
            {
                s.bucket.take(1, now);
            }
        }
        Ok(())
    }
    pub fn penalize(&mut self, origin: Option<Origin>, now: Instant) {
        if let Some(s) = origin
            .map(Origin::normalized)
            .and_then(|o| self.entry(o, now))
        {
            s.touched = now;
            s.failures = s.failures.saturating_add(1);
            if s.failures >= COOLDOWN_FAILURES {
                let seconds = ORIGIN_COOLDOWN
                    .as_secs()
                    .saturating_mul(1u64 << (s.failures - COOLDOWN_FAILURES).min(3));
                s.cooldown = Some(now + Duration::from_secs(seconds).min(MAX_BACKOFF));
            }
        }
    }
    pub fn succeed(&mut self, origin: Option<Origin>) {
        let Some(origin) = origin.map(Origin::normalized) else {
            return;
        };
        if let Some(s) = self.origins.get_mut(&origin) {
            s.failures = 0;
            s.cooldown = None;
        }
        if let Some(s) = self.retired.get_mut(origin) {
            s.failures = 0;
            s.cooldown = None;
        }
    }
}

/// Bounded frame-driven round robin. One source has at most four waiting items;
/// each waiting source gets one turn before a source gets its next turn.
pub(crate) struct FairQueue<T> {
    items: VecDeque<(Option<Origin>, T, Instant)>,
    active: VecDeque<Option<Origin>>,
}
impl<T> Default for FairQueue<T> {
    fn default() -> Self {
        Self {
            items: VecDeque::new(),
            active: VecDeque::new(),
        }
    }
}
impl<T> FairQueue<T> {
    pub fn push(
        &mut self,
        origin: Option<Origin>,
        item: T,
        now: Instant,
    ) -> Result<(), DisconnectReason> {
        let origin = origin.map(Origin::normalized);
        if self.items.len() >= MAX_PENDING_CONNECTIONS
            || self.items.iter().filter(|(o, _, _)| *o == origin).count() >= MAX_PENDING_PER_ORIGIN
        {
            return Err(DisconnectReason::JoinCapacity);
        }
        if !self.active.contains(&origin) {
            self.active.push_back(origin);
        }
        self.items.push_back((origin, item, now));
        Ok(())
    }
    pub fn pop(&mut self) -> Option<T> {
        let origin = self.active.pop_front()?;
        let index = self.items.iter().position(|(o, _, _)| *o == origin)?;
        let (origin, item, _) = self.items.remove(index)?;
        if self.items.iter().any(|(o, _, _)| *o == origin) {
            self.active.push_back(origin);
        }
        Some(item)
    }
    pub fn retain(&mut self, mut keep: impl FnMut(&T, Instant) -> bool) {
        self.items.retain(|(_, item, at)| keep(item, *at));
        self.active
            .retain(|origin| self.items.iter().any(|(o, _, _)| o == origin));
    }
    #[cfg(feature = "gns")]
    pub fn any(&self, mut predicate: impl FnMut(&T) -> bool) -> bool {
        self.items.iter().any(|(_, item, _)| predicate(item))
    }
    pub fn len(&self) -> usize {
        self.items.len()
    }
    #[cfg(feature = "gns")]
    pub fn origins(&self) -> impl Iterator<Item = Option<Origin>> + '_ {
        self.active.iter().copied()
    }
}

pub(crate) fn is_abuse(reason: DisconnectReason) -> bool {
    matches!(
        reason,
        DisconnectReason::AuthenticationFailed
            | DisconnectReason::AuthenticationTimeout
            | DisconnectReason::AuthenticatedHandoffTimeout
            | DisconnectReason::SyncPhaseTimeout
            | DisconnectReason::SyncLifetime
            | DisconnectReason::BulkStalled
            | DisconnectReason::BackendConnectionTimeout
            | DisconnectReason::InvalidMessage
            | DisconnectReason::ProtocolViolation
    )
}

// Bootstrap owns PAKE failures/timeouts. Native transport accounts only its own
// malformed traffic and establishment failures, once before removal.
#[cfg_attr(not(feature = "gns"), allow(dead_code))]
pub(crate) fn is_transport_abuse(reason: DisconnectReason) -> bool {
    is_abuse(reason)
        && !matches!(
            reason,
            DisconnectReason::AuthenticationFailed | DisconnectReason::AuthenticationTimeout
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn origin(n: u8) -> Origin {
        Origin::Ip(std::net::Ipv4Addr::new(10, 0, 0, n).into())
    }
    #[test]
    fn origin_capacity_preserves_room_for_other_users_and_survives_reconnect() {
        let now = Instant::now();
        let mut gate = Admission::joins();
        for pending in 0..MAX_PENDING_PER_ORIGIN {
            gate.admit(Some(origin(1)), pending, now).unwrap();
        }
        assert_eq!(
            gate.admit(Some(origin(1)), MAX_PENDING_PER_ORIGIN, now),
            Err(DisconnectReason::JoinCapacity)
        );
        // Closing every connection does not replenish the history bucket.
        assert_eq!(
            gate.admit(Some(origin(1)), 0, now),
            Err(DisconnectReason::RateLimited)
        );
        gate.admit(Some(origin(2)), 0, now).unwrap();
        for _ in 0..3 {
            gate.penalize(Some(origin(1)), now);
        }
        assert_eq!(
            gate.admit(Some(origin(1)), 0, now + Duration::from_secs(20)),
            Err(DisconnectReason::RateLimited)
        );
        gate.admit(Some(origin(1)), 0, now + ORIGIN_COOLDOWN)
            .unwrap();
    }
    #[test]
    fn global_refusal_preserves_origin_credit_and_origin_refusal_preserves_global_credit() {
        let now = Instant::now();
        let mut gate = Admission::joins();
        for _ in 0..JOIN_ADMISSION_BURST {
            gate.admit(None, 0, now).unwrap();
        }
        for _ in 0..ORIGIN_JOIN_BURST {
            assert_eq!(
                gate.admit(Some(origin(1)), 0, now),
                Err(DisconnectReason::RateLimited)
            );
        }
        gate.admit(Some(origin(1)), 0, now + JOIN_ADMISSION_INTERVAL)
            .unwrap();
        let mut gate = Admission::joins();
        for _ in 0..ORIGIN_JOIN_BURST {
            gate.admit(Some(origin(1)), 0, now).unwrap();
        }
        for _ in 0..JOIN_ADMISSION_BURST {
            assert_eq!(
                gate.admit(Some(origin(1)), 0, now),
                Err(DisconnectReason::RateLimited)
            );
        }
        for _ in ORIGIN_JOIN_BURST..JOIN_ADMISSION_BURST {
            gate.admit(None, 0, now).unwrap();
        }
        assert_eq!(gate.admit(None, 0, now), Err(DisconnectReason::RateLimited));
    }
    #[test]
    fn successful_authentication_cannot_bypass_global_join_rate() {
        let now = Instant::now();
        let mut gate = Admission::joins();
        for _ in 0..12 {
            gate.admit(None, 0, now).unwrap();
        }
        assert_eq!(gate.admit(None, 0, now), Err(DisconnectReason::RateLimited));
        gate.admit(None, 0, now + Duration::from_secs(2)).unwrap();
        assert_eq!(
            gate.admit(None, 0, now + Duration::from_secs(2)),
            Err(DisconnectReason::RateLimited)
        );
    }
    #[test]
    fn penalty_history_is_bounded_expires_and_does_not_evict_cooldowns() {
        let now = Instant::now();
        let mut gate = Admission::connections();
        for n in 0..MAX_ORIGINS {
            let ip = Origin::Ip(std::net::Ipv4Addr::new(10, 0, (n / 256) as u8, n as u8).into());
            gate.admit(Some(ip), 0, now).unwrap();
            gate.penalize(Some(ip), now);
        }
        assert_eq!(gate.origins.len(), MAX_ORIGINS);
        let extra = Origin::Ip("192.0.2.1".parse().unwrap());
        gate.admit(Some(extra), 0, now).unwrap();
        assert_eq!(gate.origins.len(), MAX_ORIGINS);
        gate.admit(Some(extra), 0, now + ORIGIN_TTL).unwrap();
        assert_eq!(gate.origins.len(), 1);
    }
    #[test]
    fn evicted_penalties_and_credit_survive_churn_and_active_history_is_pinned() {
        let now = Instant::now();
        let mut gate = Admission::connections();
        let bad = origin(1);
        let active = origin(2);
        gate.admit(Some(active), 0, now).unwrap();
        gate.protect([Some(active)]);
        for _ in 0..COOLDOWN_FAILURES {
            gate.penalize(Some(bad), now);
        }
        let spent = origin(3);
        for _ in 0..CONNECTION_START_BURST {
            gate.admit(Some(spent), 0, now).unwrap();
        }
        for n in 0..MAX_ORIGINS * 4 {
            let ip = Origin::ip(std::net::Ipv4Addr::new(172, 16, (n / 256) as u8, n as u8).into());
            gate.entry(ip, now).unwrap();
        }
        assert!(gate.origins.contains_key(&active));
        assert_eq!(
            gate.admit(Some(bad), 0, now),
            Err(DisconnectReason::RateLimited)
        );
        assert_eq!(
            gate.admit(Some(spent), 0, now),
            Err(DisconnectReason::RateLimited)
        );
        assert_eq!(gate.origins.len(), MAX_ORIGINS);
        assert!(gate.retired.records.len() <= RETIRED_ORIGINS);
        gate.entry(origin(4), now + ORIGIN_TTL).unwrap();
        assert!(gate.origins.contains_key(&active));
    }
    #[test]
    fn successful_authentication_clears_its_evicted_penalty_without_restoring_credit() {
        let now = Instant::now();
        let mut gate = Admission::authentication();
        let bad = origin(1);
        for _ in 0..COOLDOWN_FAILURES {
            gate.penalize(Some(bad), now);
        }
        let churn: Vec<_> = (0..4096)
            .map(|n| Origin::ip(std::net::Ipv4Addr::new(172, 16, (n / 256) as u8, n as u8).into()))
            .take(MAX_ORIGINS)
            .collect();
        for o in &churn {
            gate.entry(*o, now).unwrap();
        }
        assert!(!gate.origins.contains_key(&bad));
        let at = now + ORIGIN_COOLDOWN;
        gate.admit(Some(bad), 0, at).unwrap();
        gate.succeed(Some(bad));
        for o in &churn {
            gate.entry(*o, at).unwrap().touched = at;
        }
        // Force another eviction of the now successful, inactive origin.
        gate.entry(origin(250), at).unwrap();
        gate.penalize(Some(bad), at);
        assert_eq!(gate.origins[&bad].failures, 1);
        assert!(gate.origins[&bad].cooldown.is_none());
    }
    #[test]
    fn backoff_is_per_origin_progressive_bounded_and_success_resets_failures() {
        let now = Instant::now();
        let mut gate = Admission::authentication();
        let mut at = now;
        for failures in 1..=8 {
            gate.penalize(Some(origin(1)), at);
            if failures >= COOLDOWN_FAILURES {
                let wait = Duration::from_secs(30 * (1 << (failures - COOLDOWN_FAILURES).min(3)));
                // No probe before expiry: reconnecting exactly at the deadline
                // must retain the preceding failure count even for 120/240 s.
                assert!(gate.origins[&origin(1)]
                    .cooldown
                    .is_some_and(|until| until == at + wait));
                gate.admit(Some(origin(2)), 0, at + wait).unwrap();
                at += wait;
                gate.admit(Some(origin(1)), 0, at).unwrap();
            }
        }
        gate.succeed(Some(origin(1)));
        gate.penalize(Some(origin(1)), at);
        assert_eq!(gate.origins[&origin(1)].failures, 1);
        assert!(gate.origins[&origin(1)].cooldown.is_none());
    }
    #[test]
    fn ip_prefixes_and_reissued_routes_share_only_abuse_history() {
        let now = Instant::now();
        let mut gate = Admission::authentication();
        for (a, b) in [
            ("2001:db8:1:2::1", "2001:db8:1:2::ffff"),
            ("192.0.2.1", "::ffff:192.0.2.1"),
        ] {
            let a = Origin::Ip(a.parse().unwrap());
            let b = Origin::Ip(b.parse().unwrap());
            for _ in 0..COOLDOWN_FAILURES {
                gate.penalize(Some(a), now);
            }
            assert_eq!(
                gate.admit(Some(b), 0, now),
                Err(DisconnectReason::RateLimited)
            );
        }
        use super::super::transport::RouteOrigin;
        let a = RouteOrigin::from_authenticated_route([1; 16], [2; 16], [3; 16])
            .with_abuse_key([7; 16]);
        let b = RouteOrigin::from_authenticated_route([1; 16], [2; 16], [4; 16])
            .with_abuse_key([7; 16]);
        assert_ne!(a, b);
        for _ in 0..COOLDOWN_FAILURES {
            gate.penalize(Some(Origin::Route(a)), now);
        }
        assert_eq!(
            gate.admit(Some(Origin::Route(b)), 0, now),
            Err(DisconnectReason::RateLimited)
        );
    }
    #[test]
    fn fair_queue_serves_every_origin_before_repeating_any_origin() {
        let now = Instant::now();
        let mut queue = FairQueue::default();
        for o in 1..=3 {
            for n in 0..3 {
                queue.push(Some(origin(o)), (o, n), now).unwrap();
            }
        }
        for n in 0..3 {
            for o in 1..=3 {
                assert_eq!(queue.pop(), Some((o, n)));
            }
        }
        assert!(queue.pop().is_none());
    }
    #[test]
    fn waiting_sources_alternate_and_cleanup_preserves_bounds() {
        let now = Instant::now();
        let mut queue = FairQueue::default();
        for n in 0..4 {
            queue.push(Some(origin(1)), n, now).unwrap();
        }
        assert_eq!(
            queue.push(Some(origin(1)), 9, now),
            Err(DisconnectReason::JoinCapacity)
        );
        for n in 4..8 {
            queue.push(Some(origin(2)), n, now).unwrap();
        }
        assert_eq!(queue.pop(), Some(0));
        assert_eq!(queue.pop(), Some(4));
        assert_eq!(queue.pop(), Some(1));
        queue.retain(|item, _| *item < 4);
        assert_eq!(queue.len(), 2);
        queue.retain(|_, at| now + CONNECTING_TIMEOUT < at + CONNECTING_TIMEOUT);
        assert_eq!(queue.len(), 0);
    }
    fn numbered(n: u32) -> Origin {
        Origin::ip(std::net::Ipv4Addr::from(n).into())
    }
    #[test]
    fn exact_owners_survive_forced_collisions_and_success_never_refunds_credit() {
        let now = Instant::now();
        let mut gate = Admission::connections();
        gate.retired.force_collisions(); // Every fingerprint collides, deterministically.
        let a = numbered(1);
        for _ in 0..CONNECTION_START_BURST {
            gate.admit(Some(a), 0, now).unwrap();
        }
        for _ in 0..COOLDOWN_FAILURES {
            gate.penalize(Some(a), now);
        }
        for n in 1000..1000 + MAX_ORIGINS as u32 {
            gate.entry(numbered(n), now).unwrap();
        }
        assert!(gate.retired.get_mut(a).is_some());
        let b = numbered(2);
        gate.admit(Some(b), 0, now).unwrap();
        assert_eq!(gate.origins[&b].failures, 0);
        assert!(gate.origins[&b].cooldown.is_none());
        gate.succeed(Some(b));
        let saved = gate.retired.get_mut(a).unwrap();
        assert_eq!(saved.failures, COOLDOWN_FAILURES);
        assert!(saved.cooldown.is_some());
        gate.succeed(Some(a));
        assert_eq!(gate.retired.get_mut(a).unwrap().failures, 0);
        assert_eq!(
            gate.admit(Some(a), 0, now),
            Err(DisconnectReason::RateLimited)
        );
        gate.admit(Some(a), 0, now + CONNECTION_START_INTERVAL)
            .unwrap();
    }
    #[test]
    fn saturated_debt_cache_limits_only_unrecorded_origins_and_stays_bounded() {
        let now = Instant::now();
        let mut gate = Admission::connections();
        gate.retired.force_collisions();
        for n in 1..=(MAX_ORIGINS + RETIRED_ORIGINS) as u32 {
            for _ in 0..CONNECTION_START_BURST {
                gate.admit(Some(numbered(n)), 0, now).unwrap();
            }
        }
        let a = numbered(1);
        for n in 10000..20000 {
            let result = gate.admit(Some(numbered(n)), 0, now);
            if n < 10032 {
                assert!(result.is_ok());
            } else {
                assert_eq!(result, Err(DisconnectReason::RateLimited));
            }
            assert!(gate.origins.len() <= MAX_ORIGINS);
            assert!(gate.retired.records.len() <= RETIRED_ORIGINS);
        }
        assert_eq!(
            gate.admit(Some(a), 0, now),
            Err(DisconnectReason::RateLimited)
        );
        let at = now + Duration::from_millis(250);
        // Refilling fallback opportunity does not refill A's one-second bucket.
        gate.admit(Some(numbered(30000)), 0, at).unwrap();
        assert_eq!(
            gate.admit(Some(a), 0, at),
            Err(DisconnectReason::RateLimited)
        );
        // A remembered origin's refill bypasses an exhausted fallback.
        gate.admit(Some(a), 0, now + CONNECTION_START_INTERVAL)
            .unwrap();
        gate.entry(
            numbered(40000),
            now + CONNECTION_START_INTERVAL + ORIGIN_TTL,
        )
        .unwrap();
        assert_eq!(gate.origins.len(), 1);
        assert!(gate.retired.records.is_empty());
    }
    #[test]
    fn active_primary_and_retired_histories_are_pinned_without_excluding_newcomers() {
        let now = Instant::now();
        let mut gate = Admission::connections();
        for n in 1..=MAX_ORIGINS as u32 {
            gate.admit(Some(numbered(n)), 0, now).unwrap();
        }
        gate.protect((1..=MAX_ORIGINS as u32).map(|n| Some(numbered(n))));
        gate.admit(Some(numbered(999)), 0, now).unwrap();
        gate.entry(numbered(1000), now + ORIGIN_TTL);
        assert_eq!(gate.origins.len(), MAX_ORIGINS);
        for n in 1..=MAX_ORIGINS as u32 {
            assert!(gate.origins.contains_key(&numbered(n)));
        }
    }
    #[test]
    fn retired_active_history_cannot_be_reclaimed_even_after_refill_and_ttl() {
        let now = Instant::now();
        let mut gate = Admission::connections();
        let a = numbered(1);
        gate.entry(a, now).unwrap();
        for n in 1000..1000 + MAX_ORIGINS as u32 {
            gate.entry(numbered(n), now).unwrap();
        }
        assert!(gate.retired.get_mut(a).is_some());
        gate.protect([Some(a)]);
        let at = now + ORIGIN_TTL;
        for n in 2000..4000 {
            gate.entry(numbered(n), at).unwrap();
        }
        assert!(gate.retired.get_mut(a).is_some());
    }
    #[test]
    fn exact_retired_records_fit_previous_compressed_allocation() {
        #[allow(dead_code)]
        struct OldCompressed {
            owner: Option<Origin>,
            state: OriginState,
        }
        assert!(
            std::mem::size_of::<(u64, Origin, OriginState)>()
                <= std::mem::size_of::<Option<OldCompressed>>()
        );
    }
    #[test]
    fn repeated_refill_expiry_and_churn_keep_all_admission_structures_bounded() {
        let start = Instant::now();
        let mut gate = Admission::connections();
        for round in 0..1000u32 {
            let at = start + Duration::from_secs(round as u64);
            for n in 0..10 {
                let source = numbered(round * 10 + n + 1);
                let result = gate.admit(Some(source), 0, at);
                assert!(result.is_ok() || result == Err(DisconnectReason::RateLimited));
                gate.penalize(Some(source), at);
            }
            assert!(gate.origins.len() <= MAX_ORIGINS);
            assert!(gate.retired.records.len() <= RETIRED_ORIGINS);
            assert!(gate.active.is_empty());
        }
    }
}
