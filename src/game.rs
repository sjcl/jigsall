use bevy::prelude::*;
use crate::components::*;
use crate::resources::*;
use crate::systems::*;
use crate::puzzle::update_puzzle_image_size;

pub struct GamePlugin;

impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GameState>()
            .init_resource::<PuzzleConfig>()
            .init_resource::<NetworkInfo>()
            .init_resource::<InputState>()
            .init_resource::<PieceGenerationProgress>()
            .init_resource::<StrokeMeshCache>()
            .add_systems(Startup, setup_game)
            .add_systems(
                Update,
                (
                    // simple_game_start, // 無効化
                    update_puzzle_image_size,
                    auto_adjust_camera_zoom,
                    update_input_state,
                    reset_puzzle, // パズルリセット機能
                    
                    // 新しいマルチ選択システム（優先順位: 最初に実行）
                    handle_box_selection,
                    handle_multi_piece_drag,
                    render_selection_box,
                    
                    // 既存のレガシーシステム（マルチ選択と共存）
                    handle_piece_dragging_hybrid_legacy, // マルチ選択時は無効化
                    
                    // ハイライト関連は移動処理の後に実行
                    highlight_selected_pieces,
                    
                    check_piece_placement,
                    update_game_state,
                    spawn_puzzle_pieces,
                    spawn_puzzle_pieces_progressive, // 新しいプログレッシブ生成システム
                    handle_camera_zoom,
                    handle_camera_drag,
                    frustum_culling_system, // 画面外のピースを非表示にする
                    handle_escape_input, // ESCキー入力処理
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