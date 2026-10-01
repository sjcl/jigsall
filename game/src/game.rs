use crate::systems::image_loading::update_puzzle_image_size;
use crate::{components::*, resources::*, systems::*};
use bevy::prelude::*;
use bevy::transform::TransformSystems;
use bevy_egui::EguiPostUpdateSet;
use puzzella_core::ClientCommand;
use puzzella_core::*;

pub struct GamePlugin;
impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(crate::selection::PuzzleSelectionPlugin)
            .add_message::<ClientCommand>()
            .add_message::<PieceMoveCompleted>()
            .add_message::<PiecePlacedEvent>()
            .init_resource::<GameData>()
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
            .add_systems(Startup, (setup_game, setup_image_load_system))
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
                    reset_performance_samples,
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
                generate_puzzle_state.run_if(in_state(GameSubState::Initializing)),
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
                (
                    toggle_game_menu.run_if(escape_just_pressed),
                    (
                        toggle_performance_debug.run_if(f3_just_pressed),
                        sample_performance_frame.run_if(performance_monitoring_enabled),
                    )
                        .chain(),
                )
                    .run_if(in_state(AppState::InGame)),
            )
            .add_systems(Last, crate::resources::pieces::prepare_piece_upload);
    }
}
fn setup_game(mut commands: Commands) {
    // Single-sample normal rendering and picking share pixel coverage at edges.
    commands.spawn((Camera2d, MainCamera, Msaa::Off));
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
    entities: Query<Entity, Or<(With<GridReference>, With<SelectionBox>)>>,
    mut store: ResMut<PieceDataStore>,
    mut input: ResMut<InputState>,
    mut interaction: ResMut<crate::interaction::PieceInteraction>,
    mut selection: ResMut<crate::selection::PuzzleSelection>,
    mut progress: ResMut<PieceGenerationProgress>,
    mut game: ResMut<GameData>,
    mut config: ResMut<PuzzleConfig>,
    mut overlay: ResMut<crate::render::SelectionOverlay>,
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
    config.image_path.clear();
    commands.remove_resource::<PuzzleImage>();
    commands.remove_resource::<PuzzleDefinition>();
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
                size: Vec2::splat(200.0),
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
            app.world_mut()
                .resource_mut::<NextState<AppState>>()
                .set(AppState::Menu);
            app.update();
            assert!(app.world().resource::<PieceDataStore>().is_empty());
            assert!(app.world().get_resource::<PuzzleImage>().is_none());
            assert!(app.world().get_resource::<PuzzleDefinition>().is_none());
            assert!(app.world().get_resource::<State<GameSubState>>().is_none());
            assert_eq!(
                app.world_mut()
                    .query::<&Transform>()
                    .iter(app.world())
                    .count(),
                1
            );
        }
    }
}
