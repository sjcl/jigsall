//! P2P establishment is independent of DirectIpTransport. Once connected, lanes,
//! admission occupancy, pre-auth barriers and secured messages use Transport.
pub(super) mod native;
use super::{
    inbound::{check_message, class, InboundDecision, MAX_RECEIVE_PER_POLL},
    outbound,
    policy::{close_code, lane, termination_event},
    signaling::{self, PeerId, SignalingEndpoint, TurnUpdate},
    token, ConnectionState,
};
use crate::network::{
    lifecycle::{
        self, Admission, Bucket, CONNECTING_TIMEOUT, CONNECTION_START_BURST,
        CONNECTION_START_INTERVAL,
    },
    rate_limit::DEFAULT_INBOUND_POLICY,
    transport::*,
};
use std::{
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
    state: ConnectionState,
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
    pending_turn: Option<TurnUpdate>,
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
        super::has_connection_capacity(self.connections.values().map(|c| &c.state))
    }
    /// Queue an outgoing P2P connection, separate from IP address establishment.
    pub fn connect_peer(
        &mut self,
        peer: PeerId,
        remote_virtual_port: u16,
    ) -> Result<ConnectionId, TransportError> {
        self.apply_turn_update()?;
        if self.pending_turn.is_some() {
            return Err(TransportError::Backpressure);
        }
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
    /// Updates listener inheritance and future outgoing connection options.
    /// Existing ICE sessions retain the credentials used to create allocations.
    pub fn install_turn(&mut self, servers: &[TurnServer]) -> Result<(), TransportError> {
        let mut addresses: Vec<_> = servers.iter().map(|s| s.address.clone()).collect();
        addresses.sort_unstable();
        self.validate_turn_addresses(&addresses)?;
        self.listener.install_turn(servers)?;
        self.turn_addresses = addresses;
        Ok(())
    }
    fn validate_turn_addresses(&self, addresses: &[String]) -> Result<(), TransportError> {
        if addresses.is_empty()
            || addresses.len() > 4
            || addresses.windows(2).any(|a| a[0] >= a[1])
            || (!self.turn_addresses.is_empty() && addresses != self.turn_addresses)
            || (self.turn_addresses.is_empty() && !self.connections.is_empty())
        {
            // Reject topology changes before touching listener or connection config.
            return Err(TransportError::ProtocolViolation);
        }
        Ok(())
    }
    fn disable_turn(&mut self, addresses: &[String]) -> Result<(), TransportError> {
        self.validate_turn_addresses(addresses)?;
        self.listener.disable_turn()?;
        self.turn_addresses = addresses.to_vec();
        Ok(())
    }
    fn apply_turn_update(&mut self) -> Result<(), TransportError> {
        if let Some(update) = self.mailbox.take_turn_update() {
            self.pending_turn = Some(update);
            self.turn_retry_after = Instant::now();
        }
        if Instant::now() >= self.turn_retry_after {
            if let Some(update) = self.pending_turn.take() {
                let result = match &update {
                    TurnUpdate::Set(servers) => self.install_turn(servers),
                    // Keep the fixed endpoint set for recovery; active handles are untouched.
                    TurnUpdate::Disable { addresses } => self.disable_turn(addresses),
                };
                if result.is_err() {
                    self.pending_turn = Some(update);
                    self.turn_retry_after = Instant::now() + std::time::Duration::from_secs(1);
                }
            }
        }
        Ok(())
    }
    fn origin_pending(&self, origin: Option<Origin>) -> usize {
        self.connections
            .values()
            .filter(|c| !c.state.ready && origin.is_some() && c.origin == origin)
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
                state: ConnectionState::new(now),
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
            if self.retired_peers.len() < lifecycle::MAX_CONNECTIONS {
                self.retired_peers.insert(c.peer);
            }
            if lifecycle::is_abuse(reason) {
                self.admission.penalize(c.origin, now);
            }
            c.native.close(close_code(reason) as i32);
            events.push(termination_event(id, c.state.connected, reason));
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
                native::State::Connected => {
                    if c.state.mark_connected() {
                        events.push(TransportEvent::Connected { connection: id });
                    }
                }
                native::State::Closed => {
                    self.terminate(id, DisconnectReason::RemoteClosed, events, now)
                }
                native::State::Failed => {
                    self.terminate(id, DisconnectReason::ConnectionProblem, events, now)
                }
                native::State::Pending
                    if now.saturating_duration_since(c.state.created) >= CONNECTING_TIMEOUT =>
                {
                    self.terminate(id, DisconnectReason::BackendConnectionTimeout, events, now)
                }
                native::State::Pending => {}
            }
        }
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
        Ok(c.state
            .bulk_delivery
            .egress(queued_bytes, bulk_queued_bytes))
    }
    fn activate_secure_channel(&mut self, id: ConnectionId) -> Result<(), TransportError> {
        let c = self
            .connections
            .get_mut(&id)
            .ok_or(TransportError::UnknownConnection)?;
        c.state
            .activate_secure_channel(&DEFAULT_INBOUND_POLICY, || {
                c.native.configure_authenticated_send_rate()
            })
    }
    fn mark_ready(&mut self, id: ConnectionId) -> Result<(), TransportError> {
        self.connections
            .get_mut(&id)
            .ok_or(TransportError::UnknownConnection)?
            .state
            .mark_ready();
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
            let allow = self.pending_turn.is_none() && self.has_connection_capacity();
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
        let mut remaining = MAX_RECEIVE_PER_POLL;
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
            if !c.state.connected {
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
            let mut barrier = false;
            match check_message(
                class,
                payload,
                c.state.authenticated,
                &mut c.state.limiter,
                now,
            ) {
                InboundDecision::Allow => {
                    barrier = !c.state.authenticated;
                    events.push(TransportEvent::Message {
                        connection: id,
                        class,
                        payload: payload.to_vec(),
                    });
                }
                InboundDecision::Drop => {}
                InboundDecision::Disconnect(reason) => {
                    self.terminate(id, reason, events, now);
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
        outbound::check_payload_size(class, payload.len())?;
        let c = self
            .connections
            .get_mut(&id)
            .ok_or(TransportError::UnknownConnection)?;
        if !c.state.connected {
            return Err(TransportError::NotConnected);
        }
        if let Some(limit) = outbound::queue_limit(class) {
            outbound::check_queue(c.native.egress()?.0, payload.len(), limit)?;
        }
        c.native.send(lane(class), class, payload)?;
        if class == MessageClass::Bulk {
            c.state.bulk_delivery.record_sent(payload.len());
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
pub(super) mod tests;
