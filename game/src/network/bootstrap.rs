//! Opt-in, frame-driven session bootstrap over SecureTransport. Process every event
//! here BEFORE routing; `Syncing` and `Gameplay` have separate routers.
use super::{
    auth::{ClientHandshake, ServerHandshake, SessionPassword},
    lifecycle::{Admission, Bucket, FairQueue, AUTHENTICATED_HANDOFF_TIMEOUT},
    secure::{ChannelRole, SecureTransport},
    session::{SessionConnectionError, SessionConnections},
    session_control::*,
    syncing::SyncReadyPermit,
    transport::*,
    wire::{self, WireMessage},
};
use jigsall_core::{session::AuthorityCursor, PlayerId};
use std::{
    collections::{BTreeMap, HashSet},
    time::{Duration, Instant},
};

pub const MAX_PENDING_AUTH: usize = 32;
pub const AUTH_TIMEOUT: Duration = Duration::from_secs(10);
pub const AUTH_ATTEMPTS_PER_SECOND: u32 = 4;
pub const AUTH_ATTEMPT_BURST: u32 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionState {
    TransportConnected,
    Authenticating,
    /// Host has installed keys, but still awaits encrypted SecureChannelReady.
    Securing,
    Authenticated,
    Syncing,
    Ready,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootstrapOutcome {
    Consumed,
    Syncing,
    Gameplay,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BootstrapError {
    Rejected(DisconnectReason),
    Transport(TransportError),
    InvalidTransition,
    Registration(SessionConnectionError),
}

struct HostPeer {
    state: ConnectionState,
    origin: Option<Origin>,
    hello: ServerHello,
    started: Instant,
    authenticated_at: Option<Instant>,
    handshake: Option<ServerHandshake>,
}

/// Host identity is supplied by the authority; allocation never examines backend
/// identity or assumes that PlayerId(0) is local. IDs are never reused here.
pub struct HostBootstrap {
    password: SessionPassword,
    metadata: SessionMetadata,
    peers: BTreeMap<ConnectionId, HostPeer>,
    reserved: HashSet<PlayerId>,
    next_player: Option<u64>,
    starts: Bucket,
    admission: Admission,
    waiting: BTreeMap<ConnectionId, (Option<Origin>, Instant)>,
    queue: FairQueue<ConnectionId>,
}
impl HostBootstrap {
    pub fn new(
        password: SessionPassword,
        metadata: SessionMetadata,
        allocated: impl IntoIterator<Item = PlayerId>,
        now: Instant,
    ) -> Self {
        let mut reserved: HashSet<_> = allocated.into_iter().collect();
        reserved.insert(metadata.host);
        Self {
            password,
            metadata,
            peers: BTreeMap::new(),
            reserved,
            next_player: Some(0),
            starts: Bucket::per_second(
                AUTH_ATTEMPT_BURST.into(),
                AUTH_ATTEMPTS_PER_SECOND.into(),
                now,
            ),
            admission: Admission::authentication(),
            waiting: BTreeMap::new(),
            queue: FairQueue::default(),
        }
    }
    /// Future hellos advertise the current cursor; in-flight contexts are immutable.
    pub fn update_cursor(&mut self, cursor: AuthorityCursor) {
        self.metadata.cursor = cursor;
    }
    pub fn state(&self, connection: ConnectionId) -> Option<ConnectionState> {
        self.peers
            .get(&connection)
            .map(|peer| peer.state)
            .or_else(|| {
                self.waiting
                    .contains_key(&connection)
                    .then_some(ConnectionState::TransportConnected)
            })
    }
    pub fn assigned_player(&self, connection: ConnectionId) -> Option<PlayerId> {
        self.peers
            .get(&connection)
            .filter(|p| {
                matches!(
                    p.state,
                    ConnectionState::Authenticated
                        | ConnectionState::Syncing
                        | ConnectionState::Ready
                )
            })
            .map(|p| p.hello.reserved_player)
    }
    pub fn metadata(&self, connection: ConnectionId) -> Option<SessionMetadata> {
        self.assigned_player(connection)?;
        self.peers.get(&connection).map(|p| p.hello.metadata)
    }
    fn allocate(&mut self, connections: &SessionConnections) -> Option<PlayerId> {
        loop {
            let player = PlayerId(self.next_player?);
            self.next_player = player.0.checked_add(1);
            if !self.reserved.contains(&player)
                && !connections.peers().any(|p| p.player == Some(player))
            {
                return Some(player);
            }
        }
    }
    pub(crate) fn reject(
        &mut self,
        connection: ConnectionId,
        reason: DisconnectReason,
        transport: &mut SecureTransport<impl Transport>,
        connections: &mut SessionConnections,
    ) -> BootstrapError {
        self.peers.remove(&connection);
        self.waiting.remove(&connection);
        self.queue.retain(|id, _| *id != connection);
        disconnect_mapping(connections, connection, reason);
        match transport.close(connection, reason) {
            Ok(()) => BootstrapError::Rejected(reason),
            Err(error) => BootstrapError::Transport(error),
        }
    }
    fn reject_at(
        &mut self,
        connection: ConnectionId,
        reason: DisconnectReason,
        transport: &mut SecureTransport<impl Transport>,
        connections: &mut SessionConnections,
        now: Instant,
    ) -> BootstrapError {
        if super::lifecycle::is_abuse(reason) {
            let origin = self
                .peers
                .get(&connection)
                .map(|p| p.origin)
                .or_else(|| self.waiting.get(&connection).map(|p| p.0));
            // Taking owned state in reject makes duplicate notifications harmless.
            if let Some(origin) = origin {
                self.admission.penalize(origin, now);
            }
        }
        self.reject(connection, reason, transport, connections)
    }
    pub fn process(
        &mut self,
        event: &TransportEvent,
        transport: &mut SecureTransport<impl Transport>,
        connections: &mut SessionConnections,
        now: Instant,
    ) -> Result<BootstrapOutcome, BootstrapError> {
        match event {
            TransportEvent::Connected { connection } => {
                if self.peers.contains_key(connection) || self.waiting.contains_key(connection) {
                    // A duplicate native lifecycle token is a backend fault,
                    // not an additional remote password attempt.
                    return Err(self.reject(
                        *connection,
                        DisconnectReason::ProtocolViolation,
                        transport,
                        connections,
                    ));
                }
                // Clear stale gameplay registration even if a faulty backend reuses a token.
                disconnect_mapping(connections, *connection, DisconnectReason::Requested);
                transport.start_connection(*connection);
                connections.observe(event);
                let origin = transport.origin(*connection).map(Origin::normalized);
                self.admission.protect(
                    self.peers
                        .values()
                        .map(|p| p.origin)
                        .chain(self.waiting.values().map(|p| p.0)),
                );
                let pending = self
                    .peers
                    .values()
                    .filter(|p| {
                        p.origin == origin
                            && matches!(
                                p.state,
                                ConnectionState::Authenticating | ConnectionState::Securing
                            )
                    })
                    .count()
                    + self.waiting.values().filter(|p| p.0 == origin).count();
                let refusal = if self.waiting.len()
                    + self
                        .peers
                        .values()
                        .filter(|p| {
                            matches!(
                                p.state,
                                ConnectionState::Authenticating | ConnectionState::Securing
                            )
                        })
                        .count()
                    >= MAX_PENDING_AUTH
                {
                    Some(DisconnectReason::JoinCapacity)
                } else {
                    self.admission.admit(origin, pending, now).err()
                };
                if let Some(reason) = refusal {
                    return Err(self.reject_at(*connection, reason, transport, connections, now));
                }
                if let Err(reason) = self.queue.push(origin, *connection, now) {
                    return Err(self.reject_at(*connection, reason, transport, connections, now));
                }
                self.waiting.insert(*connection, (origin, now));
                let errors = self.pump(transport, connections, now);
                if let Some((_, error)) = errors.into_iter().find(|(id, _)| id == connection) {
                    return Err(error);
                }
                Ok(BootstrapOutcome::Consumed)
            }
            TransportEvent::Disconnected { connection, .. }
            | TransportEvent::ConnectionFailed { connection, .. } => {
                if let Some(reason) = transport.take_local_failure(*connection) {
                    if super::lifecycle::is_abuse(reason) {
                        let origin = self
                            .peers
                            .get(connection)
                            .map(|p| p.origin)
                            .or_else(|| self.waiting.get(connection).map(|p| p.0));
                        if let Some(origin) = origin {
                            self.admission.penalize(origin, now);
                        }
                    }
                }
                self.peers.remove(connection);
                self.waiting.remove(connection);
                self.queue.retain(|id, _| id != connection);
                transport.forget_connection(*connection);
                connections.observe(event);
                Ok(BootstrapOutcome::Consumed)
            }
            TransportEvent::Message {
                connection,
                class,
                payload,
            } => {
                let Some(peer) = self.peers.get_mut(connection) else {
                    return Err(self.reject_at(
                        *connection,
                        DisconnectReason::ProtocolViolation,
                        transport,
                        connections,
                        now,
                    ));
                };
                if matches!(
                    peer.state,
                    ConnectionState::Authenticating | ConnectionState::Securing
                ) && now.saturating_duration_since(peer.started) >= AUTH_TIMEOUT
                {
                    return Err(self.reject_at(
                        *connection,
                        DisconnectReason::AuthenticationTimeout,
                        transport,
                        connections,
                        now,
                    ));
                }
                let Ok(route) = wire::frame_route_for_class(payload, *class) else {
                    return Err(self.reject_at(
                        *connection,
                        DisconnectReason::ProtocolViolation,
                        transport,
                        connections,
                        now,
                    ));
                };
                if peer.state == ConnectionState::Ready && route == wire::FrameRoute::Gameplay {
                    return Ok(BootstrapOutcome::Gameplay);
                }
                if peer.state == ConnectionState::Syncing
                    && route == wire::FrameRoute::Syncing
                    && transport.has_channel(*connection)
                {
                    return Ok(BootstrapOutcome::Syncing);
                }
                if route != wire::FrameRoute::Authentication {
                    return Err(self.reject_at(
                        *connection,
                        DisconnectReason::ProtocolViolation,
                        transport,
                        connections,
                        now,
                    ));
                }
                let Ok(message) = wire::decode_for_class(payload, *class) else {
                    return Err(self.reject_at(
                        *connection,
                        DisconnectReason::ProtocolViolation,
                        transport,
                        connections,
                        now,
                    ));
                };
                if peer.state == ConnectionState::Securing
                    && message
                        == WireMessage::SessionControl(SessionControlMessage::SecureChannelReady)
                {
                    peer.state = ConnectionState::Authenticated;
                    peer.authenticated_at = Some(now);
                    return Ok(BootstrapOutcome::Consumed);
                }
                let WireMessage::SessionControl(SessionControlMessage::ClientProof(proof)) =
                    message
                else {
                    return Err(self.reject_at(
                        *connection,
                        DisconnectReason::ProtocolViolation,
                        transport,
                        connections,
                        now,
                    ));
                };
                let Some(handshake) = peer.handshake.take() else {
                    return Err(self.reject_at(
                        *connection,
                        DisconnectReason::ProtocolViolation,
                        transport,
                        connections,
                        now,
                    ));
                };
                let Ok((confirmation, secret)) = handshake.finish(proof) else {
                    return Err(self.reject_at(
                        *connection,
                        DisconnectReason::AuthenticationFailed,
                        transport,
                        connections,
                        now,
                    ));
                };
                self.admission.succeed(peer.origin);
                let accepted = AuthAccepted {
                    player: peer.hello.reserved_player,
                    confirmation,
                };
                if let Err(error) = send_control(
                    transport,
                    *connection,
                    SessionControlMessage::AuthAccepted(accepted),
                ) {
                    let _ = self.reject_at(
                        *connection,
                        DisconnectReason::BackendFailure,
                        transport,
                        connections,
                        now,
                    );
                    return Err(error);
                }
                // AuthAccepted must be queued in plaintext before installation.
                if let Err(error) = transport.install(*connection, secret, ChannelRole::Host) {
                    let _ = self.reject_at(
                        *connection,
                        DisconnectReason::BackendFailure,
                        transport,
                        connections,
                        now,
                    );
                    return Err(BootstrapError::Transport(error));
                }
                self.peers.get_mut(connection).expect("live peer").state =
                    ConnectionState::Securing;
                Ok(BootstrapOutcome::Consumed)
            }
        }
    }
    fn pump(
        &mut self,
        transport: &mut SecureTransport<impl Transport>,
        connections: &mut SessionConnections,
        now: Instant,
    ) -> Vec<(ConnectionId, BootstrapError)> {
        let mut errors = Vec::new();
        while self.queue.len() != 0 && self.starts.take(1, now) {
            let connection = self.queue.pop().expect("waiting item");
            let (origin, _) = self
                .waiting
                .remove(&connection)
                .expect("waiting connection");
            if let Err(error) = self.start_auth(connection, origin, transport, connections, now) {
                errors.push((connection, error));
            }
        }
        errors
    }
    fn start_auth(
        &mut self,
        connection: ConnectionId,
        origin: Option<Origin>,
        transport: &mut SecureTransport<impl Transport>,
        connections: &mut SessionConnections,
        now: Instant,
    ) -> Result<(), BootstrapError> {
        let Some(player) = self.allocate(connections) else {
            return Err(self.reject_at(
                connection,
                DisconnectReason::BackendFailure,
                transport,
                connections,
                now,
            ));
        };
        let Ok((handshake, hello)) = ServerHandshake::start(&self.password, self.metadata, player)
        else {
            return Err(self.reject_at(
                connection,
                DisconnectReason::BackendFailure,
                transport,
                connections,
                now,
            ));
        };
        self.peers.insert(
            connection,
            HostPeer {
                state: ConnectionState::TransportConnected,
                origin,
                hello,
                started: now,
                authenticated_at: None,
                handshake: Some(handshake),
            },
        );
        if let Err(error) = send_control(
            transport,
            connection,
            SessionControlMessage::ServerHello(hello),
        ) {
            let _ = self.reject_at(
                connection,
                DisconnectReason::BackendFailure,
                transport,
                connections,
                now,
            );
            return Err(error);
        }
        self.peers.get_mut(&connection).expect("inserted").state = ConnectionState::Authenticating;
        Ok(())
    }
    /// Nonblocking timer maintenance; no crypto or piece work for stable peers.
    pub fn expire(
        &mut self,
        transport: &mut SecureTransport<impl Transport>,
        connections: &mut SessionConnections,
        now: Instant,
    ) -> Vec<(ConnectionId, BootstrapError)> {
        let expired: Vec<_> = self
            .peers
            .iter()
            .filter_map(|(&id, p)| {
                if matches!(
                    p.state,
                    ConnectionState::TransportConnected
                        | ConnectionState::Authenticating
                        | ConnectionState::Securing
                ) && now.saturating_duration_since(p.started) >= AUTH_TIMEOUT
                {
                    Some((id, DisconnectReason::AuthenticationTimeout))
                } else if p.state == ConnectionState::Authenticated
                    && p.authenticated_at.is_some_and(|at| {
                        now.saturating_duration_since(at) >= AUTHENTICATED_HANDOFF_TIMEOUT
                    })
                {
                    Some((id, DisconnectReason::AuthenticatedHandoffTimeout))
                } else {
                    None
                }
            })
            .collect();
        let mut errors = Vec::new();
        for (id, reason) in expired {
            errors.push((id, self.reject_at(id, reason, transport, connections, now)));
        }
        let expired_waiting: Vec<_> = self
            .waiting
            .iter()
            .filter(|(_, (_, at))| now.saturating_duration_since(*at) >= AUTH_TIMEOUT)
            .map(|(&id, _)| id)
            .collect();
        for id in expired_waiting {
            errors.push((
                id,
                self.reject_at(
                    id,
                    DisconnectReason::HostCapacityTimeout,
                    transport,
                    connections,
                    now,
                ),
            ));
        }
        errors.extend(self.pump(transport, connections, now));
        errors
    }
    pub fn begin_sync(&mut self, connection: ConnectionId) -> Result<(), BootstrapError> {
        let peer = self
            .peers
            .get_mut(&connection)
            .ok_or(BootstrapError::InvalidTransition)?;
        if peer.state != ConnectionState::Authenticated {
            return Err(BootstrapError::InvalidTransition);
        }
        peer.state = ConnectionState::Syncing;
        Ok(())
    }
    /// Trusted snapshot-sync coordinator calls this only after sync completes.
    /// Repeated promotion is an error, never an implicit registration/reassignment.
    pub fn promote_ready(
        &mut self,
        connection: ConnectionId,
        connections: &mut SessionConnections,
        permit: &SyncReadyPermit,
    ) -> Result<(), BootstrapError> {
        let peer = self
            .peers
            .get_mut(&connection)
            .ok_or(BootstrapError::InvalidTransition)?;
        if peer.state != ConnectionState::Syncing
            || !permit.matches(connection, peer.hello.metadata, peer.hello.reserved_player)
        {
            return Err(BootstrapError::InvalidTransition);
        }
        connections
            .assign_player(connection, peer.hello.reserved_player)
            .map_err(BootstrapError::Registration)?;
        peer.state = ConnectionState::Ready;
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn promote_ready_for_test(
        &mut self,
        connection: ConnectionId,
        connections: &mut SessionConnections,
    ) -> Result<(), BootstrapError> {
        let peer = self
            .peers
            .get(&connection)
            .ok_or(BootstrapError::InvalidTransition)?;
        let permit =
            SyncReadyPermit::fixture(connection, peer.hello.metadata, peer.hello.reserved_player);
        self.promote_ready(connection, connections, &permit)
    }
}

/// One designated host per client bootstrap; authenticated metadata needs no snapshot.
pub struct ClientBootstrap {
    password: Option<SessionPassword>,
    host_connection: ConnectionId,
    state: Option<ConnectionState>,
    started: Option<Instant>,
    metadata: Option<SessionMetadata>,
    player: Option<PlayerId>,
    handshake: Option<ClientHandshake>,
    failure: Option<DisconnectReason>,
}
impl ClientBootstrap {
    pub fn new(password: SessionPassword, host_connection: ConnectionId) -> Self {
        Self {
            password: Some(password),
            host_connection,
            state: None,
            started: None,
            metadata: None,
            player: None,
            handshake: None,
            failure: None,
        }
    }
    pub fn state(&self) -> Option<ConnectionState> {
        self.state
    }
    pub fn host_connection(&self) -> ConnectionId {
        self.host_connection
    }
    pub fn assigned_player(&self) -> Option<PlayerId> {
        self.player
    }
    pub fn metadata(&self) -> Option<SessionMetadata> {
        self.player.and(self.metadata)
    }
    pub fn failure(&self) -> Option<DisconnectReason> {
        self.failure
    }
    #[cfg(test)]
    pub(crate) fn retains_password(&self) -> bool {
        self.password.is_some()
    }
    fn clear(&mut self) {
        self.state = None;
        self.started = None;
        self.metadata = None;
        self.player = None;
        self.handshake = None;
    }
    pub(crate) fn reject(
        &mut self,
        reason: DisconnectReason,
        transport: &mut SecureTransport<impl Transport>,
        connections: &mut SessionConnections,
    ) -> BootstrapError {
        self.clear();
        self.failure = Some(reason);
        disconnect_mapping(connections, self.host_connection, reason);
        match transport.close(self.host_connection, reason) {
            Ok(()) => BootstrapError::Rejected(reason),
            Err(error) => BootstrapError::Transport(error),
        }
    }
    pub fn process(
        &mut self,
        event: &TransportEvent,
        transport: &mut SecureTransport<impl Transport>,
        connections: &mut SessionConnections,
        now: Instant,
    ) -> Result<BootstrapOutcome, BootstrapError> {
        let connection = match event {
            TransportEvent::Connected { connection }
            | TransportEvent::Disconnected { connection, .. }
            | TransportEvent::ConnectionFailed { connection, .. }
            | TransportEvent::Message { connection, .. } => *connection,
        };
        if connection != self.host_connection {
            disconnect_mapping(connections, connection, DisconnectReason::ProtocolViolation);
            transport
                .close(connection, DisconnectReason::ProtocolViolation)
                .map_err(BootstrapError::Transport)?;
            return Err(BootstrapError::Rejected(
                DisconnectReason::ProtocolViolation,
            ));
        }
        match event {
            TransportEvent::Connected { .. } => {
                if self.state.is_some() {
                    return Err(self.reject(
                        DisconnectReason::ProtocolViolation,
                        transport,
                        connections,
                    ));
                }
                self.clear();
                self.failure = None;
                disconnect_mapping(connections, connection, DisconnectReason::Requested);
                transport.start_connection(connection);
                connections.observe(event);
                self.state = Some(ConnectionState::TransportConnected);
                self.started = Some(now);
                Ok(BootstrapOutcome::Consumed)
            }
            TransportEvent::Disconnected { reason, .. }
            | TransportEvent::ConnectionFailed { reason, .. } => {
                if self.failure.is_none() {
                    self.failure = Some(
                        if self.state == Some(ConnectionState::Authenticating)
                            && *reason == DisconnectReason::RemoteClosed
                        {
                            DisconnectReason::AuthenticationFailed
                        } else {
                            *reason
                        },
                    );
                }
                self.clear();
                transport.forget_connection(connection);
                connections.observe(event);
                Ok(BootstrapOutcome::Consumed)
            }
            TransportEvent::Message { class, payload, .. } => {
                if let Some(reason) = self.timeout_reason(now) {
                    return Err(self.reject(reason, transport, connections));
                }
                let Ok(route) = wire::frame_route_for_class(payload, *class) else {
                    return Err(self.reject(
                        DisconnectReason::ProtocolViolation,
                        transport,
                        connections,
                    ));
                };
                if self.state == Some(ConnectionState::Ready) && route == wire::FrameRoute::Gameplay
                {
                    return Ok(BootstrapOutcome::Gameplay);
                }
                // Host registration precedes ReadyCommit on Control. Live
                // Transient broadcasts may overtake that commit on another lane.
                // Drop presentation until commit; Reliable gameplay still rejects.
                if self.state == Some(ConnectionState::Syncing)
                    && route == wire::FrameRoute::Gameplay
                    && *class == MessageClass::Transient
                {
                    return Ok(BootstrapOutcome::Consumed);
                }
                if self.state == Some(ConnectionState::Syncing)
                    && route == wire::FrameRoute::Syncing
                    && transport.has_channel(connection)
                {
                    return Ok(BootstrapOutcome::Syncing);
                }
                if route != wire::FrameRoute::Authentication {
                    return Err(self.reject(
                        DisconnectReason::ProtocolViolation,
                        transport,
                        connections,
                    ));
                }
                let Ok(message) = wire::decode_for_class(payload, *class) else {
                    return Err(self.reject(
                        DisconnectReason::ProtocolViolation,
                        transport,
                        connections,
                    ));
                };
                match (self.state, message) {
                    (
                        Some(ConnectionState::TransportConnected),
                        WireMessage::SessionControl(SessionControlMessage::ServerHello(hello)),
                    ) => {
                        let Some(password) = self.password.as_ref() else {
                            return Err(self.reject(
                                DisconnectReason::ProtocolViolation,
                                transport,
                                connections,
                            ));
                        };
                        let Ok((handshake, proof)) = ClientHandshake::start(password, &hello)
                        else {
                            return Err(self.reject(
                                DisconnectReason::AuthenticationFailed,
                                transport,
                                connections,
                            ));
                        };
                        self.metadata = Some(hello.metadata);
                        self.handshake = Some(handshake);
                        if let Err(error) = send_control(
                            transport,
                            connection,
                            SessionControlMessage::ClientProof(proof),
                        ) {
                            let _ = self.reject(
                                DisconnectReason::BackendFailure,
                                transport,
                                connections,
                            );
                            return Err(error);
                        }
                        self.state = Some(ConnectionState::Authenticating);
                        Ok(BootstrapOutcome::Consumed)
                    }
                    (
                        Some(ConnectionState::Authenticating),
                        WireMessage::SessionControl(SessionControlMessage::AuthAccepted(accepted)),
                    ) => {
                        let result = self
                            .handshake
                            .take()
                            .ok_or(())
                            .and_then(|h| h.finish(accepted).map_err(|_| ()));
                        let Ok((player, secret)) = result else {
                            return Err(self.reject(
                                DisconnectReason::AuthenticationFailed,
                                transport,
                                connections,
                            ));
                        };
                        if let Err(error) =
                            transport.install(connection, secret, ChannelRole::Client)
                        {
                            let _ = self.reject(
                                DisconnectReason::ProtocolViolation,
                                transport,
                                connections,
                            );
                            return Err(BootstrapError::Transport(error));
                        }
                        self.state = Some(ConnectionState::Securing);
                        if let Err(error) = send_control(
                            transport,
                            connection,
                            SessionControlMessage::SecureChannelReady,
                        ) {
                            let _ = self.reject(
                                DisconnectReason::BackendFailure,
                                transport,
                                connections,
                            );
                            return Err(error);
                        }
                        // No reconnect/password reuse after establishment: a new
                        // client bootstrap requires fresh out-of-band provisioning.
                        self.password.take();
                        self.player = Some(player);
                        self.state = Some(ConnectionState::Authenticated);
                        self.started = Some(now); // Fresh handoff clock, not the authentication start.
                        Ok(BootstrapOutcome::Consumed)
                    }
                    _ => Err(self.reject(
                        DisconnectReason::ProtocolViolation,
                        transport,
                        connections,
                    )),
                }
            }
        }
    }
    fn timeout_reason(&self, now: Instant) -> Option<DisconnectReason> {
        let (limit, reason) = match self.state? {
            ConnectionState::Authenticated => (
                AUTHENTICATED_HANDOFF_TIMEOUT,
                DisconnectReason::AuthenticatedHandoffTimeout,
            ),
            ConnectionState::TransportConnected
            | ConnectionState::Authenticating
            | ConnectionState::Securing => (AUTH_TIMEOUT, DisconnectReason::AuthenticationTimeout),
            ConnectionState::Syncing | ConnectionState::Ready => return None,
        };
        self.started
            .filter(|t| now.saturating_duration_since(*t) >= limit)
            .map(|_| reason)
    }
    pub fn expire(
        &mut self,
        transport: &mut SecureTransport<impl Transport>,
        connections: &mut SessionConnections,
        now: Instant,
    ) -> Result<(), BootstrapError> {
        if let Some(reason) = self.timeout_reason(now) {
            return Err(self.reject(reason, transport, connections));
        }
        Ok(())
    }
    pub fn begin_sync(&mut self) -> Result<(), BootstrapError> {
        if self.state != Some(ConnectionState::Authenticated) {
            return Err(BootstrapError::InvalidTransition);
        }
        self.state = Some(ConnectionState::Syncing);
        Ok(())
    }
    pub fn promote_ready(
        &mut self,
        connections: &mut SessionConnections,
        permit: &SyncReadyPermit,
    ) -> Result<(), BootstrapError> {
        if self.state != Some(ConnectionState::Syncing)
            || !self
                .metadata
                .zip(self.player)
                .is_some_and(|(metadata, player)| {
                    permit.matches(self.host_connection, metadata, player)
                })
        {
            return Err(BootstrapError::InvalidTransition);
        }
        connections
            .assign_player(
                self.host_connection,
                self.metadata.expect("authenticated metadata").host,
            )
            .map_err(BootstrapError::Registration)?;
        self.state = Some(ConnectionState::Ready);
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn promote_ready_for_test(
        &mut self,
        connections: &mut SessionConnections,
    ) -> Result<(), BootstrapError> {
        let permit = SyncReadyPermit::fixture(
            self.host_connection,
            self.metadata.ok_or(BootstrapError::InvalidTransition)?,
            self.player.ok_or(BootstrapError::InvalidTransition)?,
        );
        self.promote_ready(connections, &permit)
    }
}

fn disconnect_mapping(
    connections: &mut SessionConnections,
    connection: ConnectionId,
    reason: DisconnectReason,
) {
    connections.observe(&TransportEvent::Disconnected { connection, reason });
}
fn send_control(
    transport: &mut dyn Transport,
    connection: ConnectionId,
    control: SessionControlMessage,
) -> Result<(), BootstrapError> {
    let bytes = wire::encode(&WireMessage::SessionControl(control))
        .map_err(|_| BootstrapError::Rejected(DisconnectReason::ProtocolViolation))?;
    transport
        .send(connection, MessageClass::Control, &bytes)
        .map_err(BootstrapError::Transport)
}
