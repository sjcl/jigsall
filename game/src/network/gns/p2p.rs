//! P2P establishment is independent of DirectIpTransport. Once connected, lanes,
//! admission occupancy, pre-auth barriers and secured messages use Transport.
pub(super) mod native;
use super::{
    signaling::{self, PeerId, SignalingEndpoint},
    token,
};
use crate::network::{
    lifecycle::{
        self, Admission, Bucket, CONNECTING_TIMEOUT, CONNECTION_START_BURST,
        CONNECTION_START_INTERVAL, MAX_BULK_QUEUE_BYTES, MAX_CONNECTING, MAX_CONNECTIONS,
        MAX_PENDING_CONNECTIONS, MAX_RELIABLE_QUEUE_BYTES,
    },
    rate_limit::{
        InboundRateLimiter, RateDecision, DEFAULT_INBOUND_POLICY, PREAUTH_INBOUND_POLICY,
    },
    secure::record_limit,
    transport::*,
    wire,
};
use std::{
    cell::Cell,
    collections::BTreeMap,
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};

/// Native ICE settings copied into listeners and outgoing connects. The default
/// uses private host candidates with no STUN/TURN or other external services.
#[derive(Clone, Debug, Default)]
pub struct IceConfig {
    pub allow_public_candidates: bool,
    pub stun_servers: Vec<String>,
}
/// Short-lived UDP TURN credentials. Debug never includes authentication values.
#[derive(Clone)]
pub struct TurnServer {
    pub address: String,
    pub username: String,
    pub password: String,
}
impl std::fmt::Debug for TurnServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TurnServer")
            .field("address", &self.address)
            .finish_non_exhaustive()
    }
}
struct Connection {
    native: native::Connection,
    peer: PeerId,
    origin: Option<Origin>,
    created: Instant,
    connected: bool,
    authenticated: bool,
    ready: bool,
    limiter: InboundRateLimiter,
    bulk_enqueued: u64,
    bulk_delivered: Cell<u64>,
}
static ACTIVE: AtomicBool = AtomicBool::new(false);
struct Lease;
impl Lease {
    fn acquire() -> Result<Self, TransportError> {
        ACTIVE
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| TransportError::Capacity)?;
        Ok(Self)
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        ACTIVE.store(false, Ordering::Release);
    }
}

/// One P2P endpoint per process (the pinned GNS wrapper has one native identity).
/// All maintenance is caller-owned; callbacks only enqueue opaque signals.
/// Wrap this in SecureTransport before passing it to the password bootstrap.
pub struct GnsP2p {
    connections: BTreeMap<ConnectionId, Connection>,
    #[cfg(feature = "rendezvous")]
    retired_peers: std::collections::BTreeSet<PeerId>,
    listener: native::Listener,
    mailbox: SignalingEndpoint,
    peer: PeerId,
    pending: Vec<TransportEvent>,
    starts: Bucket,
    admission: Admission,
    next_receive: Option<ConnectionId>,
    turn_addresses: Vec<String>,
    pending_turn: Option<Vec<TurnServer>>,
    turn_retry_after: Instant,
    _lease: Lease,
}
impl GnsP2p {
    /// Unverified local foundation fixtures only. Production must use new_routed.
    pub fn new_unverified_for_test(
        local_virtual_port: u16,
        ice: IceConfig,
    ) -> Result<Self, TransportError> {
        Self::with_signaling(local_virtual_port, ice, SignalingEndpoint::default())
    }
    /// Requires explicit authorization of every remote routing identity by a
    /// trusted local rendezvous adapter through signaling().authorize_peer().
    pub fn new_routed(local_virtual_port: u16, ice: IceConfig) -> Result<Self, TransportError> {
        Self::with_signaling(local_virtual_port, ice, SignalingEndpoint::routed())
    }
    fn with_signaling(
        local_virtual_port: u16,
        ice: IceConfig,
        mailbox: SignalingEndpoint,
    ) -> Result<Self, TransportError> {
        let lease = Lease::acquire()?;
        let peer = native::local_peer()?;
        Ok(Self {
            connections: BTreeMap::new(),
            #[cfg(feature = "rendezvous")]
            retired_peers: Default::default(),
            listener: native::Listener::new(local_virtual_port, &ice)?,
            mailbox,
            peer,
            pending: Vec::new(),
            starts: Bucket::new(
                CONNECTION_START_BURST,
                CONNECTION_START_INTERVAL,
                Instant::now(),
            ),
            admission: Admission::connections(),
            next_receive: None,
            turn_addresses: Vec::new(),
            pending_turn: None,
            turn_retry_after: Instant::now(),
            _lease: lease,
        })
    }
    pub fn peer_id(&self) -> PeerId {
        self.peer
    }
    pub fn signaling(&self) -> SignalingEndpoint {
        self.mailbox.clone()
    }
    /// Routing identity only; this never certifies a player or session password.
    pub fn remote_peer(&self, connection: ConnectionId) -> Option<PeerId> {
        self.connections.get(&connection).map(|c| c.peer)
    }
    /// Includes owned native Connecting handles, before any Connected event.
    pub fn has_peer(&self, peer: PeerId) -> bool {
        self.connections
            .values()
            .any(|connection| connection.peer == peer)
    }
    /// Bounded cleanup notifications, including native failures before Connected
    /// and handles admitted and rejected within the same owner poll.
    #[cfg(feature = "rendezvous")]
    pub(crate) fn take_retired_peers(&mut self) -> Vec<PeerId> {
        std::mem::take(&mut self.retired_peers)
            .into_iter()
            .collect()
    }
    /// Advisory native headroom, including Connecting/not-Ready handles.
    /// This does not reserve a handle or consume native admission credits.
    pub fn has_connection_capacity(&self) -> bool {
        self.connections.len() < MAX_CONNECTIONS
            && self.connections.values().filter(|c| !c.connected).count() < MAX_CONNECTING
            && self.connections.values().filter(|c| !c.ready).count() < MAX_PENDING_CONNECTIONS
    }
    /// Queue an outgoing P2P connection, separate from IP address establishment.
    pub fn connect_peer(
        &mut self,
        peer: PeerId,
        remote_virtual_port: u16,
    ) -> Result<ConnectionId, TransportError> {
        self.apply_turn_update()?;
        if peer == self.peer {
            return Err(TransportError::ProtocolViolation);
        }
        let now = Instant::now();
        let origin = self.mailbox.origin(peer)?;
        let pending = self.origin_pending(origin);
        if !self.has_connection_capacity()
            || !self.starts.available(1, now)
            || self.admission.admit(origin, pending, now).is_err()
        {
            return Err(TransportError::Capacity);
        }
        self.starts.take(1, now);
        let id = ConnectionId::new(token()?);
        let native = self
            .listener
            .connect(peer, remote_virtual_port, self.mailbox.clone())?;
        self.insert(id, native, peer, origin);
        Ok(id)
    }
    /// Updates the listener, outgoing options and every owned connection without
    /// issuing new connection IDs or touching SecureTransport/bootstrap state.
    pub fn install_turn(&mut self, servers: &[TurnServer]) -> Result<(), TransportError> {
        let mut addresses: Vec<_> = servers.iter().map(|s| s.address.clone()).collect();
        addresses.sort_unstable();
        if addresses.is_empty()
            || addresses.len() > 4
            || addresses.windows(2).any(|a| a[0] == a[1])
            || (!self.turn_addresses.is_empty() && addresses != self.turn_addresses)
            || (self.turn_addresses.is_empty() && !self.connections.is_empty())
        {
            // Reject topology changes before touching listener or connection config.
            return Err(TransportError::ProtocolViolation);
        }
        self.listener.install_turn(servers)?;
        for connection in self.connections.values() {
            connection.native.install_turn(servers)?;
        }
        self.turn_addresses = addresses;
        Ok(())
    }
    fn apply_turn_update(&mut self) -> Result<(), TransportError> {
        if let Some(servers) = self.mailbox.take_turn_update() {
            self.pending_turn = Some(servers);
            self.turn_retry_after = Instant::now();
        }
        if Instant::now() >= self.turn_retry_after {
            if let Some(servers) = self.pending_turn.take() {
                if self.install_turn(&servers).is_err() {
                    self.pending_turn = Some(servers);
                    self.turn_retry_after = Instant::now() + std::time::Duration::from_secs(1);
                }
            }
        }
        Ok(())
    }
    fn origin_pending(&self, origin: Option<Origin>) -> usize {
        self.connections
            .values()
            .filter(|c| !c.ready && origin.is_some() && c.origin == origin)
            .count()
    }
    fn insert(
        &mut self,
        id: ConnectionId,
        native: native::Connection,
        peer: PeerId,
        origin: Option<Origin>,
    ) {
        let now = Instant::now();
        self.connections.insert(
            id,
            Connection {
                native,
                peer,
                origin,
                created: now,
                connected: false,
                authenticated: false,
                ready: false,
                limiter: InboundRateLimiter::with_policy(&PREAUTH_INBOUND_POLICY, now),
                bulk_enqueued: 0,
                bulk_delivered: Cell::new(0),
            },
        );
    }
    fn terminate(
        &mut self,
        id: ConnectionId,
        reason: DisconnectReason,
        events: &mut Vec<TransportEvent>,
        now: Instant,
    ) {
        if let Some(mut c) = self.connections.remove(&id) {
            #[cfg(feature = "rendezvous")]
            if self.retired_peers.len() < MAX_CONNECTIONS {
                self.retired_peers.insert(c.peer);
            }
            if lifecycle::is_abuse(reason) {
                self.admission.penalize(c.origin, now);
            }
            let code = match reason {
                DisconnectReason::Requested => 1000,
                DisconnectReason::InvalidMessage => 1001,
                DisconnectReason::RateLimited => 1003,
                _ => 1002,
            };
            c.native.close(code);
            events.push(if c.connected {
                TransportEvent::Disconnected {
                    connection: id,
                    reason,
                }
            } else {
                TransportEvent::ConnectionFailed {
                    connection: id,
                    reason,
                }
            });
        }
    }
    fn maintain(&mut self, now: Instant, events: &mut Vec<TransportEvent>) {
        let ids: Vec<_> = self.connections.keys().copied().collect();
        for id in ids {
            let c = self.connections.get_mut(&id).expect("owned connection");
            if c.origin.is_some() && self.mailbox.origin(c.peer).ok() != Some(c.origin) {
                self.terminate(id, DisconnectReason::Requested, events, now);
                continue;
            }
            match c.native.state() {
                native::State::Connected if !c.connected => {
                    c.connected = true;
                    events.push(TransportEvent::Connected { connection: id });
                }
                native::State::Connected => {}
                native::State::Closed => {
                    self.terminate(id, DisconnectReason::RemoteClosed, events, now)
                }
                native::State::Failed => {
                    self.terminate(id, DisconnectReason::ConnectionProblem, events, now)
                }
                native::State::Pending
                    if now.saturating_duration_since(c.created) >= CONNECTING_TIMEOUT =>
                {
                    self.terminate(id, DisconnectReason::BackendConnectionTimeout, events, now)
                }
                native::State::Pending => {}
            }
        }
    }
}
fn lane(class: MessageClass) -> u16 {
    match class {
        MessageClass::Transient => 0,
        MessageClass::Control => 1,
        MessageClass::Bulk => 2,
    }
}
fn class(lane: u16) -> Option<MessageClass> {
    match lane {
        0 => Some(MessageClass::Transient),
        1 => Some(MessageClass::Control),
        2 => Some(MessageClass::Bulk),
        _ => None,
    }
}
impl Transport for GnsP2p {
    fn origin(&self, id: ConnectionId) -> Option<Origin> {
        self.connections.get(&id).and_then(|c| c.origin)
    }
    fn reliable_egress(&self, id: ConnectionId) -> Result<ReliableEgress, TransportError> {
        let c = self
            .connections
            .get(&id)
            .ok_or(TransportError::UnknownConnection)?;
        let (queued_bytes, bulk_queued_bytes) = c.native.egress()?;
        let delivered = c
            .bulk_delivered
            .get()
            .max(c.bulk_enqueued.saturating_sub(bulk_queued_bytes));
        c.bulk_delivered.set(delivered);
        Ok(ReliableEgress {
            queued_bytes,
            bulk_queued_bytes,
            bulk_delivered_bytes: delivered,
        })
    }
    fn activate_secure_channel(&mut self, id: ConnectionId) -> Result<(), TransportError> {
        let c = self
            .connections
            .get_mut(&id)
            .ok_or(TransportError::UnknownConnection)?;
        if !c.connected {
            return Err(TransportError::NotConnected);
        }
        if c.authenticated {
            return Err(TransportError::ProtocolViolation);
        }
        c.native.configure_authenticated_send_rate()?;
        c.authenticated = true;
        c.limiter = InboundRateLimiter::with_policy(&DEFAULT_INBOUND_POLICY, Instant::now());
        Ok(())
    }
    fn mark_ready(&mut self, id: ConnectionId) -> Result<(), TransportError> {
        self.connections
            .get_mut(&id)
            .ok_or(TransportError::UnknownConnection)?
            .ready = true;
        Ok(())
    }
    fn poll(&mut self, events: &mut Vec<TransportEvent>) -> Result<(), TransportError> {
        self.apply_turn_update()?;
        events.append(&mut self.pending);
        let now = Instant::now();
        self.maintain(now, events);
        for _ in 0..signaling::MAX_QUEUED_SIGNALS {
            let Some(signal) = self.mailbox.pop_inbound() else {
                break;
            };
            if signal.peer == self.peer {
                continue;
            }
            let Ok(origin) = self.mailbox.origin(signal.peer) else {
                continue;
            };
            // Do not charge a stale queued envelope to a newly authorized route.
            if origin != signal.origin {
                continue;
            }
            let origin_pending = self.origin_pending(origin);
            // Admission is sampled before GNS; the callback only accepts within
            // that capacity. GNS discards requests for which it returns null.
            // Only new requests spend start credit; stale/duplicate signals do not.
            let allow = self.has_connection_capacity();
            let admission = native::IncomingAdmission {
                allow,
                origin,
                pending: origin_pending,
                gate: &mut self.admission,
                starts: &mut self.starts,
                now,
            };
            if let Some(native) = self.listener.receive(
                signal.peer,
                &signal.payload,
                admission,
                self.mailbox.clone(),
            ) {
                let id = ConnectionId::new(token()?);
                self.insert(id, native, signal.peer, origin);
            }
        }
        super::global()?.poll_callbacks();
        self.maintain(now, events);
        let mut ids: Vec<_> = self.connections.keys().copied().collect();
        if let Some(next) = self.next_receive {
            let start = ids.iter().position(|id| *id >= next).unwrap_or(0);
            ids.rotate_left(start);
        }
        let mut remaining = 512;
        // Round robin, at most one native message per connection each pass.
        // A delivered pre-auth record stops that connection until next frame.
        let mut active = std::collections::VecDeque::from(ids);
        while remaining > 0 {
            let Some(id) = active.pop_front() else {
                break;
            };
            let Some(c) = self.connections.get_mut(&id) else {
                continue;
            };
            if !c.connected {
                continue;
            }
            let message = match c.native.receive() {
                Ok(Some(message)) => message,
                Ok(None) => continue,
                Err(_) => {
                    self.terminate(id, DisconnectReason::BackendFailure, events, now);
                    continue;
                }
            };
            remaining -= 1;
            let Some(class) = class(message.lane()) else {
                self.terminate(id, DisconnectReason::InvalidMessage, events, now);
                continue;
            };
            let payload = match message.payload() {
                Ok(payload) => payload,
                Err(_) => {
                    self.terminate(id, DisconnectReason::InvalidMessage, events, now);
                    continue;
                }
            };
            if payload.len() > record_limit(class)
                || (!c.authenticated
                    && !matches!(wire::is_session_control_for_class(payload, class), Ok(true)))
            {
                self.terminate(id, DisconnectReason::InvalidMessage, events, now);
                continue;
            }
            let mut barrier = false;
            match c.limiter.check(class, payload.len(), now) {
                RateDecision::Allow => {
                    barrier = !c.authenticated;
                    events.push(TransportEvent::Message {
                        connection: id,
                        class,
                        payload: payload.to_vec(),
                    });
                }
                RateDecision::Drop => {}
                RateDecision::Disconnect => {
                    self.terminate(id, DisconnectReason::RateLimited, events, now);
                    continue;
                }
            }
            if !barrier {
                active.push_back(id);
            }
        }
        self.next_receive = active.front().copied();
        Ok(())
    }
    fn send(
        &mut self,
        id: ConnectionId,
        class: MessageClass,
        payload: &[u8],
    ) -> Result<(), TransportError> {
        if payload.len() > record_limit(class) {
            return Err(TransportError::PayloadTooLarge);
        }
        let c = self
            .connections
            .get_mut(&id)
            .ok_or(TransportError::UnknownConnection)?;
        if !c.connected {
            return Err(TransportError::NotConnected);
        }
        if class != MessageClass::Transient {
            let limit = if class == MessageClass::Bulk {
                MAX_BULK_QUEUE_BYTES
            } else {
                MAX_RELIABLE_QUEUE_BYTES
            };
            if c.native
                .egress()?
                .0
                .saturating_add(payload.len() as u64 + 64)
                > limit
            {
                return Err(TransportError::Backpressure);
            }
        }
        c.native.send(lane(class), class, payload)?;
        if class == MessageClass::Bulk {
            c.bulk_enqueued = c.bulk_enqueued.saturating_add(payload.len() as u64);
        }
        Ok(())
    }
    fn close(&mut self, id: ConnectionId, reason: DisconnectReason) -> Result<(), TransportError> {
        if !self.connections.contains_key(&id) {
            return Err(TransportError::UnknownConnection);
        }
        let mut events = Vec::new();
        self.terminate(id, reason, &mut events, Instant::now());
        self.pending.extend(events);
        Ok(())
    }
}
impl Drop for GnsP2p {
    fn drop(&mut self) {
        self.connections.clear();
        // Late native Release owns its Arc, but cannot resurrect this mailbox.
        self.mailbox.close();
    }
}

#[cfg(test)]
mod tests;
