//! Opt-in, frame-driven session bootstrap over SecureTransport. Process every event
//! here BEFORE routing; `Syncing` and `Gameplay` have separate routers.
use super::{
    auth::{ClientHandshake, ServerHandshake, SessionPassword},
    secure::{ChannelRole, SecureTransport},
    session::{SessionConnectionError, SessionConnections},
    session_control::*,
    syncing::SyncReadyPermit,
    transport::*,
    wire::{self, WireMessage},
};
use puzzella_core::{session::AuthorityCursor, PlayerId};
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

/// Integer token bucket; shared by all host connections, with explicit test time.
struct AttemptBucket {
    credit: u128,
    updated: Instant,
}
impl AttemptBucket {
    const UNIT: u128 = 1_000_000_000;
    fn new(now: Instant) -> Self {
        Self {
            credit: AUTH_ATTEMPT_BURST as u128 * Self::UNIT,
            updated: now,
        }
    }
    fn refill(&mut self, now: Instant) {
        let elapsed = now.saturating_duration_since(self.updated).as_nanos();
        self.updated = self.updated.max(now);
        self.credit = self
            .credit
            .saturating_add(elapsed.saturating_mul(AUTH_ATTEMPTS_PER_SECOND as u128))
            .min(AUTH_ATTEMPT_BURST as u128 * Self::UNIT);
    }
    fn available(&mut self, now: Instant) -> bool {
        self.refill(now);
        self.credit >= Self::UNIT
    }
    fn take(&mut self, now: Instant) -> bool {
        if !self.available(now) {
            return false;
        }
        self.credit -= Self::UNIT;
        true
    }
}

struct HostPeer {
    state: ConnectionState,
    hello: ServerHello,
    started: Instant,
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
    starts: AttemptBucket,
    failures: AttemptBucket,
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
            starts: AttemptBucket::new(now),
            failures: AttemptBucket::new(now),
        }
    }
    /// Future hellos advertise the current cursor; in-flight contexts are immutable.
    pub fn update_cursor(&mut self, cursor: AuthorityCursor) {
        self.metadata.cursor = cursor;
    }
    pub fn state(&self, connection: ConnectionId) -> Option<ConnectionState> {
        self.peers.get(&connection).map(|peer| peer.state)
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
        disconnect_mapping(connections, connection, reason);
        match transport.close(connection, reason) {
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
        match event {
            TransportEvent::Connected { connection } => {
                if self.peers.contains_key(connection) {
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
                if self
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
                    || !self.failures.available(now)
                    || !self.starts.take(now)
                {
                    return Err(self.reject(
                        *connection,
                        DisconnectReason::RateLimited,
                        transport,
                        connections,
                    ));
                }
                let Some(player) = self.allocate(connections) else {
                    return Err(self.reject(
                        *connection,
                        DisconnectReason::BackendFailure,
                        transport,
                        connections,
                    ));
                };
                let Ok((handshake, hello)) =
                    ServerHandshake::start(&self.password, self.metadata, player)
                else {
                    return Err(self.reject(
                        *connection,
                        DisconnectReason::AuthenticationFailed,
                        transport,
                        connections,
                    ));
                };
                self.peers.insert(
                    *connection,
                    HostPeer {
                        state: ConnectionState::TransportConnected,
                        hello,
                        started: now,
                        handshake: Some(handshake),
                    },
                );
                if let Err(error) = send_control(
                    transport,
                    *connection,
                    SessionControlMessage::ServerHello(hello),
                ) {
                    let _ = self.reject(
                        *connection,
                        DisconnectReason::BackendFailure,
                        transport,
                        connections,
                    );
                    return Err(error);
                }
                self.peers.get_mut(connection).expect("inserted").state =
                    ConnectionState::Authenticating;
                Ok(BootstrapOutcome::Consumed)
            }
            TransportEvent::Disconnected { connection, .. }
            | TransportEvent::ConnectionFailed { connection, .. } => {
                self.peers.remove(connection);
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
                    return Err(self.reject(
                        *connection,
                        DisconnectReason::ProtocolViolation,
                        transport,
                        connections,
                    ));
                };
                if matches!(
                    peer.state,
                    ConnectionState::Authenticating | ConnectionState::Securing
                ) && now.saturating_duration_since(peer.started) >= AUTH_TIMEOUT
                {
                    return Err(self.reject(
                        *connection,
                        DisconnectReason::AuthenticationTimeout,
                        transport,
                        connections,
                    ));
                }
                let Ok(route) = wire::frame_route_for_class(payload, *class) else {
                    return Err(self.reject(
                        *connection,
                        DisconnectReason::ProtocolViolation,
                        transport,
                        connections,
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
                    return Err(self.reject(
                        *connection,
                        DisconnectReason::ProtocolViolation,
                        transport,
                        connections,
                    ));
                }
                let Ok(message) = wire::decode_for_class(payload, *class) else {
                    return Err(self.reject(
                        *connection,
                        DisconnectReason::ProtocolViolation,
                        transport,
                        connections,
                    ));
                };
                if peer.state == ConnectionState::Securing
                    && message
                        == WireMessage::SessionControl(SessionControlMessage::SecureChannelReady)
                {
                    peer.state = ConnectionState::Authenticated;
                    return Ok(BootstrapOutcome::Consumed);
                }
                let WireMessage::SessionControl(SessionControlMessage::ClientProof(proof)) =
                    message
                else {
                    return Err(self.reject(
                        *connection,
                        DisconnectReason::ProtocolViolation,
                        transport,
                        connections,
                    ));
                };
                let Some(handshake) = peer.handshake.take() else {
                    return Err(self.reject(
                        *connection,
                        DisconnectReason::ProtocolViolation,
                        transport,
                        connections,
                    ));
                };
                let Ok((confirmation, secret)) = handshake.finish(proof) else {
                    self.failures.take(now);
                    return Err(self.reject(
                        *connection,
                        DisconnectReason::AuthenticationFailed,
                        transport,
                        connections,
                    ));
                };
                let accepted = AuthAccepted {
                    player: peer.hello.reserved_player,
                    confirmation,
                };
                if let Err(error) = send_control(
                    transport,
                    *connection,
                    SessionControlMessage::AuthAccepted(accepted),
                ) {
                    let _ = self.reject(
                        *connection,
                        DisconnectReason::BackendFailure,
                        transport,
                        connections,
                    );
                    return Err(error);
                }
                // AuthAccepted must be queued in plaintext before installation.
                if let Err(error) = transport.install(*connection, secret, ChannelRole::Host) {
                    let _ = self.reject(
                        *connection,
                        DisconnectReason::ProtocolViolation,
                        transport,
                        connections,
                    );
                    return Err(BootstrapError::Transport(error));
                }
                self.peers.get_mut(connection).expect("live peer").state =
                    ConnectionState::Securing;
                Ok(BootstrapOutcome::Consumed)
            }
        }
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
            .filter(|(_, p)| {
                matches!(
                    p.state,
                    ConnectionState::Authenticating | ConnectionState::Securing
                ) && now.saturating_duration_since(p.started) >= AUTH_TIMEOUT
            })
            .map(|(&id, _)| id)
            .collect();
        expired
            .into_iter()
            .map(|id| {
                (
                    id,
                    self.reject(
                        id,
                        DisconnectReason::AuthenticationTimeout,
                        transport,
                        connections,
                    ),
                )
            })
            .collect()
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
                if self.timed_out(now) {
                    return Err(self.reject(
                        DisconnectReason::AuthenticationTimeout,
                        transport,
                        connections,
                    ));
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
    fn timed_out(&self, now: Instant) -> bool {
        matches!(
            self.state,
            Some(
                ConnectionState::TransportConnected
                    | ConnectionState::Authenticating
                    | ConnectionState::Securing
            )
        ) && self
            .started
            .is_some_and(|t| now.saturating_duration_since(t) >= AUTH_TIMEOUT)
    }
    pub fn expire(
        &mut self,
        transport: &mut SecureTransport<impl Transport>,
        connections: &mut SessionConnections,
        now: Instant,
    ) -> Result<(), BootstrapError> {
        if self.timed_out(now) {
            return Err(self.reject(
                DisconnectReason::AuthenticationTimeout,
                transport,
                connections,
            ));
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
