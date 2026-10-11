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
mod client;
mod cursors;
mod failure;
mod host;
mod lifecycle;
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
    policy::{HostFailurePolicy, HostInternalRecovery},
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
            let Some((connection, retained)) = self.receive_event(&event, store)? else {
                continue;
            };
            let Some(outcome) = self.bootstrap_event(&event, connection, retained, store, now)?
            else {
                continue;
            };
            // Dedicated presentation router, after bootstrap's authenticated Ready gate.
            // Cursor messages never enter HostRouter/ClientRouter or replication.
            if outcome == BootstrapOutcome::Gameplay
                && matches!(&event, TransportEvent::Message { payload, .. } if matches!(payload.get(6), Some(9 | 10)))
            {
                if let Err(error) = self.route_cursor(&event, now) {
                    if matches!(self.role, Role::Host(_)) {
                        self.note_host_failure(
                            HostFailurePolicy::Disconnect(DisconnectReason::ProtocolViolation),
                            connection,
                            &error,
                        )?;
                    } else {
                        self.status
                            .set_failure(NetworkFailureKind::Protocol, &error);
                    }
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
                Role::Host(_) => {
                    self.host_event(&event, outcome, connection, retained, store, now)?
                }
                Role::Client(_) => self.client_event(&event, outcome, store, interaction, now)?,
            }
            if let Some(visual) = visual {
                store.finish_rotation_boundary(visual);
            }
        }
        if !self.active {
            return Ok(());
        }
        match &mut self.role {
            Role::Host(_) => self.host_maintenance(store, now)?,
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
                .map_err(RuntimeFailure::internal)?;
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
            .map_err(RuntimeFailure::internal)?
        {
            self.bridge.present_release(interaction, store);
            self.send_local(command, store, interaction)?;
        }
        self.bridge.present_release(interaction, store);
        if let Some(command) = self
            .bridge
            .drag_update(self.session.as_ref().unwrap(), player, store)
            .map_err(RuntimeFailure::internal)?
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
                                .set_failure(NetworkFailureKind::Internal, &error);
                        }
                        if let Some(event) = &outcome.authority_event {
                            self.bridge
                                .reconcile(player, &event.event, interaction, store)
                                .map_err(RuntimeFailure::internal)?;
                        }
                        self.publish(None, &outcome, store)?;
                    }
                    Err(error) => {
                        self.bridge.reject(interaction, store);
                        self.status
                            .set_failure(NetworkFailureKind::Internal, &error);
                        if matches!(
                            super::host::command_failure_policy(&error),
                            HostFailurePolicy::InternalFailure(HostInternalRecovery::StopSession)
                        ) || matches!(
                            error,
                            crate::multiplayer::protocol::ProtocolCommandError::Sequence(_)
                        ) {
                            return Err(RuntimeFailure::internal(error));
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
        let reason = if matches!(self.role, Role::Host(_))
            && self.status.phase == RuntimePhase::Failed
            && self.status.failure == Some(NetworkFailureKind::Internal)
        {
            DisconnectReason::BackendFailure
        } else {
            DisconnectReason::Requested
        };
        for connection in std::mem::take(&mut self.live) {
            let _ = self.transport.close(connection, reason);
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
