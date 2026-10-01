use crate::puzzle::update_puzzle_image_size;
use crate::{components::*, gameplay::*, networking::ClientCommand, resources::*, systems::*};
use bevy::prelude::*;

pub struct GamePlugin;
impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<ClientCommand>()
            .add_message::<PieceMoveCompleted>()
            .add_message::<PiecePlacedEvent>()
            .add_message::<BatchRebuildRequest>()
            .init_resource::<GameData>()
            .init_resource::<PuzzleConfig>()
            .init_resource::<InputState>()
            .init_resource::<PieceGenerationProgress>()
            .init_resource::<StrokeMeshCache>()
            .init_resource::<PieceIdManager>()
            .init_resource::<PieceCollisionSystem>()
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
            .add_systems(
                OnEnter(GameSubState::Playing),
                register_pieces_from_data_store_to_collision_system,
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
                Update,
                (
                    update_input_state,
                    handle_piece_input,
                    handle_camera_zoom,
                    handle_camera_drag,
                    handle_edge_scrolling,
                )
                    .chain()
                    .before(apply_piece_commands)
                    .run_if(in_state(GameSubState::Playing)),
            )
            .add_systems(
                Update,
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
                    .run_if(in_state(AppState::InGame)),
            )
            .add_systems(
                Update,
                (
                    toggle_game_menu.run_if(escape_just_pressed),
                    toggle_performance_debug.run_if(f12_just_pressed),
                    performance_report_system.run_if(should_report_performance),
                    debug_collision_system_stats,
                    test_ray_casting,
                    test_collision_api,
                    performance_test_collision_system,
                    monitor_batch_system,
                )
                    .run_if(in_state(AppState::InGame)),
            )
            .add_systems(
                Last,
                performance_frame_end.run_if(performance_monitoring_enabled),
            );
    }
}
fn setup_game(mut commands: Commands) {
    commands.spawn((Camera2d, MainCamera));
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
        )>,
    >,
    mut store: ResMut<PieceDataStore>,
    mut batch: ResMut<BatchManager>,
    mut input: ResMut<InputState>,
    mut collision: ResMut<PieceCollisionSystem>,
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
    *collision = default();
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
