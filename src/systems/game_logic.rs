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

/// ピースの配置チェック - イベントドリブン版（最適化）
pub fn check_piece_placement_event_driven(
    mut commands: Commands,
    mut piece_query: Query<(Entity, &mut Transform, &mut PuzzlePiece), With<PickablePiece>>,
    puzzle_config: Res<PuzzleConfig>,
    mut move_events: EventReader<PieceMoveCompleted>,
    mut placed_events: EventWriter<PiecePlacedEvent>,
    mut perf_monitor: ResMut<PerformanceMonitor>,
) {
    let _span = info_span!("check_piece_placement_event_driven").entered();
    let start_time = perf_monitor.start_system_timing("check_piece_placement_event_driven");
    
    // 移動完了したピースのみをチェック（イベントドリブン）
    for move_event in move_events.read() {
        if let Ok((entity, mut transform, mut piece)) = piece_query.get_mut(move_event.entity) {
            // まだ配置されていないピースのみチェック
            if !piece.is_placed {
                let current_pos = transform.translation.truncate();
                let correct_pos = piece.correct_position;
                let distance = current_pos.distance(correct_pos);
                
                println!("🎯 Event-driven placement check: piece({},{}) at ({:.1},{:.1}), correct ({:.1},{:.1}), distance {:.1}, snap threshold {:.1}",
                    piece.grid_x, piece.grid_y, 
                    current_pos.x, current_pos.y, 
                    correct_pos.x, correct_pos.y, 
                    distance, puzzle_config.snap_distance);
                
                if distance < puzzle_config.snap_distance {
                    transform.translation = correct_pos.extend(-20.0); // 固定ピースは最も下のZ値
                    piece.is_placed = true;
                    piece.current_position = correct_pos;
                    
                    // ピース配置完了イベントを発火
                    placed_events.write(PiecePlacedEvent {
                        entity,
                        grid_x: piece.grid_x,
                        grid_y: piece.grid_y,
                    });
                    
                    // PickablePieceコンポーネントを削除して移動不可にする
                    commands.entity(entity).remove::<PickablePiece>();
                    
                    println!("✅ Piece({},{}) PLACED! Distance {:.1} < threshold {:.1}", 
                        piece.grid_x, piece.grid_y, distance, puzzle_config.snap_distance);
                }
            }
        }
    }
    
    perf_monitor.end_system_timing("check_piece_placement_event_driven", start_time);
}

/// レガシー版のピース配置チェック（後方互換性のため保持）
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

/// ゲーム状態の更新 - イベントドリブン版（最適化）
pub fn update_game_state_event_driven(
    mut game_state: ResMut<GameData>,
    piece_query: Query<&PuzzlePiece>,
    mut next_state: ResMut<NextState<AppState>>,
    mut placed_events: EventReader<PiecePlacedEvent>,
    mut perf_monitor: ResMut<PerformanceMonitor>,
) {
    let _span = info_span!("update_game_state_event_driven").entered();
    let start_time = perf_monitor.start_system_timing("update_game_state_event_driven");
    
    // ピース配置イベントがある場合のみ更新
    if placed_events.read().count() > 0 {
        let total_pieces = piece_query.iter().count();
        let placed_pieces = piece_query.iter().filter(|p| p.is_placed).count();
        
        if total_pieces > 0 {
            game_state.puzzle_progress = placed_pieces as f32 / total_pieces as f32;
            game_state.puzzle_completed = placed_pieces == total_pieces;
            
            println!("🎯 Game progress updated: {}/{} pieces placed ({:.1}%)", 
                placed_pieces, total_pieces, game_state.puzzle_progress * 100.0);
            
            if game_state.puzzle_completed && game_state.current_screen == GameScreen::InGame {
                game_state.current_screen = GameScreen::GameComplete;
                next_state.set(AppState::GameComplete);
                println!("🎉 Puzzle completed!");
            }
        }
    }
    
    perf_monitor.end_system_timing("update_game_state_event_driven", start_time);
}

/// レガシー版のゲーム状態更新（後方互換性のため保持）
pub fn update_game_state(
    mut game_state: ResMut<GameData>,
    piece_query: Query<&PuzzlePiece>,
    mut next_state: ResMut<NextState<AppState>>,
    mut perf_monitor: ResMut<PerformanceMonitor>,
) {
    let start_time = perf_monitor.start_system_timing("update_game_state");
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
    
    perf_monitor.end_system_timing("update_game_state", start_time);
}

