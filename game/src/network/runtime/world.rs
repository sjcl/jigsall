//! Main-thread Bevy ownership. Native backends never need Send/Sync or a worker.
use super::*;
use crate::{persistence::runtime::OriginalPuzzleImage, resources::*};
use bevy::ecs::message::MessageCursor;

pub(super) trait RuntimeDriver {
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
    /// Read-only replicated contexts. Rendering uses RemoteDragPresentation.
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
            .init_resource::<remote_cursor::RemoteCursorPresentation>()
            .init_resource::<remote_drag::RemoteDragPresentation>()
            .init_resource::<PlayerRoster>()
            .add_systems(PreUpdate, poll_network.run_if(network_active))
            .add_systems(
                PreUpdate,
                remote_cursor::smooth_remote_cursors.after(poll_network),
            )
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
fn menu_pending(world: &World) -> bool {
    world
        .get_resource::<NextState<AppState>>()
        .is_some_and(|next| {
            matches!(
                next,
                NextState::Pending(AppState::Menu) | NextState::PendingIfNeq(AppState::Menu)
            )
        })
}
pub(super) fn install_driver(
    world: &mut World,
    driver: Box<dyn RuntimeDriver>,
    status: NetworkStatus,
    roster: PlayerRoster,
) {
    world.insert_resource(remote_cursor::RemoteCursorPresentation::default());
    world.init_resource::<PieceInteraction>();
    *world.resource_mut::<PieceInteraction>() = default();
    let mut presentation = world
        .remove_resource::<remote_drag::RemoteDragPresentation>()
        .unwrap_or_default();
    if let Some(mut store) = world.get_resource_mut::<PieceDataStore>() {
        store.drag = default();
        store.clear_local_rotation();
        store.clear_rotation_visual();
        presentation.reset(store.epoch, store.len());
    }
    world.insert_resource(presentation);
    world.insert_resource(roster);
    world.insert_resource(status);
    let reader = world
        .get_resource::<Messages<ClientCommand>>()
        .map(Messages::get_cursor_current)
        .unwrap_or_default();
    world.insert_non_send(NetworkSession {
        driver: Some(driver),
        reader,
    });
}
fn prepare_host(
    world: &World,
    session: SessionDefinition,
) -> Result<PreparedHost, RuntimeStartError> {
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
        || world
            .get_resource::<PieceGenerationProgress>()
            .is_some_and(|progress| progress.is_generating || progress.receiver.is_some())
        || world
            .get_resource::<PuzzleImage>()
            .is_some_and(|image| image.logical_size != definition.image_size)
        || menu_pending(world)
    {
        return Err(RuntimeStartError::InvalidWorld);
    }
    let image = world
        .get_resource::<OriginalPuzzleImage>()
        .and_then(|original| original.encoded.clone());
    if world
        .get_resource::<OriginalPuzzleImage>()
        .is_some_and(|original| original.hash != session.image_hash)
    {
        return Err(RuntimeStartError::ImageHashMismatch);
    }
    PreparedHost::new(definition, image, session)
}
pub(super) fn prepare_host_world(world: &mut World, status: &NetworkStatus) {
    world.insert_resource(LocalPlayerId(status.local_player.unwrap()));
    world.insert_resource(SessionHostId(status.host.unwrap()));
    if let Some(mut persistence) =
        world.get_resource_mut::<crate::persistence::runtime::PersistenceState>()
    {
        persistence.generation = persistence.generation.wrapping_add(1);
        persistence.busy = false;
        persistence.autosaving = false;
        persistence.capture = None;
    }
}
fn prepare_join_limits(world: &World) -> Result<ImageDecodeLimits, RuntimeStartError> {
    if network_active(world) {
        return Err(RuntimeStartError::AlreadyActive);
    }
    if menu_pending(world) {
        return Err(RuntimeStartError::InvalidWorld);
    }
    let limits = world
        .get_resource::<PuzzleImageLimits>()
        .ok_or(RuntimeStartError::InvalidWorld)?;
    let settings = world
        .get_resource::<crate::image_settings::ImageSettingsState>()
        .ok_or(RuntimeStartError::InvalidWorld)?;
    let image_limits = limits.decode_limits(&settings.current);
    if image_limits.max_texture_dimension == 0 {
        return Err(RuntimeStartError::InvalidWorld);
    }
    Ok(image_limits)
}
/// Host the current World store. Missing encoded bytes remain unavailable.
pub fn host_with_transport<T: DirectIpTransport + 'static>(
    world: &mut World,
    backend: T,
    options: HostOptions,
) -> Result<SocketAddr, RuntimeStartError> {
    HostStartRequest::new(options).start_with_transport(world, backend)
}

impl HostStartRequest {
    pub fn start_with_transport<T: DirectIpTransport + 'static>(
        &mut self,
        world: &mut World,
        backend: T,
    ) -> Result<SocketAddr, RuntimeStartError> {
        if network_active(world) {
            return Err(RuntimeStartError::AlreadyActive);
        }
        let options = self
            .options
            .as_ref()
            .ok_or(RuntimeStartError::AlreadyActive)?;
        let prepared = prepare_host(world, options.session)?;
        let mut driver =
            DirectIpDriver::host_request(backend, &mut self.options, prepared, Instant::now())?;
        let address = driver.status.address.unwrap();
        prepare_host_world(world, &driver.status);
        let status = driver.status.clone();
        let roster = std::mem::take(&mut driver.runtime.roster);
        install_driver(world, Box::new(driver), status, roster);
        Ok(address)
    }
    #[cfg(feature = "gns")]
    pub fn start(&mut self, world: &mut World) -> Result<SocketAddr, RuntimeStartError> {
        if network_active(world) {
            return Err(RuntimeStartError::AlreadyActive);
        }
        let backend =
            super::super::gns::GnsDirectIp::new().map_err(RuntimeStartError::Transport)?;
        self.start_with_transport(world, backend)
    }
}
/// Authentication, image decode and baseline installation advance in frames.
pub fn join_with_transport<T: DirectIpTransport + 'static>(
    world: &mut World,
    backend: T,
    options: JoinOptions,
) -> Result<(), RuntimeStartError> {
    let image_limits = prepare_join_limits(world)?;
    let mut driver = DirectIpDriver::client(backend, options, image_limits)?;
    prepare_join_world(world);
    let status = driver.status.clone();
    let roster = std::mem::take(&mut driver.runtime.roster);
    install_driver(world, Box::new(driver), status, roster);
    Ok(())
}
pub(super) fn prepare_join_world(world: &mut World) {
    world.init_resource::<PieceDataStore>();
    world.init_resource::<PieceInteraction>();
    world.init_resource::<LocalPlayerId>();
    world.init_resource::<SessionHostId>();
    world.remove_resource::<OriginalPuzzleImage>();
    world.remove_resource::<PuzzleImage>();
    world.remove_resource::<PuzzleDefinition>();
    *world.resource_mut::<PieceDataStore>() = default();
    *world.resource_mut::<PieceInteraction>() = default();
    if let Some(mut selection) = world.get_resource_mut::<crate::selection::PuzzleSelection>() {
        selection.cancel();
    }
    if let Some(mut input) = world.get_resource_mut::<InputState>() {
        *input = default();
    }
    if let Some(mut overlay) = world.get_resource_mut::<crate::render::SelectionOverlay>() {
        *overlay = default();
    }
    // Joining can replace an offline puzzle without first visiting Menu.
    // These are game-level presentation entities, never per-piece entities.
    let old_entities: Vec<_> = world
        .query_filtered::<Entity, With<crate::components::GridReference>>()
        .iter(world)
        .collect();
    for entity in old_entities {
        world.despawn(entity);
    }
    if let Some(mut config) = world.get_resource_mut::<PuzzleConfig>() {
        config.image_path.clear();
    }
    if let Some(mut persistence) =
        world.get_resource_mut::<crate::persistence::runtime::PersistenceState>()
    {
        let generation = persistence.generation.wrapping_add(1);
        let entries = std::mem::take(&mut persistence.entries);
        *persistence = crate::persistence::runtime::PersistenceState {
            generation,
            entries,
            ..default()
        };
    }
    if let Some(mut timer) = world.get_resource_mut::<crate::persistence::autosave::AutosaveTimer>()
    {
        *timer = default();
    }
    world.remove_resource::<crate::persistence::runtime::PendingRestore>();
    world.remove_resource::<ImageLoadError>();
    world.insert_resource(PieceGenerationProgress::default());
    if let Some(mut next) = world.get_resource_mut::<NextState<AppState>>() {
        next.set(AppState::GameSetup);
    }
}
#[cfg(feature = "gns")]
pub fn start_host(
    world: &mut World,
    options: HostOptions,
) -> Result<SocketAddr, RuntimeStartError> {
    HostStartRequest::new(options).start(world)
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
    teardown_session(world);
    if let Some(mut next) = world.get_resource_mut::<NextState<AppState>>() {
        next.set(AppState::Menu);
    }
}
fn teardown_session(world: &mut World) {
    world.insert_resource(remote_cursor::RemoteCursorPresentation::default());
    world.init_resource::<PlayerRoster>();
    world.resource_mut::<PlayerRoster>().clear();
    if let Some(mut session) = world.remove_non_send::<NetworkSession>() {
        if let Some(mut driver) = session.driver.take() {
            driver.teardown(world);
        }
    }
    if let Some(mut messages) = world.get_resource_mut::<Messages<ClientCommand>>() {
        messages.clear();
    }
    if let Some(mut interaction) = world.get_resource_mut::<PieceInteraction>() {
        *interaction = default();
    }
    if let Some(mut selection) = world.get_resource_mut::<crate::selection::PuzzleSelection>() {
        selection.cancel();
    }
    world.insert_resource(LocalPlayerId::default());
    world.insert_resource(SessionHostId::default());
    if let Some(mut status) = world.get_resource_mut::<NetworkStatus>() {
        status.role = None;
        status.address = None;
        status.connection_method = None;
        status.room_code = None;
        status.rendezvous_control = None;
        status.image = ImageReadiness::Unavailable;
        status.image_source = HostImageSource::Unavailable;
        status.local_player = None;
        status.host = None;
        status.peers.clear();
        status.host_start_failed = false;
        if status.phase != RuntimePhase::Failed {
            status.phase = RuntimePhase::Disconnected;
        }
    }
}
fn menu_teardown(world: &mut World) {
    if network_active(world) {
        // Already entering Menu: do not schedule another Menu entry that could
        // clear a puzzle/session created after this cleanup.
        teardown_session(world);
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
            let host_start_failed = world.resource::<NetworkStatus>().host_start_failed;
            let retain_game = world.resource::<NetworkStatus>().has_disconnected_game()
                && world
                    .get_resource::<State<AppState>>()
                    .is_some_and(|state| {
                        matches!(state.get(), AppState::InGame | AppState::GameComplete)
                    })
                && world.contains_resource::<PuzzleDefinition>()
                && world.contains_resource::<OriginalPuzzleImage>()
                && !world.resource::<PieceDataStore>().is_empty();
            driver.teardown(world);
            session.driver = None;
            *world.resource_mut::<PieceInteraction>() = default();
            if let Some(mut selection) =
                world.get_resource_mut::<crate::selection::PuzzleSelection>()
            {
                selection.cancel();
            }
            if host_start_failed {
                // The host never admitted gameplay. Drop the session so another
                // start is possible, but keep the CPU store/image/render epoch.
                if let Some(mut messages) = world.get_resource_mut::<Messages<ClientCommand>>() {
                    messages.clear();
                }
                return;
            }
            if retain_game {
                // Keep the confirmed store and original image for an explicit save.
                // The empty NetworkSession still gates offline authority.
                world.init_resource::<LocalGameplayBlocked>();
                world.resource_mut::<LocalGameplayBlocked>().0 = true;
            } else if let Some(mut next) = world.get_resource_mut::<NextState<AppState>>() {
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
fn take_cursors(world: &mut World) -> (remote_cursor::RemoteCursorPresentation, u64) {
    world.init_resource::<remote_cursor::RemoteCursorPresentation>();
    let mut resource = world.resource_mut::<remote_cursor::RemoteCursorPresentation>();
    let revision = resource.revision();
    (std::mem::take(resource.bypass_change_detection()), revision)
}
fn restore_cursors(
    world: &mut World,
    presentation: remote_cursor::RemoteCursorPresentation,
    revision: u64,
) {
    let changed = presentation.revision() != revision;
    let mut resource = world.resource_mut::<remote_cursor::RemoteCursorPresentation>();
    *resource.bypass_change_detection() = presentation;
    if changed {
        resource.set_changed();
    }
}
impl<T: Transport + 'static> RuntimeDriver for Runtime<T> {
    fn poll(&mut self, world: &mut World) {
        let (presentation, cursor_revision) = take_cursors(world);
        self.cursors.presentation = presentation;
        self.roster = world.remove_resource::<PlayerRoster>().unwrap_or_default();
        self.presentation.state = world
            .remove_resource::<remote_drag::RemoteDragPresentation>()
            .unwrap_or_default();
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
        world.insert_resource(std::mem::take(&mut self.roster));
        world.insert_resource(std::mem::take(&mut self.presentation.state));
        restore_cursors(
            world,
            std::mem::take(&mut self.cursors.presentation),
            cursor_revision,
        );
        world.insert_resource(self.status.clone());
    }
    fn commands(&mut self, world: &mut World, commands: Vec<ClientCommand>) {
        let cursor_position = cursors::local_cursor(world);
        let (presentation, cursor_revision) = take_cursors(world);
        self.cursors.presentation = presentation;
        self.roster = world.remove_resource::<PlayerRoster>().unwrap_or_default();
        self.presentation.state = world
            .remove_resource::<remote_drag::RemoteDragPresentation>()
            .unwrap_or_default();
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
        self.cursor_frame(cursor_position, Instant::now());
        world.insert_resource(store);
        world.insert_resource(interaction);
        world.insert_resource(std::mem::take(&mut self.roster));
        world.insert_resource(std::mem::take(&mut self.presentation.state));
        restore_cursors(
            world,
            std::mem::take(&mut self.cursors.presentation),
            cursor_revision,
        );
        world.insert_resource(self.status.clone());
    }
    fn teardown(&mut self, world: &mut World) {
        world.insert_resource(remote_cursor::RemoteCursorPresentation::default());
        self.roster.clear();
        world.init_resource::<PlayerRoster>();
        world.resource_mut::<PlayerRoster>().clear();
        let mut presentation = world
            .remove_resource::<remote_drag::RemoteDragPresentation>()
            .unwrap_or_default();
        world.init_resource::<PieceDataStore>();
        let mut store = world.resource_mut::<PieceDataStore>();
        self.teardown(&mut store);
        presentation.reset(store.epoch, store.len());
        world.insert_resource(presentation);
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
impl<T: Transport> Runtime<T> {
    fn install_world(&mut self, world: &mut World) {
        if self.status.role == Some(RuntimeRole::Host) {
            self.status.image = if !world.contains_resource::<PuzzleImage>() {
                ImageReadiness::Unavailable
            } else if world
                .get_resource::<crate::render::RenderReady>()
                .is_some_and(|ready| ready.is_ready(world.resource::<PieceDataStore>().epoch))
            {
                ImageReadiness::Ready
            } else {
                ImageReadiness::Uploading
            };
            return;
        }
        if self.baseline_installed && !self.render_installed {
            if let Some(definition) = &self.definition {
                world.insert_resource(definition.clone());
            }
        }
        if let Some(decoded) = self.decoded.take() {
            if let Some(definition) = &self.definition {
                if decoded.logical_size != definition.image_size {
                    self.fail(RuntimeFailure::new(
                        NetworkFailureKind::Image,
                        "image dimensions differ from session definition",
                    ));
                    return;
                }
            }
            let opaque = crate::resources::images::image_is_opaque(&decoded.image);
            if let Some(mut assets) = world.get_resource_mut::<Assets<Image>>() {
                let texture_size = decoded.image.size();
                let handle = assets.add(decoded.image);
                world.insert_resource(PuzzleImage {
                    handle,
                    logical_size: decoded.logical_size,
                    texture_size,
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
                    image_lease: None,
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
                if world.resource::<PuzzleImage>().logical_size != definition.image_size {
                    self.fail(RuntimeFailure::new(
                        NetworkFailureKind::Image,
                        "image dimensions differ from session definition",
                    ));
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

#[cfg(feature = "rendezvous")]
pub fn start_rendezvous_host(
    world: &mut World,
    options: RendezvousHostOptions,
) -> Result<(), RuntimeStartError> {
    let prepared = prepare_host(world, options.session)?;
    let config = world
        .get_resource::<RendezvousRuntimeConfig>()
        .cloned()
        .ok_or(RuntimeStartError::InternetUnavailable)?;
    let (backend, adapter) = super::super::gns::rendezvous::RendezvousAdapter::new(
        config.endpoint,
        super::super::gns::rendezvous::P2P_VIRTUAL_PORT,
        config.ice,
    )
    .map_err(RuntimeStartError::Rendezvous)?;
    let mut driver = rendezvous::RendezvousRuntimeDriver::host(backend, adapter, options, prepared);
    prepare_host_world(world, driver.status());
    let status = driver.status().clone();
    let roster = driver.take_roster();
    install_driver(world, Box::new(driver), status, roster);
    Ok(())
}
#[cfg(feature = "rendezvous")]
pub fn start_rendezvous_join(
    world: &mut World,
    options: RendezvousJoinOptions,
) -> Result<(), RuntimeStartError> {
    let limits = prepare_join_limits(world)?;
    let config = world
        .get_resource::<RendezvousRuntimeConfig>()
        .cloned()
        .ok_or(RuntimeStartError::InternetUnavailable)?;
    let (backend, adapter) = super::super::gns::rendezvous::RendezvousAdapter::new(
        config.endpoint,
        super::super::gns::rendezvous::P2P_VIRTUAL_PORT,
        config.ice,
    )
    .map_err(RuntimeStartError::Rendezvous)?;
    let driver = rendezvous::RendezvousRuntimeDriver::join(backend, adapter, options, limits);
    prepare_join_world(world);
    let status = driver.status().clone();
    install_driver(world, Box::new(driver), status, PlayerRoster::default());
    Ok(())
}
