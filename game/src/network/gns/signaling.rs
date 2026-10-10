//! Caller-owned, bounded, fair opaque signaling mailboxes. Trusted route binding
//! belongs to the local rendezvous adapter, never to untrusted signaling bytes.
use crate::network::{
    lifecycle::{Bucket, HistoryCache, MAX_CONNECTIONS, MAX_ORIGINS, ORIGIN_TTL},
    transport::{Origin, RouteOrigin, TransportError},
};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub const MAX_SIGNAL_BYTES: usize = 16 * 1024;
pub const MAX_QUEUED_SIGNALS: usize = 128;
pub const MAX_SIGNAL_QUEUE_BYTES: usize = 256 * 1024;
pub const MAX_SIGNALS_PER_ROUTE: usize = 16;
pub const MAX_SIGNAL_BYTES_PER_ROUTE: usize = 32 * 1024;
pub const SIGNALS_PER_ROUTE_PER_SECOND: u64 = 32;
pub const MAX_PEERS_PER_ROUTE: usize = 8;

/// Random process routing label, never an authentication credential/native handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PeerId([u8; 16]);
impl PeerId {
    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }
    pub fn to_bytes(self) -> [u8; 16] {
        self.0
    }
    pub(super) fn random() -> Self {
        Self(*uuid::Uuid::new_v4().as_bytes())
    }
}
#[derive(Debug)]
pub struct OutboundSignal {
    pub peer: PeerId,
    pub payload: Vec<u8>,
}
pub(super) struct InboundSignal {
    pub peer: PeerId,
    pub payload: Vec<u8>,
    pub origin: Option<Origin>,
}
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum SignalKey {
    Unverified(PeerId),
    Route(RouteOrigin),
}
// At most 128 nonempty route queues + 128 retired rates: the previous
// 256-route ceiling also bounds the number of per-route token buckets.
const MAX_RETIRED_RATES: usize = MAX_ORIGINS - MAX_QUEUED_SIGNALS;
const RATE_REFILL: Duration = Duration::from_millis(500); // 16 / 32 seconds
struct RateHistory {
    bucket: Bucket,
    touched: Instant,
}
impl RateHistory {
    fn reclaimable(&mut self, now: Instant) -> Option<Instant> {
        self.bucket
            .available(MAX_SIGNALS_PER_ROUTE as u64, now)
            .then_some(self.touched)
    }
}
struct RouteQueue {
    messages: VecDeque<OutboundSignal>,
    bytes: usize,
    rate: RateHistory,
    untracked: bool,
}
struct Queue {
    routes: BTreeMap<SignalKey, RouteQueue>,
    retired: HistoryCache<SignalKey, RateHistory>,
    active: VecDeque<SignalKey>,
    count: usize,
    bytes: usize,
    global: Bucket,
    untracked: Bucket,
    untracked_until: Option<Instant>,
}
impl Default for Queue {
    fn default() -> Self {
        let now = Instant::now();
        Self {
            routes: BTreeMap::new(),
            retired: HistoryCache::new(MAX_RETIRED_RATES),
            active: VecDeque::new(),
            count: 0,
            bytes: 0,
            global: Bucket::per_second(128, 128, now),
            untracked: Bucket::per_second(128, 128, now),
            untracked_until: None,
        }
    }
}
impl Queue {
    fn push(
        &mut self,
        peer: PeerId,
        key: SignalKey,
        payload: &[u8],
        now: Instant,
    ) -> Result<(), TransportError> {
        if payload.is_empty() || payload.len() > MAX_SIGNAL_BYTES {
            return Err(TransportError::PayloadTooLarge);
        }
        if self.count >= MAX_QUEUED_SIGNALS || self.bytes + payload.len() > MAX_SIGNAL_QUEUE_BYTES {
            return Err(TransportError::Backpressure);
        }
        if !self.routes.contains_key(&key) {
            self.retired
                .prune(|_, r| now.saturating_duration_since(r.touched) < ORIGIN_TTL);
            let inherited = self.retired.take(key);
            let untracked = inherited.is_none()
                && (self.untracked_until.is_some_and(|until| now < until)
                    || !self.retired.has_room(|_, r| r.reclaimable(now)));
            let rate = inherited.unwrap_or_else(|| RateHistory {
                bucket: Bucket::per_second(
                    MAX_SIGNALS_PER_ROUTE as u64,
                    SIGNALS_PER_ROUTE_PER_SECOND,
                    now,
                ),
                touched: now,
            });
            // A restored, exhausted rate must stay retired when enqueue fails.
            // Do not create an empty queue that can consume an unbounded key slot.
            let mut rate = rate;
            if !rate.bucket.take(1, now) {
                self.retire(key, rate, now);
                return Err(TransportError::Backpressure);
            }
            rate.touched = now;
            self.routes.insert(
                key,
                RouteQueue {
                    messages: VecDeque::new(),
                    bytes: 0,
                    rate,
                    untracked,
                },
            );
            self.active.push_back(key);
        } else {
            let q = self.routes.get_mut(&key).unwrap();
            if q.messages.len() >= MAX_SIGNALS_PER_ROUTE
                || q.bytes + payload.len() > MAX_SIGNAL_BYTES_PER_ROUTE
                || !q.rate.bucket.take(1, now)
            {
                return Err(TransportError::Backpressure);
            }
            q.rate.touched = now;
        }
        let q = self.routes.get_mut(&key).unwrap();
        q.bytes += payload.len();
        q.messages.push_back(OutboundSignal {
            peer,
            payload: payload.to_vec(),
        });
        self.count += 1;
        self.bytes += payload.len();
        Ok(())
    }
    fn retire(&mut self, key: SignalKey, rate: RateHistory, now: Instant) {
        if self
            .retired
            .remember(key, rate, |_, r| r.reclaimable(now))
            .is_err()
        {
            // A lost bucket would naturally refill within 500 ms. Keep all
            // cache misses on the fallback even if an intervening restore frees
            // a slot, so they cannot immediately turn lost debt into new credit.
            self.untracked_until = Some(self.untracked_until.unwrap_or(now).max(now + RATE_REFILL));
        }
    }
    fn pop(&mut self, now: Instant, limited: bool) -> Option<(SignalKey, OutboundSignal)> {
        if self.active.is_empty() || (limited && !self.global.available(1, now)) {
            return None;
        }
        // An exhausted fallback defers messages; remembered routes can still
        // run. Rotate at most the bounded number of nonempty queues once.
        for _ in 0..self.active.len() {
            let key = self.active.pop_front()?;
            let q = self.routes.get_mut(&key)?;
            if q.untracked && !self.untracked.take(1, now) {
                self.active.push_back(key);
                continue;
            }
            if limited {
                self.global.take(1, now);
            }
            let signal = q.messages.pop_front()?;
            q.bytes -= signal.payload.len();
            if q.messages.is_empty() {
                let q = self.routes.remove(&key).unwrap();
                self.retire(key, q.rate, now);
            } else {
                self.active.push_back(key);
            }
            self.count -= 1;
            self.bytes -= signal.payload.len();
            return Some((key, signal));
        }
        None
    }
    fn revoke(&mut self, peer: PeerId, now: Instant) {
        for q in self.routes.values_mut() {
            q.messages.retain(|s| s.peer != peer);
            q.bytes = q.messages.iter().map(|s| s.payload.len()).sum();
        }
        self.active
            .retain(|key| self.routes.get(key).is_some_and(|q| !q.messages.is_empty()));
        let empty: Vec<_> = self
            .routes
            .iter()
            .filter(|(_, q)| q.messages.is_empty())
            .map(|(&k, _)| k)
            .collect();
        for key in empty {
            let q = self.routes.remove(&key).unwrap();
            self.retire(key, q.rate, now);
        }
        self.count = self.routes.values().map(|q| q.messages.len()).sum();
        self.bytes = self.routes.values().map(|q| q.bytes).sum();
    }
}
/// Latest desired TURN defaults; one slot also coalesces expiry and recovery.
#[cfg_attr(not(any(feature = "rendezvous", test)), allow(dead_code))]
pub(super) enum TurnUpdate {
    Set(Vec<super::p2p::TurnServer>),
    // Preserve topology even when expiry supersedes Set before the backend polls.
    Disable { addresses: Vec<String> },
}
#[derive(Default)]
struct Queues {
    closed: bool,
    turn_update: Option<TurnUpdate>,
    require_routes: bool,
    bindings: BTreeMap<PeerId, RouteOrigin>,
    inbound: Queue,
    outbound: Queue,
}
impl Queues {
    fn key(&self, peer: PeerId) -> Result<SignalKey, TransportError> {
        match self.bindings.get(&peer) {
            Some(route) => Ok(SignalKey::Route(*route)),
            None if !self.require_routes => Ok(SignalKey::Unverified(peer)),
            None => Err(TransportError::ProtocolViolation),
        }
    }
}

/// Clones share only a mailbox. Locks are released before entering native GNS.
/// Default permits unverified local fixtures; new_routed requires route binding.
#[derive(Clone, Default)]
pub struct SignalingEndpoint(Arc<Mutex<Queues>>);
impl SignalingEndpoint {
    #[cfg(any(feature = "rendezvous", test))]
    pub(super) fn install_turn(&self, servers: Vec<super::p2p::TurnServer>) {
        self.0.lock().unwrap().turn_update = Some(TurnUpdate::Set(servers));
    }
    #[cfg(any(feature = "rendezvous", test))]
    pub(super) fn disable_turn(&self, addresses: Vec<String>) {
        self.0.lock().unwrap().turn_update = Some(TurnUpdate::Disable { addresses });
    }
    pub(super) fn take_turn_update(&self) -> Option<TurnUpdate> {
        self.0.lock().unwrap().turn_update.take()
    }
    pub(super) fn routed() -> Self {
        Self(Arc::new(Mutex::new(Queues {
            require_routes: true,
            ..Queues::default()
        })))
    }
    /// Trusted local adapter only: verify the server's route/session/account
    /// before binding. Never take this key from a peer's signaling envelope.
    pub fn authorize_peer(&self, peer: PeerId, origin: RouteOrigin) -> Result<(), TransportError> {
        let mut q = self
            .0
            .lock()
            .map_err(|_| TransportError::ProtocolViolation)?;
        if q.closed {
            return Err(TransportError::NotConnected);
        }
        if !q.require_routes {
            return Err(TransportError::ProtocolViolation);
        }
        if let Some(existing) = q.bindings.get(&peer) {
            return if *existing == origin {
                Ok(())
            } else {
                Err(TransportError::ProtocolViolation)
            };
        }
        if q.bindings.len() >= MAX_CONNECTIONS
            || q.bindings
                .values()
                .filter(|bound| **bound == origin)
                .count()
                >= MAX_PEERS_PER_ROUTE
        {
            return Err(TransportError::Capacity);
        }
        q.bindings.insert(peer, origin);
        Ok(())
    }
    pub fn revoke_peer(&self, peer: PeerId) {
        if let Ok(mut q) = self.0.lock() {
            q.bindings.remove(&peer);
            let now = Instant::now();
            q.inbound.revoke(peer, now);
            q.outbound.revoke(peer, now);
        }
    }
    pub(super) fn origin(&self, peer: PeerId) -> Result<Option<Origin>, TransportError> {
        let q = self
            .0
            .lock()
            .map_err(|_| TransportError::ProtocolViolation)?;
        if q.closed {
            return Err(TransportError::NotConnected);
        }
        Ok(match q.key(peer)? {
            SignalKey::Route(origin) => Some(Origin::Route(origin)),
            SignalKey::Unverified(_) => None,
        })
    }
    pub fn receive(&self, peer: PeerId, payload: &[u8]) -> Result<(), TransportError> {
        let mut q = self
            .0
            .lock()
            .map_err(|_| TransportError::ProtocolViolation)?;
        if q.closed {
            return Err(TransportError::NotConnected);
        }
        let key = q.key(peer)?;
        q.inbound.push(peer, key, payload, Instant::now())
    }
    pub fn pop_outbound(&self) -> Option<OutboundSignal> {
        self.0
            .lock()
            .ok()?
            .outbound
            .pop(Instant::now(), false)
            .map(|(_, s)| s)
    }
    #[cfg(any(feature = "rendezvous", test))]
    pub(super) fn has_pending_inbound(&self, peer: PeerId) -> bool {
        let Ok(q) = self.0.lock() else {
            // An uncertain mailbox must not authorize route revocation.
            return true;
        };
        let Ok(key) = q.key(peer) else {
            return false;
        };
        q.inbound
            .routes
            .get(&key)
            .is_some_and(|route| route.messages.iter().any(|signal| signal.peer == peer))
    }
    pub(super) fn pop_inbound(&self) -> Option<InboundSignal> {
        // Global exhaustion defers signals; the fair cursor survives frames.
        let (key, signal) = self.0.lock().ok()?.inbound.pop(Instant::now(), true)?;
        Some(InboundSignal {
            peer: signal.peer,
            payload: signal.payload,
            origin: match key {
                SignalKey::Unverified(_) => None,
                SignalKey::Route(origin) => Some(Origin::Route(origin)),
            },
        })
    }
    pub(super) fn send(&self, peer: PeerId, payload: &[u8]) -> bool {
        let Ok(mut q) = self.0.lock() else {
            return false;
        };
        if q.closed {
            return false;
        }
        let Ok(key) = q.key(peer) else {
            return false;
        };
        q.outbound.push(peer, key, payload, Instant::now()).is_ok()
    }
    pub(super) fn close(&self) {
        if let Ok(mut q) = self.0.lock() {
            *q = Queues {
                closed: true,
                ..Queues::default()
            };
        }
    }
}

/// Fake rendezvous: bounded registrations, caller-polled opaque byte routing.
#[derive(Default)]
pub struct InMemorySignaling(BTreeMap<PeerId, SignalingEndpoint>);
impl InMemorySignaling {
    pub fn register(
        &mut self,
        peer: PeerId,
        endpoint: SignalingEndpoint,
    ) -> Result<(), TransportError> {
        if self.0.contains_key(&peer) {
            return Err(TransportError::ProtocolViolation);
        }
        if self.0.len() >= MAX_CONNECTIONS {
            return Err(TransportError::Capacity);
        }
        self.0.insert(peer, endpoint);
        Ok(())
    }
    pub fn unregister(&mut self, peer: PeerId) {
        self.0.remove(&peer);
    }
    pub fn poll(&mut self) {
        for (&from, endpoint) in &self.0 {
            for _ in 0..MAX_QUEUED_SIGNALS {
                let Some(signal) = endpoint.pop_outbound() else {
                    break;
                };
                if let Some(target) = self.0.get(&signal.peer) {
                    let _ = target.receive(from, &signal.payload);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
