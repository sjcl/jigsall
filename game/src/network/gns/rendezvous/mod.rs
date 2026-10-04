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
}
pub struct RendezvousAdapter {
    io: worker::Worker,
    signaling: SignalingEndpoint,
    local: PeerId,
    authority: Option<AuthorityId>,
    phase: Phase,
    routes: BTreeMap<PeerId, Bound>,
    deferred: Option<ClientMessage>,
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
            deferred: None,
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
        self.routes.remove(&peer);
        self.signaling.revoke_peer(peer);
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
            || self.routes.len() >= MAX_ROUTES
            || self.routes.contains_key(&id)
            || self
                .routes
                .values()
                .any(|r| r.member == member || pending.is_some() && r.pending == pending)
        {
            return Err(RendezvousError::ProtocolViolation);
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
        }
        self.deferred = None;
        self.phase = Phase::Closed;
    }
    fn handle(
        &mut self,
        message: ServerMessage,
        events: &mut Vec<RendezvousEvent>,
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
                self.bind(peer(peer_id), member_id, room, Some(join_id))?;
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
                let r = self
                    .routes
                    .get_mut(&id)
                    .ok_or(RendezvousError::ProtocolViolation)?;
                if r.active {
                    r.available = false;
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
    /// Call each frame alongside (before) the existing GNS/SecureTransport poll.
    /// Work and returned events per call are bounded; I/O stays on the worker.
    pub fn poll(&mut self) -> Vec<RendezvousEvent> {
        let mut events = Vec::new();
        let result = self.poll_inner(&mut events);
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
        events
    }
    fn poll_inner(&mut self, events: &mut Vec<RendezvousEvent>) -> Result<(), RendezvousError> {
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
            self.handle(message, events)?;
            if !self.flush_deferred()? || matches!(self.phase, Phase::Closed) {
                return Ok(());
            }
        }
        for _ in 0..CHANNEL_CAPACITY {
            let Some(signal) = self.signaling.pop_outbound() else {
                break;
            };
            if !self
                .routes
                .get(&signal.peer)
                .is_some_and(|r| r.active && r.available)
            {
                continue;
            }
            if signal.payload.is_empty() || signal.payload.len() > MAX_SIGNAL_BYTES {
                return Err(RendezvousError::ProtocolViolation);
            }
            self.deferred = Some(ClientMessage::Signal {
                to_peer_id: protocol::PeerId(signal.peer.to_bytes()),
                payload_base64: protocol::encode_signal(&signal.payload),
            });
            if !self.flush_deferred()? {
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
