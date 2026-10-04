//! Caller-polled, anonymous rendezvous v1 control plane. This never starts
//! password authentication, allocates PlayerId, or advances Sync/Ready.
pub mod protocol;
mod worker;
use super::{
    signaling::{PeerId, SignalingEndpoint, MAX_SIGNAL_BYTES},
    GnsP2p, IceConfig,
};
use crate::network::transport::{RouteOrigin, TransportError};
use protocol::{AuthorityId, ClientMessage, JoinId, MemberId, RoomCode, RoomId, ServerMessage};
use std::{collections::BTreeMap, fmt, net::IpAddr};

pub use super::P2P_VIRTUAL_PORT;
pub const CHANNEL_CAPACITY: usize = 32;
pub const MAX_ROUTES: usize = 64;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RendezvousError {
    InvalidEndpoint,
    InvalidState,
    Backpressure,
    ProtocolViolation,
    Transport(TransportError),
    Network,
    Timeout,
    Requested,
}
impl fmt::Display for RendezvousError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for RendezvousError {}
impl From<TransportError> for RendezvousError {
    fn from(e: TransportError) -> Self {
        Self::Transport(e)
    }
}

/// Validated before any I/O. Production trusts WebPKI certificate verification;
/// there is no certificate-validation bypass or remote plaintext option.
#[derive(Clone, Debug)]
pub struct EndpointUrl(String);
impl EndpointUrl {
    pub fn production(url: &str) -> Result<Self, RendezvousError> {
        Self::validate(url, false)
    }
    /// Explicit loopback-only integration fixture path. DNS names are forbidden.
    pub fn loopback_for_test(url: &str) -> Result<Self, RendezvousError> {
        Self::validate(url, true)
    }
    fn validate(text: &str, local: bool) -> Result<Self, RendezvousError> {
        let url = url::Url::parse(text).map_err(|_| RendezvousError::InvalidEndpoint)?;
        let loopback = match url.host() {
            Some(url::Host::Ipv4(ip)) => IpAddr::V4(ip).is_loopback(),
            Some(url::Host::Ipv6(ip)) => IpAddr::V6(ip).is_loopback(),
            _ => false,
        };
        if text.len() > 2048
            || url.host().is_none()
            || url.path() != "/v1/ws"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || !(url.scheme() == "wss" || (local && url.scheme() == "ws" && loopback))
        {
            return Err(RendezvousError::InvalidEndpoint);
        }
        Ok(Self(url.into()))
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RendezvousEvent {
    Welcome {
        authority_id: AuthorityId,
    },
    RoomCreated {
        room_id: RoomId,
        room_code: RoomCode,
        self_member_id: MemberId,
    },
    /// The host route is installed. The caller may now connect_peer, then wrap
    /// the backend in SecureTransport and run the existing password bootstrap.
    HostReady {
        peer_id: PeerId,
        room_id: RoomId,
        self_member_id: MemberId,
    },
    PeerJoined {
        peer_id: PeerId,
        member_id: MemberId,
    },
    /// Signaling unavailable only; the caller decides established/pending cleanup.
    PeerUnavailable {
        peer_id: PeerId,
    },
    /// The bounded per-route GNS mailbox shed this signal. Other routes and
    /// established gameplay connections continue; GNS may retry signaling.
    SignalBackpressure {
        peer_id: PeerId,
    },
    RoomClosed,
    ServerError(protocol::ErrorCode),
    Disconnected(RendezvousError),
}
#[derive(Clone, Copy)]
struct RoomBinding {
    room: RoomId,
    member: MemberId,
}
#[derive(Clone, Copy)]
enum Phase {
    Welcome,
    Idle,
    Creating,
    Joining,
    Host(RoomBinding),
    Joined(RoomBinding),
    Closed,
}
struct Bound {
    member: MemberId,
    pending: Option<JoinId>,
    active: bool,
    available: bool,
    confirmed: bool,
    revoking: bool,
    lifecycle: Option<ClientMessage>,
}
pub struct RendezvousAdapter {
    io: worker::Worker,
    signaling: SignalingEndpoint,
    local: PeerId,
    authority: Option<AuthorityId>,
    phase: Phase,
    routes: BTreeMap<PeerId, Bound>,
    // Server room capacity/FIFO bounds outstanding rejected transactions at 64.
    rejected: BTreeMap<PeerId, (JoinId, MemberId)>,
    deferred: Option<ClientMessage>,
    deferred_signal: Option<ClientMessage>,
    terminal_reported: bool,
}
fn peer(id: protocol::PeerId) -> PeerId {
    PeerId::from_bytes(id.0)
}
impl RendezvousAdapter {
    /// Constructs the backend itself so an unverified GnsP2p cannot be supplied.
    /// The returned backend keeps its existing caller-owned poll model.
    pub fn new(
        endpoint: EndpointUrl,
        local_virtual_port: u16,
        ice: IceConfig,
    ) -> Result<(GnsP2p, Self), RendezvousError> {
        let backend = GnsP2p::new_routed(local_virtual_port, ice)?;
        let adapter = Self::from_routed(endpoint, backend.peer_id(), backend.signaling())?;
        Ok((backend, adapter))
    }
    fn from_routed(
        endpoint: EndpointUrl,
        local: PeerId,
        signaling: SignalingEndpoint,
    ) -> Result<Self, RendezvousError> {
        Ok(Self {
            io: worker::Worker::start(endpoint)?,
            signaling,
            local,
            authority: None,
            phase: Phase::Welcome,
            routes: BTreeMap::new(),
            rejected: BTreeMap::new(),
            deferred: None,
            deferred_signal: None,
            terminal_reported: false,
        })
    }
    pub fn create_room(&mut self) -> Result<(), RendezvousError> {
        if !matches!(self.phase, Phase::Idle) {
            return Err(RendezvousError::InvalidState);
        }
        self.io.send(ClientMessage::CreateRoom {
            peer_id: protocol::PeerId(self.local.to_bytes()),
        })?;
        self.phase = Phase::Creating;
        Ok(())
    }
    pub fn join_room(&mut self, room_code: RoomCode) -> Result<(), RendezvousError> {
        if !matches!(self.phase, Phase::Idle) {
            return Err(RendezvousError::InvalidState);
        }
        self.io.send(ClientMessage::JoinRoom {
            room_code,
            peer_id: protocol::PeerId(self.local.to_bytes()),
        })?;
        self.phase = Phase::Joining;
        Ok(())
    }
    pub fn leave_room(&mut self) -> Result<(), RendezvousError> {
        if !matches!(
            self.phase,
            Phase::Host(_) | Phase::Joined(_) | Phase::Joining
        ) {
            return Err(RendezvousError::InvalidState);
        }
        if self.deferred.is_some() {
            return Err(RendezvousError::Backpressure);
        }
        self.io.send(ClientMessage::LeaveRoom {})
    }
    /// Explicit owner decision after GNS connection cleanup/security revocation.
    /// This can terminate a live routed GNS connection; never call merely because
    /// PeerUnavailable, RoomClosed, or Disconnected was observed.
    pub fn release_route(&mut self, peer: PeerId) {
        self.discard_deferred_signal(peer);
        self.routes.remove(&peer);
        self.signaling.revoke_peer(peer);
    }
    /// Called only after the game owner's SPAKE2 authenticated gate.
    pub fn confirm_peer(&mut self, peer: PeerId) {
        if !matches!(self.phase, Phase::Host(_)) {
            return;
        }
        if let Some(route) = self.routes.get_mut(&peer) {
            if route.active && route.available && !route.confirmed && !route.revoking {
                route.lifecycle = Some(ClientMessage::ConfirmPeer {
                    peer_id: protocol::PeerId(peer.to_bytes()),
                    member_id: route.member,
                });
            }
        }
    }
    /// Retain the bounded notification and routing tombstone until the server
    /// reports PeerUnavailable. Late in-flight signals cannot reopen admission.
    pub fn revoke_peer(&mut self, peer: PeerId) {
        self.discard_deferred_signal(peer);
        if !matches!(self.phase, Phase::Host(_)) {
            return;
        }
        if let Some(route) = self.routes.get_mut(&peer) {
            if route.active && route.available && !route.revoking {
                route.revoking = true;
                route.lifecycle = Some(ClientMessage::RevokePeer {
                    peer_id: protocol::PeerId(peer.to_bytes()),
                    member_id: route.member,
                });
                self.signaling.revoke_peer(peer);
            }
        }
    }
    pub(crate) fn has_pending_signal(&self, peer: PeerId) -> bool {
        self.signaling.has_pending_inbound(peer)
    }
    /// The route table is the bounded unavailable-peer set. Native ownership,
    /// including Connecting handles, belongs to the caller. Queued inbound
    /// signals keep their route until native admission has processed them.
    pub fn reclaim_unavailable_routes(&mut self, has_peer: impl Fn(PeerId) -> bool) {
        let unused: Vec<_> = self
            .routes
            .iter()
            .filter_map(|(&peer, route)| {
                (!route.available && !has_peer(peer) && !self.has_pending_signal(peer))
                    .then_some(peer)
            })
            .collect();
        for peer in unused {
            self.release_route(peer);
        }
    }
    pub fn shutdown(&mut self) {
        self.control_lost();
        self.io.shutdown();
    }
    pub fn worker_finished(&self) -> bool {
        self.io.finished()
    }
    pub fn room_id(&self) -> Option<RoomId> {
        match self.phase {
            Phase::Host(room) | Phase::Joined(room) => Some(room.room),
            _ => None,
        }
    }
    fn bind(
        &mut self,
        id: PeerId,
        member: MemberId,
        room: RoomBinding,
        pending: Option<JoinId>,
    ) -> Result<(), RendezvousError> {
        if id == self.local
            || member == room.member
            || self.routes.contains_key(&id)
            || self
                .routes
                .values()
                .any(|r| r.member == member || pending.is_some() && r.pending == pending)
        {
            return Err(RendezvousError::ProtocolViolation);
        }
        if self.routes.len() >= MAX_ROUTES {
            return Err(TransportError::Capacity.into());
        }
        // `account` in RouteOrigin is a server-issued anonymous membership, not
        // Steam/account authentication or Sybil resistance. Future authenticated
        // account support must replace this binding via a versioned protocol.
        let origin = RouteOrigin::from_authenticated_route(
            self.authority.ok_or(RendezvousError::ProtocolViolation)?.0,
            room.room.0,
            member.0,
        );
        self.signaling.authorize_peer(id, origin)?;
        self.routes.insert(
            id,
            Bound {
                member,
                pending,
                active: pending.is_none(),
                available: true,
                confirmed: false,
                revoking: false,
                lifecycle: None,
            },
        );
        Ok(())
    }
    fn control_lost(&mut self) {
        let pending = self
            .routes
            .iter()
            .filter_map(|(&id, r)| (!r.active).then_some(id))
            .collect::<Vec<_>>();
        for id in pending {
            self.release_route(id);
        }
        for bound in self.routes.values_mut() {
            bound.available = false;
            bound.lifecycle = None;
        }
        self.deferred = None;
        self.deferred_signal = None;
        self.rejected.clear();
        self.phase = Phase::Closed;
    }
    #[cfg(test)]
    fn handle(
        &mut self,
        message: ServerMessage,
        events: &mut Vec<RendezvousEvent>,
    ) -> Result<(), RendezvousError> {
        self.handle_with_admission(message, events, true)
    }
    fn handle_with_admission(
        &mut self,
        message: ServerMessage,
        events: &mut Vec<RendezvousEvent>,
        native_capacity: bool,
    ) -> Result<(), RendezvousError> {
        match message {
            ServerMessage::Welcome { authority_id } if matches!(self.phase, Phase::Welcome) => {
                self.authority = Some(authority_id);
                self.phase = Phase::Idle;
                events.push(RendezvousEvent::Welcome { authority_id });
            }
            ServerMessage::RoomCreated {
                room_id,
                room_code,
                self_member_id,
            } if matches!(self.phase, Phase::Creating) => {
                self.phase = Phase::Host(RoomBinding {
                    room: room_id,
                    member: self_member_id,
                });
                events.push(RendezvousEvent::RoomCreated {
                    room_id,
                    room_code,
                    self_member_id,
                });
            }
            ServerMessage::AuthorizePeer {
                join_id,
                peer_id,
                member_id,
            } => {
                let Phase::Host(room) = self.phase else {
                    return Err(RendezvousError::ProtocolViolation);
                };
                let id = peer(peer_id);
                // Identity contradictions remain fatal, including at capacity.
                if id == self.local
                    || member_id == room.member
                    || self.rejected.contains_key(&id)
                    || self
                        .routes
                        .values()
                        .any(|r| r.member == member_id || r.pending == Some(join_id))
                    || self
                        .rejected
                        .values()
                        .any(|&(join, member)| join == join_id || member == member_id)
                    || self
                        .routes
                        .get(&id)
                        .is_some_and(|r| r.available && !r.revoking)
                {
                    return Err(RendezvousError::ProtocolViolation);
                }
                let shortage = self.routes.contains_key(&id)
                    || !native_capacity
                    || self.routes.len() >= MAX_ROUTES;
                let bind = if shortage {
                    Err(TransportError::Capacity.into())
                } else {
                    self.bind(id, member_id, room, Some(join_id))
                };
                if matches!(
                    bind,
                    Err(RendezvousError::Transport(TransportError::Capacity))
                ) {
                    if self.rejected.len() >= MAX_ROUTES {
                        return Err(RendezvousError::ProtocolViolation);
                    }
                    self.rejected.insert(id, (join_id, member_id));
                    self.deferred = Some(ClientMessage::AuthorizeReject { join_id });
                    return Ok(());
                }
                bind?;
                // Install route BEFORE enqueueing ACK. One retained command is
                // sufficient to retry bounded channel backpressure on next poll.
                self.deferred = Some(ClientMessage::AuthorizeAck { join_id });
            }
            ServerMessage::RoomJoined {
                room_id,
                self_member_id,
                host_peer_id,
                host_member_id,
            } if matches!(self.phase, Phase::Joining) => {
                let room = RoomBinding {
                    room: room_id,
                    member: self_member_id,
                };
                self.bind(peer(host_peer_id), host_member_id, room, None)?;
                self.phase = Phase::Joined(room);
                events.push(RendezvousEvent::HostReady {
                    peer_id: peer(host_peer_id),
                    room_id,
                    self_member_id,
                });
            }
            ServerMessage::PeerJoined { peer_id, member_id }
                if matches!(self.phase, Phase::Host(_)) =>
            {
                let r = self
                    .routes
                    .get_mut(&peer(peer_id))
                    .ok_or(RendezvousError::ProtocolViolation)?;
                if r.member != member_id || r.pending.is_none() || !r.available {
                    return Err(RendezvousError::ProtocolViolation);
                }
                r.pending = None;
                r.active = true;
                events.push(RendezvousEvent::PeerJoined {
                    peer_id: peer(peer_id),
                    member_id,
                });
            }
            ServerMessage::PeerUnavailable { peer_id }
                if matches!(self.phase, Phase::Host(_) | Phase::Joined(_)) =>
            {
                let id = peer(peer_id);
                // This completion belongs to the rejected pending incarnation,
                // not to an older native route with this same PeerId. Server FIFO
                // places it before any subsequent authorization reusing the id.
                if self.rejected.remove(&id).is_some() {
                    return Ok(());
                }
                self.discard_deferred_signal(id);
                let r = self
                    .routes
                    .get_mut(&id)
                    .ok_or(RendezvousError::ProtocolViolation)?;
                if r.active {
                    r.available = false;
                    r.lifecycle = None;
                } else {
                    self.release_route(id);
                }
                events.push(RendezvousEvent::PeerUnavailable { peer_id: id });
            }
            ServerMessage::Signal {
                from_peer_id,
                payload_base64,
            } => {
                let id = peer(from_peer_id);
                // Protocol v1 guarantees PeerJoined (host) / RoomJoined (joiner)
                // before any Signal on this same FIFO control socket. A pending
                // route must never be promoted implicitly by an early signal.
                if !self
                    .routes
                    .get(&id)
                    .is_some_and(|r| r.active && r.available)
                {
                    return Err(RendezvousError::ProtocolViolation);
                }
                let bytes = protocol::decode_signal(&payload_base64)
                    .map_err(|_| RendezvousError::ProtocolViolation)?;
                if self.routes[&id].revoking {
                    return Ok(());
                }
                match self.signaling.receive(id, &bytes) {
                    Ok(()) => {}
                    Err(TransportError::Backpressure) => {
                        events.push(RendezvousEvent::SignalBackpressure { peer_id: id })
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            ServerMessage::RoomClosed {} => {
                self.control_lost();
                self.io.shutdown();
                events.push(RendezvousEvent::RoomClosed);
            }
            ServerMessage::Error { code } => {
                if matches!(self.phase, Phase::Creating | Phase::Joining) {
                    self.phase = Phase::Idle;
                }
                events.push(RendezvousEvent::ServerError(code));
            }
            _ => return Err(RendezvousError::ProtocolViolation),
        }
        Ok(())
    }
    fn flush_deferred(&mut self) -> Result<bool, RendezvousError> {
        let Some(message) = self.deferred.take() else {
            return Ok(true);
        };
        match self.io.send_owned(message) {
            Ok(()) => Ok(true),
            Err((RendezvousError::Backpressure, message)) => {
                self.deferred = Some(message);
                Ok(false)
            }
            Err((error, _)) => Err(error),
        }
    }
    fn discard_deferred_signal(&mut self, peer: PeerId) {
        if matches!(&self.deferred_signal, Some(ClientMessage::Signal { to_peer_id, .. }) if to_peer_id.0 == peer.to_bytes())
        {
            self.deferred_signal = None;
        }
    }
    fn flush_signal(&mut self) -> Result<bool, RendezvousError> {
        let Some(message) = self.deferred_signal.take() else {
            return Ok(true);
        };
        let ClientMessage::Signal { to_peer_id, .. } = &message else {
            return Err(RendezvousError::ProtocolViolation);
        };
        if !self
            .routes
            .get(&peer(*to_peer_id))
            .is_some_and(|r| r.active && r.available && !r.revoking)
        {
            return Ok(true);
        }
        match self.io.send_owned(message) {
            Ok(()) => Ok(true),
            Err((RendezvousError::Backpressure, message)) => {
                self.deferred_signal = Some(message);
                Ok(false)
            }
            Err((error, _)) => Err(error),
        }
    }
    fn flush_lifecycle(&mut self) -> Result<bool, RendezvousError> {
        for revoke in [true, false] {
            for route in self.routes.values_mut() {
                if !route
                    .lifecycle
                    .as_ref()
                    .is_some_and(|m| matches!(m, ClientMessage::RevokePeer { .. }) == revoke)
                {
                    continue;
                }
                let Some(message) = route.lifecycle.take() else {
                    continue;
                };
                let confirm = matches!(message, ClientMessage::ConfirmPeer { .. });
                match self.io.send_owned(message) {
                    Ok(()) => {
                        route.confirmed |= confirm;
                    }
                    Err((RendezvousError::Backpressure, message)) => {
                        route.lifecycle = Some(message);
                        return Ok(false);
                    }
                    Err((error, _)) => return Err(error),
                }
            }
        }
        Ok(true)
    }
    /// Call each frame alongside (before) the existing GNS/SecureTransport poll.
    /// Work and returned events per call are bounded; I/O stays on the worker.
    pub fn poll(&mut self) -> Vec<RendezvousEvent> {
        self.poll_with_owner(None, true)
    }
    /// Production owner reconciliation also runs between control messages, so
    /// join/leave churn cannot exhaust bind capacity within one poll batch.
    pub fn poll_with_peer_connections(
        &mut self,
        has_peer: impl Fn(PeerId) -> bool,
    ) -> Vec<RendezvousEvent> {
        self.poll_with_owner(Some(&has_peer), true)
    }
    /// Native headroom is an advisory snapshot; GNS still admits each request
    /// independently at its existing native boundary.
    pub fn poll_with_admission(
        &mut self,
        has_peer: impl Fn(PeerId) -> bool,
        native_capacity: bool,
    ) -> Vec<RendezvousEvent> {
        self.poll_with_owner(Some(&has_peer), native_capacity)
    }
    fn poll_with_owner(
        &mut self,
        has_peer: Option<&dyn Fn(PeerId) -> bool>,
        native_capacity: bool,
    ) -> Vec<RendezvousEvent> {
        let mut events = Vec::new();
        self.reconcile_announced_peers(has_peer, &events);
        let result = self.poll_inner(&mut events, has_peer, native_capacity);
        if let Err(error) = result {
            self.io.shutdown();
            self.control_lost();
            if !self.terminal_reported {
                events.push(RendezvousEvent::Disconnected(error));
                self.terminal_reported = true;
            }
        }
        if !self.terminal_reported {
            if let Some(error) = self.io.terminal() {
                self.control_lost();
                events.push(RendezvousEvent::Disconnected(error));
                self.terminal_reported = true;
            }
        }
        self.reconcile_announced_peers(has_peer, &events);
        events
    }
    fn reconcile_announced_peers(
        &mut self,
        has_peer: Option<&dyn Fn(PeerId) -> bool>,
        events: &[RendezvousEvent],
    ) {
        if let Some(has_peer) = has_peer {
            // HostReady must retain its installed route through event delivery:
            // the caller cannot connect_peer until this poll returns, even when
            // RoomClosed/Disconnected follows RoomJoined in the same batch.
            self.reclaim_unavailable_routes(|peer| {
                has_peer(peer)
                    || events.iter().any(|event| {
                        matches!(event, RendezvousEvent::HostReady { peer_id, .. } if *peer_id == peer)
                    })
            });
        }
    }
    fn poll_inner(
        &mut self,
        events: &mut Vec<RendezvousEvent>,
        has_peer: Option<&dyn Fn(PeerId) -> bool>,
        native_capacity: bool,
    ) -> Result<(), RendezvousError> {
        if matches!(self.phase, Phase::Closed) {
            for _ in 0..CHANNEL_CAPACITY {
                if self.io.pop().is_none() {
                    break;
                }
            }
            return Ok(());
        }
        if !self.flush_deferred()? {
            return Ok(());
        }
        for _ in 0..CHANNEL_CAPACITY {
            let Some(message) = self.io.pop() else {
                break;
            };
            self.handle_with_admission(message, events, native_capacity)?;
            self.reconcile_announced_peers(has_peer, events);
            if !self.flush_deferred()? || matches!(self.phase, Phase::Closed) {
                return Ok(());
            }
        }
        if !self.flush_lifecycle()? || !self.flush_signal()? {
            return Ok(());
        }
        for _ in 0..CHANNEL_CAPACITY {
            let Some(signal) = self.signaling.pop_outbound() else {
                break;
            };
            if !self
                .routes
                .get(&signal.peer)
                .is_some_and(|r| r.active && r.available && !r.revoking)
            {
                continue;
            }
            if signal.payload.is_empty() || signal.payload.len() > MAX_SIGNAL_BYTES {
                return Err(RendezvousError::ProtocolViolation);
            }
            self.deferred_signal = Some(ClientMessage::Signal {
                to_peer_id: protocol::PeerId(signal.peer.to_bytes()),
                payload_base64: protocol::encode_signal(&signal.payload),
            });
            if !self.flush_signal()? {
                break;
            }
        }
        Ok(())
    }
}

impl Drop for RendezvousAdapter {
    fn drop(&mut self) {
        // Pending authorization cannot survive its control owner. Established
        // routes remain installed for the independently owned GNS backend.
        self.control_lost();
        self.io.shutdown();
    }
}

#[cfg(test)]
mod tests;
