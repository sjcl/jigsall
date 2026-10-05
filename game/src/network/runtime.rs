//! Shared bootstrap, sync, gameplay and teardown after transport establishment.
//! The CPU store is borrowed from the game World; no parallel gameplay state exists.
mod direct;
use direct::DirectIpDriver;
#[cfg(feature = "rendezvous")]
mod rendezvous;
#[cfg(feature = "rendezvous")]
pub use super::gns::rendezvous::protocol::RoomCode;
#[cfg(feature = "rendezvous")]
pub use rendezvous::{RendezvousHostOptions, RendezvousJoinOptions, RendezvousRuntimeConfig};
#[cfg(feature = "rendezvous")]
pub use world::{start_rendezvous_host, start_rendezvous_join};
mod bridge;
mod cursors;
mod failure;
pub use failure::NetworkFailureKind;
use failure::RuntimeFailure;
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
    players::{PlayerRoster, PresenceMessage, RosterPlayer},
    resources::{ImageDecodeLimits, PieceDataStore},
};
use bevy::prelude::*;
use bridge::CommandBridge;
use jigsall_core::{
    protocol::*, session::*, ClientCommand, PieceCommand, PlayerId, PuzzleDefinition,
};
#[cfg(test)]
use std::time::Duration;
use std::{collections::BTreeSet, net::SocketAddr, sync::Arc, time::Instant};

pub struct HostOptions {
    pub display_name: Option<jigsall_core::PlayerDisplayName>,
    pub address: SocketAddr,
    pub session: SessionDefinition,
    pub host: PlayerId,
    pub password: SessionPassword,
}
/// Owns the password once. A failed listen keeps the request available for a
/// port/address retry; success transfers it into the runtime and consumes it.
pub struct HostStartRequest {
    options: Option<HostOptions>,
}
impl HostStartRequest {
    pub fn new(options: HostOptions) -> Self {
        Self {
            options: Some(options),
        }
    }
    pub fn set_address(&mut self, address: SocketAddr) {
        if let Some(options) = &mut self.options {
            options.address = address;
        }
    }
}
pub struct JoinOptions {
    pub display_name: Option<jigsall_core::PlayerDisplayName>,
    pub address: SocketAddr,
    pub password: SessionPassword,
    pub cached_image: Option<Arc<[u8]>>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeConnectionMethod {
    DirectIp,
    Internet,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RendezvousControlStatus {
    Connecting,
    Available,
    Unavailable,
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
    pub connection_method: Option<RuntimeConnectionMethod>,
    pub room_code: Option<String>,
    pub rendezvous_control: Option<RendezvousControlStatus>,
    pub image: ImageReadiness,
    pub image_source: HostImageSource,
    pub peers: Vec<RuntimePeer>,
    pub error: Option<String>,
    pub failure: Option<NetworkFailureKind>,
    /// Internet Host failed before RoomCreated; retain the prepared puzzle for retry.
    pub host_start_failed: bool,
}
impl Default for NetworkStatus {
    fn default() -> Self {
        Self {
            role: None,
            phase: RuntimePhase::Offline,
            local_player: None,
            host: None,
            address: None,
            connection_method: None,
            room_code: None,
            rendezvous_control: None,
            image: ImageReadiness::Unavailable,
            image_source: HostImageSource::Unavailable,
            peers: Vec::new(),
            error: None,
            failure: None,
            host_start_failed: false,
        }
    }
}
impl NetworkStatus {
    /// A client that reached Ready can save its last confirmed replica after loss.
    pub fn has_disconnected_game(&self) -> bool {
        self.role == Some(RuntimeRole::Client)
            && self.local_player.is_some()
            && matches!(
                self.phase,
                RuntimePhase::Failed | RuntimePhase::Disconnected
            )
    }
}
#[derive(Debug, PartialEq, Eq)]
pub enum RuntimeStartError {
    AlreadyActive,
    DefinitionUnavailable,
    InvalidWorld,
    ImageHashMismatch,
    Transport(TransportError),
    InternetUnavailable,
    #[cfg(feature = "rendezvous")]
    Rendezvous(super::gns::rendezvous::RendezvousError),
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
    display_name: Option<jigsall_core::PlayerDisplayName>,
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
    roster: PlayerRoster,
    transport: SecureTransport<T>,
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
    cursors: super::cursor::CursorPresence,
    status: NetworkStatus,
    baseline_installed: bool,
    render_installed: bool,
    active: bool,
}

fn publish_presence(
    connections: &SessionConnections,
    transport: &mut dyn Transport,
    source: Option<ConnectionId>,
    event: PresenceMessage,
) -> Result<Vec<ConnectionId>, RuntimeFailure> {
    let payload = wire::encode(&WireMessage::Presence(event)).map_err(RuntimeFailure::protocol)?;
    let mut failures = Vec::new();
    for peer in connections
        .peers()
        .filter(|p| p.player.is_some() && Some(p.connection) != source)
    {
        if transport
            .send(peer.connection, MessageClass::Control, &payload)
            .is_err()
        {
            failures.push(peer.connection);
        }
    }
    Ok(failures)
}

struct HostRuntimeOptions {
    display_name: Option<jigsall_core::PlayerDisplayName>,
    session: SessionDefinition,
    host: PlayerId,
    password: SessionPassword,
}
struct ClientRuntimeOptions {
    display_name: Option<jigsall_core::PlayerDisplayName>,
    password: SessionPassword,
    cached_image: Option<Arc<[u8]>>,
}
struct PreparedHost {
    definition: PuzzleDefinition,
    image: Option<Arc<[u8]>>,
    sync: HostSyncCoordinator,
}
impl PreparedHost {
    fn new(
        definition: PuzzleDefinition,
        image: Option<Arc<[u8]>>,
        session: SessionDefinition,
    ) -> Result<Self, RuntimeStartError> {
        let verified = image
            .as_ref()
            .map(|bytes| super::bulk::VerifiedPuzzleImage::verify(bytes.clone(), session))
            .transpose()
            .map_err(|_| RuntimeStartError::ImageHashMismatch)?;
        let mut sync = HostSyncCoordinator::default();
        if let Some(image) = verified {
            sync.set_image(image);
        }
        Ok(Self {
            definition,
            image,
            sync,
        })
    }
}
impl<T: Transport> Runtime<T> {
    fn host_with_transport(
        backend: T,
        options: HostRuntimeOptions,
        prepared: PreparedHost,
        now: Instant,
    ) -> Self {
        let transport = SecureTransport::new(backend);
        let PreparedHost {
            definition,
            image,
            sync,
        } = prepared;
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
        Self {
            transport,
            roster: PlayerRoster::host_only(options.host, options.display_name),
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
            cursors: Default::default(),
            baseline_installed: true,
            render_installed: true,
            active: true,
            status: NetworkStatus {
                role: Some(RuntimeRole::Host),
                phase: RuntimePhase::Hosting,
                local_player: Some(options.host),
                host: Some(options.host),
                connection_method: Some(RuntimeConnectionMethod::DirectIp),
                image_source,
                ..default()
            },
        }
    }
    fn client_with_connection(
        backend: T,
        connection: ConnectionId,
        options: ClientRuntimeOptions,
        image_limits: ImageDecodeLimits,
    ) -> Self {
        let transport = SecureTransport::new(backend);
        Self {
            transport,
            roster: PlayerRoster::default(),
            connections: Default::default(),
            live: BTreeSet::from([connection]),
            role: Role::Client(Box::new(ClientState {
                display_name: options.display_name,
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
            cursors: Default::default(),
            baseline_installed: false,
            render_installed: false,
            active: true,
            status: NetworkStatus {
                role: Some(RuntimeRole::Client),
                phase: RuntimePhase::Connecting,
                connection_method: Some(RuntimeConnectionMethod::DirectIp),
                ..default()
            },
        }
    }
    fn fail(&mut self, error: RuntimeFailure) {
        self.status.failure = Some(self.status.classify_failure(error.kind));
        self.status.error = Some(error.diagnostic);
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
    ) -> Result<(), RuntimeFailure> {
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
            .map_err(RuntimeFailure::protocol)?
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
    ) -> Result<(), RuntimeFailure> {
        let mut pending = vec![(connection, reason, retained)];
        while let Some((connection, reason, retained)) = pending.pop() {
            if !self.live.remove(&connection) {
                continue;
            }
            let player = retained.or_else(|| self.connections.player(connection));
            if let Some(player) = player {
                self.cursors.remove(player);
            }
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
                            .map_err(RuntimeFailure::protocol)?
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
                                self.status.set_failure(NetworkFailureKind::Protocol, &e);
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
                            .map_err(RuntimeFailure::protocol)?;
                            pending.extend(
                                failures
                                    .into_iter()
                                    .map(|(id, _)| (id, DisconnectReason::ConnectionProblem, None)),
                            );
                        }
                        if self.roster.get(player).is_some() {
                            let left = self
                                .roster
                                .remove_ready(player)
                                .map_err(RuntimeFailure::protocol)?;
                            let failures = publish_presence(
                                &self.connections,
                                &mut self.transport,
                                None,
                                left,
                            )?;
                            pending.extend(
                                failures
                                    .into_iter()
                                    .map(|id| (id, DisconnectReason::ConnectionProblem, None)),
                            );
                        }
                    }
                }
                Role::Client(client) => {
                    self.roster.clear();
                    self.cursors.reset();
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
                    // Keep the specific bootstrap/sync diagnostic when the close
                    // reason only says that the protocol was rejected.
                    if self.status.error.is_none() || reason != DisconnectReason::ProtocolViolation
                    {
                        self.status
                            .set_failure(NetworkFailureKind::disconnect(reason), reason);
                    }
                    self.status.phase = RuntimePhase::Disconnected;
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
    ) -> Result<(), RuntimeFailure> {
        if let Some(session) = &self.session {
            self.cursors
                .synchronize(session.session_definition().id, session.cursor().epoch);
            self.presentation.synchronize(session, store);
            self.bridge.synchronize(session, interaction, store);
        }
        let mut events = Vec::new();
        self.transport
            .poll(&mut events)
            .map_err(RuntimeFailure::transport)?;
        for event in events {
            if !self.active {
                break;
            }
            let visual = matches!(
                event,
                TransportEvent::Message {
                    class: MessageClass::Control,
                    ..
                }
            )
            .then(|| store.capture_rotation_boundary());
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
                    self.status
                        .set_failure(NetworkFailureKind::disconnect(reason), reason);
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
                    self.status
                        .set_failure(NetworkFailureKind::bootstrap(&error), &error);
                    self.disconnect(
                        connection,
                        DisconnectReason::ProtocolViolation,
                        retained,
                        store,
                    )?;
                    continue;
                }
            };
            // Dedicated presentation router, after bootstrap's authenticated Ready gate.
            // Cursor messages never enter HostRouter/ClientRouter or replication.
            if outcome == BootstrapOutcome::Gameplay
                && matches!(&event, TransportEvent::Message { payload, .. } if matches!(payload.get(6), Some(9 | 10)))
            {
                if let Err(error) = self.route_cursor(&event, now) {
                    self.status
                        .set_failure(NetworkFailureKind::Protocol, &error);
                    self.disconnect(
                        connection,
                        DisconnectReason::ProtocolViolation,
                        retained,
                        store,
                    )?;
                }
                continue;
            }
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
                                roster: &mut self.roster,
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
                        self.status
                            .set_failure(NetworkFailureKind::sync(&error), &error);
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
                    let joined_now = outcome == BootstrapOutcome::Syncing
                        && host.bootstrap.state(connection) == Some(ConnectionState::Ready);
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
                                    self.status
                                        .set_failure(NetworkFailureKind::Protocol, &error);
                                }
                                if let HostRouteOutcome::Applied(outcome) = routed.gameplay {
                                    self.publish(Some(connection), &outcome, store)?;
                                }
                            }
                            Err(error) => {
                                self.status
                                    .set_failure(NetworkFailureKind::Protocol, &error);
                                self.disconnect(
                                    connection,
                                    DisconnectReason::ProtocolViolation,
                                    retained,
                                    store,
                                )?;
                            }
                        }
                    }
                    if joined_now {
                        let player = self.connections.player(connection).unwrap();
                        let info = self.roster.get(player).unwrap();
                        let joined = PresenceMessage::PlayerJoined {
                            revision: self.roster.revision(),
                            player: RosterPlayer {
                                player,
                                display_name: info.display_name.clone(),
                            },
                        };
                        let failures = publish_presence(
                            &self.connections,
                            &mut self.transport,
                            Some(connection),
                            joined,
                        )?;
                        for id in failures {
                            self.disconnect(id, DisconnectReason::ConnectionProblem, None, store)?;
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
                            client.display_name.clone(),
                            now,
                        )
                        .map_err(RuntimeFailure::sync)?;
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
                                    roster: &mut self.roster,
                                    replica: &mut client.replica,
                                    session: self.session.as_mut().unwrap(),
                                    store,
                                },
                                now,
                            )
                            .map_err(RuntimeFailure::sync)?;
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
                            ClientSyncOutcome::BaselineInstalled => {
                                self.cursors.reset();
                                self.baseline_installed = true;
                                self.bridge = default();
                                *interaction = default();
                                store.drag = default();
                                store.clear_local_rotation();
                                store.clear_rotation_visual();
                            }
                            ClientSyncOutcome::Ready => {
                                self.cursors.reset();
                                self.bridge = default();
                                *interaction = default();
                                store.drag = default();
                                store.clear_local_rotation();
                                store.clear_rotation_visual();
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
                            roster: &mut self.roster,
                            local_player: player,
                            host_connection: client.bootstrap.host_connection(),
                            connections: &self.connections,
                            replica: &mut client.replica,
                            session: self.session.as_mut().unwrap(),
                            store,
                            definition: self.definition.as_ref(),
                        }
                        .route(&event)
                        .map_err(RuntimeFailure::protocol)?;
                        if let TransportEvent::Message { class, payload, .. } = &event {
                            match wire::decode_for_class(payload, *class)
                                .map_err(RuntimeFailure::protocol)?
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
                                        .map_err(RuntimeFailure::protocol)?;
                                    // Exclusive PreUpdate: canonical apply + prefix retirement
                                    // + suffix replay complete before Last/upload/extraction.
                                    self.bridge.refresh_prediction(
                                        player,
                                        self.definition.as_ref(),
                                        interaction,
                                        store,
                                    );
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
                                WireMessage::Presence(PresenceMessage::PlayerLeft {
                                    player,
                                    ..
                                }) => {
                                    self.cursors.remove(player);
                                }
                                _ => {}
                            }
                        }
                    } else if matches!(
                        client.bootstrap.state(),
                        Some(
                            ConnectionState::TransportConnected
                                | ConnectionState::Authenticating
                                | ConnectionState::Securing
                        )
                    ) {
                        self.status.phase = RuntimePhase::Authenticating;
                    }
                }
            }
            if let Some(visual) = visual {
                store.finish_rotation_boundary(visual);
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
                    self.status
                        .set_failure(NetworkFailureKind::bootstrap(&error), &error);
                }
                for (id, error) in host.sync.expire(
                    &mut SyncHost {
                        roster: &mut self.roster,
                        bootstrap: &mut host.bootstrap,
                        connections: &mut self.connections,
                    },
                    &mut self.transport,
                    now,
                ) {
                    host.joining.remove(&id);
                    self.live.remove(&id);
                    self.status
                        .set_failure(NetworkFailureKind::sync(&error), &error);
                }
                let mut joining: Vec<_> = host.joining.iter().copied().collect();
                if let Some(next) = host.next_pump {
                    let pivot = joining.partition_point(|id| *id < next);
                    joining.rotate_left(pivot);
                }
                host.next_pump = joining.get(1).copied().or_else(|| joining.first().copied());
                // Each pump generates at most one message. Give active transfers
                // several rotating passes to use the existing host-wide byte/rate
                // budget even at low frame rates, without pre-generating chunks.
                for (pass, id) in
                    (0..4).flat_map(|pass| joining.iter().copied().map(move |id| (pass, id)))
                {
                    let Role::Host(host) = &mut self.role else {
                        unreachable!()
                    };
                    if pass != 0
                        && !matches!(
                            host.sync.phase(id),
                            Some(SyncPhase::ImageTransfer | SyncPhase::BaselineTransfer)
                        )
                    {
                        continue;
                    }
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
                        self.status
                            .set_failure(NetworkFailureKind::sync(&error), &error);
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
                    .map_err(RuntimeFailure::bootstrap)?;
                if let Some(error) = client.sync.as_ref().and_then(|sync| sync.timeout(now)) {
                    client.sync.as_mut().unwrap().invalidate();
                    client.sync = None;
                    return Err(RuntimeFailure::sync(error));
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
                Ok(Err(error)) => {
                    return Err(RuntimeFailure::new(NetworkFailureKind::Image, error))
                }
                Err(crossbeam::channel::TryRecvError::Disconnected) => {
                    return Err(RuntimeFailure::new(
                        NetworkFailureKind::Image,
                        "image worker stopped",
                    ))
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
    ) -> Result<(), RuntimeFailure> {
        if let Some(session) = &self.session {
            self.presentation.synchronize(session, store);
            self.bridge.synchronize(session, interaction, store);
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
        self.bridge.prediction_enabled = matches!(self.role, Role::Client(_));
        for request in commands {
            if request.player != player {
                return Err("local command identity mismatch".into());
            }
            let rotating = matches!(
                request.command,
                PieceCommand::Rotate { .. } | PieceCommand::RotateDrag { .. }
            );
            let visual = (self.bridge.prediction_enabled && rotating).then(|| {
                store.capture_rotation_command(&request.command, self.definition.as_ref())
            });
            self.bridge
                .enqueue(
                    request.command,
                    pointer,
                    interaction.network_gesture_token(),
                )
                .map_err(RuntimeFailure::protocol)?;
            if let Some(visual) = visual {
                self.bridge.refresh_prediction(
                    player,
                    self.definition.as_ref(),
                    interaction,
                    store,
                );
                store.finish_rotation_boundary(visual);
            }
        }
        while let Some(command) = self
            .bridge
            .next(self.session.as_ref().unwrap(), player, store)
            .map_err(RuntimeFailure::protocol)?
        {
            self.bridge.present_release(interaction, store);
            self.send_local(command, store, interaction)?;
        }
        self.bridge.present_release(interaction, store);
        if let Some(command) = self
            .bridge
            .drag_update(self.session.as_ref().unwrap(), player, store)
            .map_err(RuntimeFailure::protocol)?
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
    ) -> Result<(), RuntimeFailure> {
        let player = self.status.local_player.unwrap();
        match &mut self.role {
            Role::Host(host) => {
                let session = self.session.as_mut().unwrap();
                let mut visual =
                    if matches!(command.command, ProtocolPieceCommand::DragUpdate { .. }) {
                        store.empty_rotation_boundary()
                    } else {
                        store.capture_rotation_boundary()
                    };
                if let Some(definition) = self.definition.as_ref() {
                    match &command.command {
                        ProtocolPieceCommand::Rotate {
                            target,
                            quarter_turns,
                        } => {
                            visual = store.capture_rotation_command(
                                &PieceCommand::Rotate {
                                    target: target.clone(),
                                    quarter_turns: *quarter_turns,
                                },
                                Some(definition),
                            );
                        }
                        ProtocolPieceCommand::RotateDrag {
                            quarter_turns,
                            final_delta,
                            ..
                        } => {
                            if let Some(drag) = host.contexts.active_drag(session, store, player) {
                                match &drag.target {
                                    ActiveDragTarget::Sparse(refs) => store.plan_rotation_visual(
                                        &mut visual,
                                        refs.iter().map(|r| r.member),
                                        *quarter_turns,
                                        definition,
                                        *final_delta,
                                    ),
                                    ActiveDragTarget::Dense(dense) => store.plan_rotation_visual(
                                        &mut visual,
                                        dense.members.iter().filter(|&id| {
                                            store.connectivity.minimum_member(id) == id
                                        }),
                                        *quarter_turns,
                                        definition,
                                        *final_delta,
                                    ),
                                }
                            }
                        }
                        _ => {}
                    }
                }
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
                            self.status
                                .set_failure(NetworkFailureKind::Protocol, &error);
                        }
                        if let Some(event) = &outcome.authority_event {
                            self.bridge
                                .reconcile(player, &event.event, interaction, store)
                                .map_err(RuntimeFailure::protocol)?;
                        }
                        self.publish(None, &outcome, store)?;
                    }
                    Err(error) => {
                        self.bridge.reject(interaction, store);
                        self.status
                            .set_failure(NetworkFailureKind::Protocol, &error);
                        if matches!(
                            error,
                            crate::multiplayer::protocol::ProtocolCommandError::Sequence(_)
                        ) {
                            return Err(RuntimeFailure::protocol(error));
                        }
                    }
                }
                store.finish_rotation_boundary(visual);
            }
            Role::Client(client) => ClientRouter {
                roster: &mut self.roster,
                local_player: player,
                host_connection: client.bootstrap.host_connection(),
                connections: &self.connections,
                replica: &mut client.replica,
                session: self.session.as_mut().unwrap(),
                store,
                definition: self.definition.as_ref(),
            }
            .send_command(&mut self.transport, &command)
            .map_err(RuntimeFailure::send)?,
        }
        Ok(())
    }
    fn teardown(&mut self, store: &mut PieceDataStore) {
        self.cursors.reset();
        self.roster.clear();
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
        let players: std::collections::HashSet<_> =
            store.held_by.iter().map(|(_, &player)| player).collect();
        for player in players {
            store.clear_player_holds(player);
        }
        store.drag = default();
        store.clear_local_rotation();
        store.clear_rotation_visual();
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
