use bevy::prelude::*;
use crate::components::*;
use crate::resources::*;
use crate::systems::*;
use crate::puzzle::update_puzzle_image_size;

pub struct GamePlugin;

impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        app.add_event::<PieceMoveCompleted>()
            .add_event::<PiecePlacedEvent>()
            .init_resource::<GameData>()
            .init_resource::<PuzzleConfig>()
            .init_resource::<NetworkInfo>()
            .init_resource::<InputState>()
            .init_resource::<PieceGenerationProgress>()
            .init_resource::<StrokeMeshCache>()
            .init_resource::<PieceSelectionCache>()
            .init_resource::<PieceIdManager>()
            .init_resource::<PieceCollisionSystem>()
            .init_resource::<PerformanceMonitor>()
            .init_resource::<HighlightState>()
            .insert_state(AppState::Loading)
            .insert_state(GameSubState::Initializing)
            .add_systems(Startup, (setup_game, setup_highlight_materials, setup_image_load_system))
            .add_systems(First, performance_frame_start.run_if(performance_monitoring_enabled))
            // State transition systems
            .add_systems(OnEnter(AppState::Loading), transition_to_menu)
            .add_systems(OnEnter(AppState::InGame), (initialize_game, auto_adjust_camera_zoom))
            .add_systems(OnExit(AppState::InGame), cleanup_game)
            // GameSetup state systems
            .add_systems(Update, 
                (handle_image_load_results, update_puzzle_image_size)
                .run_if(in_state(AppState::GameSetup))
            )
            // InGame state systems - パズル生成中のみ実行
            .add_systems(Update, (
                spawn_puzzle_pieces_progressive,
                register_new_pieces_to_id_manager,
                register_new_pieces_to_collision_system,
            ).run_if(in_state(AppState::InGame).and(in_state(GameSubState::Initializing))))
            // InGame state systems - プレイ中に実行（基本システム）
            .add_systems(Update, (
                // 基本システム
                update_input_state,
                
                // メニュー操作（プレイ中にESCを検出してポーズに移行）
                toggle_game_menu.run_if(escape_just_pressed),
                
                // 選択システム
                update_piece_cache,
                render_selection_box.run_if(should_render_selection_box),
                handle_box_selection,
                handle_multi_piece_drag,
                
                // レガシー & ハイライト
                handle_piece_dragging_hybrid_legacy,
                highlight_selected_pieces,
                
                // ゲームロジック
                check_piece_placement_event_driven,
                update_game_state_event_driven,
                
                // カメラ
                handle_camera_zoom,
                handle_camera_drag,
                handle_edge_scrolling,
            ).run_if(in_state(AppState::InGame).and(in_state(GameSubState::Playing))))
            // InGame state systems - プレイ中に実行（管理システム）
            .add_systems(Update, (
                // ID管理システム
                cleanup_removed_pieces_from_id_manager,
                debug_id_manager_stats,
                
                // コリジョンシステム
                update_collision_system_positions,
                cleanup_removed_pieces_from_collision_system,
                debug_collision_system_stats,
                optimize_collision_system,
                test_ray_casting,
                test_collision_api,
                performance_test_collision_system,
                
                // パフォーマンス計測システム
                toggle_performance_debug.run_if(f12_just_pressed),
                performance_report_system.run_if(should_report_performance),
            ).run_if(in_state(AppState::InGame).and(in_state(GameSubState::Playing))))
            // InGame state systems - ポーズ中に実行
            .add_systems(Update, (
                // メニュー操作（ポーズ中にESCを検出してプレイに復帰）
                toggle_game_menu.run_if(escape_just_pressed),
                
                // パフォーマンス計測システム（ポーズ中でも利用可能）
                toggle_performance_debug.run_if(f12_just_pressed),
                performance_report_system.run_if(should_report_performance),
            ).run_if(in_state(AppState::InGame).and(in_state(GameSubState::Paused))))
            .add_systems(Last, performance_frame_end.run_if(performance_monitoring_enabled));
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

/// Loading -> Menu への遷移
fn transition_to_menu(mut next_state: ResMut<NextState<AppState>>) {
    println!("🚀 Application loaded, transitioning to menu");
    next_state.set(AppState::Menu);
}

// 画像読み込み完了チェック関数は削除
// 開始ボタンを押したときのみゲームを開始するようにしました

/// ゲーム開始時の初期化
fn initialize_game(
    mut game_data: ResMut<GameData>,
    mut input_state: ResMut<InputState>,
    mut piece_cache: ResMut<PieceSelectionCache>,
    mut highlight_state: ResMut<HighlightState>,
    mut game_sub_state: ResMut<NextState<GameSubState>>,
) {
    println!("🎮 Initializing game...");
    
    // ゲームデータをリセット
    game_data.puzzle_completed = false;
    game_data.puzzle_progress = 0.0;
    game_data.needs_reset = false;
    
    // 入力状態をリセット
    input_state.selected_piece = None;
    input_state.next_z_order = 1.0;
    input_state.selected_pieces.clear();
    input_state.selected_pieces_set.clear();
    input_state.multi_drag_offset.clear();
    input_state.last_selection_rect = None;
    input_state.cached_drag_entity = None;
    
    // パフォーマンスキャッシュをクリア
    piece_cache.all_pieces.clear();
    piece_cache.piece_positions.clear();
    piece_cache.piece_bounds.clear();
    piece_cache.need_refresh = true;
    
    // ハイライト状態をリセット
    highlight_state.last_selected_pieces.clear();
    highlight_state.last_preview_pieces.clear();
    highlight_state.selection_changed = false;
    highlight_state.preview_changed = false;
    highlight_state.frame_count = 0;
    
    // ゲームサブ状態を初期化に設定
    game_sub_state.set(GameSubState::Initializing);
}

/// ゲーム終了時のクリーンアップ（reset_puzzle機能も統合）
fn cleanup_game(
    mut commands: Commands,
    puzzle_pieces: Query<Entity, With<PuzzlePiece>>,
    grid_references: Query<Entity, With<GridReference>>,
    outline_entities: Query<Entity, With<PieceOutline>>,
    mut piece_cache: ResMut<PieceSelectionCache>,
    mut game_data: ResMut<GameData>,
    mut input_state: ResMut<InputState>,
    mut puzzle_config: ResMut<PuzzleConfig>,
) {
    println!("🧹 Cleaning up game...");
    
    // すべてのパズルピースを削除
    for entity in puzzle_pieces.iter() {
        commands.entity(entity).despawn();
    }
    
    // グリッド背景画像も削除
    for entity in grid_references.iter() {
        commands.entity(entity).despawn();
    }
    
    // アウトラインエンティティを削除
    for entity in outline_entities.iter() {
        commands.entity(entity).despawn();
    }
    
    // ゲーム状態をリセット
    game_data.puzzle_completed = false;
    game_data.puzzle_progress = 0.0;
    game_data.needs_reset = false;
    
    // 入力状態をリセット
    input_state.selected_piece = None;
    input_state.next_z_order = 1.0;
    input_state.selected_pieces.clear();
    input_state.selected_pieces_set.clear();
    input_state.multi_drag_offset.clear();
    input_state.last_selection_rect = None;
    input_state.cached_drag_entity = None;
    
    // パフォーマンスキャッシュをクリア
    piece_cache.all_pieces.clear();
    piece_cache.piece_positions.clear();
    piece_cache.piece_bounds.clear();
    piece_cache.need_refresh = true;
    
    // 画像設定を完全にクリア
    puzzle_config.image_path.clear();
    
    // PuzzleImageリソースを削除して再読み込みを強制
    commands.remove_resource::<PuzzleImage>();
    
    println!("✅ Game cleanup completed (all entities, states, and resources cleared)");
}

