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
            
            println!("📝 Registered piece {} to collision system", piece_id);
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
) {
    if input.just_pressed(KeyCode::F11) {
        println!("🔍 {}", collision_system.get_performance_stats());
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
        collision_system.rebuild_quad_tree();
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
        collision_system.rebuild_quad_tree();
        
        println!("🔧 Collision system optimized - QuadTree rebuilt");
    }
}

/// レイキャスティングのテストシステム（デバッグ用）
pub fn test_ray_casting(
    mut collision_system: ResMut<PieceCollisionSystem>,
    input: Res<ButtonInput<KeyCode>>,
    camera_query: Query<(&Camera, &GlobalTransform)>,
    windows: Query<&Window>,
) {
    if input.just_pressed(KeyCode::KeyR) {
        if let Ok(window) = windows.single() {
            if let Some(cursor_position) = window.cursor_position() {
                if let Ok((camera, camera_transform)) = camera_query.single() {
                    if let Ok(world_position) = camera.viewport_to_world_2d(camera_transform, cursor_position) {
                        // マウス位置から下向きのレイキャスト
                        let ray_direction = Vec2::new(0.0, -1.0);
                        
                        if let Some(hit_piece) = collision_system.ray_cast(world_position, ray_direction) {
                            println!("🎯 Ray hit piece: {}", hit_piece);
                        } else {
                            println!("🎯 Ray missed all pieces");
                        }
                    }
                }
            }
        }
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
                        // マウス位置でのピース検索テスト
                        if let Some(piece_id) = collision_system.find_piece_at_position(world_position) {
                            println!("🔍 Found piece at cursor: {}", piece_id);
                            
                            // 精密な当たり判定テスト
                            let precise_hit = collision_system.precise_point_in_piece(piece_id, world_position);
                            println!("🎯 Precise hit test: {}", precise_hit);
                        } else {
                            println!("🔍 No piece found at cursor position");
                        }
                        
                        // 範囲選択テスト（カーソル周辺100x100の範囲）
                        let rect = Rect::new(
                            world_position.x - 50.0,
                            world_position.y - 50.0,
                            world_position.x + 50.0,
                            world_position.y + 50.0,
                        );
                        let pieces_in_rect = collision_system.find_pieces_in_rect(rect);
                        println!("📦 Pieces in 100x100 rect: {} pieces", pieces_in_rect.len());
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