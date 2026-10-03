//! Main-thread Bevy ownership. Native backends never need Send/Sync or a worker.
use super::*;
use crate::{persistence::runtime::OriginalPuzzleImage, resources::*};
use bevy::ecs::message::MessageCursor;

trait RuntimeDriver {
    fn poll(&mut self, world: &mut World);
    fn commands(&mut self, world: &mut World, commands: Vec<ClientCommand>);
    fn teardown(&mut self, world: &mut World);
    fn active(&self) -> bool;
    fn authority(&self) -> Option<&AuthoritySession>;
    fn replica(&self) -> Option<&PeerReplicationState>;
    #[cfg(test)]
    fn host_state(&self) -> Option<&HostState>;
}
/// Presence gates offline authority, including failure awaiting Menu cleanup.
pub struct NetworkSession {
    driver: Option<Box<dyn RuntimeDriver>>,
    reader: MessageCursor<ClientCommand>,
}
impl NetworkSession {
    pub fn authority(&self) -> Option<&AuthoritySession> {
        self.driver.as_ref()?.authority()
    }
    /// Read-only remote presentation for the future renderer integration.
    pub fn replica(&self) -> Option<&PeerReplicationState> {
        self.driver.as_ref()?.replica()
    }
    #[cfg(test)]
    pub(super) fn host_state(&self) -> Option<&HostState> {
        self.driver.as_ref()?.host_state()
    }
}
pub struct NetworkRuntimePlugin;
impl Plugin for NetworkRuntimePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NetworkStatus>()
            .add_systems(PreUpdate, poll_network.run_if(network_active))
            .add_systems(
                PostUpdate,
                network_commands
                    .after(crate::systems::handle_piece_input)
                    .before(crate::systems::apply_piece_commands)
                    .before(crate::persistence::runtime::capture_requested_save)
                    .run_if(network_active),
            )
            .add_systems(
                OnEnter(AppState::Menu),
                menu_teardown.before(crate::game::cleanup_game),
            );
    }
}
pub(crate) fn network_active(world: &World) -> bool {
    world.contains_non_send::<NetworkSession>()
}
pub(crate) fn offline(world: &World) -> bool {
    !network_active(world)
}
fn install_driver<T: DirectIpTransport + 'static>(world: &mut World, runtime: Runtime<T>) {
    world.insert_resource(runtime.status.clone());
    let reader = world
        .get_resource::<Messages<ClientCommand>>()
        .map(Messages::get_cursor_current)
        .unwrap_or_default();
    world.insert_non_send(NetworkSession {
        driver: Some(Box::new(runtime)),
        reader,
    });
}
/// Host the current World store. Missing encoded bytes remain unavailable.
pub fn host_with_transport<T: DirectIpTransport + 'static>(
    world: &mut World,
    backend: T,
    options: HostOptions,
) -> Result<SocketAddr, RuntimeStartError> {
    if network_active(world) {
        return Err(RuntimeStartError::AlreadyActive);
    }
    let definition = world
        .get_resource::<PuzzleDefinition>()
        .cloned()
        .ok_or(RuntimeStartError::DefinitionUnavailable)?;
    if definition.validate().is_err()
        || world.get_resource::<PieceDataStore>().is_none_or(|store| {
            store.len() != definition.piece_count() || !store.held_by.is_empty()
        })
    {
        return Err(RuntimeStartError::InvalidWorld);
    }
    let image = world
        .get_resource::<OriginalPuzzleImage>()
        .and_then(|original| original.encoded.clone());
    if world
        .get_resource::<OriginalPuzzleImage>()
        .is_some_and(|original| original.hash != options.session.image_hash)
    {
        return Err(RuntimeStartError::ImageHashMismatch);
    }
    let runtime = Runtime::host(backend, options, definition, image, Instant::now())?;
    let address = runtime.status.address.unwrap();
    world.insert_resource(LocalPlayerId(runtime.status.local_player.unwrap()));
    world.insert_resource(SessionHostId(runtime.status.host.unwrap()));
    install_driver(world, runtime);
    Ok(address)
}
/// Authentication, image decode and baseline installation advance in frames.
pub fn join_with_transport<T: DirectIpTransport + 'static>(
    world: &mut World,
    backend: T,
    options: JoinOptions,
) -> Result<(), RuntimeStartError> {
    if network_active(world) {
        return Err(RuntimeStartError::AlreadyActive);
    }
    let runtime = Runtime::client(backend, options)?;
    world.init_resource::<PieceDataStore>();
    world.init_resource::<PieceInteraction>();
    world.init_resource::<LocalPlayerId>();
    world.init_resource::<SessionHostId>();
    world.remove_resource::<OriginalPuzzleImage>();
    world.remove_resource::<PuzzleImage>();
    world.remove_resource::<PuzzleDefinition>();
    *world.resource_mut::<PieceDataStore>() = default();
    *world.resource_mut::<PieceInteraction>() = default();
    if let Some(mut config) = world.get_resource_mut::<PuzzleConfig>() {
        config.image_path.clear();
    }
    if let Some(mut persistence) =
        world.get_resource_mut::<crate::persistence::runtime::PersistenceState>()
    {
        persistence.generation = persistence.generation.wrapping_add(1);
        persistence.busy = false;
        persistence.capture = None;
    }
    world.remove_resource::<crate::persistence::runtime::PendingRestore>();
    world.remove_resource::<ImageLoadError>();
    world.insert_resource(PieceGenerationProgress::default());
    if let Some(mut next) = world.get_resource_mut::<NextState<AppState>>() {
        next.set(AppState::GameSetup);
    }
    install_driver(world, runtime);
    Ok(())
}
#[cfg(feature = "gns")]
pub fn start_host(
    world: &mut World,
    options: HostOptions,
) -> Result<SocketAddr, RuntimeStartError> {
    if network_active(world) {
        return Err(RuntimeStartError::AlreadyActive);
    }
    host_with_transport(
        world,
        super::super::gns::GnsDirectIp::new().map_err(RuntimeStartError::Transport)?,
        options,
    )
}
#[cfg(feature = "gns")]
pub fn start_join(world: &mut World, options: JoinOptions) -> Result<(), RuntimeStartError> {
    if network_active(world) {
        return Err(RuntimeStartError::AlreadyActive);
    }
    join_with_transport(
        world,
        super::super::gns::GnsDirectIp::new().map_err(RuntimeStartError::Transport)?,
        options,
    )
}
/// Closes sockets and destroys all session state; Menu performs game cleanup.
pub fn stop_session(world: &mut World) {
    if let Some(mut session) = world.remove_non_send::<NetworkSession>() {
        if let Some(mut driver) = session.driver.take() {
            driver.teardown(world);
        }
    }
    if let Some(mut messages) = world.get_resource_mut::<Messages<ClientCommand>>() {
        messages.clear();
    }
    world.insert_resource(LocalPlayerId::default());
    world.insert_resource(SessionHostId::default());
    if let Some(mut status) = world.get_resource_mut::<NetworkStatus>() {
        status.local_player = None;
        status.host = None;
        status.peers.clear();
        if status.phase != RuntimePhase::Failed {
            status.phase = RuntimePhase::Disconnected;
        }
    }
    if let Some(mut next) = world.get_resource_mut::<NextState<AppState>>() {
        next.set(AppState::Menu);
    }
}
fn menu_teardown(world: &mut World) {
    if network_active(world) {
        stop_session(world);
    }
}
fn drive(world: &mut World, commands: bool) {
    let Some(mut session) = world.remove_non_send::<NetworkSession>() else {
        return;
    };
    if let Some(driver) = &mut session.driver {
        if commands {
            let requests = world
                .get_resource::<Messages<ClientCommand>>()
                .map(|messages| session.reader.read(messages).cloned().collect())
                .unwrap_or_default();
            driver.commands(world, requests);
        } else {
            driver.poll(world);
        }
        if !driver.active() {
            driver.teardown(world);
            session.driver = None;
            *world.resource_mut::<PieceInteraction>() = default();
            if let Some(mut selection) =
                world.get_resource_mut::<crate::selection::PuzzleSelection>()
            {
                selection.cancel();
            }
            if let Some(mut next) = world.get_resource_mut::<NextState<AppState>>() {
                next.set(AppState::Menu);
            }
        }
    }
    // A failed client cannot fall through to offline authority before Menu.
    world.insert_non_send(session);
}
fn poll_network(world: &mut World) {
    drive(world, false);
}
fn network_commands(world: &mut World) {
    drive(world, true);
}
impl<T: DirectIpTransport + 'static> RuntimeDriver for Runtime<T> {
    fn poll(&mut self, world: &mut World) {
        let mut store = world
            .remove_resource::<PieceDataStore>()
            .unwrap_or_default();
        let mut interaction = world
            .remove_resource::<PieceInteraction>()
            .unwrap_or_default();
        if let Err(error) = self.poll(&mut store, &mut interaction, Instant::now()) {
            self.fail(error);
        }
        world.insert_resource(store);
        world.insert_resource(interaction);
        if self.active {
            self.install_world(world);
        }
        world.insert_resource(self.status.clone());
    }
    fn commands(&mut self, world: &mut World, commands: Vec<ClientCommand>) {
        let mut store = world
            .remove_resource::<PieceDataStore>()
            .unwrap_or_default();
        let mut interaction = world
            .remove_resource::<PieceInteraction>()
            .unwrap_or_default();
        let pointer = world
            .get_resource::<InputState>()
            .and_then(|input| input.mouse_position);
        if let Err(error) = self.local_frame(commands, &mut store, &mut interaction, pointer) {
            self.fail(error);
        }
        world.insert_resource(store);
        world.insert_resource(interaction);
        world.insert_resource(self.status.clone());
    }
    fn teardown(&mut self, world: &mut World) {
        if let Some(mut store) = world.get_resource_mut::<PieceDataStore>() {
            self.teardown(&mut store);
        }
    }
    fn active(&self) -> bool {
        self.active
    }
    fn authority(&self) -> Option<&AuthoritySession> {
        self.session.as_ref()
    }
    fn replica(&self) -> Option<&PeerReplicationState> {
        match &self.role {
            Role::Client(client) => Some(&client.replica),
            Role::Host(_) => None,
        }
    }
    #[cfg(test)]
    fn host_state(&self) -> Option<&HostState> {
        match &self.role {
            Role::Host(host) => Some(host),
            Role::Client(_) => None,
        }
    }
}
impl<T: DirectIpTransport> Runtime<T> {
    fn install_world(&mut self, world: &mut World) {
        if self.status.role == Some(RuntimeRole::Host) {
            return;
        }
        if self.baseline_installed && !self.render_installed {
            if let Some(definition) = &self.definition {
                world.insert_resource(definition.clone());
            }
        }
        if let Some(decoded) = self.decoded.take() {
            if let Some(definition) = &self.definition {
                if decoded.image.size() != definition.image_size {
                    self.fail("image dimensions differ from session definition");
                    return;
                }
            }
            let opaque = crate::resources::images::image_is_opaque(&decoded.image);
            if let Some(mut assets) = world.get_resource_mut::<Assets<Image>>() {
                let size = decoded.image.size().as_vec2();
                let handle = assets.add(decoded.image);
                world.insert_resource(PuzzleImage {
                    handle,
                    size,
                    opaque,
                });
                world.insert_resource(OriginalPuzzleImage {
                    hash: self
                        .session
                        .as_ref()
                        .unwrap()
                        .session_definition()
                        .image_hash,
                    encoded: Some(decoded.encoded),
                });
                self.status.image = ImageReadiness::Uploading;
            } else {
                self.decoded = Some(decoded);
            }
        }
        if self.status.phase == RuntimePhase::Ready {
            world.insert_resource(LocalPlayerId(self.status.local_player.unwrap()));
            world.insert_resource(SessionHostId(self.status.host.unwrap()));
            if !self.render_installed
                && world.contains_resource::<PuzzleImage>()
                && self.baseline_installed
            {
                let definition = self.definition.as_ref().unwrap();
                if world.resource::<PuzzleImage>().size.as_uvec2() != definition.image_size {
                    self.fail("image dimensions differ from session definition");
                    return;
                }
                let store = world.resource::<PieceDataStore>();
                let len = store.len();
                let placed = store.placed_count;
                world.insert_resource(PieceGenerationProgress {
                    is_generating: true,
                    total_pieces: len,
                    pieces_created: len,
                    generation_phase: GenerationPhase::UploadingGpu,
                    grid_size: (
                        definition.grid_size.x as usize,
                        definition.grid_size.y as usize,
                    ),
                    ..default()
                });
                world.insert_resource(GameData {
                    puzzle_progress: placed as f32 / len as f32,
                    puzzle_completed: placed == len,
                    ..default()
                });
                self.render_installed = true;
                if let Some(mut next) = world.get_resource_mut::<NextState<AppState>>() {
                    next.set(AppState::InGame);
                }
            }
            if self.render_installed
                && world
                    .get_resource::<crate::render::RenderReady>()
                    .is_some_and(|ready| ready.is_ready(world.resource::<PieceDataStore>().epoch))
            {
                self.status.image = ImageReadiness::Ready;
            }
        }
    }
}
