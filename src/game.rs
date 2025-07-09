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
            .init_resource::<PieceSelectionCache>()
            .init_resource::<PerformanceMonitor>()
            .init_resource::<HighlightState>()
            .add_systems(Startup, (setup_game, setup_highlight_materials))
            .add_systems(First, performance_frame_start)
            .add_systems(
                Update,
                (
                    // 基本システム
                    update_puzzle_image_size,
                    auto_adjust_camera_zoom,
                    update_input_state,
                    reset_puzzle,
                    
                    // 選択システム
                    update_piece_cache,
                    handle_box_selection,
                    handle_multi_piece_drag,
                    render_selection_box,
                    
                    // レガシー & ハイライト
                    handle_piece_dragging_hybrid_legacy,
                    highlight_selected_pieces,
                    
                    // ゲームロジック
                    check_piece_placement,
                    update_game_state,
                    spawn_puzzle_pieces,
                ),
            )
            .add_systems(
                Update,
                (
                    // パズル生成とカメラ
                    spawn_puzzle_pieces_progressive,
                    handle_camera_zoom,
                    handle_camera_drag,
                    frustum_culling_system,
                    handle_escape_input,
                    
                    // パフォーマンス計測システム
                    performance_toggle_system,
                    performance_report_system,
                ),
            )
            .add_systems(Last, performance_frame_end);
    }
}

fn setup_game(mut commands: Commands) {
    // 2Dカメラを設定
    commands.spawn((
        Camera2d,
        MainCamera,
    ));
}

fn setup_highlight_materials(
    mut commands: Commands,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    // プレビュー用マテリアル（薄い青色）
    let preview_material = ColorMaterial {
        color: Color::srgba(0.3, 0.6, 1.0, 0.8),
        ..Default::default()
    };
    let preview_material_handle = materials.add(preview_material);
    
    // 選択用マテリアル（黄色）
    let selected_material = ColorMaterial {
        color: Color::srgba(1.0, 0.8, 0.0, 1.0),
        ..Default::default()
    };
    let selected_material_handle = materials.add(selected_material);
    
    // リソースとして登録
    commands.insert_resource(HighlightMaterials {
        preview_material: preview_material_handle,
        selected_material: selected_material_handle,
    });
}