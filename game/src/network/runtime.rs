//! One owner for Direct-IP bootstrap, sync, gameplay, and connection teardown.
//! The CPU store is borrowed from the game World; no parallel gameplay state exists.
mod bridge;
mod presentation;
mod world;
pub use bridge::BridgeError;
pub(crate) use world::offline as world_offline;
pub use world::{
    host_with_transport, join_with_transport, stop_session, NetworkRuntimePlugin, NetworkSession,
};
#[cfg(feature = "gns")]
pub use world::{start_host, start_join};

use super::{
    auth::SessionPassword,
    bootstrap::{BootstrapOutcome, ClientBootstrap, ConnectionState, HostBootstrap},
    client::ClientRouter,
    host::{HostRouteOutcome, HostRouter},
    secure::SecureTransport,
    session::SessionConnections,
    session_control::SessionMetadata,
    syncing::{
        ClientSyncOutcome, ClientSyncRouter, HostSyncCoordinator, SyncAuthority, SyncHost,
        SyncPhase, SyncReplica,
    },
    transport::*,
    wire::{self, WireMessage},
};
use crate::{
    interaction::PieceInteraction,
    multiplayer::{
        protocol::{HostCommandOutcome, ProtocolDragContexts},
        replication::PeerReplicationState,
    },
    resources::{ImageDecodeLimits, PieceDataStore},
};
use bevy::prelude::*;
use bridge::CommandBridge;
use puzzella_core::{protocol::*, session::*, ClientCommand, PlayerId, PuzzleDefinition};
#[cfg(test)]
use std::time::Duration;
use std::{collections::BTreeSet, net::SocketAddr, sync::Arc, time::Instant};

pub struct HostOptions {
    pub address: SocketAddr,
    pub session: SessionDefinition,
    pub host: PlayerId,
    pub password: SessionPassword,
}
pub struct JoinOptions {
    pub address: SocketAddr,
    pub password: SessionPassword,
    pub cached_image: Option<Arc<[u8]>>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeRole {
    Host,
    Client,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimePhase {
    Offline,
    Hosting,
    Connecting,
    Authenticating,
    Syncing(SyncPhase),
    Ready,
    Disconnected,
    Failed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageReadiness {
    Unavailable,
    Decoding,
    Decoded,
    Uploading,
    Ready,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostImageSource {
    Available,
    Unavailable,
}
#[derive(Clone, Debug)]
pub struct RuntimePeer {
    pub connection: ConnectionId,
    pub player: Option<PlayerId>,
    pub state: Option<ConnectionState>,
}
/// UI reads this resource and uses the start/stop APIs, never bootstrap or sockets.
#[derive(Resource, Clone, Debug)]
pub struct NetworkStatus {
    pub role: Option<RuntimeRole>,
    pub phase: RuntimePhase,
    pub local_player: Option<PlayerId>,
    pub host: Option<PlayerId>,
    pub address: Option<SocketAddr>,
    pub image: ImageReadiness,
    pub image_source: HostImageSource,
    pub peers: Vec<RuntimePeer>,
    pub error: Option<String>,
}
impl Default for NetworkStatus {
    fn default() -> Self {
        Self {
            role: None,
            phase: RuntimePhase::Offline,
            local_player: None,
            host: None,
            address: None,
            image: ImageReadiness::Unavailable,
            image_source: HostImageSource::Unavailable,
            peers: Vec::new(),
            error: None,
        }
    }
}
#[derive(Debug, PartialEq, Eq)]
pub enum RuntimeStartError {
    AlreadyActive,
    DefinitionUnavailable,
    InvalidWorld,
    ImageHashMismatch,
    Transport(TransportError),
}
impl std::fmt::Display for RuntimeStartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for RuntimeStartError {}

struct HostState {
    bootstrap: HostBootstrap,
    sync: HostSyncCoordinator,
    contexts: ProtocolDragContexts,
    joining: BTreeSet<ConnectionId>,
    next_pump: Option<ConnectionId>,
}
struct ClientState {
    bootstrap: ClientBootstrap,
    sync: Option<ClientSyncRouter>,
    replica: PeerReplicationState,
    cached_image: Option<Arc<[u8]>>,
    image_limits: ImageDecodeLimits,
}
enum Role {
    Host(Box<HostState>),
    Client(Box<ClientState>),
}
struct DecodedImage {
    image: Image,
    logical_size: UVec2,
    encoded: Arc<[u8]>,
}
struct Runtime<T> {
    transport: SecureTransport<T>,
    listener: Option<ListenerId>,
    connections: SessionConnections,
    live: BTreeSet<ConnectionId>,
    role: Role,
    session: Option<AuthoritySession>,
    definition: Option<PuzzleDefinition>,
    image: Option<Arc<[u8]>>,
    decode: Option<crossbeam::channel::Receiver<Result<DecodedImage, String>>>,
    decoded: Option<DecodedImage>,
    bridge: CommandBridge,
    presentation: presentation::RemotePresentationBridge,
    status: NetworkStatus,
    baseline_installed: bool,
    render_installed: bool,
    active: bool,
}

impl<T: DirectIpTransport> Runtime<T> {
    fn host(
        backend: T,
        options: HostOptions,
        definition: PuzzleDefinition,
        image: Option<Arc<[u8]>>,
        now: Instant,
    ) -> Result<Self, RuntimeStartError> {
        let verified_image = image
            .as_ref()
            .map(|bytes| super::bulk::VerifiedPuzzleImage::verify(bytes.clone(), options.session))
            .transpose()
            .map_err(|_| RuntimeStartError::ImageHashMismatch)?;
        let mut sync = HostSyncCoordinator::default();
        if let Some(image) = verified_image {
            sync.set_image(image);
        }
        let mut transport = SecureTransport::new(backend);
        let listener = transport
            .listen(options.address)
            .map_err(RuntimeStartError::Transport)?;
        let address = transport
            .listener_address(listener)
            .map_err(RuntimeStartError::Transport)?;
        let metadata = SessionMetadata {
            definition: options.session,
            host: options.host,
            cursor: AuthorityCursor::new(0, 0),
        };
        let image_source = if image.is_some() {
            HostImageSource::Available
        } else {
            HostImageSource::Unavailable
        };
        Ok(Self {
            transport,
            listener: Some(listener),
            connections: Default::default(),
            live: Default::default(),
            role: Role::Host(Box::new(HostState {
                bootstrap: HostBootstrap::new(options.password, metadata, [], now),
                sync,
                contexts: Default::default(),
                joining: Default::default(),
                next_pump: None,
            })),
            session: Some(AuthoritySession::new(
                options.session,
                options.host,
                metadata.cursor,
            )),
            definition: Some(definition),
            image,
            decode: None,
            decoded: None,
            bridge: Default::default(),
            presentation: Default::default(),
            baseline_installed: true,
            render_installed: true,
            active: true,
            status: NetworkStatus {
                role: Some(RuntimeRole::Host),
                phase: RuntimePhase::Hosting,
                local_player: Some(options.host),
                host: Some(options.host),
                address: Some(address),
                image_source,
                ..default()
            },
        })
    }
    fn client(
        backend: T,
        options: JoinOptions,
        image_limits: ImageDecodeLimits,
    ) -> Result<Self, RuntimeStartError> {
        let mut transport = SecureTransport::new(backend);
        let connection = transport
            .connect(options.address)
            .map_err(RuntimeStartError::Transport)?;
        Ok(Self {
            transport,
            listener: None,
            connections: Default::default(),
            live: BTreeSet::from([connection]),
            role: Role::Client(Box::new(ClientState {
                bootstrap: ClientBootstrap::new(options.password, connection),
                sync: None,
                replica: Default::default(),
                cached_image: options.cached_image,
                image_limits,
            })),
            session: None,
            definition: None,
            image: None,
            decode: None,
            decoded: None,
            bridge: Default::default(),
            presentation: Default::default(),
            baseline_installed: false,
            render_installed: false,
            active: true,
            status: NetworkStatus {
                role: Some(RuntimeRole::Client),
                phase: RuntimePhase::Connecting,
                address: Some(options.address),
                ..default()
            },
        })
    }
    fn fail(&mut self, error: impl std::fmt::Debug) {
        self.status.error = Some(format!("{error:?}"));
        self.status.phase = RuntimePhase::Failed;
        self.active = false;
    }
    fn start_decode(&mut self, bytes: impl FnOnce() -> Result<Arc<[u8]>, String> + Send + 'static) {
        let Role::Client(client) = &self.role else {
            unreachable!()
        };
        let limits = client.image_limits;
        let (tx, rx) = crossbeam::channel::bounded(1);
        self.decode = Some(rx);
        self.status.image = ImageReadiness::Decoding;
        std::thread::spawn(move || {
            let result = bytes().and_then(|encoded| {
                crate::asset_reader::decode_image_bytes(&encoded, limits).map(|decoded| {
                    DecodedImage {
                        image: decoded.image,
                        logical_size: decoded.logical_size,
                        encoded,
                    }
                })
            });
            let _ = tx.send(result);
        });
    }
    fn publish(
        &mut self,
        source: Option<ConnectionId>,
        outcome: &HostCommandOutcome,
        store: &mut PieceDataStore,
    ) -> Result<(), String> {
        if let Some(event) = &outcome.authority_event {
            let Role::Host(host) = &self.role else {
                unreachable!()
            };
            let drag = host.contexts.active_drag(
                self.session.as_ref().unwrap(),
                store,
                presentation::event_player(&event.event),
            );
            self.presentation
                .event(self.status.local_player.unwrap(), &event.event, drag, store);
        }
        if let Some(update) = &outcome.drag_update {
            self.presentation.delta(update.player, update.delta);
        }
        let failures = {
            let Role::Host(host) = &mut self.role else {
                unreachable!()
            };
            HostRouter {
                local_player: self.status.local_player.unwrap(),
                connections: &self.connections,
                contexts: &mut host.contexts,
                session: self.session.as_mut().unwrap(),
                store,
                definition: self.definition.as_ref(),
            }
            .publish(&mut self.transport, source, outcome)
            .map_err(|e| format!("{e:?}"))?
        };
        for (connection, _) in failures {
            self.disconnect(connection, DisconnectReason::ConnectionProblem, None, store)?;
        }
        Ok(())
    }
    /// Retain Ready identity before bootstrap removes mappings. Only live removal
    /// owns cancellation, including synthetic failures and duplicate backend closes.
    fn disconnect(
        &mut self,
        connection: ConnectionId,
        reason: DisconnectReason,
        retained: Option<PlayerId>,
        store: &mut PieceDataStore,
    ) -> Result<(), String> {
        let mut pending = vec![(connection, reason, retained)];
        while let Some((connection, reason, retained)) = pending.pop() {
            if !self.live.remove(&connection) {
                continue;
            }
            let player = retained.or_else(|| self.connections.player(connection));
            let event = TransportEvent::Disconnected { connection, reason };
            let _ = self.transport.close(connection, reason);
            match &mut self.role {
                Role::Host(host) => {
                    host.joining.remove(&connection);
                    host.sync.disconnect(connection);
                    let _ = host.bootstrap.process(
                        &event,
                        &mut self.transport,
                        &mut self.connections,
                        Instant::now(),
                    );
                    if let Some(player) = player {
                        let session = self.session.as_mut().unwrap();
                        if let Some(cancel) = host
                            .contexts
                            .cancel_replicated(session, store, player)
                            .map_err(|e| format!("{e:?}"))?
                        {
                            self.presentation.event(
                                self.status.local_player.unwrap(),
                                &cancel.authority_event.event,
                                None,
                                store,
                            );
                            // Retention errors cannot suppress ordinary publication.
                            if let Err(e) = host.sync.record_authority_event(
                                session,
                                store,
                                &cancel.authority_event,
                            ) {
                                self.status.error = Some(format!("{e:?}"));
                            }
                            let failures = HostRouter {
                                local_player: self.status.local_player.unwrap(),
                                connections: &self.connections,
                                contexts: &mut host.contexts,
                                session,
                                store,
                                definition: self.definition.as_ref(),
                            }
                            .publish_authority_event(&mut self.transport, &cancel.authority_event)
                            .map_err(|e| format!("{e:?}"))?;
                            pending.extend(
                                failures
                                    .into_iter()
                                    .map(|(id, _)| (id, DisconnectReason::ConnectionProblem, None)),
                            );
                        }
                    }
                }
                Role::Client(client) => {
                    if let Some(sync) = &mut client.sync {
                        sync.invalidate();
                    }
                    client.sync = None;
                    client.cached_image = None;
                    self.decode = None;
                    self.decoded = None;
                    let _ = client.bootstrap.process(
                        &event,
                        &mut self.transport,
                        &mut self.connections,
                        Instant::now(),
                    );
                    self.status.phase = RuntimePhase::Disconnected;
                    self.status.error = Some(format!("{reason:?}"));
                    self.active = false;
                }
            }
        }
        Ok(())
    }
    fn poll(
        &mut self,
        store: &mut PieceDataStore,
        interaction: &mut PieceInteraction,
        now: Instant,
    ) -> Result<(), String> {
        if let Some(session) = &self.session {
            self.presentation.synchronize(session, store);
        }
        let mut events = Vec::new();
        self.transport
            .poll(&mut events)
            .map_err(|e| format!("{e:?}"))?;
        for event in events {
            if !self.active {
                break;
            }
            let connection = match event {
                TransportEvent::Connected { connection }
                | TransportEvent::Disconnected { connection, .. }
                | TransportEvent::ConnectionFailed { connection, .. }
                | TransportEvent::Message { connection, .. } => connection,
            };
            if let TransportEvent::Disconnected { reason, .. }
            | TransportEvent::ConnectionFailed { reason, .. } = event
            {
                if reason == DisconnectReason::BackendConnectionTimeout {
                    self.status.error = Some(format!("{reason:?}"));
                }
                self.disconnect(connection, reason, None, store)?;
                continue;
            }
            if matches!(event, TransportEvent::Connected { .. }) {
                self.live.insert(connection);
            }
            if !self.live.contains(&connection) {
                continue;
            }
            let retained = self.connections.player(connection);
            let outcome = match &mut self.role {
                Role::Host(host) => {
                    host.bootstrap
                        .update_cursor(self.session.as_ref().unwrap().cursor());
                    host.bootstrap
                        .process(&event, &mut self.transport, &mut self.connections, now)
                }
                Role::Client(client) => client.bootstrap.process(
                    &event,
                    &mut self.transport,
                    &mut self.connections,
                    now,
                ),
            };
            let outcome = match outcome {
                Ok(outcome) => outcome,
                Err(error) => {
                    self.status.error = Some(format!("{error:?}"));
                    self.disconnect(
                        connection,
                        DisconnectReason::ProtocolViolation,
                        retained,
                        store,
                    )?;
                    continue;
                }
            };
            match &mut self.role {
                Role::Host(host) => {
                    let session = self.session.as_mut().unwrap();
                    let authority = SyncAuthority {
                        session,
                        store,
                        contexts: &host.contexts,
                        definition: self.definition.as_ref().unwrap(),
                    };
                    let result = if outcome == BootstrapOutcome::Syncing {
                        host.sync.route(
                            &mut SyncHost {
                                bootstrap: &mut host.bootstrap,
                                connections: &mut self.connections,
                            },
                            &event,
                            &mut self.transport,
                            &authority,
                            self.image.clone(),
                            now,
                        )
                    } else if host.bootstrap.state(connection)
                        == Some(ConnectionState::Authenticated)
                    {
                        host.joining.insert(connection);
                        host.sync.start(
                            &mut host.bootstrap,
                            connection,
                            &mut self.transport,
                            &authority,
                            now,
                        )
                    } else {
                        Ok(())
                    };
                    if let Err(error) = result {
                        let reason = error.disconnect_reason();
                        self.status.error = Some(format!("{error:?}"));
                        if super::lifecycle::is_abuse(reason)
                            && host.sync.phase(connection).is_some()
                        {
                            host.sync.penalize(self.transport.origin(connection), now);
                        }
                        self.disconnect(connection, reason, retained, store)?;
                        continue;
                    }
                    if host.bootstrap.state(connection) == Some(ConnectionState::Ready) {
                        host.joining.remove(&connection);
                    }
                    if outcome == BootstrapOutcome::Gameplay {
                        let routed = HostRouter {
                            local_player: self.status.local_player.unwrap(),
                            connections: &self.connections,
                            contexts: &mut host.contexts,
                            session,
                            store,
                            definition: self.definition.as_ref(),
                        }
                        .route_with_sync(&event, &mut host.sync);
                        match routed {
                            Ok(routed) => {
                                if let Err(error) = routed.retention {
                                    self.status.error = Some(format!("{error:?}"));
                                }
                                if let HostRouteOutcome::Applied(outcome) = routed.gameplay {
                                    self.publish(Some(connection), &outcome, store)?;
                                }
                            }
                            Err(error) => {
                                self.status.error = Some(format!("{error:?}"));
                                self.disconnect(
                                    connection,
                                    DisconnectReason::ProtocolViolation,
                                    retained,
                                    store,
                                )?;
                            }
                        }
                    }
                }
                Role::Client(client) => {
                    if client.bootstrap.state() == Some(ConnectionState::Authenticated) {
                        let metadata = client.bootstrap.metadata().unwrap();
                        self.session = Some(AuthoritySession::new(
                            metadata.definition,
                            metadata.host,
                            metadata.cursor,
                        ));
                        let sync = ClientSyncRouter::start(
                            &mut client.bootstrap,
                            client.cached_image.as_deref(),
                            now,
                        )
                        .map_err(|e| format!("{e:?}"))?;
                        let cached = sync
                            .image_ready()
                            .then(|| client.cached_image.take())
                            .flatten();
                        client.cached_image = None;
                        client.sync = Some(sync);
                        self.status.phase = RuntimePhase::Syncing(SyncPhase::ImageNegotiation);
                        if let Some(bytes) = cached {
                            self.start_decode(move || Ok(bytes));
                        }
                    } else if outcome == BootstrapOutcome::Syncing {
                        let sync = client.sync.as_mut().ok_or("missing client sync")?;
                        let result = sync
                            .route(
                                &mut client.bootstrap,
                                &mut self.connections,
                                &event,
                                &mut self.transport,
                                &mut SyncReplica {
                                    replica: &mut client.replica,
                                    session: self.session.as_mut().unwrap(),
                                    store,
                                },
                                now,
                            )
                            .map_err(|e| format!("{e:?}"))?;
                        if let Some(definition) = sync.definition() {
                            self.definition = Some(definition.clone());
                        }
                        self.status.phase = RuntimePhase::Syncing(sync.phase());
                        match result {
                            ClientSyncOutcome::ImageReady(bytes) => self.start_decode(move || {
                                bytes
                                    .into_bytes()
                                    .map(Arc::from)
                                    .map_err(|e| format!("{e:?}"))
                            }),
                            ClientSyncOutcome::BaselineInstalled => self.baseline_installed = true,
                            ClientSyncOutcome::Ready => {
                                self.status.local_player = client.bootstrap.assigned_player();
                                self.status.host = Some(self.session.as_ref().unwrap().host());
                                self.status.phase = RuntimePhase::Ready;
                                let session = self.session.as_ref().unwrap();
                                self.presentation.initialize(
                                    self.status.local_player.unwrap(),
                                    session,
                                    store,
                                    client.replica.remote_drags(session, store),
                                );
                                client.sync = None;
                            }
                            _ => {}
                        }
                    } else if outcome == BootstrapOutcome::Gameplay {
                        let player = self.status.local_player.ok_or("missing Ready identity")?;
                        let routed = ClientRouter {
                            local_player: player,
                            host_connection: client.bootstrap.host_connection(),
                            connections: &self.connections,
                            replica: &mut client.replica,
                            session: self.session.as_mut().unwrap(),
                            store,
                            definition: self.definition.as_ref(),
                        }
                        .route(&event)
                        .map_err(|e| format!("{e:?}"))?;
                        if let TransportEvent::Message { class, payload, .. } = &event {
                            match wire::decode_for_class(payload, *class)
                                .map_err(|e| format!("{e:?}"))?
                            {
                                WireMessage::AuthorityEvent(envelope) => {
                                    let drag = client.replica.remote_drag(
                                        self.session.as_ref().unwrap(),
                                        store,
                                        presentation::event_player(&envelope.event),
                                    );
                                    self.presentation
                                        .event(player, &envelope.event, drag, store);
                                    self.bridge
                                        .reconcile(player, &envelope.event, interaction, store)
                                        .map_err(|e| format!("{e:?}"))?;
                                }
                                WireMessage::DragUpdate(update)
                                    if matches!(
                                        routed,
                                        super::client::ClientRouteOutcome::Drag(_)
                                    ) =>
                                {
                                    self.presentation.delta(
                                        update.player,
                                        client
                                            .replica
                                            .remote_drag(
                                                self.session.as_ref().unwrap(),
                                                store,
                                                update.player,
                                            )
                                            .unwrap()
                                            .delta,
                                    );
                                }
                                _ => {}
                            }
                        }
                    } else if client.bootstrap.state().is_some() {
                        self.status.phase = RuntimePhase::Authenticating;
                    }
                }
            }
        }
        if !self.active {
            return Ok(());
        }
        match &mut self.role {
            Role::Host(host) => {
                for (id, error) in
                    host.bootstrap
                        .expire(&mut self.transport, &mut self.connections, now)
                {
                    host.sync.disconnect(id);
                    host.joining.remove(&id);
                    self.live.remove(&id);
                    self.status.error = Some(format!("{error:?}"));
                }
                for (id, error) in host.sync.expire(
                    &mut SyncHost {
                        bootstrap: &mut host.bootstrap,
                        connections: &mut self.connections,
                    },
                    &mut self.transport,
                    now,
                ) {
                    host.joining.remove(&id);
                    self.live.remove(&id);
                    self.status.error = Some(format!("{error:?}"));
                }
                let mut joining: Vec<_> = host.joining.iter().copied().collect();
                if let Some(next) = host.next_pump {
                    let pivot = joining.partition_point(|id| *id < next);
                    joining.rotate_left(pivot);
                }
                host.next_pump = joining.get(1).copied().or_else(|| joining.first().copied());
                for id in joining {
                    let Role::Host(host) = &mut self.role else {
                        unreachable!()
                    };
                    let result = {
                        let authority = SyncAuthority {
                            session: self.session.as_ref().unwrap(),
                            store,
                            contexts: &host.contexts,
                            definition: self.definition.as_ref().unwrap(),
                        };
                        if host.sync.phase(id) == Some(SyncPhase::RestartRequired) {
                            host.sync.restart(
                                &host.bootstrap,
                                id,
                                &mut self.transport,
                                &authority,
                                now,
                            )
                        } else {
                            host.sync.pump(
                                &host.bootstrap,
                                id,
                                &mut self.transport,
                                &authority,
                                now,
                            )
                        }
                    };
                    if let Err(error) = result {
                        let reason = error.disconnect_reason();
                        if super::lifecycle::is_abuse(reason) {
                            host.sync.penalize(self.transport.origin(id), now);
                        }
                        self.status.error = Some(format!("{error:?}"));
                        self.disconnect(id, reason, None, store)?;
                    }
                }
                let Role::Host(host) = &self.role else {
                    unreachable!()
                };
                self.status.peers = self
                    .connections
                    .peers()
                    .map(|peer| RuntimePeer {
                        connection: peer.connection,
                        player: peer.player,
                        state: host.bootstrap.state(peer.connection),
                    })
                    .collect();
            }
            Role::Client(client) => {
                client
                    .bootstrap
                    .expire(&mut self.transport, &mut self.connections, now)
                    .map_err(|e| format!("{e:?}"))?;
                if let Some(error) = client.sync.as_ref().and_then(|sync| sync.timeout(now)) {
                    client.sync.as_mut().unwrap().invalidate();
                    client.sync = None;
                    return Err(format!("{error:?}"));
                }
            }
        }
        if let Some(rx) = &self.decode {
            match rx.try_recv() {
                Ok(Ok(image)) => {
                    self.decoded = Some(image);
                    self.decode = None;
                    self.status.image = ImageReadiness::Decoded;
                }
                Ok(Err(error)) => return Err(error),
                Err(crossbeam::channel::TryRecvError::Disconnected) => {
                    return Err("image worker stopped".into())
                }
                Err(crossbeam::channel::TryRecvError::Empty) => {}
            }
        }
        Ok(())
    }
    fn local_frame(
        &mut self,
        commands: Vec<ClientCommand>,
        store: &mut PieceDataStore,
        interaction: &mut PieceInteraction,
        pointer: Option<Vec2>,
    ) -> Result<(), String> {
        if let Some(session) = &self.session {
            self.presentation.synchronize(session, store);
        }
        let Some(player) = self.status.local_player else {
            return Ok(());
        };
        if !matches!(
            self.status.phase,
            RuntimePhase::Hosting | RuntimePhase::Ready
        ) {
            return Ok(());
        }
        for request in commands {
            if request.player != player {
                return Err("local command identity mismatch".into());
            }
            self.bridge
                .enqueue(
                    request.command,
                    pointer,
                    interaction.network_gesture_token(),
                )
                .map_err(|e| format!("{e:?}"))?;
        }
        while let Some(command) = self
            .bridge
            .next(self.session.as_ref().unwrap(), player, store)
            .map_err(|e| format!("{e:?}"))?
        {
            self.send_local(command, store, interaction)?;
        }
        if let Some(command) = self
            .bridge
            .drag_update(self.session.as_ref().unwrap(), player, store)
            .map_err(|e| format!("{e:?}"))?
        {
            self.send_local(command, store, interaction)?;
        }
        Ok(())
    }
    fn send_local(
        &mut self,
        command: ProtocolCommandEnvelope,
        store: &mut PieceDataStore,
        interaction: &mut PieceInteraction,
    ) -> Result<(), String> {
        let player = self.status.local_player.unwrap();
        match &mut self.role {
            Role::Host(host) => {
                let session = self.session.as_mut().unwrap();
                match host.contexts.apply_replicated(
                    session,
                    store,
                    player,
                    &command,
                    self.definition.as_ref(),
                    player,
                ) {
                    Ok(outcome) => {
                        if let Err(error) =
                            host.sync.record_command_outcome(session, store, &outcome)
                        {
                            self.status.error = Some(format!("{error:?}"));
                        }
                        if let Some(event) = &outcome.authority_event {
                            self.bridge
                                .reconcile(player, &event.event, interaction, store)
                                .map_err(|e| format!("{e:?}"))?;
                        }
                        self.publish(None, &outcome, store)?;
                    }
                    Err(error) => {
                        self.bridge.reject(interaction, store);
                        self.status.error = Some(format!("{error:?}"));
                        if matches!(
                            error,
                            crate::multiplayer::protocol::ProtocolCommandError::Sequence(_)
                        ) {
                            return Err(format!("{error:?}"));
                        }
                    }
                }
            }
            Role::Client(client) => ClientRouter {
                local_player: player,
                host_connection: client.bootstrap.host_connection(),
                connections: &self.connections,
                replica: &mut client.replica,
                session: self.session.as_mut().unwrap(),
                store,
                definition: self.definition.as_ref(),
            }
            .send_command(&mut self.transport, &command)
            .map_err(|e| format!("{e:?}"))?,
        }
        Ok(())
    }
    fn teardown(&mut self, store: &mut PieceDataStore) {
        for connection in std::mem::take(&mut self.live) {
            let _ = self
                .transport
                .close(connection, DisconnectReason::Requested);
        }
        // An outgoing connect may not have emitted Connected yet.
        if let Role::Client(client) = &self.role {
            let _ = self.transport.close(
                client.bootstrap.host_connection(),
                DisconnectReason::Requested,
            );
        }
        if let Some(listener) = self.listener.take() {
            let _ = self.transport.close_listener(listener);
        }
        let players: std::collections::HashSet<_> =
            store.held_by.iter().map(|(_, &player)| player).collect();
        for player in players {
            store.clear_player_holds(player);
        }
        store.drag = default();
        self.connections = default();
        self.bridge = default();
        self.decode = None;
        self.decoded = None;
        self.active = false;
        // Dropping Runtime destroys bootstrap passwords, sync, contexts, replica,
        // secure channels and backend. No state is reused by a new session.
    }
}

#[cfg(test)]
mod tests;
