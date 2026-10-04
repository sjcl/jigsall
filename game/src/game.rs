use crate::{components::*, resources::*, systems::*};
use bevy::prelude::*;
use bevy::transform::TransformSystems;
use bevy_egui::EguiPostUpdateSet;
use puzzella_core::ClientCommand;
use puzzella_core::*;

use crate::persistence::runtime::{
    OriginalPuzzleImage, PendingRestore, PersistenceService, PersistenceState,
};

pub struct GamePlugin;
impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(crate::settings::DisplaySettingsPlugin);
        app.add_plugins(crate::network::runtime::NetworkRuntimePlugin);
        app.add_plugins(crate::selection::PuzzleSelectionPlugin)
            .init_resource::<crate::keybindings::KeyBindingsState>()
            .init_resource::<crate::image_settings::ImageSettingsState>()
            .init_resource::<crate::player_settings::PlayerSettingsState>()
            .init_resource::<crate::keybindings::KeyPresses>()
            .add_systems(
                Update,
                (
                    |mut state: ResMut<crate::player_settings::PlayerSettingsState>| {
                        state.poll_save()
                    },
                    |mut state: ResMut<crate::image_settings::ImageSettingsState>| {
                        state.poll_save()
                    },
                    |mut state: ResMut<crate::keybindings::KeyBindingsState>| state.poll_save(),
                    |mut state: ResMut<crate::persistence::autosave::AutosaveSettingsState>| {
                        state.poll_save()
                    },
                ),
            )
            .add_systems(
                PreUpdate,
                crate::keybindings::sample_key_presses.after(bevy::input::InputSystems),
            )
            .add_message::<ClientCommand>()
            .add_message::<PieceMoveCompleted>()
            .add_message::<PiecePlacedEvent>()
            .init_resource::<PersistenceService>()
            .init_resource::<PersistenceState>()
            .init_resource::<crate::persistence::autosave::AutosaveSettingsState>()
            .init_resource::<crate::persistence::autosave::AutosaveTimer>()
            .init_resource::<GameData>()
            .init_resource::<PlayerRoster>()
            .init_resource::<LocalPlayerId>()
            .init_resource::<SessionHostId>()
            .init_resource::<PuzzleConfig>()
            .init_resource::<InputState>()
            .init_resource::<GameUiPointerCapture>()
            .init_resource::<crate::interaction::PieceInteraction>()
            .init_resource::<PieceGenerationProgress>()
            .init_resource::<PerformanceMonitor>()
            .init_resource::<PieceDataStore>()
            .init_resource::<PieceUpload>()
            .init_resource::<crate::render::SelectionOverlay>()
            .init_state::<AppState>()
            .add_sub_state::<GameSubState>()
            .add_sub_state::<GameCompleteSubState>()
            .add_systems(Startup, (setup_game, setup_image_load_system))
            .add_systems(
                OnEnter(AppState::Menu),
                (cleanup_game, clear_session_messages, reset_local_player),
            )
            .add_systems(
                OnEnter(AppState::InGame),
                (
                    initialize_game,
                    spawn_grid_reference,
                    auto_adjust_camera_zoom,
                    reset_performance_samples,
                )
                    .chain(),
            )
            .add_systems(OnEnter(GameSubState::Paused), release_local_drag)
            .add_systems(
                OnEnter(AppState::GameComplete),
                (release_local_drag, auto_adjust_camera_zoom).chain(),
            )
            .add_systems(OnEnter(GameCompleteSubState::Paused), release_local_drag)
            .add_systems(
                Update,
                handle_image_load_results.run_if(in_state(AppState::GameSetup)),
            )
            .add_systems(
                Update,
                generate_puzzle_state.run_if(in_state(GameSubState::Initializing)),
            )
            .add_systems(
                PostUpdate,
                (handle_camera_zoom, handle_camera_drag)
                    .chain()
                    .after(EguiPostUpdateSet::EndPass)
                    .after(bevy::camera::CameraUpdateSystems)
                    .before(handle_edge_scrolling)
                    .before(TransformSystems::Propagate)
                    .run_if(
                        in_state(GameSubState::Playing)
                            .or_else(in_state(GameCompleteSubState::Viewing)),
                    ),
            )
            .add_systems(
                PostUpdate,
                (
                    handle_edge_scrolling,
                    update_input_state,
                    handle_piece_input,
                )
                    .chain()
                    .after(EguiPostUpdateSet::EndPass)
                    .after(bevy::camera::CameraUpdateSystems)
                    .before(apply_piece_commands)
                    .run_if(in_state(GameSubState::Playing)),
            )
            .add_systems(
                PostUpdate,
                (
                    apply_piece_commands.run_if(crate::network::runtime::world_offline),
                    check_piece_placement_event_driven
                        .run_if(crate::network::runtime::world_offline),
                    update_game_state_event_driven,
                    render_selection_box,
                )
                    .chain()
                    .after(EguiPostUpdateSet::EndPass)
                    .before(TransformSystems::Propagate)
                    .run_if(in_state(AppState::InGame)),
            )
            .add_systems(
                Update,
                toggle_completed_puzzle_menu
                    .run_if(in_state(AppState::GameComplete).and_then(escape_just_pressed)),
            )
            .add_systems(
                Update,
                (
                    toggle_game_menu.run_if(escape_just_pressed),
                    (
                        toggle_performance_debug.run_if(performance_key_just_pressed),
                        sample_performance_frame.run_if(performance_monitoring_enabled),
                    )
                        .chain(),
                )
                    .run_if(in_state(AppState::InGame)),
            )
            .add_systems(
                Update,
                (
                    crate::persistence::runtime::poll_results,
                    crate::persistence::autosave::tick_autosave,
                )
                    .chain()
                    .after(handle_image_load_results),
            )
            .add_systems(
                PostUpdate,
                crate::persistence::runtime::capture_requested_save
                    .after(EguiPostUpdateSet::EndPass)
                    .after(apply_piece_commands),
            )
            .add_systems(Last, crate::resources::pieces::prepare_piece_upload);
    }
}
fn setup_game(mut commands: Commands) {
    // Single-sample normal rendering and picking share pixel coverage at edges.
    commands.spawn((Camera2d, MainCamera, Msaa::Off));
}
#[allow(clippy::too_many_arguments)] // Explicit ECS resources include process identity.
fn initialize_game(
    local_player: Res<LocalPlayerId>,
    profile: Option<Res<crate::player_settings::PlayerSettingsState>>,
    mut commands: Commands,
    config: Res<PuzzleConfig>,
    image: Res<PuzzleImage>,
    mut game: ResMut<GameData>,
    mut roster: ResMut<PlayerRoster>,
    mut progress: ResMut<PieceGenerationProgress>,
    mut store: ResMut<PieceDataStore>,
    pending: Option<ResMut<PendingRestore>>,
    mut persistence: ResMut<PersistenceState>,
    network: Option<NonSend<crate::network::runtime::NetworkSession>>,
) {
    // A join already installed the canonical store and definition. Continue
    // UploadingGpu without regenerating either resource.
    if network.is_some() {
        return;
    }
    *game = GameData::default();
    *roster = PlayerRoster::host_only(
        local_player.0,
        profile.and_then(|p| p.current.display_name.clone()),
    );
    *progress = PieceGenerationProgress::default();
    if let Some(mut pending) = pending {
        if let Some(restored) = pending.0.take() {
            *store = restored.store;
            game.puzzle_progress = store.placed_count as f32 / store.len() as f32;
            game.puzzle_completed = store.placed_count == store.len();
            progress.is_generating = true;
            progress.total_pieces = store.len();
            progress.pieces_created = store.len();
            progress.grid_size = (
                restored.definition.grid_size.x as usize,
                restored.definition.grid_size.y as usize,
            );
            progress.generation_phase = GenerationPhase::UploadingGpu;
            commands.insert_resource(restored.definition);
            commands.remove_resource::<PendingRestore>();
            return;
        }
    }
    persistence.game_id = crate::persistence::GameId::default();
    let definition = PuzzleDefinition {
        generator_version: GENERATOR_VERSION,
        seed: config.seed,
        grid_size: UVec2::new(config.grid_size.0 as u32, config.grid_size.1 as u32),
        image_size: image.logical_size,
        snap_distance: config.snap_distance,
    };
    if let Err(error) = definition.validate() {
        progress.error = Some(GenerationError::InvalidDefinition(error.into()));
        progress.generation_phase = GenerationPhase::Failed;
        return;
    }
    commands.insert_resource(definition);
}

/// Runs when leaving a session for Menu, including the completed puzzle viewer.
// ECS dependencies and query filters are explicit to keep Bevy access visible.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(crate) fn cleanup_game(
    mut commands: Commands,
    entities: Query<Entity, Or<(With<GridReference>, With<SelectionBox>)>>,
    mut store: ResMut<PieceDataStore>,
    mut input: ResMut<InputState>,
    mut interaction: ResMut<crate::interaction::PieceInteraction>,
    mut selection: ResMut<crate::selection::PuzzleSelection>,
    mut progress: ResMut<PieceGenerationProgress>,
    mut game: ResMut<GameData>,
    mut roster: ResMut<PlayerRoster>,
    mut config: ResMut<PuzzleConfig>,
    file_registry: Res<crate::asset_reader::ExternalFileRegistry>,
    mut overlay: ResMut<crate::render::SelectionOverlay>,
    mut persistence: ResMut<PersistenceState>,
    mut autosave: ResMut<crate::persistence::autosave::AutosaveTimer>,
) {
    for entity in &entities {
        commands.entity(entity).despawn();
    }
    *store = default();
    *overlay = default();
    *input = default();
    *interaction = default();
    selection.cancel();
    *progress = default();
    *game = default();
    roster.clear();
    file_registry.clear();
    config.image_path.clear();
    persistence.generation = persistence.generation.wrapping_add(1);
    persistence.current_save = None;
    persistence.current_autosave = None;
    persistence.autosaving = false;
    persistence.autosave_error = None;
    *autosave = default();
    persistence.busy = false;
    persistence.title_dialog_open = false;
    persistence.error = None;
    persistence.message = None;
    persistence.capture = None;
    commands.remove_resource::<OriginalPuzzleImage>();
    commands.remove_resource::<PendingRestore>();
    commands.remove_resource::<PuzzleImage>();
    commands.remove_resource::<ImageLoadError>();
    commands.remove_resource::<PuzzleDefinition>();
}

fn reset_local_player(mut local_player: ResMut<LocalPlayerId>, mut host: ResMut<SessionHostId>) {
    *local_player = LocalPlayerId::default();
    *host = default();
}

fn clear_session_messages(
    mut intents: ResMut<Messages<ClientCommand>>,
    mut moves: ResMut<Messages<PieceMoveCompleted>>,
    mut placed: ResMut<Messages<PiecePlacedEvent>>,
) {
    intents.clear();
    moves.clear();
    placed.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::{
        asset::AssetPlugin, input::InputPlugin, state::app::StatesPlugin,
        transform::TransformPlugin,
    };

    #[test]
    fn returning_to_menu_clears_image_load_failure_and_external_paths() {
        let (service, _requests) = PersistenceService::with_storage_requests();
        let mut app = App::new();
        app.insert_resource(service)
            .add_plugins((
                MinimalPlugins,
                StatesPlugin,
                InputPlugin,
                TransformPlugin,
                AssetPlugin::default(),
                crate::asset_reader::DirectFileAssetPlugin,
                GamePlugin,
            ))
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<ColorMaterial>>()
            .init_resource::<Assets<Image>>()
            .init_resource::<bevy_egui::EguiUserTextures>();
        app.update();
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::GameSetup);
        app.update();
        let key = app
            .world()
            .resource::<crate::asset_reader::ExternalFileRegistry>()
            .register_file("broken.png");
        app.world()
            .resource::<crate::asset_reader::ExternalFileRegistry>()
            .register_file("older.png");
        app.world_mut().resource_mut::<PuzzleConfig>().image_path = key.clone();
        app.world()
            .resource::<ImageLoadSender>()
            .tx_results
            .send(crate::asset_reader::ImageLoadResult {
                virtual_key: key,
                image: Err("Image is too large".into()),
                original: None,
            })
            .unwrap();
        app.update();
        assert!(app.world().contains_resource::<ImageLoadError>());
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::Menu);
        app.update();
        assert!(!app.world().contains_resource::<ImageLoadError>());
        assert!(app.world().resource::<PuzzleConfig>().image_path.is_empty());
        assert!(app
            .world()
            .resource::<crate::asset_reader::ExternalFileRegistry>()
            .is_empty());
    }

    #[test]
    fn selected_image_is_imported_and_encoded_ram_is_released_before_play() {
        use crate::persistence::{runtime::*, *};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("source.png");
        image::RgbImage::from_pixel(2, 2, image::Rgb([73, 191, 15]))
            .save(&path)
            .unwrap();
        let original_bytes = std::fs::read(&path).unwrap();
        let root = dir.path().join("data");
        let mut app = App::new();
        app.insert_resource(PersistenceService::for_test(root.clone()));
        app.add_plugins((
            MinimalPlugins,
            StatesPlugin,
            InputPlugin,
            TransformPlugin,
            AssetPlugin::default(),
            crate::asset_reader::DirectFileAssetPlugin,
            GamePlugin,
        ))
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<ColorMaterial>>()
        .init_resource::<Assets<Image>>()
        .init_resource::<bevy_egui::EguiUserTextures>();
        app.update();
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::GameSetup);
        app.update();
        app.world_mut().resource_mut::<PuzzleConfig>().image_path = "fixture.png".into();
        crate::asset_reader::start_thread_image_load(
            "fixture.png".into(),
            path.clone(),
            app.world().resource::<ImageLoadSender>().tx_results.clone(),
            ImageDecodeLimits {
                max_texture_dimension: 8192,
            },
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            app.update();
            if app
                .world()
                .get_resource::<OriginalPuzzleImage>()
                .is_some_and(|r| r.encoded.is_none())
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "Image import must release retained bytes"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        std::fs::remove_file(path).unwrap();
        let hash = app.world().resource::<OriginalPuzzleImage>().hash;
        assert_eq!(hash, image_hash(&original_bytes));
        assert_eq!(
            SaveRepository::new(FilesystemStorage::new(root))
                .read_image(hash)
                .unwrap(),
            original_bytes
        );
        assert!(app.world().get_resource::<PuzzleImage>().is_some());
    }

    #[test]
    fn persistent_load_installs_directly_waits_for_gpu_and_resumes_same_save() {
        use crate::{checkpoint::*, persistence::runtime::*, persistence::*};
        let dir = tempfile::tempdir().unwrap();
        let mut encoded = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            2,
            2,
            image::Rgb([20, 80, 190]),
        ))
        .write_to(&mut encoded, image::ImageFormat::Png)
        .unwrap();
        let encoded = encoded.into_inner();
        let repo = SaveRepository::new(FilesystemStorage::new(dir.path()));
        for (complete, during_drag) in [(false, false), (false, true), (true, false)] {
            let definition = PuzzleDefinition {
                generator_version: GENERATOR_VERSION,
                seed: 271,
                grid_size: UVec2::splat(2),
                image_size: UVec2::splat(2),
                snap_distance: 0.5,
            };
            let pieces = (0..4)
                .map(|index| SnapshotPieceState {
                    position: definition.correct_position(PieceId(index))
                        + if complete || index == 0 {
                            Vec2::ZERO
                        } else {
                            Vec2::splat(2.5)
                        },
                    z_order: index,
                    flags: if complete || index == 0 {
                        SNAPSHOT_PLACED
                    } else {
                        0
                    },
                })
                .collect();
            let checkpoint = PuzzleCheckpoint {
                definition: definition.clone(),
                image_hash: image_hash(&encoded),
                next_z_order: 4,
                pieces,
            };
            let metadata = repo
                .create(
                    SaveTitle::new("Restored puzzle").unwrap(),
                    checkpoint.clone(),
                    Some(&encoded),
                )
                .unwrap();
            let mut app = App::new();
            app.insert_resource(PersistenceService::for_test(dir.path().to_path_buf()));
            app.add_plugins((
                MinimalPlugins,
                StatesPlugin,
                InputPlugin,
                TransformPlugin,
                AssetPlugin::default(),
                crate::asset_reader::DirectFileAssetPlugin,
                GamePlugin,
            ))
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<ColorMaterial>>()
            .init_resource::<Assets<Image>>()
            .init_resource::<bevy_egui::EguiUserTextures>();
            app.update();
            app.insert_resource(crate::render::RenderReady::waiting_for_test());
            app.world_mut()
                .resource_scope(|world, service: Mut<PersistenceService>| {
                    service.load(
                        &mut world.resource_mut::<PersistenceState>(),
                        metadata.id,
                        ImageDecodeLimits {
                            max_texture_dimension: 8192,
                        },
                    );
                });
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while *app.world().resource::<State<AppState>>().get() == AppState::Menu {
                app.update();
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            for _ in 0..3 {
                app.update();
            }
            assert_eq!(
                *app.world().resource::<State<AppState>>().get(),
                AppState::InGame
            );
            assert_eq!(
                *app.world().resource::<State<GameSubState>>().get(),
                GameSubState::Initializing
            );
            let progress = app.world().resource::<PieceGenerationProgress>();
            assert_eq!(progress.generation_phase, GenerationPhase::UploadingGpu);
            assert!(
                progress.receiver.is_none(),
                "Restore must never start random placement generation"
            );
            let store = app.world().resource::<PieceDataStore>();
            assert_eq!(
                PuzzleCheckpoint::capture(store, &definition, image_hash(&encoded)).unwrap(),
                checkpoint
            );
            assert_eq!(store.placed_count, if complete { 4 } else { 1 });
            assert_eq!(
                app.world().resource::<GameData>().puzzle_progress,
                if complete { 1.0 } else { 0.25 }
            );
            assert_eq!(
                app.world().resource::<GameData>().puzzle_completed,
                complete
            );
            assert!(app
                .world()
                .resource::<OriginalPuzzleImage>()
                .encoded
                .is_none());
            let epoch = store.epoch;
            app.world()
                .resource::<crate::render::RenderReady>()
                .signal_for_test(epoch);
            app.update();
            app.update();
            if complete {
                assert_eq!(
                    *app.world().resource::<State<AppState>>().get(),
                    AppState::GameComplete
                );
            } else {
                assert_eq!(
                    *app.world().resource::<State<GameSubState>>().get(),
                    GameSubState::Playing
                );
                app.world_mut()
                    .resource_mut::<NextState<GameSubState>>()
                    .set(GameSubState::Paused);
                app.update();
            }
            if !complete {
                // Exercise both a save during an ongoing drag and a UI save
                // queued in the release frame, through the actual GamePlugin schedule.
                {
                    let mut store = app.world_mut().resource_mut::<PieceDataStore>();
                    store.apply_command(
                        LOCAL_PLAYER,
                        &PieceCommand::Grab(PieceId(1)),
                        Some(&definition),
                        puzzella_core::LOCAL_PLAYER,
                    );
                    if during_drag {
                        store.drag.members = std::sync::Arc::from([1 << 1]);
                        store.drag.delta = Vec2::splat(100.0);
                    } else {
                        // The interaction adapter clears presentation when it queues release.
                        store.drag = default();
                    }
                }
                if !during_drag {
                    let mut members = PieceBitSet::new(4);
                    members.insert(PieceId(1));
                    app.world_mut().write_message(ClientCommand {
                        player: LOCAL_PLAYER,
                        command: PieceCommand::ReleaseGroup {
                            members,
                            delta: Vec2::splat(4.0),
                        },
                    });
                }
            }
            app.world_mut()
                .resource_scope(|world, service: Mut<PersistenceService>| {
                    let mut state = world.resource_mut::<PersistenceState>();
                    assert_eq!(state.current_save.as_ref().unwrap().id, metadata.id);
                    service.request_save(&mut state, SaveTitle::new("Renamed").unwrap());
                });
            while app.world().resource::<PersistenceState>().busy {
                app.update();
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            assert!(
                app.world().resource::<PersistenceState>().error.is_none(),
                "{:?}",
                app.world().resource::<PersistenceState>().error
            );
            let current = app
                .world()
                .resource::<PersistenceState>()
                .current_save
                .as_ref()
                .unwrap()
                .clone();
            assert_eq!(current.id, metadata.id);
            assert_eq!(current.revision, 2);
            assert_eq!(current.created_at, metadata.created_at);
            assert_eq!(current.title.as_str(), "Renamed");
            assert_eq!(repo.load(metadata.id).unwrap().save.metadata, current);
            if !complete {
                assert_eq!(
                    repo.load(metadata.id).unwrap().save.checkpoint.pieces[1].position,
                    checkpoint.pieces[1].position
                        + if during_drag {
                            Vec2::ZERO
                        } else {
                            Vec2::splat(4.0)
                        }
                );
                if during_drag {
                    let store = app.world().resource::<PieceDataStore>();
                    assert_eq!(store.states[1].position, checkpoint.pieces[1].position);
                    assert_eq!(store.held_by.get(&PieceId(1)), Some(&LOCAL_PLAYER));
                    assert_eq!(&*store.drag.members, &[1 << 1]);
                    assert_eq!(store.drag.delta, Vec2::splat(100.0));
                    assert_ne!(store.states[1].flags & crate::resources::pieces::HELD, 0);
                }
            }
            // Another writer advances the save while this session retains its
            // loaded revision. A stale Save must leave both the game and file intact.
            let latest = repo.load(current.id).unwrap().save;
            let external = repo
                .update(
                    current.id,
                    current.revision,
                    SaveTitle::new("Saved elsewhere").unwrap(),
                    latest.checkpoint,
                    None,
                )
                .unwrap();
            let states_before = app.world().resource::<PieceDataStore>().states.clone();
            app.world_mut()
                .resource_scope(|world, service: Mut<PersistenceService>| {
                    let mut state = world.resource_mut::<PersistenceState>();
                    service.request_save(&mut state, SaveTitle::new("Stale update").unwrap());
                });
            while app.world().resource::<PersistenceState>().busy {
                app.update();
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            let state = app.world().resource::<PersistenceState>();
            assert!(matches!(
                state.error.as_ref(),
                Some(crate::persistence::runtime::PersistenceError::Save(
                    crate::persistence::SaveError::Conflict {
                        expected_revision: 2,
                        actual_revision: 3,
                        ..
                    }
                ))
            ));
            assert_eq!(state.current_save.as_ref(), Some(&current));
            assert!(state.message.is_none());
            assert_eq!(repo.load(current.id).unwrap().save.metadata, external);
            assert_eq!(
                app.world().resource::<PieceDataStore>().states,
                states_before
            );
            app.world_mut()
                .resource_mut::<NextState<AppState>>()
                .set(AppState::Menu);
            app.update();
            assert!(app.world().get_resource::<OriginalPuzzleImage>().is_none());
            assert!(app
                .world()
                .resource::<PersistenceState>()
                .current_save
                .is_none());
        }
    }

    #[test]
    fn session_lifecycle_resets_dense_state_workers_and_substate() {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            StatesPlugin,
            InputPlugin,
            TransformPlugin,
            AssetPlugin::default(),
            crate::asset_reader::DirectFileAssetPlugin,
            GamePlugin,
        ))
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<ColorMaterial>>()
        .init_resource::<Assets<Image>>()
        .init_resource::<bevy_egui::EguiUserTextures>();
        app.update();
        assert_eq!(
            *app.world().resource::<State<AppState>>().get(),
            AppState::Menu
        );
        for seed in [42, 43] {
            app.world_mut()
                .resource_mut::<NextState<AppState>>()
                .set(AppState::GameSetup);
            app.update();
            let handle = app
                .world_mut()
                .resource_mut::<Assets<Image>>()
                .add(Image::default());
            app.world_mut().insert_resource(PuzzleImage {
                handle,
                logical_size: UVec2::splat(200),
                texture_size: UVec2::ONE,
                opaque: true,
            });
            let mut config = app.world_mut().resource_mut::<PuzzleConfig>();
            config.grid_size = (2, 2);
            config.seed = seed;
            config.image_path = "fixture".into();
            app.world_mut()
                .resource_mut::<NextState<AppState>>()
                .set(AppState::InGame);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                app.update();
                if app
                    .world()
                    .get_resource::<State<GameSubState>>()
                    .is_some_and(|s| *s.get() == GameSubState::Playing)
                {
                    break;
                }
                assert!(std::time::Instant::now() < deadline, "generation timed out");
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            assert_eq!(app.world().resource::<PieceDataStore>().len(), 4);
            assert_eq!(app.world().resource::<Assets<Mesh>>().len(), 0);
            assert_eq!(
                app.world_mut()
                    .query::<&Transform>()
                    .iter(app.world())
                    .count(),
                2
            );
            assert_eq!(app.world().resource::<PuzzleDefinition>().seed, seed);
            app.world_mut()
                .resource_mut::<NextState<GameSubState>>()
                .set(GameSubState::Paused);
            app.update();
            assert_eq!(
                *app.world().resource::<State<GameSubState>>().get(),
                GameSubState::Paused
            );
            app.world_mut()
                .resource_mut::<NextState<GameSubState>>()
                .set(GameSubState::Playing);
            app.update();
            for index in 0..4 {
                let id = PieceId(index);
                let position = app
                    .world()
                    .resource::<PuzzleDefinition>()
                    .piece(id.0, Vec2::ZERO)
                    .correct_position;
                for command in [
                    PieceCommand::Grab(id),
                    PieceCommand::Move { id, position },
                    PieceCommand::Release(id),
                ] {
                    app.world_mut().write_message(ClientCommand {
                        player: LOCAL_PLAYER,
                        command,
                    });
                }
            }
            app.update();
            app.update();
            assert_eq!(
                *app.world().resource::<State<AppState>>().get(),
                AppState::GameComplete
            );
            assert_eq!(app.world().resource::<PieceDataStore>().placed_count, 4);
            assert_eq!(
                *app.world().resource::<State<GameCompleteSubState>>().get(),
                GameCompleteSubState::Summary
            );
            assert!(app.world().get_resource::<State<GameSubState>>().is_none());

            // ESC on the result card must not open the viewer's pause menu.
            press_escape(&mut app);
            assert_eq!(
                *app.world().resource::<State<GameCompleteSubState>>().get(),
                GameCompleteSubState::Summary
            );
            if seed == 43 {
                let epoch = app.world().resource::<PieceDataStore>().epoch;
                let states = app.world().resource::<PieceDataStore>().states.to_vec();
                let image = app.world().resource::<PuzzleImage>().handle.clone();
                app.world_mut()
                    .resource_mut::<NextState<GameCompleteSubState>>()
                    .set(GameCompleteSubState::Viewing);
                app.update();

                let camera = app
                    .world_mut()
                    .query_filtered::<Entity, With<MainCamera>>()
                    .single(app.world())
                    .unwrap();
                let transform =
                    Transform::from_xyz(45.0, -20.0, 0.0).with_scale(Vec3::new(2.0, 2.0, 1.0));
                *app.world_mut().get_mut::<Transform>(camera).unwrap() = transform;
                app.world_mut()
                    .resource_mut::<InputState>()
                    .is_camera_dragging = true;
                press_escape(&mut app);
                assert_eq!(
                    *app.world().resource::<State<GameCompleteSubState>>().get(),
                    GameCompleteSubState::Paused
                );
                assert!(!app.world().resource::<InputState>().is_camera_dragging);
                press_escape(&mut app);
                assert_eq!(
                    *app.world().resource::<State<GameCompleteSubState>>().get(),
                    GameCompleteSubState::Viewing
                );
                // Viewing and resuming preserve the session, locked pieces and camera.
                let store = app.world().resource::<PieceDataStore>();
                assert_eq!(store.epoch, epoch);
                assert_eq!(&*store.states, states.as_slice());
                assert_eq!(store.placed_count, 4);
                assert_eq!(app.world().resource::<PuzzleImage>().handle, image);
                assert_eq!(app.world().resource::<PuzzleDefinition>().seed, seed);
                assert!(app.world().resource::<GameData>().puzzle_completed);
                assert_eq!(app.world().resource::<GameData>().puzzle_progress, 1.0);
                assert_eq!(app.world().get::<Transform>(camera).unwrap(), &transform);
                press_escape(&mut app);
            }
            app.world_mut()
                .resource_mut::<NextState<AppState>>()
                .set(AppState::Menu);
            app.update();
            assert!(app.world().resource::<PieceDataStore>().is_empty());
            assert!(app.world().get_resource::<PuzzleImage>().is_none());
            assert!(app.world().get_resource::<PuzzleDefinition>().is_none());
            assert!(app.world().get_resource::<State<GameSubState>>().is_none());
            assert!(app
                .world()
                .get_resource::<State<GameCompleteSubState>>()
                .is_none());
            assert_eq!(
                app.world_mut()
                    .query::<&Transform>()
                    .iter(app.world())
                    .count(),
                1
            );
        }
    }

    fn press_escape(app: &mut App) {
        // Inject after InputPlugin's PreUpdate reset, then apply the queued transition.
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Escape);
        app.world_mut().run_schedule(Update);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .reset(KeyCode::Escape);
        app.update();
    }
}

#[cfg(test)]
mod local_identity_tests {
    use super::*;
    use crate::multiplayer::{GameSnapshot, SnapshotExpectation};
    use bevy::ecs::system::RunSystemOnce;
    use bevy::{
        asset::AssetPlugin, input::InputPlugin, state::app::StatesPlugin,
        transform::TransformPlugin,
    };
    use puzzella_core::session::{AuthorityCursor, ImageHash, SessionDefinition, SessionId};

    #[test]
    fn initialization_and_snapshot_restore_use_independent_process_identity() {
        for local in [LocalPlayerId::default(), LocalPlayerId(PlayerId(42))] {
            let mut app = App::new();
            app.insert_resource(local)
                .init_resource::<PersistenceState>()
                .init_resource::<PuzzleConfig>()
                .init_resource::<GameData>()
                .init_resource::<PlayerRoster>()
                .init_resource::<PieceGenerationProgress>()
                .init_resource::<PieceDataStore>()
                .insert_resource(PuzzleImage {
                    handle: default(),
                    logical_size: UVec2::new(40, 20),
                    texture_size: UVec2::new(40, 20),
                    opaque: true,
                });
            app.world_mut().run_system_once(initialize_game).unwrap();
            let first_game_id = app.world().resource::<PersistenceState>().game_id;
            assert_eq!(uuid::Uuid::from_u128(first_game_id.0).get_version_num(), 4);
            assert_eq!(
                app.world()
                    .resource::<PlayerRoster>()
                    .players()
                    .next()
                    .unwrap()
                    .id,
                local.0
            );
            let definition = PuzzleDefinition {
                generator_version: GENERATOR_VERSION,
                seed: 42,
                grid_size: UVec2::new(2, 1),
                image_size: UVec2::new(40, 20),
                snap_distance: 5.0,
            };
            let mut source = PieceDataStore::default();
            source.initialize(vec![Vec2::splat(100.0); 2]);
            let session = SessionDefinition {
                id: SessionId(123),
                image_hash: ImageHash([7; 32]),
            };
            let cursor = AuthorityCursor::new(3, 0);
            let snapshot = GameSnapshot::capture(&source, &definition, session, cursor).unwrap();
            snapshot
                .install(
                    &mut app.world_mut().resource_mut::<PieceDataStore>(),
                    SnapshotExpectation {
                        session: session.id,
                        image_hash: session.image_hash,
                        cursor,
                        definition: &definition,
                    },
                )
                .unwrap();
            assert_eq!(*app.world().resource::<LocalPlayerId>(), local);
            assert_eq!(app.world().resource::<PieceDataStore>().len(), 2);
            let mut profile = crate::player_settings::PlayerSettingsState::load(None);
            assert!(profile.commit("Offline 🧩"));
            app.insert_resource(profile);
            app.world_mut().run_system_once(initialize_game).unwrap();
            assert_eq!(
                app.world()
                    .resource::<PlayerRoster>()
                    .get(local.0)
                    .unwrap()
                    .display_name
                    .as_ref()
                    .unwrap()
                    .as_ref(),
                "Offline 🧩"
            );
            assert_eq!(app.world().resource::<PlayerRoster>().len(), 1);
            assert_ne!(
                app.world().resource::<PersistenceState>().game_id,
                first_game_id
            );
            assert_eq!(
                app.world()
                    .resource::<PlayerRoster>()
                    .players()
                    .next()
                    .unwrap()
                    .id,
                local.0
            );
        }
        assert_eq!(LocalPlayerId::default().0, PlayerId(0));
    }

    #[test]
    fn returning_to_menu_resets_identity_before_next_offline_game() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new();
        app.insert_resource(PersistenceService::for_test(dir.path().join("data")))
            .add_plugins((
                MinimalPlugins,
                StatesPlugin,
                InputPlugin,
                TransformPlugin,
                AssetPlugin::default(),
                crate::asset_reader::DirectFileAssetPlugin,
                GamePlugin,
            ))
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<ColorMaterial>>()
            .init_resource::<Assets<Image>>()
            .init_resource::<bevy_egui::EguiUserTextures>();
        app.update();
        assert_eq!(
            *app.world().resource::<LocalPlayerId>(),
            LocalPlayerId::default()
        );
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::GameSetup);
        app.update();
        app.insert_resource(LocalPlayerId(PlayerId(42)));
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::Menu);
        app.update();
        assert_eq!(
            *app.world().resource::<LocalPlayerId>(),
            LocalPlayerId::default()
        );
        app.insert_resource(PuzzleImage {
            handle: default(),
            logical_size: UVec2::new(40, 20),
            texture_size: UVec2::new(40, 20),
            opaque: true,
        });
        app.world_mut().run_system_once(initialize_game).unwrap();
        assert_eq!(
            app.world()
                .resource::<PlayerRoster>()
                .players()
                .next()
                .unwrap()
                .id,
            PlayerId(0)
        );
    }
}
