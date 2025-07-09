use bevy::prelude::*;
use bevy::sprite::ColorMaterial;
use crate::components::*;
use crate::resources::*;

/// 矩形範囲内にピースが含まれているかチェック
fn is_piece_in_selection_box(piece_pos: Vec2, selection_start: Vec2, selection_end: Vec2) -> bool {
    let min_x = selection_start.x.min(selection_end.x);
    let max_x = selection_start.x.max(selection_end.x);
    let min_y = selection_start.y.min(selection_end.y);
    let max_y = selection_start.y.max(selection_end.y);
    
    piece_pos.x >= min_x && piece_pos.x <= max_x && 
    piece_pos.y >= min_y && piece_pos.y <= max_y
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

// 旧システムは互換性のため残す（後で削除予定）
pub fn handle_piece_dragging(
    _commands: Commands,
    mut piece_query: Query<(Entity, &mut Transform, &mut Draggable, &PuzzlePiece)>,
    mut input_state: ResMut<InputState>,
    mut game_state: ResMut<GameState>,
    mouse_input: Res<ButtonInput<MouseButton>>,
    puzzle_config: Res<PuzzleConfig>,
    puzzle_image: Option<Res<PuzzleImage>>,
    camera_query: Query<&Transform, (With<MainCamera>, Without<PuzzlePiece>)>,
) {
    // 旧システムは無効化 - picking systemを使用
    return;
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
    
    // 現在ドラッグ中のピースがあるかチェック
    let mut current_dragging_piece: Option<Entity> = None;
    for (entity, _transform, pickable, _piece, _shape) in piece_query.iter() {
        if input_state.selected_piece == Some(entity) {
            current_dragging_piece = Some(entity);
            break;
        }
    }
    
    // マウスクリック開始
    if mouse_just_pressed && !input_state.is_camera_dragging {
        let world_pos = input_state.mouse_position; // 既にワールド座標に変換済み
        
        // ピースをクリックしたかチェック（Z順序を考慮して最前面のピースを優先）
        let mut clicked_piece = None;
        let mut highest_z = f32::NEG_INFINITY;
        
        // PieceShapeを使用して正確な判定を行う
        for (entity, transform, _pickable, piece, shape) in piece_query.iter() {
            // ピースのローカル座標に変換
            let local_pos = world_pos - transform.translation.truncate();
            
            // まず境界ボックス内かチェック（高速フィルタリング）
            if local_pos.x >= piece.bounds.min.x && local_pos.x <= piece.bounds.max.x &&
               local_pos.y >= piece.bounds.min.y && local_pos.y <= piece.bounds.max.y {
                
                // 実際のジグソー形状内かチェック
                if point_in_mesh(&shape.vertices, &shape.indices, local_pos) {
                    // より前面にあるピースを優先
                    if transform.translation.z > highest_z {
                        clicked_piece = Some(entity);
                        highest_z = transform.translation.z;
                    }
                }
            }
        }
        
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
    mut piece_query: Query<(Entity, &mut Transform, &PuzzlePiece, &PieceShape), With<PickablePiece>>,
    mut selected_query: Query<Entity, With<SelectedPiece>>,
    game_state: Res<GameState>,
) {
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
        input_state.selection_mode = SelectionMode::Single;
        input_state.selection_start = None;
        input_state.selection_current = None;
        println!("🔄 Selection cleared");
        return;
    }
    
    match input_state.selection_mode {
        SelectionMode::Single => {
            // マウスクリック開始 - どこをクリックしたかで動作分岐
            if mouse_just_pressed && !input_state.is_camera_dragging {
                let world_pos = input_state.mouse_position; // 既にワールド座標に変換済み
                
                // ピースをクリックしたかチェック（Z順序を考慮して最前面のピースを優先）
                let mut clicked_piece = None;
                let mut highest_z = f32::NEG_INFINITY;
                
                // PieceShapeを使用して正確な判定を行う
                for (entity, transform, piece, shape) in piece_query.iter() {
                    // ピースのローカル座標に変換
                    let local_pos = world_pos - transform.translation.truncate();
                    
                    // まず境界ボックス内かチェック（高速フィルタリング）
                    if local_pos.x >= piece.bounds.min.x && local_pos.x <= piece.bounds.max.x &&
                       local_pos.y >= piece.bounds.min.y && local_pos.y <= piece.bounds.max.y {
                        
                        // 実際のジグソー形状内かチェック
                        if point_in_mesh(&shape.vertices, &shape.indices, local_pos) {
                            // より前面にあるピースを優先
                            if transform.translation.z > highest_z {
                                clicked_piece = Some(entity);
                                highest_z = transform.translation.z;
                            }
                        }
                    }
                }
                
                if let Some(piece_entity) = clicked_piece {
                    // ピースをクリック - 選択状態をトグル
                    if input_state.selected_pieces.contains(&piece_entity) {
                        // 既に選択済み → 複数ピース移動モードに移行
                        input_state.selection_mode = SelectionMode::MultiDrag;
                        
                        // 各ピースのドラッグオフセットを計算
                        input_state.multi_drag_offset.clear();
                        let selected_pieces_copy = input_state.selected_pieces.clone();
                        for selected_entity in selected_pieces_copy {
                            // 全てのピース（選択済みも含む）から検索
                            for (entity, transform, _, _) in piece_query.iter() {
                                if entity == selected_entity {
                                    let piece_world_pos = transform.translation.truncate();
                                    let offset = piece_world_pos - world_pos;
                                    input_state.multi_drag_offset.insert(selected_entity, offset);
                                    break;
                                }
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
                    // 既存のプレビューをクリア
                    for entity in selected_query.iter() {
                        commands.entity(entity).remove::<SelectionPreview>();
                    }
                    // プレビュー用にも全ピースをチェック
                    for (entity, _, _, _) in piece_query.iter() {
                        commands.entity(entity).remove::<SelectionPreview>();
                    }
                    
                    // 範囲内のピースにプレビューマークを追加
                    for (entity, transform, piece, _) in piece_query.iter() {
                        let piece_pos = transform.translation.truncate();
                        
                        // ピースの境界ボックスと選択範囲の重なりをチェック
                        let piece_min = piece_pos + piece.bounds.min;
                        let piece_max = piece_pos + piece.bounds.max;
                        
                        // 選択範囲の境界
                        let select_min_x = start.x.min(end.x);
                        let select_max_x = start.x.max(end.x);
                        let select_min_y = start.y.min(end.y);
                        let select_max_y = start.y.max(end.y);
                        
                        // 矩形の重なり判定
                        let overlaps = piece_min.x <= select_max_x && 
                                      piece_max.x >= select_min_x &&
                                      piece_min.y <= select_max_y && 
                                      piece_max.y >= select_min_y;
                        
                        if overlaps && !input_state.selected_pieces.contains(&entity) {
                            commands.entity(entity).insert(SelectionPreview);
                        }
                    }
                }
            }
            
            if mouse_just_released {
                // 範囲選択終了 - 範囲内のピースを選択
                if let (Some(start), Some(end)) = (input_state.selection_start, input_state.selection_current) {
                    let mut newly_selected = 0;
                    
                    for (entity, transform, piece, _) in piece_query.iter() {
                        let piece_pos = transform.translation.truncate();
                        
                        // ピースの境界ボックスと選択範囲の重なりをチェック
                        let piece_min = piece_pos + piece.bounds.min;
                        let piece_max = piece_pos + piece.bounds.max;
                        
                        // 選択範囲の境界
                        let select_min_x = start.x.min(end.x);
                        let select_max_x = start.x.max(end.x);
                        let select_min_y = start.y.min(end.y);
                        let select_max_y = start.y.max(end.y);
                        
                        // 矩形の重なり判定
                        let overlaps = piece_min.x <= select_max_x && 
                                      piece_max.x >= select_min_x &&
                                      piece_min.y <= select_max_y && 
                                      piece_max.y >= select_min_y;
                        
                        if overlaps {
                            commands.entity(entity).insert(SelectedPiece);
                            input_state.selected_pieces.push(entity);
                            newly_selected += 1;
                        }
                    }
                    
                    println!("📦 Box selection completed: {} new pieces selected (total: {})", 
                             newly_selected, input_state.selected_pieces.len());
                }
                
                // プレビューをクリア
                for (entity, _, _, _) in piece_query.iter() {
                    commands.entity(entity).remove::<SelectionPreview>();
                }
                
                // 範囲選択モード終了
                input_state.selection_mode = SelectionMode::Single;
                input_state.selection_start = None;
                input_state.selection_current = None;
            }
        }
        
        SelectionMode::MultiDrag => {
            // MultiDragモードの処理は handle_multi_piece_drag で行う
        }
    }
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
    camera_query: Query<&Transform, (With<MainCamera>, Without<SelectionBox>)>,
    mut selection_box_query: Query<(Entity, &mut Transform), (With<SelectionBox>, Without<MainCamera>)>,
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
fn create_outline_mesh(vertices: &[[f32; 2]], indices: &[u32]) -> Mesh {
    use bevy::render::render_asset::RenderAssetUsages;
    use bevy::render::render_resource::PrimitiveTopology;
    
    // 元のメッシュをそのまま使用（スケールは Transform で調整）
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    
    // 頂点データをそのままコピー
    let positions: Vec<[f32; 3]> = vertices.iter()
        .map(|v| [v[0], v[1], 0.0])
        .collect();
    
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_indices(bevy::render::mesh::Indices::U32(indices.to_vec()));
    
    mesh
}

/// 選択されたピースのハイライト表示システム（ストロークメッシュキャッシュ版）
pub fn highlight_selected_pieces(
    mut commands: Commands,
    mut materials: ResMut<Assets<ColorMaterial>>,
    selected_pieces_query: Query<Entity, (With<SelectedPiece>, With<PuzzlePiece>)>,
    preview_pieces_query: Query<Entity, (With<SelectionPreview>, With<PuzzlePiece>)>,
    all_pieces_query: Query<Entity, With<PuzzlePiece>>,
    mut piece_query: Query<&mut MeshMaterial2d<ColorMaterial>, With<PuzzlePiece>>,
    piece_transform_query: Query<&Transform, With<PuzzlePiece>>,
    piece_shape_query: Query<&PieceShape, With<PuzzlePiece>>,
    existing_outline_query: Query<Entity, With<PieceOutline>>,
    stroke_cache: Res<StrokeMeshCache>,
    game_state: Res<GameState>,
) {
    // ゲーム内メニューが表示されている間は無効化
    if game_state.current_screen != GameScreen::InGame {
        return;
    }
    
    // まず、全てのピースを通常の色に戻す
    for entity in all_pieces_query.iter() {
        if let Ok(mut material_handle) = piece_query.get_mut(entity) {
            if let Some(material) = materials.get_mut(&material_handle.0) {
                // 通常の白色に戻す（テクスチャの元の色）
                material.color = Color::srgba(1.0, 1.0, 1.0, 1.0);
            }
        }
    }
    
    // 既存の枠線を削除
    for outline_entity in existing_outline_query.iter() {
        commands.entity(outline_entity).despawn();
    }
    
    // プレビュー中のピースにストロークハイライトを追加
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
                // ストロークメッシュを使ってハイライト表示
                let stroke_material = ColorMaterial {
                    color: Color::srgba(0.3, 0.6, 1.0, 0.8), // 薄い青色
                    ..Default::default()
                };
                let stroke_material_handle = materials.add(stroke_material);
                
                let outline_entity = commands.spawn((
                    Mesh2d(stroke_mesh_handle.clone()),
                    MeshMaterial2d(stroke_material_handle),
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
    
    // 選択されたピースにストロークハイライトを追加
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
                // ストロークメッシュを使ってハイライト表示
                let stroke_material = ColorMaterial {
                    color: Color::srgba(1.0, 0.8, 0.0, 1.0), // 黄色
                    ..Default::default()
                };
                let stroke_material_handle = materials.add(stroke_material);
                
                let outline_entity = commands.spawn((
                    Mesh2d(stroke_mesh_handle.clone()),
                    MeshMaterial2d(stroke_material_handle),
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
}