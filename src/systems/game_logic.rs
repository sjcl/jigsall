use bevy::prelude::*;
use crate::components::*;
use crate::resources::*;

/// ESCキーが押されたかチェックするRun Condition
pub fn escape_just_pressed(keyboard_input: Res<ButtonInput<KeyCode>>) -> bool {
    keyboard_input.just_pressed(KeyCode::Escape)
}

/// InGame状態かつゲーム画面でのESCキー処理
pub fn toggle_game_menu(
    mut game_state: ResMut<GameData>,
    mut next_sub_state: ResMut<NextState<GameSubState>>,
) {
    match game_state.current_screen {
        GameScreen::InGame => {
            // ゲーム中にESCキーが押されたらメニューを表示
            game_state.current_screen = GameScreen::InGameMenu;
            next_sub_state.set(GameSubState::Paused);
            println!("🎮 Opening in-game menu");
        },
        GameScreen::InGameMenu => {
            // メニュー表示中にESCキーが押されたらゲームに戻る
            game_state.current_screen = GameScreen::InGame;
            next_sub_state.set(GameSubState::Playing);
            println!("🎮 Resuming game");
        },
        _ => {
            // 他の画面では何もしない
        }
    }
}

/// ピースの配置チェック - 正しい位置に近い場合にスナップする
pub fn check_piece_placement(
    mut commands: Commands,
    mut piece_query: Query<(Entity, &mut Transform, &mut PuzzlePiece), With<PickablePiece>>,
    selected_pieces_query: Query<Entity, With<SelectedPiece>>,
    puzzle_config: Res<PuzzleConfig>,
    input_state: Res<InputState>,
) {
    // 複数選択中または複数ドラッグ中の場合はスナップを無効化
    let is_multi_selection_active = matches!(input_state.selection_mode, SelectionMode::BoxSelection | SelectionMode::MultiDrag);
    let selected_count = selected_pieces_query.iter().count();
    let has_multiple_selected = selected_count > 1;
    
    if is_multi_selection_active || has_multiple_selected {
        // 複数選択関連のモードの場合はスナップを無効化
        static mut SNAP_DISABLE_LOG_COUNT: usize = 0;
        unsafe {
            if SNAP_DISABLE_LOG_COUNT < 5 {
                println!("🚫 Snap disabled: mode={:?}, selected_count={}, multi_active={}", 
                    input_state.selection_mode, selected_count, is_multi_selection_active);
                SNAP_DISABLE_LOG_COUNT += 1;
            }
        }
        return;
    }
    
    for (entity, mut transform, mut piece) in piece_query.iter_mut() {
        // 現在ドラッグ中でなく、かつまだ配置されていないピースのみチェック
        let is_currently_dragged = input_state.selected_piece == Some(entity);
        if !is_currently_dragged && !piece.is_placed {
            let current_pos = transform.translation.truncate();
            let correct_pos = piece.correct_position;
            let distance = current_pos.distance(correct_pos);
            
            // Only log successful placements to reduce noise
            // println!("🎯 Checking placement: piece({},{}) at ({:.1},{:.1}), correct ({:.1},{:.1}), distance {:.1}, snap threshold {:.1}",
            //     piece.grid_x, piece.grid_y, 
            //     current_pos.x, current_pos.y, 
            //     correct_pos.x, correct_pos.y, 
            //     distance, puzzle_config.snap_distance);
            
            if distance < puzzle_config.snap_distance {
                transform.translation = correct_pos.extend(-20.0); // 固定ピースは最も下のZ値
                piece.is_placed = true;
                piece.current_position = correct_pos;
                
                // PickablePieceコンポーネントを削除して移動不可にする
                commands.entity(entity).remove::<PickablePiece>();
                
                println!("✅ Piece({},{}) PLACED! Distance {:.1} < threshold {:.1}", 
                    piece.grid_x, piece.grid_y, distance, puzzle_config.snap_distance);
            }
        }
    }
}

/// ゲーム状態の更新 - 完了チェックとプログレス計算
pub fn update_game_state(
    mut game_state: ResMut<GameData>,
    piece_query: Query<&PuzzlePiece>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    let total_pieces = piece_query.iter().count();
    let placed_pieces = piece_query.iter().filter(|p| p.is_placed).count();
    
    if total_pieces > 0 {
        game_state.puzzle_progress = placed_pieces as f32 / total_pieces as f32;
        game_state.puzzle_completed = placed_pieces == total_pieces;
        
        if game_state.puzzle_completed && game_state.current_screen == GameScreen::InGame {
            game_state.current_screen = GameScreen::GameComplete;
            next_state.set(AppState::GameComplete);
        }
    }
}

