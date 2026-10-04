//! Internet establishment. Control routing and game connections have separate lifetimes.
use super::*;
use crate::network::gns::{
    rendezvous::{
        protocol::{ErrorCode, RoomCode},
        EndpointUrl, RendezvousAdapter, RendezvousError, RendezvousEvent, P2P_VIRTUAL_PORT,
    },
    signaling::PeerId,
    GnsP2p, IceConfig,
};
use world::RuntimeDriver;

/// Deployment configuration is injected by the application, never by a game menu.
/// Its absence disables Internet multiplayer. No endpoint/STUN defaults are supplied.
#[derive(Resource, Clone)]
pub struct RendezvousRuntimeConfig {
    pub endpoint: EndpointUrl,
    pub ice: IceConfig,
}
pub struct RendezvousHostOptions {
    pub display_name: Option<puzzella_core::PlayerDisplayName>,
    pub session: SessionDefinition,
    pub host: PlayerId,
    pub password: SessionPassword,
}
pub struct RendezvousJoinOptions {
    pub display_name: Option<puzzella_core::PlayerDisplayName>,
    pub room_code: RoomCode,
    pub password: SessionPassword,
    pub cached_image: Option<Arc<[u8]>>,
}
// Narrow private seams let CI exercise the real driver with local transports.
pub(super) trait PeerTransport: Transport {
    fn connect_peer(&mut self, peer: PeerId) -> Result<ConnectionId, TransportError>;
    fn remote_peer(&self, connection: ConnectionId) -> Option<PeerId>;
    fn has_peer(&self, peer: PeerId) -> bool;
    fn take_retired_peers(&mut self) -> Vec<PeerId>;
}
impl PeerTransport for GnsP2p {
    fn connect_peer(&mut self, peer: PeerId) -> Result<ConnectionId, TransportError> {
        self.connect_peer(peer, P2P_VIRTUAL_PORT)
    }
    fn remote_peer(&self, connection: ConnectionId) -> Option<PeerId> {
        self.remote_peer(connection)
    }
    fn has_peer(&self, peer: PeerId) -> bool {
        self.has_peer(peer)
    }
    fn take_retired_peers(&mut self) -> Vec<PeerId> {
        self.take_retired_peers()
    }
}
pub(super) trait ControlPlane {
    fn poll(&mut self, has_peer: &dyn Fn(PeerId) -> bool) -> Vec<RendezvousEvent>;
    fn reclaim_unavailable_routes(&mut self, has_peer: &dyn Fn(PeerId) -> bool);
    fn has_pending_signal(&self, peer: PeerId) -> bool;
    fn create_room(&mut self) -> Result<(), RendezvousError>;
    fn join_room(&mut self, code: RoomCode) -> Result<(), RendezvousError>;
    fn release_route(&mut self, peer: PeerId);
    fn confirm_peer(&mut self, peer: PeerId);
    fn revoke_peer(&mut self, peer: PeerId);
    fn shutdown(&mut self);
}
impl ControlPlane for RendezvousAdapter {
    fn poll(&mut self, has_peer: &dyn Fn(PeerId) -> bool) -> Vec<RendezvousEvent> {
        self.poll_with_peer_connections(has_peer)
    }
    fn reclaim_unavailable_routes(&mut self, has_peer: &dyn Fn(PeerId) -> bool) {
        self.reclaim_unavailable_routes(has_peer);
    }
    fn has_pending_signal(&self, peer: PeerId) -> bool {
        self.has_pending_signal(peer)
    }
    fn create_room(&mut self) -> Result<(), RendezvousError> {
        self.create_room()
    }
    fn join_room(&mut self, code: RoomCode) -> Result<(), RendezvousError> {
        self.join_room(code)
    }
    fn release_route(&mut self, peer: PeerId) {
        self.release_route(peer);
    }
    fn confirm_peer(&mut self, peer: PeerId) {
        self.confirm_peer(peer);
    }
    fn revoke_peer(&mut self, peer: PeerId) {
        self.revoke_peer(peer);
    }
    fn shutdown(&mut self) {
        self.shutdown();
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    ControlConnecting,
    CreatingRoom,
    JoiningRoom,
    PeerConnecting,
    Running,
}
pub(super) struct RendezvousRuntimeDriver<T = GnsP2p, C = RendezvousAdapter> {
    adapter: C,
    stage: Stage,
    // Host owns its one bootstrap immediately, but polls it only after RoomCreated.
    runtime: Option<Runtime<T>>,
    // Join owns backend/password until HostReady; no ClientBootstrap exists here.
    backend: Option<T>,
    join: Option<RendezvousJoinOptions>,
    limits: ImageDecodeLimits,
    pending_status: NetworkStatus,
    deadline: Instant,
    active: bool,
}
fn internet_status(role: RuntimeRole) -> NetworkStatus {
    NetworkStatus {
        role: Some(role),
        phase: RuntimePhase::Connecting,
        connection_method: Some(RuntimeConnectionMethod::Internet),
        rendezvous_control: Some(RendezvousControlStatus::Connecting),
        ..default()
    }
}
impl<T: PeerTransport + 'static, C: ControlPlane> RendezvousRuntimeDriver<T, C> {
    pub(super) fn host(
        backend: T,
        adapter: C,
        options: RendezvousHostOptions,
        prepared: PreparedHost,
    ) -> Self {
        let mut runtime = Runtime::host_with_transport(
            backend,
            HostRuntimeOptions {
                display_name: options.display_name,
                session: options.session,
                host: options.host,
                password: options.password,
            },
            prepared,
            Instant::now(),
        );
        runtime.status.connection_method = Some(RuntimeConnectionMethod::Internet);
        runtime.status.phase = RuntimePhase::Connecting;
        runtime.status.rendezvous_control = Some(RendezvousControlStatus::Connecting);
        Self {
            adapter,
            stage: Stage::ControlConnecting,
            runtime: Some(runtime),
            backend: None,
            join: None,
            limits: ImageDecodeLimits {
                max_texture_dimension: 0,
            },
            pending_status: internet_status(RuntimeRole::Host),
            deadline: Instant::now() + std::time::Duration::from_secs(25),
            active: true,
        }
    }
    pub(super) fn join(
        backend: T,
        adapter: C,
        options: RendezvousJoinOptions,
        limits: ImageDecodeLimits,
    ) -> Self {
        Self {
            adapter,
            stage: Stage::ControlConnecting,
            runtime: None,
            backend: Some(backend),
            join: Some(options),
            limits,
            pending_status: internet_status(RuntimeRole::Client),
            deadline: Instant::now() + std::time::Duration::from_secs(25),
            active: true,
        }
    }
    pub(super) fn status(&self) -> &NetworkStatus {
        self.runtime
            .as_ref()
            .map(|r| &r.status)
            .unwrap_or(&self.pending_status)
    }
    fn status_mut(&mut self) -> &mut NetworkStatus {
        self.runtime
            .as_mut()
            .map(|r| &mut r.status)
            .unwrap_or(&mut self.pending_status)
    }
    pub(super) fn take_roster(&mut self) -> PlayerRoster {
        self.runtime
            .as_mut()
            .map(|r| std::mem::take(&mut r.roster))
            .unwrap_or_default()
    }
    fn fail(&mut self, kind: NetworkFailureKind, error: impl std::fmt::Debug) {
        let host_start_failed = self.status().role == Some(RuntimeRole::Host)
            && matches!(self.stage, Stage::ControlConnecting | Stage::CreatingRoom);
        let status = self.status_mut();
        status.set_failure(kind, error);
        status.phase = RuntimePhase::Failed;
        status.host_start_failed = host_start_failed;
        self.active = false;
    }
    fn control_lost(&mut self, kind: NetworkFailureKind, error: impl std::fmt::Debug) {
        let established = matches!(self.stage, Stage::Running | Stage::PeerConnecting);
        let status = self.status_mut();
        status.rendezvous_control = Some(RendezvousControlStatus::Unavailable);
        status.room_code = None;
        if !established {
            self.fail(kind, error);
        }
        // Never release an active route/close gameplay here, including pending ICE.
    }
    fn event(&mut self, event: RendezvousEvent) {
        match event {
            RendezvousEvent::Welcome { .. } if self.stage == Stage::ControlConnecting => {
                self.status_mut().rendezvous_control = Some(RendezvousControlStatus::Available);
                let result = if let Some(join) = &self.join {
                    self.stage = Stage::JoiningRoom;
                    self.adapter.join_room(join.room_code.clone())
                } else {
                    self.stage = Stage::CreatingRoom;
                    self.adapter.create_room()
                };
                if let Err(error) = result {
                    self.fail(error_kind(&error), error);
                }
            }
            RendezvousEvent::RoomCreated { room_code, .. } if self.stage == Stage::CreatingRoom => {
                self.stage = Stage::Running;
                let status = self.status_mut();
                status.phase = RuntimePhase::Hosting;
                status.room_code = Some(room_code.to_string());
            }
            RendezvousEvent::HostReady { peer_id, .. } if self.stage == Stage::JoiningRoom => {
                let mut backend = self.backend.take().unwrap();
                match backend.connect_peer(peer_id) {
                    Ok(connection) => {
                        // This order is the only ClientBootstrap construction path.
                        let options = self.join.take().unwrap();
                        let mut runtime = Runtime::client_with_connection(
                            backend,
                            connection,
                            ClientRuntimeOptions {
                                display_name: options.display_name,
                                password: options.password,
                                cached_image: options.cached_image,
                            },
                            self.limits,
                        );
                        runtime.status.connection_method = Some(RuntimeConnectionMethod::Internet);
                        runtime.status.rendezvous_control = self.pending_status.rendezvous_control;
                        self.runtime = Some(runtime);
                        self.stage = Stage::PeerConnecting;
                    }
                    Err(error) => self.fail(NetworkFailureKind::transport(&error), error),
                }
            }
            RendezvousEvent::RoomClosed => {
                self.control_lost(NetworkFailureKind::Connection, "RoomClosed")
            }
            RendezvousEvent::Disconnected(error) => self.control_lost(error_kind(&error), error),
            RendezvousEvent::ServerError(code) => {
                let kind = server_error_kind(code);
                if matches!(self.stage, Stage::Running | Stage::PeerConnecting) {
                    let lifecycle_race = self.status().role == Some(RuntimeRole::Host)
                        && matches!(code, ErrorCode::UnknownTarget | ErrorCode::JoinTimeout);
                    if !lifecycle_race
                        && !matches!(code, ErrorCode::Backpressure | ErrorCode::RateLimited)
                    {
                        self.control_lost(kind, code);
                        self.adapter.shutdown();
                    }
                } else {
                    self.fail(kind, code);
                }
            }
            RendezvousEvent::PeerUnavailable { .. }
                if matches!(self.stage, Stage::Running | Stage::PeerConnecting) =>
            {
                // Loss of a signal route is not loss of the GNS connection.
            }
            RendezvousEvent::PeerJoined { .. } | RendezvousEvent::SignalBackpressure { .. } => {}
            _ => self.fail(
                NetworkFailureKind::Protocol,
                "unexpected rendezvous transition",
            ),
        }
    }
    fn poll_control(&mut self, now: Instant) {
        let backend = self
            .runtime
            .as_ref()
            .map(|r| r.transport.backend())
            .or(self.backend.as_ref());
        let events = self
            .adapter
            .poll(&|peer| backend.is_some_and(|b| b.has_peer(peer)));
        for event in events {
            self.event(event);
            if !self.active {
                break;
            }
        }
        if self.active
            && !matches!(self.stage, Stage::Running | Stage::PeerConnecting)
            && now >= self.deadline
        {
            self.fail(
                NetworkFailureKind::Timeout,
                "rendezvous establishment deadline",
            );
        }
    }
}
pub(super) fn server_error_kind(code: ErrorCode) -> NetworkFailureKind {
    match code {
        ErrorCode::UnknownRoom => NetworkFailureKind::RoomNotFound,
        ErrorCode::RoomFull
        | ErrorCode::Capacity
        | ErrorCode::RateLimited
        | ErrorCode::Backpressure => NetworkFailureKind::Capacity,
        ErrorCode::JoinTimeout => NetworkFailureKind::Timeout,
        _ => NetworkFailureKind::Protocol,
    }
}
pub(super) fn error_kind(error: &RendezvousError) -> NetworkFailureKind {
    match error {
        RendezvousError::Timeout => NetworkFailureKind::Timeout,
        RendezvousError::Backpressure => NetworkFailureKind::Capacity,
        RendezvousError::InvalidEndpoint
        | RendezvousError::InvalidState
        | RendezvousError::ProtocolViolation => NetworkFailureKind::Protocol,
        RendezvousError::Transport(error) => NetworkFailureKind::transport(error),
        RendezvousError::Network | RendezvousError::Requested => NetworkFailureKind::Connection,
    }
}
impl<T: PeerTransport + 'static, C: ControlPlane> RuntimeDriver for RendezvousRuntimeDriver<T, C> {
    fn poll(&mut self, world: &mut World) {
        self.poll_control(Instant::now());
        if self.active && matches!(self.stage, Stage::Running | Stage::PeerConnecting) {
            let runtime = self.runtime.as_mut().unwrap();
            RuntimeDriver::poll(runtime, world);
            if let Role::Host(host) = &runtime.role {
                // Sync starts in the same common-runtime poll as Authenticated.
                // assigned_player also covers that state, without waiting for Ready.
                for &id in &runtime.live {
                    if host.bootstrap.assigned_player(id).is_some() {
                        if let Some(peer) = runtime.transport.backend().remote_peer(id) {
                            self.adapter.confirm_peer(peer);
                        }
                    }
                }
            }
            for peer in runtime.transport.backend_mut().take_retired_peers() {
                if !runtime.transport.backend().has_peer(peer) {
                    if matches!(runtime.role, Role::Host(_)) {
                        self.adapter.revoke_peer(peer);
                    } else if !self.adapter.has_pending_signal(peer) {
                        self.adapter.release_route(peer);
                    }
                }
            }
            // Native failures before Connected never entered runtime.live.
            // Reconcile all unavailable bindings after native maintenance too.
            self.adapter
                .reclaim_unavailable_routes(&|peer| runtime.transport.backend().has_peer(peer));
            self.active = runtime.active;
            if self.stage == Stage::PeerConnecting
                && runtime.status.phase != RuntimePhase::Connecting
            {
                self.stage = Stage::Running;
            }
        }
        world.insert_resource(self.status().clone());
    }
    fn commands(&mut self, world: &mut World, commands: Vec<ClientCommand>) {
        if self.active && matches!(self.stage, Stage::Running | Stage::PeerConnecting) {
            if let Some(runtime) = &mut self.runtime {
                RuntimeDriver::commands(runtime, world, commands);
                self.active = runtime.active;
            }
        }
    }
    fn teardown(&mut self, world: &mut World) {
        // Stop WS routing first, explicitly close game handles second, then wipe
        // bootstrap/sync/secure channels/backend and any waiting join password.
        self.adapter.shutdown();
        if let Some(runtime) = &mut self.runtime {
            RuntimeDriver::teardown(runtime, world);
        }
        self.runtime = None;
        self.backend = None;
        self.join = None;
        self.active = false;
    }
    fn active(&self) -> bool {
        self.active
    }
    fn authority(&self) -> Option<&AuthoritySession> {
        self.runtime.as_ref()?.session.as_ref()
    }
    fn replica(&self) -> Option<&PeerReplicationState> {
        RuntimeDriver::replica(self.runtime.as_ref()?)
    }
    #[cfg(test)]
    fn host_state(&self) -> Option<&HostState> {
        RuntimeDriver::host_state(self.runtime.as_ref()?)
    }
}

#[cfg(test)]
mod tests;
