use bevy::prelude::*;
use crate::components::*;
use crate::resources::*;
use crate::systems::*;
use crate::puzzle::{setup_puzzle_from_image, update_puzzle_image_size};

pub struct GamePlugin;

impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GameState>()
            .init_resource::<PuzzleConfig>()
            .init_resource::<NetworkInfo>()
            .init_resource::<InputState>()
            .add_systems(Startup, (setup_game, setup_puzzle_from_image))
            .add_systems(
                Update,
                (
                    // simple_game_start, // 無効化
                    update_puzzle_image_size,
                    auto_adjust_camera_zoom,
                    update_input_state,
                    handle_piece_dragging,
                    check_piece_placement,
                    update_game_state,
                    spawn_puzzle_pieces,
                    handle_camera_zoom,
                    handle_camera_drag,
                ),
            );
    }
}

fn setup_game(mut commands: Commands) {
    // 2Dカメラを設定
    commands.spawn((
        Camera2d,
        MainCamera,
    ));
}