use bevy::prelude::*;
use bevy::sprite::ColorMaterial;
use std::collections::HashSet;
use crate::components::*;
use crate::resources::*;
use crate::time_scope;

/// ピース検索キャッシュの更新システム
pub fn update_piece_cache(
    mut cache: ResMut<PieceSelectionCache>,
    piece_query: Query<(Entity, &Transform, &PuzzlePiece), (With<PickablePiece>, Changed<Transform>)>,
    mut perf_monitor: ResMut<PerformanceMonitor>,
) {
    let start_time = perf_monitor.start_system_timing("update_piece_cache");
    
    // Transformが変更されたピースのみキャッシュ更新
    let mut updated_count = 0;
    for (entity, transform, piece) in piece_query.iter() {
        let position = transform.translation.truncate();
        cache.piece_positions.insert(entity, position);
        
        let min_bound = position + piece.bounds.min;
        let max_bound = position + piece.bounds.max;
        cache.piece_bounds.insert(entity, (min_bound, max_bound));
        
        cache.need_refresh = true;
        updated_count += 1;
    }
    
    // 詳細計測（High レベル）
    if perf_monitor.debug_level == PerformanceDebugLevel::High && updated_count > 0 {
        time_scope!(perf_monitor, "update_piece_cache_detailed", {
            let timing = perf_monitor.system_timings
                .entry("update_piece_cache_pieces_updated".to_string())
                .or_insert_with(|| SystemTiming::new("update_piece_cache_pieces_updated".to_string()));
            timing.record_timing(std::time::Duration::from_nanos(updated_count as u64));
        });
    }
    
    perf_monitor.end_system_timing("update_piece_cache", start_time);
}

/// 効率的なピースクリック判定（レガシーシステム用）
fn find_clicked_piece_for_legacy(
    world_pos: Vec2,
    cache: &PieceSelectionCache,
    piece_query: &Query<(Entity, &mut Transform, &mut PickablePiece, &PuzzlePiece, &PieceShape)>,
) -> Option<Entity> {
    let mut clicked_piece = None;
    let mut highest_z = f32::NEG_INFINITY;
    
    // キャッシュされた境界ボックスで高速フィルタリング
    for &entity in &cache.all_pieces {
        if let Some((min_bound, max_bound)) = cache.piece_bounds.get(&entity) {
            // 境界ボックス判定
            if world_pos.x >= min_bound.x && world_pos.x <= max_bound.x &&
               world_pos.y >= min_bound.y && world_pos.y <= max_bound.y {
                
                // 実際のジグソー形状内かチェック
                if let Ok((_, transform, _, _piece, shape)) = piece_query.get(entity) {
                    let local_pos = world_pos - transform.translation.truncate();
                    
                    if point_in_mesh(&shape.vertices, &shape.indices, local_pos) {
                        if transform.translation.z > highest_z {
                            clicked_piece = Some(entity);
                            highest_z = transform.translation.z;
                        }
                    }
                }
            }
        }
    }
    
    clicked_piece
}

/// 効率的なピースクリック判定（ボックス選択用）
fn find_clicked_piece_for_box_selection(
    world_pos: Vec2,
    cache: &PieceSelectionCache,
    piece_query: &Query<(Entity, &mut Transform, &PuzzlePiece, &PieceShape), With<PickablePiece>>,
) -> Option<Entity> {
    let mut clicked_piece = None;
    let mut highest_z = f32::NEG_INFINITY;
    
    // キャッシュされた境界ボックスで高速フィルタリング
    for &entity in &cache.all_pieces {
        if let Some((min_bound, max_bound)) = cache.piece_bounds.get(&entity) {
            // 境界ボックス判定
            if world_pos.x >= min_bound.x && world_pos.x <= max_bound.x &&
               world_pos.y >= min_bound.y && world_pos.y <= max_bound.y {
                
                // 実際のジグソー形状内かチェック
                if let Ok((_, transform, _piece, shape)) = piece_query.get(entity) {
                    let local_pos = world_pos - transform.translation.truncate();
                    
                    if point_in_mesh(&shape.vertices, &shape.indices, local_pos) {
                        if transform.translation.z > highest_z {
                            clicked_piece = Some(entity);
                            highest_z = transform.translation.z;
                        }
                    }
                }
            }
        }
    }
    
    clicked_piece
}

/// Ray-casting アルゴリズムを使った点内判定
fn point_in_mesh(vertices: &[[f32; 2]], indices: &[u32], point: Vec2) -> bool {
    let mut intersections = 0;
    let ray_y = point.y;
    
    
    // 全ての三角形の辺をチェック
    for triangle in indices.chunks(3) {
        if triangle.len() < 3 { continue; }
        
        let v0 = vertices[triangle[0] as usize];
        let v1 = vertices[triangle[1] as usize];
        let v2 = vertices[triangle[2] as usize];
        
        // 点が三角形内にあるかの直接チェック（より確実）
        if point_in_triangle(point, v0, v1, v2) {
            return true;
        }
        
        // 三角形の各辺について交点チェック
        intersections += count_ray_edge_intersections(point, ray_y, v0, v1);
        intersections += count_ray_edge_intersections(point, ray_y, v1, v2);
        intersections += count_ray_edge_intersections(point, ray_y, v2, v0);
    }
    
    // 奇数個の交点 = 点が内部にある
    intersections % 2 == 1
}


/// 点が三角形内にあるかを重心座標で判定（より正確）
fn point_in_triangle(point: Vec2, v0: [f32; 2], v1: [f32; 2], v2: [f32; 2]) -> bool {
    let px = point.x;
    let py = point.y;
    
    let x0 = v0[0];
    let y0 = v0[1];
    let x1 = v1[0];
    let y1 = v1[1];
    let x2 = v2[0];
    let y2 = v2[1];
    
    // 重心座標による判定
    let denom = (y1 - y2) * (x0 - x2) + (x2 - x1) * (y0 - y2);
    if denom.abs() < 1e-10 { return false; } // 三角形が縮退している
    
    let a = ((y1 - y2) * (px - x2) + (x2 - x1) * (py - y2)) / denom;
    let b = ((y2 - y0) * (px - x2) + (x0 - x2) * (py - y2)) / denom;
    let c = 1.0 - a - b;
    
    // 重心座標がすべて非負なら三角形内
    a >= -1e-6 && b >= -1e-6 && c >= -1e-6
}


/// 水平線と線分の交点数を計算（改善版）
fn count_ray_edge_intersections(point: Vec2, ray_y: f32, edge_start: [f32; 2], edge_end: [f32; 2]) -> usize {
    let y1 = edge_start[1];
    let y2 = edge_end[1];
    
    // 微小な誤差を考慮した範囲チェック
    let epsilon = 1e-8;
    
    // 水平線が線分のY範囲内にない場合は交点なし
    let min_y = y1.min(y2);
    let max_y = y1.max(y2);
    
    if ray_y < min_y - epsilon || ray_y > max_y + epsilon {
        return 0;
    }
    
    // 線分が水平の場合（Y座標が同じ）は特別処理
    if (y2 - y1).abs() < epsilon {
        return 0;
    }
    
    // 水平線と線分の交点のX座標を計算
    let x1 = edge_start[0];
    let x2 = edge_end[0];
    let t = (ray_y - y1) / (y2 - y1);
    let intersection_x = x1 + t * (x2 - x1);
    
    // 交点が点より右側にある場合のみカウント（微小な誤差を考慮）
    if intersection_x > point.x + epsilon {
        1
    } else {
        0
    }
}


// レガシーシステム: 新しいマルチ選択システムに置き換え予定
pub fn handle_piece_dragging_hybrid_legacy(
    mut piece_query: Query<(Entity, &mut Transform, &mut PickablePiece, &PuzzlePiece, &PieceShape)>,
    mut input_state: ResMut<InputState>,
    mouse_input: Res<ButtonInput<MouseButton>>,
    puzzle_config: Res<PuzzleConfig>,
    puzzle_image: Option<Res<PuzzleImage>>,
    camera_query: Query<&Transform, (With<MainCamera>, Without<PuzzlePiece>)>,
    game_state: Res<GameState>,
    cache: Res<PieceSelectionCache>,
) {
    // ゲーム内メニューが表示されている間はピースドラッグを無効化
    if game_state.current_screen == GameScreen::InGameMenu {
        return;
    }
    
    // 新しいマルチ選択システムが有効な場合は無効化
    if !matches!(input_state.selection_mode, SelectionMode::Single) || !input_state.selected_pieces.is_empty() {
        return;
    }
    
    let mouse_just_pressed = mouse_input.just_pressed(MouseButton::Left);
    let mouse_pressed = mouse_input.pressed(MouseButton::Left);
    let mouse_just_released = mouse_input.just_released(MouseButton::Left);
    
    // Camera transform for debugging
    let camera_info = if let Ok(camera_transform) = camera_query.single() {
        Some((camera_transform.scale.x, camera_transform.translation.truncate()))
    } else {
        None
    };
    
    // 現在ドラッグ中のピースがあるかチェック（キャッシュ使用）
    let current_dragging_piece = input_state.cached_drag_entity;
    
    // キャッシュ更新
    if let Some(entity) = input_state.selected_piece {
        input_state.cached_drag_entity = Some(entity);
    } else {
        input_state.cached_drag_entity = None;
    }
    
    // マウスクリック開始
    if mouse_just_pressed && !input_state.is_camera_dragging {
        let world_pos = input_state.mouse_position; // 既にワールド座標に変換済み
        
        // 最適化されたピースクリック判定
        let clicked_piece = find_clicked_piece_for_legacy(world_pos, &cache, &piece_query);
        
        // デバッグログ（頻度制限）
        static mut CLICK_LOG_COUNT: usize = 0;
        unsafe {
            CLICK_LOG_COUNT += 1;
            if CLICK_LOG_COUNT <= 5 || CLICK_LOG_COUNT % 100 == 0 {
                let total_pieces = piece_query.iter().count();
                let max_piece_size = if let Some(img) = puzzle_image.as_ref() {
                    let (gw, gh) = puzzle_config.grid_size;
                    (img.size.x / gw as f32).max(img.size.y / gh as f32)
                } else {
                    100.0
                };
                
                println!("🖱️ CLICK DEBUG #{}: Mouse world pos: ({:.1}, {:.1}), Total pieces: {}, Max piece size: {:.1}", 
                        CLICK_LOG_COUNT, input_state.mouse_position.x, input_state.mouse_position.y, total_pieces, max_piece_size);
                
                if let Some(camera_info) = camera_info {
                    println!("   📷 Camera scale: {:.3}, position: ({:.1}, {:.1})", 
                        camera_info.0, camera_info.1.x, camera_info.1.y);
                }
                
                if let Some(entity) = clicked_piece {
                    println!("   ✅ Clicked piece: {:?}", entity);
                } else {
                    println!("   ❌ No piece clicked");
                }
            }
        }
        
        // ピースをクリックした場合、そのピースを選択
        if let Some(entity) = clicked_piece {
            input_state.selected_piece = Some(entity);
            // 即座にドラッグ開始
            for (e, mut transform, mut pickable, piece, _shape) in piece_query.iter_mut() {
                if e == entity {
                    let piece_world_pos = transform.translation.truncate();
                    pickable.drag_offset = piece_world_pos - world_pos;
                    
                    // Z-orderを更新（ドラッグ中のピースが最前面に）
                    transform.translation.z = input_state.next_z_order;
                    input_state.next_z_order += 0.1;
                    
                    println!("🎯 Started dragging piece at grid({}, {}), world pos({:.1}, {:.1})", 
                        piece.grid_x, piece.grid_y, piece_world_pos.x, piece_world_pos.y);
                    
                    // デバッグ情報（100ピース以上の場合）
                    if puzzle_config.grid_size.0 * puzzle_config.grid_size.1 > 100 {
                        static mut PIECE_SIZE_LOG_COUNT: usize = 0;
                        unsafe {
                            PIECE_SIZE_LOG_COUNT += 1;
                            if PIECE_SIZE_LOG_COUNT <= 3 {
                                let (grid_width, grid_height) = puzzle_config.grid_size;
                                println!("🧩 PIECE SIZE DEBUG #{}: Grid size: {}x{}, Piece bounds: ({:.1}, {:.1}) to ({:.1}, {:.1})", 
                                        PIECE_SIZE_LOG_COUNT, grid_width, grid_height, 
                                        piece.bounds.min.x, piece.bounds.min.y,
                                        piece.bounds.max.x, piece.bounds.max.y);
                                println!("   📏 Approximate piece size: {:.1} x {:.1}", 
                                        piece.bounds.width(), piece.bounds.height());
                            }
                        }
                    }
                    break;
                }
            }
        } else {
            // 何もクリックしなかった場合、選択解除
            input_state.selected_piece = None;
        }
    }
    
    // ドラッグ中の処理（現在選択されているピースのみ）
    if mouse_pressed && current_dragging_piece.is_some() {
        let world_pos = input_state.mouse_position; // 既にワールド座標に変換済み
        
        for (entity, mut transform, pickable, _piece, _shape) in piece_query.iter_mut() {
            if input_state.selected_piece == Some(entity) {
                let new_position = world_pos + pickable.drag_offset;
                transform.translation = new_position.extend(transform.translation.z);
            }
        }
    }
    
    // マウスリリース（ドラッグ終了）
    if mouse_just_released {
        // ドラッグ終了
        if let Some(entity) = input_state.selected_piece {
            // ドラッグが終了したピースの情報をログ
            for (e, transform, mut pickable, piece, _shape) in piece_query.iter_mut() {
                if e == entity {
                    pickable.drag_offset = Vec2::ZERO;
                    println!("🎯 Dropped piece at grid({}, {}), world pos({:.1}, {:.1})", 
                        piece.grid_x, piece.grid_y, transform.translation.x, transform.translation.y);
                    break;
                }
            }
        }
        
        input_state.selected_piece = None;
    }
}

pub fn handle_box_selection(
    mut commands: Commands,
    mut input_state: ResMut<InputState>,
    mouse_input: Res<ButtonInput<MouseButton>>,
    keyboard_input: Res<ButtonInput<KeyCode>>,
    piece_query: Query<(Entity, &mut Transform, &PuzzlePiece, &PieceShape), With<PickablePiece>>,
    selected_query: Query<Entity, With<SelectedPiece>>,
    mut cache: ResMut<PieceSelectionCache>,
    game_state: Res<GameState>,
    mut perf_monitor: ResMut<PerformanceMonitor>,
) {
    let start_time = perf_monitor.start_system_timing("handle_box_selection");
    
    // キャッシュ更新が必要な場合のみ更新
    if cache.need_refresh {
        let cache_start = perf_monitor.start_system_timing("box_selection_cache_update");
        
        cache.all_pieces.clear();
        cache.piece_positions.clear();
        cache.piece_bounds.clear();
        
        let mut cache_piece_count = 0;
        for (entity, transform, piece, _) in piece_query.iter() {
            let position = transform.translation.truncate();
            cache.all_pieces.push(entity);
            cache.piece_positions.insert(entity, position);
            
            let min_bound = position + piece.bounds.min;
            let max_bound = position + piece.bounds.max;
            cache.piece_bounds.insert(entity, (min_bound, max_bound));
            cache_piece_count += 1;
        }
        
        cache.need_refresh = false;
        perf_monitor.end_system_timing("box_selection_cache_update", cache_start);
        
        if perf_monitor.debug_level == PerformanceDebugLevel::High {
            time_scope!(perf_monitor, "box_selection_cache_pieces", {
                let timing = perf_monitor.system_timings
                    .entry("box_selection_cached_pieces".to_string())
                    .or_insert_with(|| SystemTiming::new("box_selection_cached_pieces".to_string()));
                timing.record_timing(std::time::Duration::from_nanos(cache_piece_count as u64));
            });
        }
    }
    // ゲーム内メニューが表示されている間は無効化
    if game_state.current_screen != GameScreen::InGame {
        return;
    }

    let mouse_just_pressed = mouse_input.just_pressed(MouseButton::Left);
    let mouse_pressed = mouse_input.pressed(MouseButton::Left);
    let mouse_just_released = mouse_input.just_released(MouseButton::Left);
    
    // ESCキーで選択解除
    if keyboard_input.just_pressed(KeyCode::Escape) {
        // 全ての選択を解除
        for entity in selected_query.iter() {
            commands.entity(entity).remove::<SelectedPiece>();
        }
        input_state.selected_pieces.clear();
        input_state.selected_pieces_set.clear();
        input_state.selection_mode = SelectionMode::Single;
        input_state.selection_start = None;
        input_state.selection_current = None;
        input_state.last_selection_rect = None;
        println!("🔄 Selection cleared");
        return;
    }
    
    match input_state.selection_mode {
        SelectionMode::Single => {
            // マウスクリック開始 - どこをクリックしたかで動作分岐
            if mouse_just_pressed && !input_state.is_camera_dragging {
                let world_pos = input_state.mouse_position; // 既にワールド座標に変換済み
                
                // 最適化されたピースクリック判定
                let clicked_piece = find_clicked_piece_for_box_selection(world_pos, &cache, &piece_query);
                
                if let Some(piece_entity) = clicked_piece {
                    // ピースをクリック - 選択状態をトグル（キャッシュ使用）
                    if input_state.selected_pieces_set.contains(&piece_entity) {
                        // 既に選択済み → 複数ピース移動モードに移行
                        input_state.selection_mode = SelectionMode::MultiDrag;
                        
                        // 各ピースのドラッグオフセットを計算（キャッシュ使用）
                        input_state.multi_drag_offset.clear();
                        let selected_pieces = input_state.selected_pieces.clone();
                        for selected_entity in &selected_pieces {
                            if let Some(cached_pos) = cache.piece_positions.get(selected_entity) {
                                let offset = *cached_pos - world_pos;
                                input_state.multi_drag_offset.insert(*selected_entity, offset);
                            }
                        }
                        println!("🎯 Started multi-piece drag with {} pieces", input_state.selected_pieces.len());
                    } else {
                        // 単一ピースのドラッグ - 既存の選択をクリアして新しいピースを選択
                        // 既存の選択を全てクリア
                        for entity in selected_query.iter() {
                            commands.entity(entity).remove::<SelectedPiece>();
                        }
                        input_state.selected_pieces.clear();
                        input_state.selected_pieces_set.clear();
                        input_state.selected_pieces_set.clear();
                        input_state.multi_drag_offset.clear();
                        
                        // レガシーシステムに処理を委譲（単一ピースドラッグ）
                        input_state.selected_piece = Some(piece_entity);
                        
                        println!("🎯 Started single piece drag");
                    }
                } else {
                    // 空の場所をクリック - 範囲選択モードに移行
                    input_state.selection_mode = SelectionMode::BoxSelection;
                    input_state.selection_start = Some(world_pos);
                    input_state.selection_current = Some(world_pos);
                    
                    // 既存の選択をクリア（Ctrlキー押下でない場合）
                    if !keyboard_input.pressed(KeyCode::ControlLeft) && !keyboard_input.pressed(KeyCode::ControlRight) {
                        for entity in selected_query.iter() {
                            commands.entity(entity).remove::<SelectedPiece>();
                        }
                        input_state.selected_pieces.clear();
                        input_state.selected_pieces_set.clear();
                    }
                    println!("📦 Started box selection");
                }
            }
        }
        
        SelectionMode::BoxSelection => {
            if mouse_pressed {
                // 範囲選択中 - 現在位置を更新
                let world_pos = input_state.mouse_position; // 既にワールド座標に変換済み
                input_state.selection_current = Some(world_pos);
                
                // 範囲選択プレビューを更新
                if let (Some(start), Some(end)) = (input_state.selection_start, input_state.selection_current) {
                    // 選択範囲が前回と異なる場合のみプレビュー更新を実行
                    let current_rect = (start, end);
                    if input_state.last_selection_rect != Some(current_rect) {
                        input_state.last_selection_rect = Some(current_rect);
                        
                        // 既存のプレビューをクリア（キャッシュ使用）
                        for &entity in &cache.all_pieces {
                            commands.entity(entity).remove::<SelectionPreview>();
                        }
                        
                        // 選択範囲の境界を事前計算
                        let select_min_x = start.x.min(end.x);
                        let select_max_x = start.x.max(end.x);
                        let select_min_y = start.y.min(end.y);
                        let select_max_y = start.y.max(end.y);
                        
                        // キャッシュされた境界ボックスで高速判定
                        for &entity in &cache.all_pieces {
                            if let Some((piece_min, piece_max)) = cache.piece_bounds.get(&entity) {
                                // 矩形の重なり判定
                                let overlaps = piece_min.x <= select_max_x && 
                                              piece_max.x >= select_min_x &&
                                              piece_min.y <= select_max_y && 
                                              piece_max.y >= select_min_y;
                                
                                if overlaps && !input_state.selected_pieces_set.contains(&entity) {
                                    commands.entity(entity).insert(SelectionPreview);
                                }
                            }
                        }
                    }
                }
            }
            
            if mouse_just_released {
                // 範囲選択終了 - 範囲内のピースを選択（キャッシュ使用）
                if let (Some(start), Some(end)) = (input_state.selection_start, input_state.selection_current) {
                    let mut newly_selected = 0;
                    
                    // 選択範囲の境界を事前計算
                    let select_min_x = start.x.min(end.x);
                    let select_max_x = start.x.max(end.x);
                    let select_min_y = start.y.min(end.y);
                    let select_max_y = start.y.max(end.y);
                    
                    // キャッシュされた境界ボックスで高速判定
                    for &entity in &cache.all_pieces {
                        if let Some((piece_min, piece_max)) = cache.piece_bounds.get(&entity) {
                            // 矩形の重なり判定
                            let overlaps = piece_min.x <= select_max_x && 
                                          piece_max.x >= select_min_x &&
                                          piece_min.y <= select_max_y && 
                                          piece_max.y >= select_min_y;
                            
                            if overlaps {
                                commands.entity(entity).insert(SelectedPiece);
                                input_state.selected_pieces.push(entity);
                                input_state.selected_pieces_set.insert(entity);
                                newly_selected += 1;
                            }
                        }
                    }
                    
                    println!("📦 Box selection completed: {} new pieces selected (total: {})", 
                             newly_selected, input_state.selected_pieces.len());
                }
                
                // プレビューをクリア（キャッシュ使用）
                for &entity in &cache.all_pieces {
                    commands.entity(entity).remove::<SelectionPreview>();
                }
                
                // 範囲選択モード終了
                input_state.selection_mode = SelectionMode::Single;
                input_state.selection_start = None;
                input_state.selection_current = None;
                input_state.last_selection_rect = None;
            }
        }
        
        SelectionMode::MultiDrag => {
            // MultiDragモードの処理は handle_multi_piece_drag で行う
        }
    }
    
    perf_monitor.end_system_timing("handle_box_selection", start_time);
}

pub fn handle_multi_piece_drag(
    mut input_state: ResMut<InputState>,
    mouse_input: Res<ButtonInput<MouseButton>>,
    mut piece_query: Query<(Entity, &mut Transform, &PuzzlePiece), With<SelectedPiece>>,
    game_state: Res<GameState>,
) {
    // ゲーム内メニューが表示されている間は無効化
    if game_state.current_screen != GameScreen::InGame {
        return;
    }
    
    // MultiDragモードでない場合は何もしない
    if !matches!(input_state.selection_mode, SelectionMode::MultiDrag) {
        return;
    }
    
    let mouse_pressed = mouse_input.pressed(MouseButton::Left);
    let mouse_just_released = mouse_input.just_released(MouseButton::Left);
    
    if mouse_pressed {
        // ドラッグ中 - 全ての選択されたピースを移動
        let current_world_pos = input_state.mouse_position; // 既にワールド座標に変換済み
        
        for (entity, mut transform, _piece) in piece_query.iter_mut() {
            if let Some(offset) = input_state.multi_drag_offset.get(&entity) {
                let new_position = current_world_pos + *offset;
                transform.translation = new_position.extend(input_state.next_z_order);
            }
        }
        
        // Z-orderを更新（ドラッグ中のピースが最前面に）
        if piece_query.iter().count() > 0 {
            input_state.next_z_order += 0.1;
        }
    }
    
    if mouse_just_released {
        // ドラッグ終了 - MultiDragモードを終了
        input_state.selection_mode = SelectionMode::Single;
        input_state.multi_drag_offset.clear();
        
        // ドラッグ終了時にZ-orderを調整
        for (_, mut transform, _) in piece_query.iter_mut() {
            transform.translation.z = input_state.next_z_order;
            input_state.next_z_order += 0.1;
        }
        
        println!("🎯 Multi-piece drag completed");
    }
}

pub fn render_selection_box(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
    input_state: Res<InputState>,
    _camera_query: Query<&Transform, (With<MainCamera>, Without<SelectionBox>)>,
    selection_box_query: Query<(Entity, &mut Transform), (With<SelectionBox>, Without<MainCamera>)>,
    game_state: Res<GameState>,
) {
    // ゲーム内メニューが表示されている間は無効化
    if game_state.current_screen != GameScreen::InGame {
        return;
    }
    
    // 既存の選択ボックスを削除
    for (entity, _) in selection_box_query.iter() {
        commands.entity(entity).despawn();
    }
    
    // 範囲選択中の場合のみ描画
    if matches!(input_state.selection_mode, SelectionMode::BoxSelection) {
        if let (Some(start), Some(current)) = (input_state.selection_start, input_state.selection_current) {
            // 矩形の大きさを計算
            let width = (current.x - start.x).abs();
            let height = (current.y - start.y).abs();
            let center_x = (start.x + current.x) / 2.0;
            let center_y = (start.y + current.y) / 2.0;
            
            if width > 1.0 && height > 1.0 {
                // 選択範囲の矩形メッシュを作成
                let mesh = Mesh::from(Rectangle::new(width, height));
                let material = ColorMaterial::from(Color::srgba(0.3, 0.6, 1.0, 0.3)); // 半透明の青
                
                // 選択ボックスエンティティを生成
                commands.spawn((
                    Mesh2d(meshes.add(mesh)),
                    MeshMaterial2d(materials.add(material)),
                    Transform::from_translation(Vec3::new(center_x, center_y, 100.0)), // 最前面に表示
                    SelectionBox,
                ));
            }
        }
    }
}

/// 枠線表示のためのシンプルなアプローチ - 元のメッシュをそのまま使用

/// 選択されたピースのハイライト表示システム（最適化版）
pub fn highlight_selected_pieces(
    mut commands: Commands,
    selected_pieces_query: Query<Entity, (With<SelectedPiece>, With<PuzzlePiece>)>,
    preview_pieces_query: Query<Entity, (With<SelectionPreview>, With<PuzzlePiece>)>,
    _all_pieces_query: Query<Entity, With<PuzzlePiece>>,
    mut piece_query: Query<&mut MeshMaterial2d<ColorMaterial>, With<PuzzlePiece>>,
    piece_transform_query: Query<&Transform, With<PuzzlePiece>>,
    piece_shape_query: Query<&PieceShape, With<PuzzlePiece>>,
    existing_outline_query: Query<Entity, With<PieceOutline>>,
    stroke_cache: Res<StrokeMeshCache>,
    game_state: Res<GameState>,
    cache: Res<PieceSelectionCache>,
    mut perf_monitor: ResMut<PerformanceMonitor>,
    mut highlight_state: ResMut<HighlightState>,
    highlight_materials: Res<HighlightMaterials>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    let start_time = perf_monitor.start_system_timing("highlight_selected_pieces");
    
    // ゲーム内メニューが表示されている間は無効化
    if game_state.current_screen != GameScreen::InGame {
        perf_monitor.end_system_timing("highlight_selected_pieces", start_time);
        return;
    }
    
    // 現在の選択状態を取得
    let change_detection_start = perf_monitor.start_system_timing("highlight_change_detection");
    let current_selected: HashSet<Entity> = selected_pieces_query.iter().collect();
    let current_preview: HashSet<Entity> = preview_pieces_query.iter().collect();
    
    // 選択状態変更を検出
    let selection_changed = current_selected != highlight_state.last_selected_pieces;
    let preview_changed = current_preview != highlight_state.last_preview_pieces;
    
    highlight_state.frame_count += 1;
    highlight_state.selection_changed = selection_changed;
    highlight_state.preview_changed = preview_changed;
    
    perf_monitor.end_system_timing("highlight_change_detection", change_detection_start);
    
    // 変更がない場合は早期リターン
    if !selection_changed && !preview_changed {
        // 定期的にスキップ情報を出力
        if perf_monitor.debug_level == PerformanceDebugLevel::High && highlight_state.frame_count % 300 == 0 {
            println!("🚀 HIGHLIGHT OPTIMIZATION: Skipped {} frames (no changes)", highlight_state.frame_count);
        }
        perf_monitor.end_system_timing("highlight_selected_pieces", start_time);
        return;
    }
    
    // 変更があった場合のみ処理を実行
    if perf_monitor.debug_level == PerformanceDebugLevel::Medium {
        println!("🔄 HIGHLIGHT UPDATE: Selection changed={}, Preview changed={}, Frame={}", 
            selection_changed, preview_changed, highlight_state.frame_count);
    }
    
    // 変更されたピースのみ通常の色に戻す（最適化）
    let reset_color_start = perf_monitor.start_system_timing("highlight_reset_colors");
    let mut reset_count = 0;
    
    // 前回選択されていたが今回選択されていないピースの色をリセット
    for &entity in &highlight_state.last_selected_pieces {
        if !current_selected.contains(&entity) {
            if let Ok(material_handle) = piece_query.get_mut(entity) {
                if let Some(material) = materials.get_mut(&material_handle.0) {
                    material.color = Color::srgba(1.0, 1.0, 1.0, 1.0);
                    reset_count += 1;
                }
            }
        }
    }
    
    // 前回プレビューだったが今回プレビューでないピースの色をリセット
    for &entity in &highlight_state.last_preview_pieces {
        if !current_preview.contains(&entity) {
            if let Ok(material_handle) = piece_query.get_mut(entity) {
                if let Some(material) = materials.get_mut(&material_handle.0) {
                    material.color = Color::srgba(1.0, 1.0, 1.0, 1.0);
                    reset_count += 1;
                }
            }
        }
    }
    
    perf_monitor.end_system_timing("highlight_reset_colors", reset_color_start);
    
    // 既存の枠線を削除（変更があった場合のみ）
    let outline_removal_start = perf_monitor.start_system_timing("highlight_outline_removal");
    for outline_entity in existing_outline_query.iter() {
        commands.entity(outline_entity).despawn();
    }
    perf_monitor.end_system_timing("highlight_outline_removal", outline_removal_start);
    
    // プレビュー中のピースにストロークハイライトを追加（共有マテリアル使用）
    let preview_outline_start = perf_monitor.start_system_timing("highlight_preview_outlines");
    for entity in preview_pieces_query.iter() {
        if let (Ok(_transform), Ok(piece_shape)) = (
            piece_transform_query.get(entity),
            piece_shape_query.get(entity)
        ) {
            // キャッシュからストロークメッシュを取得
            if let Some(stroke_mesh_handle) = stroke_cache.stroke_meshes.get(&piece_shape.shape_hash) {
                static mut PREVIEW_CACHE_LOG_COUNT: usize = 0;
                unsafe {
                    if PREVIEW_CACHE_LOG_COUNT < 5 {
                        println!("🔍 PREVIEW: Using cached stroke mesh for shape: '{}'", piece_shape.shape_hash);
                        PREVIEW_CACHE_LOG_COUNT += 1;
                    }
                }
                
                // 共有マテリアルを使用（マテリアル作成のオーバーヘッドを削減）
                let outline_entity = commands.spawn((
                    Mesh2d(stroke_mesh_handle.clone()),
                    MeshMaterial2d(highlight_materials.preview_material.clone()),
                    Transform {
                        translation: Vec3::new(0.0, 0.0, -0.1), // 親からの相対位置
                        rotation: Quat::IDENTITY,
                        scale: Vec3::ONE,
                    },
                    PieceOutline {
                        piece_entity: entity,
                    },
                )).id();
                
                // 輪郭線をピースの子エンティティとして設定
                commands.entity(entity).add_child(outline_entity);
            }
        }
    }
    perf_monitor.end_system_timing("highlight_preview_outlines", preview_outline_start);
    
    // 選択されたピースにストロークハイライトを追加（共有マテリアル使用）
    let selected_outline_start = perf_monitor.start_system_timing("highlight_selected_outlines");
    for entity in selected_pieces_query.iter() {
        if let (Ok(_transform), Ok(piece_shape)) = (
            piece_transform_query.get(entity),
            piece_shape_query.get(entity)
        ) {
            // キャッシュからストロークメッシュを取得
            if let Some(stroke_mesh_handle) = stroke_cache.stroke_meshes.get(&piece_shape.shape_hash) {
                static mut SELECTED_CACHE_LOG_COUNT: usize = 0;
                unsafe {
                    if SELECTED_CACHE_LOG_COUNT < 5 {
                        println!("✨ SELECTED: Using cached stroke mesh for shape: '{}'", piece_shape.shape_hash);
                        SELECTED_CACHE_LOG_COUNT += 1;
                    }
                }
                
                // 共有マテリアルを使用（マテリアル作成のオーバーヘッドを削減）
                let outline_entity = commands.spawn((
                    Mesh2d(stroke_mesh_handle.clone()),
                    MeshMaterial2d(highlight_materials.selected_material.clone()),
                    Transform {
                        translation: Vec3::new(0.0, 0.0, -0.05), // 親からの相対位置（プレビューより上）
                        rotation: Quat::IDENTITY,
                        scale: Vec3::ONE,
                    },
                    PieceOutline {
                        piece_entity: entity,
                    },
                )).id();
                
                // 輪郭線をピースの子エンティティとして設定
                commands.entity(entity).add_child(outline_entity);
            }
        }
    }
    perf_monitor.end_system_timing("highlight_selected_outlines", selected_outline_start);
    
    // 最適化の統計情報を出力（移動前に値を取得）
    if perf_monitor.debug_level == PerformanceDebugLevel::High {
        let total_pieces = cache.all_pieces.len();
        let selected_count = current_selected.len();
        let preview_count = current_preview.len();
        println!("🔥 HIGHLIGHT OPTIMIZATION: Reset {} pieces, Current selected: {}, Current preview: {}, Total pieces: {}", 
            reset_count, selected_count, preview_count, total_pieces);
    }
    
    // 次フレーム用に現在の選択状態を保存
    let state_update_start = perf_monitor.start_system_timing("highlight_state_update");
    highlight_state.last_selected_pieces = current_selected;
    highlight_state.last_preview_pieces = current_preview;
    perf_monitor.end_system_timing("highlight_state_update", state_update_start);
    
    perf_monitor.end_system_timing("highlight_selected_pieces", start_time);
}