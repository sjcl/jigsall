use crate::{components::*, resources::*};
use bevy::prelude::*;

pub fn debug_collision_system_stats(
    collision_system: Res<PieceCollisionSystem>,
    input: Res<ButtonInput<KeyCode>>,
    puzzle_pieces_query: Query<Entity, With<PuzzlePiece>>,
    id_manager: Res<PieceIdManager>,
    store: Res<PieceDataStore>,
) {
    if input.just_pressed(KeyCode::F11) {
        println!("🔍 === Collision System Debug Stats ===");
        println!("{}", collision_system.get_performance_stats());

        let bevy_piece_count = puzzle_pieces_query.iter().count();
        let id_manager_count = id_manager.len();

        println!("📊 Cross-system comparison:");
        println!("  Bevy ECS pieces: {}", bevy_piece_count);
        println!("  ID Manager pieces: {}", id_manager_count);
        println!(
            "  Collision system pieces: {}",
            collision_system.pieces.len()
        );

        let unplaced_count = store.pieces.len() - store.placed_pieces.len();
        println!("  Canonical unplaced pieces: {}", unplaced_count);
        if collision_system.pieces.len() != unplaced_count {
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
        println!(
            "  Point searches: 5000 queries in {:?} ({:.2} μs/query)",
            search_time,
            search_time.as_micros() as f64 / 5000.0
        );
        println!("  Found pieces: {}", found_count);
        println!(
            "  Rect searches: 100 queries in {:?} ({:.2} μs/query)",
            rect_search_time,
            rect_search_time.as_micros() as f64 / 100.0
        );
        println!("  Total pieces found in rects: {}", total_pieces_found);
        println!("  QuadTree rebuild: {:?}", rebuild_time);
        println!("  System has {} pieces", collision_system.pieces.len());
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
                    if let Ok(world_position) =
                        camera.viewport_to_world_2d(camera_transform, cursor_position)
                    {
                        println!("🌍 World position: {:?}", world_position);

                        // カメラ情報の表示
                        let camera_translation = camera_transform.translation();
                        println!("📷 Camera position: {:?}", camera_translation);
                        println!("📷 Camera scale: {:?}", camera_transform.scale());

                        // 詳細なバウンディングボックスデバッグを実行
                        let (detailed_hit, detailed_debug) =
                            collision_system.find_piece_at_position_debug(world_position);

                        if let Some(piece_id) = detailed_hit {
                            println!("✅ DETAILED SEARCH HIT piece: {}", piece_id);
                        } else {
                            println!("❌ DETAILED SEARCH MISSED - no piece at cursor position");
                        }

                        println!("📊 Detailed Debug Info:\n{}", detailed_debug);

                        // 従来のレイキャストも実行して比較
                        let ray_direction = Vec2::new(0.0, -1.0);
                        let (_hit_piece, ray_debug_info) =
                            collision_system.ray_cast_debug(world_position, ray_direction);

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
                    if let Ok(world_position) =
                        camera.viewport_to_world_2d(camera_transform, cursor_position)
                    {
                        println!("🔍 === Collision API Test ===");
                        println!("🌍 World position: {:?}", world_position);

                        // 1. 詳細なピース検索テスト（バウンディングボックスデバッグ付き）
                        let (detailed_hit, detailed_debug) =
                            collision_system.find_piece_at_position_debug(world_position);

                        if let Some(piece_id) = detailed_hit {
                            println!("🔍 Found piece at cursor: {}", piece_id);
                        } else {
                            println!("🔍 No piece found at cursor position");
                        }

                        println!("📊 Detailed Debug:\n{}", detailed_debug);

                        // 2. 最も近いピース検索（1000px範囲）
                        if let Some((nearest_id, distance)) =
                            collision_system.find_nearest_piece(world_position, 1000.0)
                        {
                            println!(
                                "🎯 Nearest piece within 1000px: {} (distance: {:.1}px)",
                                nearest_id, distance
                            );
                        } else {
                            println!("🎯 No pieces within 1000px");
                        }

                        // 3. 広範囲検索（500px範囲）
                        let large_area_pieces =
                            collision_system.find_pieces_in_large_area(world_position, 500.0);
                        println!(
                            "📦 Pieces in 500px radius: {} pieces",
                            large_area_pieces.len()
                        );

                        // 4. 精密形状判定での検索（100px範囲）
                        let precise_hits =
                            collision_system.find_pieces_with_precise_hit(world_position, 100.0);
                        println!(
                            "🎯 Precise shape hits (100px): {} pieces",
                            precise_hits.len()
                        );

                        if !precise_hits.is_empty() {
                            println!("   Pieces with cursor inside shape:");
                            for (i, (piece_id, distance)) in precise_hits.iter().take(3).enumerate()
                            {
                                println!(
                                    "   {}. {} (distance: {:.1}px)",
                                    i + 1,
                                    piece_id,
                                    distance
                                );
                            }
                        } else if !large_area_pieces.is_empty() {
                            println!("   Top 5 closest pieces (no precise hits):");
                            for (i, (piece_id, distance)) in
                                large_area_pieces.iter().take(5).enumerate()
                            {
                                println!(
                                    "   {}. {} (distance: {:.1}px)",
                                    i + 1,
                                    piece_id,
                                    distance
                                );
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
pub fn register_pieces_from_data_store_to_collision_system(
    mut collision_system: ResMut<PieceCollisionSystem>,
    piece_data_store: Res<PieceDataStore>,
    perf_monitor: Res<PerformanceMonitor>,
) {
    // 既存データをクリア
    collision_system.pieces.clear();
    collision_system.need_rebuild = true;

    let mut registered_count = 0;

    println!("🔄 Registering pieces from PieceDataStore to collision system...");

    // PieceDataStoreから全ピースを登録
    for (piece_id, piece_data) in &piece_data_store.pieces {
        if piece_data.state.placed {
            continue;
        }
        if let Some(transform) = piece_data_store.transforms.get(piece_id) {
            // 頂点データを Vec2 に変換
            let vertices: Vec<Vec2> = piece_data
                .render
                .shape
                .vertices
                .iter()
                .map(|&[x, y]| Vec2::new(x, y))
                .collect();

            // Bounds were computed from the same vertices during generation.
            let position = transform.translation.truncate();
            let bounding_box = Rect::new(
                position.x + piece_data.render.bounds.min.x,
                position.y + piece_data.render.bounds.min.y,
                position.x + piece_data.render.bounds.max.x,
                position.y + piece_data.render.bounds.max.y,
            );

            // デバッグ: 最初の数個のピースのみログ出力（verticesを使う前に）
            if registered_count < 3
                && matches!(
                    perf_monitor.debug_level,
                    PerformanceDebugLevel::Medium | PerformanceDebugLevel::High
                )
            {
                println!(
                    "📝 Registering piece {} from data store (pos: {:?}, bounds: {:?})",
                    piece_id, position, bounding_box
                );

                if !vertices.is_empty() {
                    let vertex_range = (
                        vertices
                            .iter()
                            .fold(Vec2::splat(f32::INFINITY), |acc, &v| acc.min(v)),
                        vertices
                            .iter()
                            .fold(Vec2::splat(f32::NEG_INFINITY), |acc, &v| acc.max(v)),
                    );
                    println!(
                        "   📏 {} vertices, range: ({:.1}, {:.1}) to ({:.1}, {:.1})",
                        vertices.len(),
                        vertex_range.0.x,
                        vertex_range.0.y,
                        vertex_range.1.x,
                        vertex_range.1.y
                    );
                }
            }

            let collision_data = PieceCollisionData {
                piece_id: *piece_id,
                position,
                bounding_box,
                vertices,
                indices: piece_data.render.shape.indices.clone(),
            };

            collision_system.add_piece(collision_data);
            registered_count += 1;
        } else {
            println!("⚠️ No transform found for piece {}", piece_id);
        }
    }

    println!(
        "✅ Data store collision registration completed: {} pieces registered",
        registered_count
    );

    // R-Tree を再構築
    collision_system.rebuild_rtree();
    println!(
        "✅ Collision R-Tree rebuilt with {} pieces",
        collision_system.pieces.len()
    );
}
