use bevy::prelude::*;
use bevy::input::mouse::MouseWheel;
use bevy::sprite::ColorMaterial;
use bevy::tasks::AsyncComputeTaskPool;
use crate::components::*;
use crate::resources::*;
use crate::puzzle::*;
use crate::jigsaw_shapes::{JigsawShapeGenerator, JigsawPieceShape};
use std::path::Path;

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

pub fn update_input_state(
    mut input_state: ResMut<InputState>,
    windows: Query<&Window>,
    camera_q: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    mouse_input: Res<ButtonInput<MouseButton>>,
) {
    let Ok(window) = windows.single() else { return; };
    let Ok((camera, camera_transform)) = camera_q.single() else { return; };
    
    // 前のマウス位置を保存
    input_state.last_mouse_position = input_state.mouse_position;
    
    if let Some(cursor_pos) = window.cursor_position() {
        if let Ok(world_pos) = camera.viewport_to_world_2d(camera_transform, cursor_pos) {
            input_state.mouse_position = world_pos;
            // デバッグ: マウス座標変換を確認（頻繁すぎるので制限）
            if mouse_input.just_pressed(MouseButton::Left) {
                println!("Cursor: ({:.1}, {:.1}) -> World: ({:.1}, {:.1})", 
                    cursor_pos.x, cursor_pos.y, world_pos.x, world_pos.y);
            }
        }
    }
    
    input_state.is_mouse_pressed = mouse_input.pressed(MouseButton::Left);
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

// ハイブリッドアプローチ: 手動当たり判定 + picking eventの合成
pub fn handle_piece_dragging_hybrid(
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
    
    // マウスがクリックされた瞬間かつ、他にドラッグ中のピースがない場合のみ新しい選択を行う
    if mouse_just_pressed && current_dragging_piece.is_none() && !input_state.is_camera_dragging {
        let mut closest_piece: Option<(Entity, f32, f32)> = None;
        
        // 動的な距離制限を設定（実際のピースサイズに基づく）
        // First, find the actual maximum piece bounds size from all pieces
        let max_piece_size = piece_query.iter()
            .map(|(_, _, _, piece, _)| {
                let bounds_size = Vec2::new(
                    piece.bounds.max.x - piece.bounds.min.x,
                    piece.bounds.max.y - piece.bounds.min.y
                );
                bounds_size.x.max(bounds_size.y)
            })
            .fold(0.0, f32::max);
        
        // Use the actual maximum piece size for distance checking
        // Increase multiplier to cover jigsaw tab extensions beyond the base piece bounds
        let max_check_distance = max_piece_size * 3.0;
        
        // デバッグ: マウスクリック位置とピース総数（頻度制限）
        let total_pieces = puzzle_config.grid_size.0 * puzzle_config.grid_size.1;
        static mut CLICK_LOG_COUNT: usize = 0;
        unsafe {
            CLICK_LOG_COUNT += 1;
            if CLICK_LOG_COUNT % 20 == 0 {  // 20回に1回のみ出力
                println!("🖱️ Mouse click #{} at ({:.1}, {:.1}) - Total pieces: {}, max_piece_size: {:.1}", 
                    CLICK_LOG_COUNT, input_state.mouse_position.x, input_state.mouse_position.y, total_pieces, max_piece_size);
            }
        }
        
        
        let mut pieces_checked = 0;
        let mut pieces_in_bounds = 0;
        let mut pieces_mesh_hit = 0;
        
        for (entity, transform, _pickable, piece, shape) in piece_query.iter() {
            pieces_checked += 1;
            
            if input_state.selected_piece == Some(entity) {
                continue;
            }
            
            let piece_pos = transform.translation.truncate();
            let distance_to_mouse = piece_pos.distance(input_state.mouse_position);
            
            // パフォーマンス最適化: 距離が遠すぎる場合は早期スキップ
            if distance_to_mouse > max_check_distance {
                continue;
            }
            
            
            // 精密なジグソー形状当たり判定を試行、フォールバックで境界判定
            let mouse_x = input_state.mouse_position.x;
            let mouse_y = input_state.mouse_position.y;
            
            // マウス座標をピース座標系に変換
            let piece_relative_point = Vec2::new(mouse_x - piece_pos.x, mouse_y - piece_pos.y);
            
            // まず境界チェックで高速に除外（マージン付き）
            let piece_bounds = piece.bounds;
            
            // 境界を動的に拡張してジグソー形状の凸部分を完全にカバー
            // For a 3840x2160 image with 5x3 grid, piece size should be 768x720
            // Since bounds are (-384,-360 to 384,360), the piece size is 768x720
            let piece_bounds_size = Vec2::new(
                piece_bounds.max.x - piece_bounds.min.x,
                piece_bounds.max.y - piece_bounds.min.y
            );
            
            // Use the bounds size for margin calculation
            let piece_size = piece_bounds_size;
            
            // Debug: Log piece size and camera info for first few pieces
            static mut PIECE_SIZE_LOG_COUNT: usize = 0;
            unsafe {
                if PIECE_SIZE_LOG_COUNT < 5 {
                    let camera_scale = if let Ok(cam_transform) = camera_query.single() {
                        cam_transform.scale.x
                    } else {
                        1.0
                    };
                    println!("🔍 Collision detection piece {}: bounds({:.1},{:.1} to {:.1},{:.1}) size:({:.1}x{:.1}), camera_scale:{:.3}",
                        PIECE_SIZE_LOG_COUNT, 
                        piece_bounds.min.x, piece_bounds.min.y, piece_bounds.max.x, piece_bounds.max.y,
                        piece_bounds_size.x, piece_bounds_size.y, camera_scale);
                    PIECE_SIZE_LOG_COUNT += 1;
                }
            }
            
            // Add margin to account for jigsaw tabs extending beyond base bounds
            // Jigsaw tabs can extend significantly beyond the base piece rectangle
            let margin = piece_size.x.max(piece_size.y) * 0.4; // 40% additional margin for tab extensions
            let expanded_bounds = Rect::new(
                piece_bounds.min.x - margin,
                piece_bounds.min.y - margin, 
                piece_bounds.max.x + margin,
                piece_bounds.max.y + margin
            );
            
            let in_bounds = expanded_bounds.contains(piece_relative_point);
            
            // DEBUG: Always log bounds check for pieces close to mouse
            if distance_to_mouse < max_check_distance {
                println!("🔍 DEBUG piece({},{}) mouse_rel:({:.1},{:.1}) bounds:({:.1},{:.1} to {:.1},{:.1}) in_bounds:{}",
                    piece.grid_x, piece.grid_y,
                    piece_relative_point.x, piece_relative_point.y,
                    expanded_bounds.min.x, expanded_bounds.min.y, expanded_bounds.max.x, expanded_bounds.max.y,
                    in_bounds);
            }
            
            // デバッグ: 距離の近いピースの境界情報を詳しく出力
            if distance_to_mouse < max_check_distance * 0.5 {
                let bounds_width = piece_bounds.max.x - piece_bounds.min.x;
                let bounds_height = piece_bounds.max.y - piece_bounds.min.y;
                let expected_piece_size = piece_size;
                println!("🔍 Piece({},{}) entity:{:?} pos:({:.1},{:.1}) mouse_world:({:.1},{:.1}) mouse_rel:({:.1},{:.1}) dist:{:.1}", 
                    piece.grid_x, piece.grid_y, entity, piece_pos.x, piece_pos.y,
                    input_state.mouse_position.x, input_state.mouse_position.y,
                    piece_relative_point.x, piece_relative_point.y, distance_to_mouse);
                println!("    orig_bounds:({:.1},{:.1} to {:.1},{:.1}) size:{:.1}x{:.1} calc_piece_size:{:.1}x{:.1} margin:{:.1} in_bounds:{}", 
                    piece_bounds.min.x, piece_bounds.min.y, piece_bounds.max.x, piece_bounds.max.y,
                    bounds_width, bounds_height, piece_size.x, piece_size.y, margin, in_bounds);
                
                // Additional debug for expanded bounds
                println!("    expanded_bounds:({:.1},{:.1} to {:.1},{:.1}) mouse_rel:({:.1},{:.1})",
                    expanded_bounds.min.x, expanded_bounds.min.y, expanded_bounds.max.x, expanded_bounds.max.y,
                    piece_relative_point.x, piece_relative_point.y);
            }
            
            if in_bounds {
                pieces_in_bounds += 1;
            }
            
            // 精密メッシュ形状判定
            let mouse_in_bounds = if in_bounds {
                // 境界内の場合、実際のメッシュ形状で精密判定
                let mesh_result = point_in_mesh(&shape.vertices, &shape.indices, piece_relative_point);
                if mesh_result {
                    pieces_mesh_hit += 1;
                }
                mesh_result
            } else {
                false
            };
            
            
            if mouse_in_bounds {
                
                // Z値が高い（より前面）ピースを優先、同じZ値なら距離が近いピースを選択
                match closest_piece {
                    None => closest_piece = Some((entity, distance_to_mouse, transform.translation.z)),
                    Some((_, closest_distance, closest_z)) => {
                        if transform.translation.z > closest_z || 
                           (transform.translation.z == closest_z && distance_to_mouse < closest_distance) {
                            closest_piece = Some((entity, distance_to_mouse, transform.translation.z));
                        }
                    }
                }
            }
        }
        
        // デバッグ: 当たり判定統計
        println!("📊 Hit detection stats: checked={}, in_bounds={}, mesh_hit={}, selected={}", 
            pieces_checked, pieces_in_bounds, pieces_mesh_hit, closest_piece.is_some());
        
        // 選択されたピースをドラッグ開始
        if let Some((selected_entity, _, _)) = closest_piece {
            println!("✅ Starting manual drag for entity: {:?}", selected_entity);
            input_state.selected_piece = Some(selected_entity);
            
            for (entity, mut transform, mut pickable, piece, _shape) in piece_query.iter_mut() {
                if entity == selected_entity {
                    let piece_pos = transform.translation.truncate();
                    pickable.drag_offset = piece_pos - input_state.mouse_position;
                    
                    // ドラッグ開始時に最前面に移動
                    transform.translation.z = 100.0;
                    
                    println!("🔧 Drag offset set: piece at ({:.1}, {:.1}), mouse at ({:.1}, {:.1}), offset ({:.1}, {:.1})",
                        piece_pos.x, piece_pos.y, 
                        input_state.mouse_position.x, input_state.mouse_position.y,
                        pickable.drag_offset.x, pickable.drag_offset.y);
                    break;
                }
            }
        }
    }
    
    // ドラッグ中の処理
    if let Some(dragging_entity) = input_state.selected_piece {
        for (entity, mut transform, pickable, piece, _shape) in piece_query.iter_mut() {
            if entity == dragging_entity {
                if mouse_pressed {
                    let new_pos = input_state.mouse_position + pickable.drag_offset;
                    transform.translation.x = new_pos.x;
                    transform.translation.y = new_pos.y;
                    transform.translation.z = 100.0;
                } else if mouse_just_released {
                    input_state.selected_piece = None;
                    
                    // ドロップ時に新しいZ値を割り当て（最前面に配置）
                    input_state.next_z_order += 1.0;
                    transform.translation.z = input_state.next_z_order;
                    
                    println!("✅ Manual drag ended: piece dropped at ({:.2}, {:.2})", 
                        transform.translation.x, transform.translation.y);
                }
                break;
            }
        }
    }
}

pub fn check_piece_placement(
    mut commands: Commands,
    mut piece_query: Query<(Entity, &mut Transform, &mut PuzzlePiece), With<PickablePiece>>,
    puzzle_config: Res<PuzzleConfig>,
    input_state: Res<InputState>,
) {
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

pub fn update_game_state(
    mut game_state: ResMut<GameState>,
    piece_query: Query<&PuzzlePiece>,
) {
    let total_pieces = piece_query.iter().count();
    let placed_pieces = piece_query.iter().filter(|p| p.is_placed).count();
    
    if total_pieces > 0 {
        game_state.puzzle_progress = placed_pieces as f32 / total_pieces as f32;
        game_state.puzzle_completed = placed_pieces == total_pieces;
        
        if game_state.puzzle_completed && game_state.current_screen == GameScreen::InGame {
            game_state.current_screen = GameScreen::GameComplete;
        }
    }
}

/// パズルをリセットする（既存のピース、グリッド背景、画像設定を削除）
pub fn reset_puzzle(
    mut commands: Commands,
    mut game_state: ResMut<GameState>,
    mut puzzle_config: ResMut<PuzzleConfig>,
    puzzle_pieces: Query<Entity, With<PuzzlePiece>>,
    grid_references: Query<Entity, With<GridReference>>,
    mut input_state: ResMut<InputState>,
) {
    if !game_state.needs_reset {
        return;
    }
    
    println!("🔄 Resetting puzzle completely...");
    
    // すべてのパズルピースを削除
    for entity in puzzle_pieces.iter() {
        commands.entity(entity).despawn();
    }
    
    // グリッド背景画像も削除
    for entity in grid_references.iter() {
        commands.entity(entity).despawn();
    }
    
    // ゲーム状態をリセット
    game_state.puzzle_completed = false;
    game_state.puzzle_progress = 0.0;
    game_state.needs_reset = false;
    
    // 入力状態をリセット
    input_state.selected_piece = None;
    input_state.next_z_order = 1.0;
    
    // 画像設定を完全にクリア
    puzzle_config.image_path.clear();
    
    // PuzzleImageリソースを削除して再読み込みを強制
    commands.remove_resource::<crate::resources::PuzzleImage>();
    
    println!("✅ Puzzle reset completed (pieces, grid background, and image settings cleared)");
}

pub fn spawn_puzzle_pieces(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    puzzle_config: Res<PuzzleConfig>,
    puzzle_image: Option<Res<PuzzleImage>>,
    game_state: Res<GameState>,
    existing_pieces: Query<&PuzzlePiece>,
    existing_grid_ref: Query<&GridReference>,
    images: Res<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
    mut progress: ResMut<PieceGenerationProgress>,
) {
    if game_state.current_screen == GameScreen::InGame && existing_pieces.is_empty() {
        // 画像パスが設定されている場合
        if !puzzle_config.image_path.is_empty() {
            // PuzzleImageリソースがまだない場合は作成
            if puzzle_image.is_none() {
                // ファイルの存在確認
                if !std::path::Path::new(&puzzle_config.image_path).exists() {
                    println!("Error: Image file does not exist: {}", puzzle_config.image_path);
                    return;
                }
                
                // Bevyアセットシステム用のパス変換
                let asset_path = if Path::new(&puzzle_config.image_path).is_absolute() {
                    // 絶対パスの場合は、assetsフォルダにコピーして相対パスを使用
                    let source_path = Path::new(&puzzle_config.image_path);
                    let file_name = source_path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("puzzle_image.png");
                    
                    let assets_dir = Path::new("assets");
                    let dest_path = assets_dir.join(file_name);
                    
                    // assetsディレクトリが存在しない場合は作成
                    if !assets_dir.exists() {
                        if let Err(e) = std::fs::create_dir_all(assets_dir) {
                            println!("Failed to create assets directory: {}", e);
                            return;
                        }
                    }
                    
                    // ファイルをassetsフォルダにコピー
                    if let Err(e) = std::fs::copy(source_path, &dest_path) {
                        println!("Failed to copy image to assets folder: {}", e);
                        return;
                    }
                    
                    println!("Copied image to: {:?}", dest_path);
                    file_name.to_string()
                } else {
                    puzzle_config.image_path.clone()
                };
                
                println!("Original path: {}", puzzle_config.image_path);
                println!("Asset path: {}", asset_path);
                
                let image_handle = asset_server.load(&asset_path);
                println!("Loading image with handle: {:?}", image_handle);
                
                // 読み込み直後の状態もチェック
                let initial_state = asset_server.load_state(&image_handle);
                println!("Initial load state: {:?}", initial_state);
                
                commands.insert_resource(PuzzleImage {
                    handle: image_handle,
                    size: Vec2::new(1920.0, 1080.0), // 16:9の仮定値（update_puzzle_image_sizeで実際のサイズに更新される）
                });
                return; // 次フレームで再実行
            }
            
            // プログレッシブ生成システムが処理するので、ここでは何もしない
            // spawn_puzzle_pieces_progressiveシステムが実際の生成を行う
        } else {
            // 画像が選択されていない場合はエラーメッセージ
            println!("⚠️ No image selected for puzzle creation! Current screen: {:?}, image_path: '{}'", 
                game_state.current_screen, puzzle_config.image_path);
            return;
        }
    }
}

fn spawn_grid_reference(
    commands: &mut Commands,
    asset_server: &Res<AssetServer>,
    puzzle_config: &PuzzleConfig,
    puzzle_image: Option<&Res<PuzzleImage>>,
) {
    // 選択された画像を半透明で表示
    if !puzzle_config.image_path.is_empty() {
        // メインの画像読み込みと同じパス変換処理を適用
        let asset_path = if Path::new(&puzzle_config.image_path).is_absolute() {
            // 絶対パスの場合は、ファイル名のみを使用（既にassetsフォルダにコピー済み）
            Path::new(&puzzle_config.image_path)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("puzzle_image.png")
                .to_string()
        } else {
            puzzle_config.image_path.clone()
        };
        
        println!("Loading grid reference image: {}", asset_path);
        let texture_handle = asset_server.load(&asset_path);
        
        // Use the same size calculation as pieces to ensure alignment
        let custom_size = puzzle_image.map(|img| img.size);
        
        commands.spawn((
            Sprite {
                color: Color::srgb(1.0, 1.0, 1.0).with_alpha(0.3), // 半透明
                image: texture_handle,
                custom_size, // Match the size used for piece calculations
                ..default()
            },
            Transform::from_translation(Vec3::new(0.0, 0.0, -10.0)), // 背景に配置
            // 参照画像としてマーク
            GridReference,
        ));
        
        if let Some(img) = puzzle_image {
            println!("Grid reference spawned with size: ({:.1}, {:.1})", img.size.x, img.size.y);
        }
    }
}

pub fn auto_adjust_camera_zoom(
    mut camera_query: Query<&mut Transform, With<MainCamera>>,
    puzzle_image: Option<Res<PuzzleImage>>,
    windows: Query<&Window>,
) {
    if let Some(puzzle_image) = puzzle_image.as_ref() {
        if let Ok(window) = windows.single() {
            for mut transform in camera_query.iter_mut() {
                // 現在のスケールが1.0（初期状態）の場合のみ自動調整
                if (transform.scale.x - 1.0).abs() < 0.01 {
                    let window_width = window.width();
                    let window_height = window.height();
                    
                    // 画像がウィンドウに収まるように初期ズームを計算
                    let scale_x = window_width / puzzle_image.size.x * 0.8; // 80%のマージン
                    let scale_y = window_height / puzzle_image.size.y * 0.8;
                    let initial_scale = scale_x.min(scale_y).clamp(0.1, 5.0);
                    
                    transform.scale = Vec3::splat(initial_scale);
                    println!("Auto-adjusted camera zoom to {:.2} for image {}x{}", 
                        initial_scale, puzzle_image.size.x, puzzle_image.size.y);
                }
            }
        }
    }
}

pub fn handle_camera_zoom(
    mut scroll_evr: EventReader<MouseWheel>,
    mut camera_query: Query<&mut Transform, With<MainCamera>>,
) {
    for ev in scroll_evr.read() {
        for mut transform in camera_query.iter_mut() {
            let zoom_factor = if ev.y > 0.0 { 0.9 } else { 1.1 };
            
            // ズーム制限 (0.1倍から5.0倍まで - 大きな画像に対応)
            let current_scale = transform.scale.x;
            let new_scale = (current_scale * zoom_factor).clamp(0.1, 5.0);
            
            transform.scale = Vec3::splat(new_scale);
            
            println!("Camera zoom: {:.2}", new_scale);
        }
    }
}

// デバッグ用: ピースの実際のTransform位置を確認
pub fn debug_piece_positions(
    piece_query: Query<(&Transform, &PuzzlePiece), (With<PickablePiece>, Without<MainCamera>)>,
    camera_query: Query<&Transform, (With<MainCamera>, Without<PuzzlePiece>)>,
    puzzle_config: Res<PuzzleConfig>,
) {
    static mut DEBUG_FRAME_COUNT: usize = 0;
    unsafe {
        DEBUG_FRAME_COUNT += 1;
        
        // 100ピース超の場合のみ、60フレーム後に1回だけ実行
        if puzzle_config.grid_size.0 * puzzle_config.grid_size.1 > 100 && DEBUG_FRAME_COUNT == 60 {
            // カメラの状態を確認
            if let Ok(camera_transform) = camera_query.single() {
                println!("📹 Camera status (frame {}):", DEBUG_FRAME_COUNT);
                println!("  Position: ({:.1}, {:.1}, {:.1})", 
                    camera_transform.translation.x, camera_transform.translation.y, camera_transform.translation.z);
                println!("  Scale: ({:.3}, {:.3}, {:.3})", 
                    camera_transform.scale.x, camera_transform.scale.y, camera_transform.scale.z);
            }
            
            println!("🔍 Actual Transform positions for pieces (frame {}):", DEBUG_FRAME_COUNT);
            let mut count = 0;
            for (transform, piece) in piece_query.iter() {
                if count < 10 { // 最初の10ピースの位置を確認
                    println!("  Piece({},{}) Transform: ({:.1}, {:.1}, {:.3})", 
                        piece.grid_x, piece.grid_y, 
                        transform.translation.x, transform.translation.y, transform.translation.z);
                    count += 1;
                } else {
                    break;
                }
            }
            
            // 統計情報も出力
            let total_pieces = piece_query.iter().count();
            println!("🔍 Total pieces found: {}", total_pieces);
        }
    }
}

pub fn handle_camera_drag(
    mut input_state: ResMut<InputState>,
    mut camera_query: Query<&mut Transform, With<MainCamera>>,
    mouse_input: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
) {
    let mouse_just_pressed = mouse_input.just_pressed(MouseButton::Right);
    let mouse_pressed = mouse_input.pressed(MouseButton::Right);
    let mouse_just_released = mouse_input.just_released(MouseButton::Right);
    
    let Ok(window) = windows.single() else { return; };
    
    // 右クリックでカメラドラッグ開始
    if mouse_just_pressed {
        input_state.is_camera_dragging = true;
        input_state.last_cursor_position = window.cursor_position();
    }
    
    // カメラドラッグ中の処理
    if input_state.is_camera_dragging && mouse_pressed {
        if let (Some(current_cursor), Some(last_cursor)) = (window.cursor_position(), input_state.last_cursor_position) {
            // スクリーン座標での移動量を計算
            let cursor_movement = current_cursor - last_cursor;
            
            // 移動量が0でない場合のみカメラを移動
            if cursor_movement.length() > 0.5 {
                for mut transform in camera_query.iter_mut() {
                    // カメラスケールを考慮した移動量
                    let scale_factor = transform.scale.x;
                    let movement = cursor_movement * scale_factor;
                    
                    // カメラの移動（マウスの動きと逆方向に移動、Y軸は反転）
                    transform.translation.x -= movement.x;
                    transform.translation.y += movement.y; // スクリーン座標系ではY軸が反転
                }
                
                // カーソル位置を更新
                input_state.last_cursor_position = Some(current_cursor);
            }
        }
    }
    
    // 右クリックリリースでカメラドラッグ終了
    if mouse_just_released {
        input_state.is_camera_dragging = false;
        input_state.last_cursor_position = None;
    }
}

/// Frustum cullingシステム - 画面外のピースを非表示にする
pub fn frustum_culling_system(
    mut piece_query: Query<(&Transform, &PuzzlePiece, &mut Visibility), With<PickablePiece>>,
    camera_query: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
) {
    let Ok((camera, camera_transform)) = camera_query.single() else {
        return;
    };
    
    // カメラの視錐台を取得
    let Some(viewport_size) = camera.logical_viewport_size() else {
        return;
    };
    
    // ビューポートの境界を計算（マージンを追加して急な消失を防ぐ）
    let margin = 200.0; // ピースサイズより大きめのマージン
    let half_width = viewport_size.x / 2.0 + margin;
    let half_height = viewport_size.y / 2.0 + margin;
    
    // カメラのスケールを考慮
    let camera_scale = camera_transform.compute_transform().scale.x;
    let scaled_half_width = half_width * camera_scale;
    let scaled_half_height = half_height * camera_scale;
    
    let camera_pos = camera_transform.translation().truncate();
    
    let mut visible_count = 0;
    let mut culled_count = 0;
    
    for (transform, piece, mut visibility) in piece_query.iter_mut() {
        let piece_pos = transform.translation.truncate();
        
        // ピースの境界を考慮した判定
        let piece_bounds_half = Vec2::new(
            piece.bounds.width() / 2.0,
            piece.bounds.height() / 2.0,
        );
        
        // ピースが視錐台内にあるか判定
        let in_frustum = 
            piece_pos.x + piece_bounds_half.x >= camera_pos.x - scaled_half_width &&
            piece_pos.x - piece_bounds_half.x <= camera_pos.x + scaled_half_width &&
            piece_pos.y + piece_bounds_half.y >= camera_pos.y - scaled_half_height &&
            piece_pos.y - piece_bounds_half.y <= camera_pos.y + scaled_half_height;
        
        if in_frustum {
            *visibility = Visibility::Inherited;
            visible_count += 1;
        } else {
            *visibility = Visibility::Hidden;
            culled_count += 1;
        }
    }
    
    // デバッグ出力（頻度を制限）
    static mut FRAME_COUNT: usize = 0;
    unsafe {
        FRAME_COUNT += 1;
        if FRAME_COUNT % 600 == 0 {  // 10秒ごとに出力（60FPSの場合）
            println!("🎯 Frustum Culling: {} visible, {} culled", visible_count, culled_count);
        }
    }
}

/// ピース生成を段階的に実行する新しいシステム
pub fn spawn_puzzle_pieces_progressive(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    puzzle_config: Res<PuzzleConfig>,
    puzzle_image: Option<Res<PuzzleImage>>,
    game_state: Res<GameState>,
    existing_pieces: Query<&PuzzlePiece>,
    existing_grid_ref: Query<&GridReference>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
    mut progress: ResMut<PieceGenerationProgress>,
) {
    // システム実行のデバッグログ（スポーン中のみ）
    if progress.generation_phase == GenerationPhase::SpawningEntities {
        static mut SYSTEM_CALL_COUNT: usize = 0;
        unsafe {
            SYSTEM_CALL_COUNT += 1;
            println!("🔄 System call #{}: spawned={}, pending={}", 
                SYSTEM_CALL_COUNT, progress.pieces_created, progress.pending_pieces.len());
        }
    }
    
    // ゲーム画面でない場合は何もしない
    if game_state.current_screen != GameScreen::InGame {
        return;
    }
    
    // 生成中でない場合で、かつ既にピースがある場合は何もしない
    if !progress.is_generating && !existing_pieces.is_empty() {
        return;
    }

    // 画像が選択されていない場合
    if puzzle_config.image_path.is_empty() {
        println!("⚠️ No image selected for puzzle creation!");
        return;
    }

    // PuzzleImageがまだない場合は待機
    let Some(ref puzzle_image) = puzzle_image else {
        return;
    };

    // 画像サイズが適切でない場合は待機
    if puzzle_image.size.x <= 10.0 || puzzle_image.size.y <= 10.0 {
        return;
    }

    // グリッド背景を表示（まだない場合）
    if existing_grid_ref.is_empty() && !progress.is_generating {
        spawn_grid_reference(&mut commands, &asset_server, &puzzle_config, Some(puzzle_image));
    }

    // 生成を開始
    if !progress.is_generating {
        let (grid_width, grid_height) = puzzle_config.grid_size;
        let total_pieces = grid_width * grid_height;
        
        println!("🎮 Starting progressive puzzle generation: {} pieces", total_pieces);
        
        progress.is_generating = true;
        progress.total_pieces = total_pieces;
        progress.grid_size = puzzle_config.grid_size;
        progress.generation_phase = GenerationPhase::PreparingShapes;
        progress.current_piece = 0;
        progress.shapes_generated = 0;
        progress.pieces_created = 0;

        // 画像サイズとピースサイズを計算
        let display_width = puzzle_image.size.x;
        let display_height = puzzle_image.size.y;
        let piece_width = display_width / grid_width as f32;
        let piece_height = display_height / grid_height as f32;

        // 標準スレッドでバックグラウンド処理を実行（crossbeam channelを使用）
        let (sender, receiver) = crossbeam::channel::unbounded();
        let (progress_sender, progress_receiver) = crossbeam::channel::unbounded();
        
        std::thread::spawn(move || {
            println!("🧵 Background thread started for shape generation");
            
            // 画像サイズとピースサイズを計算
            let piece_width = display_width / grid_width as f32;
            let piece_height = display_height / grid_height as f32;

            // ジグソー形状ジェネレータを初期化
            let mut shape_generator = JigsawShapeGenerator::new(
                (piece_width, piece_height),
                (grid_width, grid_height),
            );

            // ジグソーテンプレートを先に生成
            if let Err(e) = shape_generator.generate_jigsaw_template() {
                println!("Failed to generate jigsaw template: {}", e);
                return;
            }

            // 全ての形状を生成（進捗を定期的に送信）
            let mut generated_count = 0;
            for y in 0..grid_height {
                for x in 0..grid_width {
                    if let Err(e) = shape_generator.generate_piece_shape(x, y) {
                        println!("Failed to generate jigsaw shape ({}, {}): {}", x, y, e);
                    }
                    
                    generated_count += 1;
                    
                    // 10個ごとに進捗を送信
                    if generated_count % 10 == 0 {
                        let send_result = progress_sender.send(ProgressMessage::ShapeProgress(generated_count));
                        // 100個ごとにログで確認
                        if generated_count % 100 == 0 {
                            match send_result {
                                Ok(_) => println!("🧵 Sent progress: {} shapes", generated_count),
                                Err(e) => println!("🧵 Failed to send progress: {}", e),
                            }
                        }
                    }
                }
            }
            
            println!("🧵 Background thread completed all {} shapes", total_pieces);

            // 配置位置を事前生成
            let mut placement_positions = generate_placement_grid(
                grid_width, grid_height, 
                piece_width, piece_height, 
                display_width, display_height
            );
            
            // 順番をランダムにシャッフル
            use rand::seq::SliceRandom;
            placement_positions.shuffle(&mut rand::thread_rng());

            let result = ShapeGenerationResult {
                shape_generator,
                placement_positions,
                grid_size: (grid_width, grid_height),
                total_pieces,
            };
            
            // 完了通知を送信
            let _ = progress_sender.send(ProgressMessage::ShapeCompleted);
            
            if let Err(e) = sender.send(result) {
                println!("Failed to send result from background thread: {}", e);
            }
        });

        progress.bg_thread_receiver = Some(receiver);
        progress.progress_receiver = Some(progress_receiver);
        
        return;
    }

    // 進捗メッセージを処理
    let mut messages_to_process = Vec::new();
    if let Some(progress_receiver) = &progress.progress_receiver {
        while let Ok(message) = progress_receiver.try_recv() {
            messages_to_process.push(message);
        }
    }
    
    // 借用の競合を避けるため、メッセージを別途処理
    for message in messages_to_process {
        match message {
            ProgressMessage::ShapeProgress(count) => {
                progress.shapes_generated = count;
                // デバッグ: 進捗更新をログ
                if count % 100 == 0 {
                    println!("📊 Progress update: {} shapes generated", count);
                }
            }
            ProgressMessage::PieceProgress(count) => {
                progress.pieces_created = count;
                // デバッグ: 進捗更新をログ
                if count % 100 == 0 {
                    println!("📊 Progress update: {} pieces created", count);
                }
            }
            ProgressMessage::ShapeCompleted => {
                // 形状生成完了は別途処理される
            }
            ProgressMessage::PieceCompleted => {
                // ピース作成完了は別途処理される
            }
        }
    }

    // 形状生成フェーズ（バックグラウンドスレッドの完了チェック）
    if progress.generation_phase == GenerationPhase::PreparingShapes {
        if let Some(receiver) = &progress.bg_thread_receiver {
            // ノンブロッキングでメッセージをチェック
            match receiver.try_recv() {
                Ok(result) => {
                    println!("✅ Background shape generation completed: {} shapes", result.total_pieces);
                    
                    progress.shape_generator = Some(result.shape_generator);
                    progress.placement_positions = result.placement_positions;
                    progress.shapes_generated = result.total_pieces;
                    progress.generation_phase = GenerationPhase::CreatingPieces;
                    progress.pieces_created = 0;
                    progress.bg_thread_receiver = None; // レシーバーをクリア
                }
                Err(crossbeam::channel::TryRecvError::Empty) => {
                    // まだ完了していない - 進捗表示のためのダミー更新
                    static mut DUMMY_COUNTER: usize = 0;
                    unsafe {
                        DUMMY_COUNTER += 1;
                        if DUMMY_COUNTER % 60 == 0 { // 約1秒ごと
                            progress.shapes_generated = (progress.shapes_generated + 100).min(progress.total_pieces);
                            println!("🧵 Shape generation in progress... (estimated {}/{})", 
                                progress.shapes_generated, progress.total_pieces);
                        }
                    }
                }
                Err(crossbeam::channel::TryRecvError::Disconnected) => {
                    println!("❌ Background thread disconnected unexpectedly");
                    progress.generation_phase = GenerationPhase::Completed;
                    progress.is_generating = false;
                }
            }
        }
        return;
    }

    // ピース作成フェーズ（バックグラウンドスレッド）
    if progress.generation_phase == GenerationPhase::CreatingPieces {
        // バックグラウンドスレッドがまだ開始されていない場合、開始する
        if progress.piece_thread_receiver.is_none() {
            if let Some(shape_generator) = progress.shape_generator.take() {
                let placement_positions = progress.placement_positions.clone();
                let (grid_width, grid_height) = progress.grid_size;
                let total_pieces = progress.total_pieces;
                let display_width = puzzle_image.size.x;
                let display_height = puzzle_image.size.y;
                let image_handle = puzzle_image.handle.clone();
                
                println!("🧵 Starting background piece creation: {} pieces", total_pieces);
                
                let (sender, receiver) = crossbeam::channel::unbounded();
                
                // 進捗送信用のチャンネル（既存のものを再利用）
                let progress_sender = if let Some(existing_receiver) = &progress.progress_receiver {
                    // 既存のチャンネルに送信するために新しい送信者を作成
                    let (new_sender, _) = crossbeam::channel::unbounded::<ProgressMessage>();
                    new_sender
                } else {
                    crossbeam::channel::unbounded().0
                };
                
                std::thread::spawn(move || {
                    println!("🧵 Background thread started for piece creation");
                    
                    // ピースデータを作成（標準スレッド内で同期実行）
                    let result = create_all_pieces_sync(
                        shape_generator,
                        placement_positions,
                        grid_width,
                        grid_height,
                        total_pieces,
                        display_width,
                        display_height,
                        image_handle,
                    );
                    
                    if let Err(e) = sender.send(result) {
                        println!("Failed to send piece creation result: {}", e);
                    }
                });
                
                progress.piece_thread_receiver = Some(receiver);
            }
        }
        
        // バックグラウンドスレッドの完了をチェック
        if let Some(receiver) = &progress.piece_thread_receiver {
            match receiver.try_recv() {
                Ok(result) => {
                    let pieces_count = result.pieces.len();
                    println!("✅ Background piece creation completed: {} pieces, moving to spawn phase", pieces_count);
                    
                    // 待機列に追加
                    progress.pending_pieces = result.pieces;
                    progress.generation_phase = GenerationPhase::SpawningEntities;
                    progress.pieces_spawned_this_frame = 0;
                    progress.pieces_created = 0; // スポーン済み数をリセット
                    progress.piece_thread_receiver = None; // レシーバーをクリア
                    
                    println!("📦 Ready to spawn {} pieces", progress.pending_pieces.len());
                }
                Err(crossbeam::channel::TryRecvError::Empty) => {
                    // まだ完了していない - 進捗表示のためのダミー更新
                    static mut PIECE_DUMMY_COUNTER: usize = 0;
                    unsafe {
                        PIECE_DUMMY_COUNTER += 1;
                        if PIECE_DUMMY_COUNTER % 60 == 0 { // 約1秒ごと
                            progress.pieces_created = (progress.pieces_created + 50).min(progress.total_pieces);
                            println!("🧵 Piece creation in progress... (estimated {}/{})", 
                                progress.pieces_created, progress.total_pieces);
                        }
                    }
                }
                Err(crossbeam::channel::TryRecvError::Disconnected) => {
                    println!("❌ Piece creation thread disconnected unexpectedly");
                    progress.generation_phase = GenerationPhase::Completed;
                    progress.is_generating = false;
                }
            }
        }
        
        return;
    }

    // エンティティスポーンフェーズ（大量のピースを効率的にスポーン）
    if progress.generation_phase == GenerationPhase::SpawningEntities {
        // 毎フレームログ出力（デバッグ用）
        println!("📦 Spawn frame - spawned: {}, pending: {}, phase: {:?}", 
            progress.pieces_created, progress.pending_pieces.len(), progress.generation_phase);
        
        let mut spawned_this_frame = 0;
        const MAX_SPAWNS_PER_FRAME: usize = 1000; // より多くのピースを一度に処理（調整済み）
        
        // 大量のピースを効率的にスポーン（フレーム制限あり）
        while !progress.pending_pieces.is_empty() && spawned_this_frame < MAX_SPAWNS_PER_FRAME {
            let piece_data = progress.pending_pieces.pop().unwrap(); // 末尾から取得（O(1)）
            
            let mesh_handle = meshes.add(piece_data.mesh);
            
            // マテリアルを作成
            let material = ColorMaterial {
                texture: Some(puzzle_image.handle.clone()),
                ..default()
            };
            let material_handle = materials.add(material);
            
            commands.spawn((
                Mesh2d(mesh_handle),
                MeshMaterial2d(material_handle),
                piece_data.transform,
                piece_data.piece_component,
                piece_data.piece_shape,
                PickablePiece {
                    drag_offset: Vec2::ZERO,
                },
                Draggable {
                    is_dragging: false,
                    drag_offset: Vec2::ZERO,
                },
            ));
            
            progress.pieces_created += 1;
            spawned_this_frame += 1;
        }
        
        // 進捗表示（フレームごと、または最後）
        if spawned_this_frame == MAX_SPAWNS_PER_FRAME || progress.pending_pieces.is_empty() {
            println!("📦 Spawning entities: {}/{} (remaining: {})", 
                progress.pieces_created, progress.total_pieces, progress.pending_pieces.len());
        }
        
        // 全てのエンティティがスポーンされた場合
        if progress.pending_pieces.is_empty() {
            progress.generation_phase = GenerationPhase::Completed;
            progress.is_generating = false;
            progress.placement_positions.clear();
            
            println!("🎉 All entities spawned: {} pieces", progress.pieces_created);
        }
        
        return;
    }
}

/// 単一のピースを作成するヘルパー関数
fn create_single_puzzle_piece(
    commands: &mut Commands,
    x: usize,
    y: usize,
    puzzle_config: &PuzzleConfig,
    puzzle_image: &PuzzleImage,
    meshes: &mut ResMut<Assets<Mesh>>,
    materials: &mut ResMut<Assets<ColorMaterial>>,
    shape: &JigsawPieceShape,
    placement_positions: &[Vec2],
    position_index: usize,
) {
    use crate::jigsaw_shapes::clone_mesh_from_shape;
    use uuid::Uuid;

    let piece_id = Uuid::new_v4();
    let (grid_width, grid_height) = puzzle_config.grid_size;
    
    let display_width = puzzle_image.size.x;
    let display_height = puzzle_image.size.y;
    let piece_width = display_width / grid_width as f32;
    let piece_height = display_height / grid_height as f32;
    
    // 正しい位置を計算
    let correct_x = (x as f32 - (grid_width as f32 - 1.0) / 2.0) * piece_width;
    let correct_y = ((grid_height as f32 - 1.0) / 2.0 - y as f32) * piece_height;
    let correct_position = Vec2::new(correct_x, correct_y);
    
    // 開始位置を取得
    let start_position = if position_index < placement_positions.len() {
        placement_positions[position_index]
    } else {
        println!("⚠️ Position index {} exceeds placement_positions length {}", position_index, placement_positions.len());
        Vec2::new(0.0, 0.0)
    };
    
    let texture_coords = Vec4::new(
        x as f32 / grid_width as f32,
        y as f32 / grid_height as f32,
        (x + 1) as f32 / grid_width as f32,
        (y + 1) as f32 / grid_height as f32,
    );
    
    // 当たり判定用の境界
    let margin_ratio = 1.3;
    let half_width = (piece_width * margin_ratio) / 2.0;
    let half_height = (piece_height * margin_ratio) / 2.0;
    let collision_bounds = Rect::new(
        -half_width,
        -half_height,
        half_width,
        half_height
    );
    
    let piece = PuzzlePiece {
        id: piece_id,
        original_position: start_position,
        current_position: start_position,
        correct_position,
        texture_coords,
        is_placed: false,
        grid_x: x,
        grid_y: y,
        bounds: collision_bounds,
    };
    
    let z_offset = (y * grid_width + x) as f32 * 0.001;
    
    // メッシュをクローン
    let mesh = clone_mesh_from_shape(shape);
    let piece_shape = extract_shape_data(&mesh);
    let mesh_handle = meshes.add(mesh);
    
    // マテリアルを作成
    let material = ColorMaterial {
        texture: Some(puzzle_image.handle.clone()),
        ..default()
    };
    let material_handle = materials.add(material);
    
    // エンティティを生成
    commands.spawn((
        Mesh2d(mesh_handle),
        MeshMaterial2d(material_handle),
        Transform::from_translation(start_position.extend(z_offset)),
        piece,
        piece_shape,
        PickablePiece {
            drag_offset: Vec2::ZERO,
        },
        Draggable {
            is_dragging: false,
            drag_offset: Vec2::ZERO,
        },
    ));
}

/// 非同期でピース作成を行う関数
async fn create_all_pieces_async(
    shape_generator: JigsawShapeGenerator,
    placement_positions: Vec<Vec2>,
    grid_width: usize,
    grid_height: usize,
    total_pieces: usize,
    display_width: f32,
    display_height: f32,
    image_handle: Handle<Image>,
) -> PieceCreationResult {
    use crate::jigsaw_shapes::clone_mesh_from_shape;
    use crate::components::*;
    use uuid::Uuid;
    use bevy::sprite::ColorMaterial;
    
    let mut pieces = Vec::with_capacity(total_pieces);
    let piece_width = display_width / grid_width as f32;
    let piece_height = display_height / grid_height as f32;
    
    for piece_index in 0..total_pieces {
        let y = piece_index / grid_width;
        let x = piece_index % grid_width;
        
        if let Some(shape) = shape_generator.get_shape(x, y) {
            let piece_id = Uuid::new_v4();
            
            // 正しい位置を計算
            let correct_x = (x as f32 - (grid_width as f32 - 1.0) / 2.0) * piece_width;
            let correct_y = ((grid_height as f32 - 1.0) / 2.0 - y as f32) * piece_height;
            let correct_position = Vec2::new(correct_x, correct_y);
            
            // 開始位置を取得
            let start_position = if piece_index < placement_positions.len() {
                placement_positions[piece_index]
            } else {
                Vec2::new(0.0, 0.0)
            };
            
            let texture_coords = Vec4::new(
                x as f32 / grid_width as f32,
                y as f32 / grid_height as f32,
                (x + 1) as f32 / grid_width as f32,
                (y + 1) as f32 / grid_height as f32,
            );
            
            // 当たり判定用の境界
            let margin_ratio = 1.3;
            let half_width = (piece_width * margin_ratio) / 2.0;
            let half_height = (piece_height * margin_ratio) / 2.0;
            let collision_bounds = Rect::new(
                -half_width,
                -half_height,
                half_width,
                half_height
            );
            
            let piece_component = PuzzlePiece {
                id: piece_id,
                original_position: start_position,
                current_position: start_position,
                correct_position,
                texture_coords,
                is_placed: false,
                grid_x: x,
                grid_y: y,
                bounds: collision_bounds,
            };
            
            let z_offset = (y * grid_width + x) as f32 * 0.001;
            
            // メッシュをクローン（ここが重い処理）
            let mesh = clone_mesh_from_shape(shape);
            let piece_shape = extract_shape_data(&mesh);
            
            // マテリアルハンドルを作成（Bevyアセットは非同期では作成できないため、ハンドルのみ）
            let material_handle = Handle::<ColorMaterial>::default(); // 後でメインスレッドで設定
            
            let transform = Transform::from_translation(start_position.extend(z_offset));
            
            pieces.push(PieceData {
                mesh,
                piece_component,
                piece_shape,
                transform,
                material_handle, // 一時的な値
            });
        }
        
        // 進捗表示
        if piece_index % 1000 == 0 {
            println!("🔧 Async piece creation progress: {}/{}", piece_index, total_pieces);
        }
    }
    
    println!("✅ Async piece creation completed: {} pieces", pieces.len());
    PieceCreationResult { pieces }
}

/// 同期版のピース作成関数（標準スレッド用）
fn create_all_pieces_sync(
    shape_generator: JigsawShapeGenerator,
    placement_positions: Vec<Vec2>,
    grid_width: usize,
    grid_height: usize,
    total_pieces: usize,
    display_width: f32,
    display_height: f32,
    image_handle: Handle<Image>,
) -> PieceCreationResult {
    use crate::jigsaw_shapes::clone_mesh_from_shape;
    use crate::components::*;
    use uuid::Uuid;
    use bevy::sprite::ColorMaterial;
    
    let mut pieces = Vec::with_capacity(total_pieces);
    let piece_width = display_width / grid_width as f32;
    let piece_height = display_height / grid_height as f32;
    
    println!("🧵 Starting piece creation loop for {} pieces", total_pieces);
    
    for piece_index in 0..total_pieces {
        let y = piece_index / grid_width;
        let x = piece_index % grid_width;
        
        if let Some(shape) = shape_generator.get_shape(x, y) {
            let piece_id = Uuid::new_v4();
            
            // 正しい位置を計算
            let correct_x = (x as f32 - (grid_width as f32 - 1.0) / 2.0) * piece_width;
            let correct_y = ((grid_height as f32 - 1.0) / 2.0 - y as f32) * piece_height;
            let correct_position = Vec2::new(correct_x, correct_y);
            
            // 開始位置を取得
            let start_position = if piece_index < placement_positions.len() {
                placement_positions[piece_index]
            } else {
                Vec2::new(0.0, 0.0)
            };
            
            let texture_coords = Vec4::new(
                x as f32 / grid_width as f32,
                y as f32 / grid_height as f32,
                (x + 1) as f32 / grid_width as f32,
                (y + 1) as f32 / grid_height as f32,
            );
            
            // 当たり判定用の境界
            let margin_ratio = 1.3;
            let half_width = (piece_width * margin_ratio) / 2.0;
            let half_height = (piece_height * margin_ratio) / 2.0;
            let collision_bounds = Rect::new(
                -half_width,
                -half_height,
                half_width,
                half_height
            );
            
            let piece_component = PuzzlePiece {
                id: piece_id,
                original_position: start_position,
                current_position: start_position,
                correct_position,
                texture_coords,
                is_placed: false,
                grid_x: x,
                grid_y: y,
                bounds: collision_bounds,
            };
            
            let z_offset = (y * grid_width + x) as f32 * 0.001;
            
            // メッシュをクローン（ここが重い処理だがバックグラウンドで実行）
            if piece_index < 5 {
                println!("🧵 Cloning mesh for piece {}", piece_index);
            }
            let mesh = clone_mesh_from_shape(shape);
            if piece_index < 5 {
                println!("🧵 Extracting shape data for piece {}", piece_index);
            }
            let piece_shape = extract_shape_data(&mesh);
            
            // マテリアルハンドルを作成（Bevyアセットは非同期では作成できないため、ハンドルのみ）
            let material_handle = Handle::<ColorMaterial>::default(); // 後でメインスレッドで設定
            
            let transform = Transform::from_translation(start_position.extend(z_offset));
            
            pieces.push(PieceData {
                mesh,
                piece_component,
                piece_shape,
                transform,
                material_handle, // 一時的な値
            });
        }
        
        // 進捗表示（より頻繁に）
        if piece_index % 100 == 0 {
            println!("🧵 Background piece creation progress: {}/{}", piece_index, total_pieces);
        }
    }
    
    println!("✅ Background piece creation completed: {} pieces", pieces.len());
    PieceCreationResult { pieces }
}

/// ESCキー入力でゲーム内メニューの表示/非表示を切り替え
pub fn handle_escape_input(
    mut game_state: ResMut<GameState>,
    keyboard_input: Res<ButtonInput<KeyCode>>,
) {
    if keyboard_input.just_pressed(KeyCode::Escape) {
        match game_state.current_screen {
            GameScreen::InGame => {
                // ゲーム中にESCキーが押されたらメニューを表示
                game_state.current_screen = GameScreen::InGameMenu;
                println!("🎮 Opening in-game menu");
            },
            GameScreen::InGameMenu => {
                // メニュー表示中にESCキーが押されたらゲームに戻る
                game_state.current_screen = GameScreen::InGame;
                println!("🎮 Resuming game");
            },
            _ => {
                // 他の画面では何もしない
            }
        }
    }
}

