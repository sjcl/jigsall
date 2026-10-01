use crate::puzzle::update_puzzle_image_size;
use crate::{components::*, gameplay::*, networking::ClientCommand, resources::*, systems::*};
use bevy::prelude::*;
use bevy::transform::TransformSystems;
use bevy_egui::EguiPostUpdateSet;

pub struct GamePlugin;
impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(crate::selection::PuzzleSelectionPlugin)
            .add_message::<ClientCommand>()
            .add_message::<PieceMoveCompleted>()
            .add_message::<PiecePlacedEvent>()
            .add_message::<BatchRebuildRequest>()
            .init_resource::<GameData>()
            .init_resource::<PuzzleConfig>()
            .init_resource::<InputState>()
            .init_resource::<GameUiPointerCapture>()
            .init_resource::<crate::interaction::PieceInteraction>()
            .init_resource::<PieceGenerationProgress>()
            .init_resource::<StrokeMeshCache>()
            .init_resource::<PieceIdManager>()
            .init_resource::<PerformanceMonitor>()
            .init_resource::<HighlightState>()
            .init_resource::<PieceDataStore>()
            .init_resource::<BatchManager>()
            .init_state::<AppState>()
            .add_sub_state::<GameSubState>()
            .add_systems(
                Startup,
                (
                    setup_game,
                    setup_highlight_materials,
                    setup_image_load_system,
                ),
            )
            .add_systems(
                First,
                performance_frame_start.run_if(performance_monitoring_enabled),
            )
            .add_systems(
                OnEnter(AppState::Menu),
                (cleanup_game, clear_session_messages),
            )
            .add_systems(
                OnEnter(AppState::InGame),
                (
                    initialize_game,
                    spawn_grid_reference,
                    auto_adjust_camera_zoom,
                )
                    .chain(),
            )
            .add_systems(OnEnter(GameSubState::Paused), release_local_drag)
            .add_systems(
                Update,
                (handle_image_load_results, update_puzzle_image_size)
                    .chain()
                    .run_if(in_state(AppState::GameSetup)),
            )
            .add_systems(
                Update,
                spawn_puzzle_pieces_progressive.run_if(in_state(GameSubState::Initializing)),
            )
            .add_systems(
                PostUpdate,
                (
                    handle_camera_zoom,
                    handle_camera_drag,
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
                    apply_piece_commands,
                    check_piece_placement_event_driven,
                    project_piece_states,
                    update_game_state_event_driven,
                    reconcile_piece_rendering,
                    cleanup_temporary_entities,
                    create_temporary_entities,
                    sync_selection_markers,
                    handle_batch_rebuild_requests,
                    highlight_selected_pieces,
                    render_selection_box.run_if(should_render_selection_box),
                )
                    .chain()
                    .after(EguiPostUpdateSet::EndPass)
                    .before(TransformSystems::Propagate)
                    .run_if(in_state(AppState::InGame)),
            )
            .add_systems(
                Update,
                (
                    toggle_game_menu.run_if(escape_just_pressed),
                    toggle_performance_debug.run_if(f12_just_pressed),
                    performance_report_system.run_if(should_report_performance),
                    monitor_batch_system,
                )
                    .run_if(in_state(AppState::InGame)),
            )
            .add_systems(
                Last,
                performance_frame_end.run_if(performance_monitoring_enabled),
            );
        #[cfg(any(test, feature = "cpu-picking-debug"))]
        app.init_resource::<PieceCollisionSystem>()
            .add_systems(
                OnEnter(GameSubState::Playing),
                register_pieces_from_data_store_to_collision_system,
            )
            .add_systems(
                Update,
                (
                    debug_collision_system_stats,
                    test_ray_casting,
                    test_collision_api,
                    performance_test_collision_system,
                )
                    .run_if(in_state(AppState::InGame)),
            );
    }
}
fn setup_game(mut commands: Commands) {
    // Single-sample normal rendering and picking share pixel coverage at edges.
    commands.spawn((Camera2d, MainCamera, Msaa::Off));
}
fn setup_highlight_materials(mut commands: Commands, mut materials: ResMut<Assets<ColorMaterial>>) {
    commands.insert_resource(HighlightMaterials {
        preview_material: materials.add(ColorMaterial::from(Color::srgba(0.3, 0.6, 1.0, 0.8))),
        selected_material: materials.add(ColorMaterial::from(Color::srgba(1.0, 0.8, 0.0, 1.0))),
    });
}
fn initialize_game(
    mut commands: Commands,
    config: Res<PuzzleConfig>,
    image: Res<PuzzleImage>,
    mut game: ResMut<GameData>,
    mut progress: ResMut<PieceGenerationProgress>,
) {
    *game = GameData {
        players: vec![PlayerInfo {
            id: LOCAL_PLAYER,
            name: "Player".into(),
            score: 0,
        }],
        ..default()
    };
    *progress = PieceGenerationProgress::default();
    let definition = PuzzleDefinition {
        generator_version: GENERATOR_VERSION,
        seed: config.seed,
        grid_size: UVec2::new(config.grid_size.0 as u32, config.grid_size.1 as u32),
        image_size: image.size.as_uvec2(),
        snap_distance: config.snap_distance,
    };
    if let Err(error) = definition.validate() {
        progress.error = Some(error.into());
        progress.generation_phase = GenerationPhase::Failed;
        return;
    }
    commands.insert_resource(definition);
}

/// Runs when leaving a session for Menu, including completion -> new game.
// ECS dependencies and query filters are explicit to keep Bevy access visible.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn cleanup_game(
    mut commands: Commands,
    entities: Query<
        Entity,
        Or<(
            With<PuzzlePiece>,
            With<BatchedMeshEntity>,
            With<GridReference>,
            With<SelectionBox>,
            With<crate::selection::PuzzlePieceId>,
        )>,
    >,
    mut store: ResMut<PieceDataStore>,
    mut batch: ResMut<BatchManager>,
    mut input: ResMut<InputState>,
    mut interaction: ResMut<crate::interaction::PieceInteraction>,
    mut selection: ResMut<crate::selection::PuzzleSelection>,
    #[cfg(any(test, feature = "cpu-picking-debug"))] mut collision: ResMut<PieceCollisionSystem>,
    mut ids: ResMut<PieceIdManager>,
    mut progress: ResMut<PieceGenerationProgress>,
    mut stroke: ResMut<StrokeMeshCache>,
    mut highlight: ResMut<HighlightState>,
    mut game: ResMut<GameData>,
    mut config: ResMut<PuzzleConfig>,
) {
    for entity in &entities {
        commands.entity(entity).despawn();
    }
    *store = default();
    *batch = default();
    *input = default();
    *interaction = default();
    selection.cancel();
    #[cfg(any(test, feature = "cpu-picking-debug"))]
    {
        *collision = default();
    }
    *ids = default();
    *progress = default();
    *stroke = default();
    *highlight = default();
    *game = default();
    config.image_path.clear();
    commands.remove_resource::<PuzzleImage>();
    commands.remove_resource::<PuzzleDefinition>();
}

fn clear_session_messages(
    mut intents: ResMut<Messages<ClientCommand>>,
    mut moves: ResMut<Messages<PieceMoveCompleted>>,
    mut placed: ResMut<Messages<PiecePlacedEvent>>,
    mut rebuild: ResMut<Messages<BatchRebuildRequest>>,
) {
    intents.clear();
    moves.clear();
    placed.clear();
    rebuild.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::{
        asset::AssetPlugin, input::InputPlugin, state::app::StatesPlugin,
        transform::TransformPlugin,
    };

    #[test]
    fn session_lifecycle_resets_batches_workers_and_substate() {
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
                size: Vec2::splat(200.0),
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
            assert_eq!(app.world().resource::<PieceDataStore>().pieces.len(), 4);
            assert_eq!(
                app.world().resource::<PieceCollisionSystem>().pieces.len(),
                4
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
                let position = app.world().resource::<PieceDataStore>().pieces[&id]
                    .definition
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
            assert_eq!(
                app.world().resource::<PieceDataStore>().placed_pieces.len(),
                4
            );
            app.world_mut()
                .resource_mut::<NextState<AppState>>()
                .set(AppState::Menu);
            app.update();
            assert!(app.world().resource::<PieceDataStore>().pieces.is_empty());
            assert!(app
                .world()
                .resource::<BatchManager>()
                .batched_pieces
                .is_empty());
            assert!(app.world().get_resource::<PuzzleImage>().is_none());
            assert!(app.world().get_resource::<PuzzleDefinition>().is_none());
            assert!(app.world().get_resource::<State<GameSubState>>().is_none());
            assert_eq!(
                app.world_mut()
                    .query_filtered::<Entity, With<BatchedMeshEntity>>()
                    .iter(app.world())
                    .count(),
                0
            );
        }
    }
}
