//! Bounded resource policy for establishment and join; origins are abuse keys,
//! never authenticated player identities. All clocks are supplied by the caller.
use super::transport::{DisconnectReason, Origin};
use std::{
    collections::BTreeMap,
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
struct OriginState {
    bucket: Bucket,
    touched: Instant,
    failures: u8,
    cooldown: Option<Instant>,
}
pub(crate) struct Admission {
    global: Option<Bucket>,
    global_policy: Option<(u64, Duration)>,
    origins: BTreeMap<Origin, OriginState>,
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
    fn new(burst: u64, interval: Duration, global: Option<(u64, Duration)>) -> Self {
        Self {
            global: None,
            global_policy: global,
            origins: BTreeMap::new(),
            burst,
            interval,
        }
    }
    fn entry(&mut self, origin: Origin, now: Instant) -> Option<&mut OriginState> {
        // Fail closed on a full table; do not evict a live cooldown to make room.
        self.origins
            .retain(|_, s| now.saturating_duration_since(s.touched) < ORIGIN_TTL);
        if !self.origins.contains_key(&origin) && self.origins.len() >= MAX_ORIGINS {
            return None;
        }
        Some(self.origins.entry(origin).or_insert_with(|| OriginState {
            bucket: Bucket::new(self.burst, self.interval, now),
            touched: now,
            failures: 0,
            cooldown: None,
        }))
    }
    pub fn admit(
        &mut self,
        origin: Option<Origin>,
        pending: usize,
        now: Instant,
    ) -> Result<(), DisconnectReason> {
        if let Some(origin) = origin {
            if pending >= MAX_PENDING_PER_ORIGIN {
                return Err(DisconnectReason::JoinCapacity);
            }
            let s = self
                .entry(origin, now)
                .ok_or(DisconnectReason::RateLimited)?;
            s.touched = now;
            if s.cooldown.is_some_and(|until| now < until) || !s.bucket.available(1, now) {
                return Err(DisconnectReason::RateLimited);
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
        // Both limits passed. A global refusal must not spend origin credit,
        // and an origin refusal must not spend global credit.
        if let Some(s) = origin.and_then(|o| self.origins.get_mut(&o)) {
            s.bucket.take(1, now);
        }
        Ok(())
    }
    pub fn penalize(&mut self, origin: Option<Origin>, now: Instant) {
        if let Some(s) = origin.and_then(|o| self.entry(o, now)) {
            s.touched = now;
            s.failures = s.failures.saturating_add(1);
            if s.failures >= COOLDOWN_FAILURES {
                s.cooldown = Some(now + ORIGIN_COOLDOWN);
                s.failures = 0;
            }
        }
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
        assert_eq!(
            gate.admit(Some(extra), 0, now),
            Err(DisconnectReason::RateLimited)
        );
        gate.admit(Some(extra), 0, now + ORIGIN_TTL).unwrap();
        assert_eq!(gate.origins.len(), 1);
    }
}
