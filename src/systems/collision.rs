use bevy::prelude::*;
use crate::components::*;
use crate::resources::*;

/// 新しく作成されたピースをPieceCollisionSystemに追加するシステム
pub fn register_new_pieces_to_collision_system(
    mut collision_system: ResMut<PieceCollisionSystem>,
    id_manager: Res<PieceIdManager>,
    new_pieces: Query<(Entity, &PuzzlePiece, &PieceShape, &Transform), Added<PuzzlePiece>>,
) {
    for (entity, puzzle_piece, piece_shape, transform) in new_pieces.iter() {
        if let Some(piece_id) = id_manager.get_piece_id(entity) {
            // テッセレーション結果から頂点とインデックスを取得
            let vertices: Vec<Vec2> = piece_shape.vertices.iter()
                .map(|&[x, y]| Vec2::new(x, y))
                .collect();
            let indices = piece_shape.indices.clone();
            
            // バウンディングボックスを計算
            let mut min_x = f32::INFINITY;
            let mut max_x = f32::NEG_INFINITY;
            let mut min_y = f32::INFINITY;
            let mut max_y = f32::NEG_INFINITY;
            
            for vertex in &vertices {
                min_x = min_x.min(vertex.x);
                max_x = max_x.max(vertex.x);
                min_y = min_y.min(vertex.y);
                max_y = max_y.max(vertex.y);
            }
            
            let position = transform.translation.truncate();
            let bounding_box = Rect::new(
                position.x + min_x,
                position.y + min_y,
                position.x + max_x,
                position.y + max_y,
            );
            
            let collision_data = PieceCollisionData {
                piece_id,
                position,
                bounding_box,
                vertices,
                indices,
            };
            
            collision_system.add_piece(collision_data);
            
            println!("📝 Registered piece {} to collision system (pos: {:?}, bounds: {:?})", 
                    piece_id, position, bounding_box);
        }
    }
}

/// ピース位置が変更されたときにコリジョンシステムを更新するシステム
pub fn update_collision_system_positions(
    mut collision_system: ResMut<PieceCollisionSystem>,
    id_manager: Res<PieceIdManager>,
    changed_pieces: Query<(Entity, &Transform), (With<PuzzlePiece>, Changed<Transform>)>,
) {
    for (entity, transform) in changed_pieces.iter() {
        if let Some(piece_id) = id_manager.get_piece_id(entity) {
            let new_position = transform.translation.truncate();
            collision_system.update_piece_position(piece_id, new_position);
        }
    }
}

/// ピース削除時にコリジョンシステムからクリーンアップするシステム
pub fn cleanup_removed_pieces_from_collision_system(
    _collision_system: ResMut<PieceCollisionSystem>,
    mut removed_pieces: RemovedComponents<PuzzlePiece>,
    _id_manager: Res<PieceIdManager>,
    // 削除されたエンティティのIDは取得できないため、別の方法が必要
) {
    // RemovedComponentsからは削除されたEntityしか取得できないため、
    // ID管理システムと連携してクリーンアップする必要がある
    // この実装は id_management.rs の cleanup と連携する
    
    for _entity in removed_pieces.read() {
        // エンティティが削除されている場合、IDManagerからも削除されているはず
        // そのため、ここでは特別な処理は不要
        // 代わりに定期的なクリーンアップを実装
    }
}

/// コリジョンシステムの統計情報を表示するシステム（デバッグ用）
pub fn debug_collision_system_stats(
    collision_system: Res<PieceCollisionSystem>,
    input: Res<ButtonInput<KeyCode>>,
    puzzle_pieces_query: Query<Entity, With<PuzzlePiece>>,
    id_manager: Res<PieceIdManager>,
) {
    if input.just_pressed(KeyCode::F11) {
        println!("🔍 === Collision System Debug Stats ===");
        println!("{}", collision_system.get_performance_stats());
        
        let bevy_piece_count = puzzle_pieces_query.iter().count();
        let id_manager_count = id_manager.get_all_piece_ids().len();
        
        println!("📊 Cross-system comparison:");
        println!("  Bevy ECS pieces: {}", bevy_piece_count);
        println!("  ID Manager pieces: {}", id_manager_count);
        println!("  Collision system pieces: {}", collision_system.pieces.len());
        
        if collision_system.pieces.len() != bevy_piece_count {
            println!("⚠️ WARNING: Piece count mismatch detected!");
            println!("   This suggests pieces are not being registered to collision system");
        }
        
        if !collision_system.pieces.is_empty() {
            println!("📦 Sample piece data:");
            let sample_piece = collision_system.pieces.iter().next().unwrap();
            println!("   ID: {}", sample_piece.0);
            println!("   Position: {:?}", sample_piece.1.position);
            println!("   Bounds: {:?}", sample_piece.1.bounding_box);
        }
        
        println!("🔍 === End Debug Stats ===");
    }
}

/// パフォーマンステスト用システム
pub fn performance_test_collision_system(
    mut collision_system: ResMut<PieceCollisionSystem>,
    input: Res<ButtonInput<KeyCode>>,
) {
    if input.just_pressed(KeyCode::KeyP) {
        use std::time::Instant;
        
        println!("🚀 Starting collision system performance test...");
        
        // テスト用の座標
        let test_positions = vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(100.0, 100.0),
            Vec2::new(-100.0, -100.0),
            Vec2::new(50.0, -50.0),
            Vec2::new(-50.0, 50.0),
        ];
        
        // 1. ピース検索パフォーマンステスト
        let start = Instant::now();
        let mut found_count = 0;
        for _ in 0..1000 {
            for &pos in &test_positions {
                if collision_system.find_piece_at_position(pos).is_some() {
                    found_count += 1;
                }
            }
        }
        let search_time = start.elapsed();
        
        // 2. 範囲検索パフォーマンステスト
        let start = Instant::now();
        let mut total_pieces_found = 0;
        for _ in 0..100 {
            let test_rect = Rect::new(-200.0, -200.0, 200.0, 200.0);
            let pieces = collision_system.find_pieces_in_rect(test_rect);
            total_pieces_found += pieces.len();
        }
        let rect_search_time = start.elapsed();
        
        // 3. QuadTree再構築パフォーマンステスト
        let start = Instant::now();
        collision_system.need_rebuild = true;
        collision_system.rebuild_rtree();
        let rebuild_time = start.elapsed();
        
        println!("📊 Performance Test Results:");
        println!("  Point searches: 5000 queries in {:?} ({:.2} μs/query)", 
                 search_time, search_time.as_micros() as f64 / 5000.0);
        println!("  Found pieces: {}", found_count);
        println!("  Rect searches: 100 queries in {:?} ({:.2} μs/query)", 
                 rect_search_time, rect_search_time.as_micros() as f64 / 100.0);
        println!("  Total pieces found in rects: {}", total_pieces_found);
        println!("  QuadTree rebuild: {:?}", rebuild_time);
        println!("  System has {} pieces", collision_system.pieces.len());
    }
}

/// QuadTreeの最適化とクリーンアップシステム
pub fn optimize_collision_system(
    mut collision_system: ResMut<PieceCollisionSystem>,
    time: Res<Time>,
    mut last_optimize_time: Local<f32>,
) {
    let current_time = time.elapsed_secs();
    
    // 5秒ごとに最適化
    if current_time - *last_optimize_time > 5.0 {
        *last_optimize_time = current_time;
        
        // QuadTreeの再構築を強制
        collision_system.need_rebuild = true;
        collision_system.rebuild_rtree();
        
        println!("🔧 Collision system optimized - QuadTree rebuilt");
    }
}

/// レイキャスティングのテストシステム（詳細デバッグ付き）
pub fn test_ray_casting(
    mut collision_system: ResMut<PieceCollisionSystem>,
    input: Res<ButtonInput<KeyCode>>,
    camera_query: Query<(&Camera, &GlobalTransform)>,
    windows: Query<&Window>,
) {
    if input.just_pressed(KeyCode::KeyR) {
        println!("🔍 === Ray Casting Debug Test ===");
        
        if let Ok(window) = windows.single() {
            if let Some(cursor_position) = window.cursor_position() {
                println!("🖱️ Cursor screen position: {:?}", cursor_position);
                
                if let Ok((camera, camera_transform)) = camera_query.single() {
                    if let Ok(world_position) = camera.viewport_to_world_2d(camera_transform, cursor_position) {
                        println!("🌍 World position: {:?}", world_position);
                        
                        // カメラ情報の表示
                        let camera_translation = camera_transform.translation();
                        println!("📷 Camera position: {:?}", camera_translation);
                        println!("📷 Camera scale: {:?}", camera_transform.scale());
                        
                        // 詳細なバウンディングボックスデバッグを実行
                        let (detailed_hit, detailed_debug) = collision_system.find_piece_at_position_debug(world_position);
                        
                        if let Some(piece_id) = detailed_hit {
                            println!("✅ DETAILED SEARCH HIT piece: {}", piece_id);
                        } else {
                            println!("❌ DETAILED SEARCH MISSED - no piece at cursor position");
                        }
                        
                        println!("📊 Detailed Debug Info:\n{}", detailed_debug);
                        
                        // 従来のレイキャストも実行して比較
                        let ray_direction = Vec2::new(0.0, -1.0);
                        let (hit_piece, ray_debug_info) = collision_system.ray_cast_debug(world_position, ray_direction);
                        
                        println!("\n📊 Ray Cast Debug Info:\n{}", ray_debug_info);
                    } else {
                        println!("❌ Failed to convert cursor to world position");
                    }
                } else {
                    println!("❌ Camera not found");
                }
            } else {
                println!("❌ Cursor position not available");
            }
        } else {
            println!("❌ Window not found");
        }
        
        println!("🔍 === End Ray Casting Debug ===");
    }
}

/// IDベースの当たり判定API使用例システム（デバッグ用）
pub fn test_collision_api(
    mut collision_system: ResMut<PieceCollisionSystem>,
    input: Res<ButtonInput<KeyCode>>,
    camera_query: Query<(&Camera, &GlobalTransform)>,
    windows: Query<&Window>,
) {
    if input.just_pressed(KeyCode::KeyT) {
        if let Ok(window) = windows.single() {
            if let Some(cursor_position) = window.cursor_position() {
                if let Ok((camera, camera_transform)) = camera_query.single() {
                    if let Ok(world_position) = camera.viewport_to_world_2d(camera_transform, cursor_position) {
                        println!("🔍 === Collision API Test ===");
                        println!("🌍 World position: {:?}", world_position);
                        
                        // 1. 詳細なピース検索テスト（バウンディングボックスデバッグ付き）
                        let (detailed_hit, detailed_debug) = collision_system.find_piece_at_position_debug(world_position);
                        
                        if let Some(piece_id) = detailed_hit {
                            println!("🔍 Found piece at cursor: {}", piece_id);
                        } else {
                            println!("🔍 No piece found at cursor position");
                        }
                        
                        println!("📊 Detailed Debug:\n{}", detailed_debug);
                        
                        // 2. 最も近いピース検索（1000px範囲）
                        if let Some((nearest_id, distance)) = collision_system.find_nearest_piece(world_position, 1000.0) {
                            println!("🎯 Nearest piece within 1000px: {} (distance: {:.1}px)", nearest_id, distance);
                        } else {
                            println!("🎯 No pieces within 1000px");
                        }
                        
                        // 3. 広範囲検索（500px範囲）
                        let large_area_pieces = collision_system.find_pieces_in_large_area(world_position, 500.0);
                        println!("📦 Pieces in 500px radius: {} pieces", large_area_pieces.len());
                        
                        // 4. 精密形状判定での検索（100px範囲）
                        let precise_hits = collision_system.find_pieces_with_precise_hit(world_position, 100.0);
                        println!("🎯 Precise shape hits (100px): {} pieces", precise_hits.len());
                        
                        if !precise_hits.is_empty() {
                            println!("   Pieces with cursor inside shape:");
                            for (i, (piece_id, distance)) in precise_hits.iter().take(3).enumerate() {
                                println!("   {}. {} (distance: {:.1}px)", i + 1, piece_id, distance);
                            }
                        } else if !large_area_pieces.is_empty() {
                            println!("   Top 5 closest pieces (no precise hits):");
                            for (i, (piece_id, distance)) in large_area_pieces.iter().take(5).enumerate() {
                                println!("   {}. {} (distance: {:.1}px)", i + 1, piece_id, distance);
                            }
                        }
                        
                        // 4. 範囲選択テスト（カーソル周辺100x100の範囲）
                        let rect = Rect::new(
                            world_position.x - 50.0,
                            world_position.y - 50.0,
                            world_position.x + 50.0,
                            world_position.y + 50.0,
                        );
                        let pieces_in_rect = collision_system.find_pieces_in_rect(rect);
                        println!("📦 Pieces in 100x100 rect: {} pieces", pieces_in_rect.len());
                        
                        println!("🔍 === End API Test ===");
                    }
                }
            }
        }
    }
}

/// 実際のドラッグ操作でのIDベース当たり判定統合ヘルパー
pub fn id_based_piece_picker(
    collision_system: &mut PieceCollisionSystem,
    world_position: Vec2,
    use_precise_detection: bool,
) -> Option<PieceId> {
    // 基本的なピース検索
    if let Some(piece_id) = collision_system.find_piece_at_position(world_position) {
        if use_precise_detection {
            // 精密判定を使用する場合
            if collision_system.precise_point_in_piece(piece_id, world_position) {
                Some(piece_id)
            } else {
                None
            }
        } else {
            // バウンディングボックスのみの場合
            Some(piece_id)
        }
    } else {
        None
    }
}

/// 範囲選択でのIDベース当たり判定統合ヘルパー
pub fn id_based_rect_selector(
    collision_system: &mut PieceCollisionSystem,
    selection_rect: Rect,
    use_precise_detection: bool,
) -> Vec<PieceId> {
    let candidate_pieces = collision_system.find_pieces_in_rect(selection_rect);
    
    if use_precise_detection {
        // 精密判定: 範囲内の各コーナーでテスト
        let corners = [
            Vec2::new(selection_rect.min.x, selection_rect.min.y),
            Vec2::new(selection_rect.max.x, selection_rect.min.y),
            Vec2::new(selection_rect.min.x, selection_rect.max.y),
            Vec2::new(selection_rect.max.x, selection_rect.max.y),
        ];
        
        candidate_pieces.into_iter().filter(|&piece_id| {
            // 少なくとも1つのコーナーがピース内にあるかチェック
            corners.iter().any(|&corner| {
                collision_system.precise_point_in_piece(piece_id, corner)
            })
        }).collect()
    } else {
        // バウンディングボックスのみ
        candidate_pieces
    }
}

/// 手動でコリジョンシステムを再構築するシステム（デバッグ用）
pub fn manual_rebuild_collision_system(
    mut collision_system: ResMut<PieceCollisionSystem>,
    id_manager: Res<PieceIdManager>,
    pieces_query: Query<(Entity, &PuzzlePiece, &PieceShape, &Transform)>,
    input: Res<ButtonInput<KeyCode>>,
) {
    if input.just_pressed(KeyCode::KeyU) {
        println!("🔄 Manual collision system rebuild triggered...");
        
        // 既存データをクリア
        collision_system.pieces.clear();
        collision_system.need_rebuild = true;
        
        let mut registered_count = 0;
        
        // 全ピースを再登録
        for (entity, _puzzle_piece, piece_shape, transform) in pieces_query.iter() {
            if let Some(piece_id) = id_manager.get_piece_id(entity) {
                // テッセレーション結果から頂点とインデックスを取得
                let vertices: Vec<Vec2> = piece_shape.vertices.iter()
                    .map(|&[x, y]| Vec2::new(x, y))
                    .collect();
                let indices = piece_shape.indices.clone();
                
                // バウンディングボックスを計算
                let mut min_x = f32::INFINITY;
                let mut max_x = f32::NEG_INFINITY;
                let mut min_y = f32::INFINITY;
                let mut max_y = f32::NEG_INFINITY;
                
                for vertex in &vertices {
                    min_x = min_x.min(vertex.x);
                    max_x = max_x.max(vertex.x);
                    min_y = min_y.min(vertex.y);
                    max_y = max_y.max(vertex.y);
                }
                
                let position = transform.translation.truncate();
                let bounding_box = Rect::new(
                    position.x + min_x,
                    position.y + min_y,
                    position.x + max_x,
                    position.y + max_y,
                );
                
                let collision_data = PieceCollisionData {
                    piece_id,
                    position,
                    bounding_box,
                    vertices,
                    indices,
                };
                
                collision_system.add_piece(collision_data);
                registered_count += 1;
            }
        }
        
        println!("✅ Manual rebuild completed: {} pieces registered", registered_count);
        
        // QuadTreeも再構築
        collision_system.rebuild_rtree();
        println!("✅ QuadTree rebuilt");
    }
}