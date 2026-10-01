use bevy::prelude::*;
use bevy::render::mesh::Indices;
use bevy::render::render_asset::RenderAssetUsages;
use bevy::render::render_resource::PrimitiveTopology;

use crate::components::*;
use crate::resources::*;

// ==========================================
// Mesh Combination Functions
// ==========================================

/// 複数のメッシュを1つに結合する
pub fn combine_meshes(
    meshes_with_transforms: Vec<(Mesh, Transform)>,
    debug_level: &PerformanceDebugLevel,
) -> Result<Mesh, String> {
    if meshes_with_transforms.is_empty() {
        return Err("No meshes to combine".to_string());
    }

    let mut combined_vertices: Vec<[f32; 3]> = Vec::new();
    let mut combined_uvs: Vec<[f32; 2]> = Vec::new();
    let mut combined_indices: Vec<u32> = Vec::new();
    let mut vertex_offset = 0u32;
    let mesh_count = meshes_with_transforms.len();

    for (mesh, transform) in meshes_with_transforms {
        // 頂点位置を取得
        let positions = match mesh.attribute(Mesh::ATTRIBUTE_POSITION) {
            Some(bevy::render::mesh::VertexAttributeValues::Float32x3(positions)) => positions,
            _ => return Err("Mesh missing position attribute".to_string()),
        };

        // UV座標を取得
        let uvs = match mesh.attribute(Mesh::ATTRIBUTE_UV_0) {
            Some(bevy::render::mesh::VertexAttributeValues::Float32x2(uvs)) => uvs,
            _ => return Err("Mesh missing UV attribute".to_string()),
        };

        // インデックスを取得
        let indices = match mesh.indices() {
            Some(Indices::U32(indices)) => indices.clone(),
            Some(Indices::U16(indices)) => indices.iter().map(|&i| i as u32).collect::<Vec<_>>(),
            None => return Err("Mesh missing indices".to_string()),
        };

        // デバッグ: 最初の数個のメッシュでインデックス数を確認
        if (combined_vertices.len() / 3) < 3 {
            println!(
                "🔍 Combine mesh #{}: {} vertices, {} indices",
                combined_vertices.len() / 3,
                positions.len(),
                indices.len()
            );
        }

        // 頂点を変換行列で変換してから結合
        let transform_matrix = transform.compute_matrix();
        for &position in positions {
            let world_pos = transform_matrix.transform_point3(Vec3::from(position));
            combined_vertices.push([world_pos.x, world_pos.y, world_pos.z]);
        }

        // UV座標をそのまま追加
        combined_uvs.extend_from_slice(uvs);

        // インデックスを頂点オフセットを加えて追加
        for &index in &indices {
            combined_indices.push(index + vertex_offset);
        }

        vertex_offset += positions.len() as u32;
    }

    println!(
        "🔄 Combined {} meshes into single mesh: {} vertices, {} indices",
        mesh_count,
        combined_vertices.len(),
        combined_indices.len()
    );

    // 結合されたメッシュを作成
    let mut combined_mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );

    combined_mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, combined_vertices);
    combined_mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, combined_uvs);
    combined_mesh.insert_indices(Indices::U32(combined_indices));

    Ok(combined_mesh)
}

// ==========================================
// Batch Management Systems
// ==========================================

/// バッチ再構築システム（イベント駆動）
pub fn handle_batch_rebuild_requests(
    mut commands: Commands,
    mut batch_manager: ResMut<BatchManager>,
    mut rebuild_events: EventReader<BatchRebuildRequest>,
    mut completed_events: EventWriter<BatchRebuildCompleted>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
    piece_query: Query<(Entity, &PuzzlePiece, &PieceShape, &Transform), Without<BatchedMeshEntity>>,
    batched_query: Query<Entity, With<BatchedMeshEntity>>,
    puzzle_image: Option<Res<PuzzleImage>>,
    id_manager: Res<PieceIdManager>,
    perf_monitor: ResMut<PerformanceMonitor>,
    piece_data_store: Res<PieceDataStore>,
) {
    // バッチ再構築要求を処理
    for rebuild_request in rebuild_events.read() {
        if batch_manager.is_rebuilding {
            if matches!(
                perf_monitor.debug_level,
                PerformanceDebugLevel::Medium | PerformanceDebugLevel::High
            ) {
                println!("⚠️ Batch rebuild already in progress, skipping request");
            }
            continue;
        }

        if matches!(
            perf_monitor.debug_level,
            PerformanceDebugLevel::Medium | PerformanceDebugLevel::High
        ) {
            println!(
                "🔄 Processing batch rebuild request: {:?} (affected pieces: {})",
                rebuild_request.reason,
                rebuild_request.affected_pieces.len()
            );
        }

        let start_time = std::time::Instant::now();
        batch_manager.is_rebuilding = true;

        // 既存のバッチエンティティを削除
        for entity in batched_query.iter() {
            commands.entity(entity).despawn();
        }
        batch_manager.batched_entity = None;

        // 🚀 NEW: PieceDataStoreからバッチに含めるピースを収集
        let mut meshes_to_combine = Vec::new();

        // バッチに含まれるピースIDを取得 - BatchManagerから取得するように変更
        let batched_piece_ids: Vec<PieceId> =
            batch_manager.batched_pieces.iter().cloned().collect();

        println!(
            "📊 Batch rebuild: BatchManager has {} pieces to batch",
            batched_piece_ids.len()
        );
        println!(
            "📊 PieceDataStore state: {} total pieces, {} in batch",
            piece_data_store.total_pieces, piece_data_store.pieces_in_batch
        );

        for piece_id in batched_piece_ids {
            if let (Some(piece_data), Some(transform)) = (
                piece_data_store.pieces.get(&piece_id),
                piece_data_store.transforms.get(&piece_id),
            ) {
                // このピースのメッシュを作成
                let mut piece_mesh = Mesh::new(
                    PrimitiveTopology::TriangleList,
                    RenderAssetUsages::RENDER_WORLD,
                );

                // 頂点を3D座標に変換
                let vertices_3d: Vec<[f32; 3]> = piece_data
                    .shape
                    .vertices
                    .iter()
                    .map(|&[x, y]| [x, y, 0.0])
                    .collect();

                let vertex_count = vertices_3d.len();
                piece_mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, vertices_3d);

                // UV座標を設定
                let texture_width = piece_data.texture_coords.z - piece_data.texture_coords.x;
                let texture_height = piece_data.texture_coords.w - piece_data.texture_coords.y;

                let uvs: Vec<[f32; 2]> = piece_data
                    .shape
                    .vertices
                    .iter()
                    .map(|&[x, y]| {
                        let u = piece_data.texture_coords.x
                            + (x - piece_data.bounds.min.x) / piece_data.bounds.width()
                                * texture_width;
                        // 🔧 FIXED: V座標を反転（テクスチャ座標系とワールド座標系の向きが異なるため）
                        let v_normalized =
                            (y - piece_data.bounds.min.y) / piece_data.bounds.height();
                        let v = piece_data.texture_coords.y + (1.0 - v_normalized) * texture_height;

                        // UV座標を[0,1]範囲にクランプ
                        [u.clamp(0.0, 1.0), v.clamp(0.0, 1.0)]
                    })
                    .collect();

                // 🔍 DEBUG: UV座標の妥当性をチェック（最初の数個のピースのみ）
                if meshes_to_combine.len() < 3 {
                    let (min_u, max_u) = uvs.iter().fold(
                        (f32::INFINITY, f32::NEG_INFINITY),
                        |(min_u, max_u), &[u, _]| (min_u.min(u), max_u.max(u)),
                    );
                    let (min_v, max_v) = uvs.iter().fold(
                        (f32::INFINITY, f32::NEG_INFINITY),
                        |(min_v, max_v), &[_, v]| (min_v.min(v), max_v.max(v)),
                    );

                    println!("🎨 Piece {} UV mapping:", piece_id);
                    println!(
                        "   Texture region: ({:.3}, {:.3}) to ({:.3}, {:.3}) [{}x{}]",
                        piece_data.texture_coords.x,
                        piece_data.texture_coords.y,
                        piece_data.texture_coords.z,
                        piece_data.texture_coords.w,
                        texture_width,
                        texture_height
                    );
                    println!(
                        "   UV range: U({:.3}..{:.3}) V({:.3}..{:.3})",
                        min_u, max_u, min_v, max_v
                    );
                    println!(
                        "   Bounds: ({:.1}, {:.1}) to ({:.1}, {:.1}) [{}x{}]",
                        piece_data.bounds.min.x,
                        piece_data.bounds.min.y,
                        piece_data.bounds.max.x,
                        piece_data.bounds.max.y,
                        piece_data.bounds.width(),
                        piece_data.bounds.height()
                    );
                }

                piece_mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
                piece_mesh.insert_indices(Indices::U32(piece_data.shape.indices.clone()));

                // デバッグ: 最初の数個のピースでインデックス数を確認
                if meshes_to_combine.len() < 3 {
                    println!(
                        "🔍 Piece {} mesh: {} vertices, {} indices",
                        piece_id,
                        vertex_count,
                        piece_data.shape.indices.len()
                    );
                }

                meshes_to_combine.push((piece_mesh, *transform));
            } else {
                println!("⚠️ Failed to get data for piece {}", piece_id);
            }
        }

        let mesh_count = meshes_to_combine.len();
        println!("📊 Prepared {} meshes for combination", mesh_count);

        // メッシュを結合
        if !meshes_to_combine.is_empty() {
            match combine_meshes(meshes_to_combine, &perf_monitor.debug_level) {
                Ok(combined_mesh) => {
                    let mesh_handle = meshes.add(combined_mesh);

                    // 結合されたマテリアルを作成
                    let material = if let Some(ref puzzle_img) = puzzle_image {
                        ColorMaterial {
                            texture: Some(puzzle_img.handle.clone()),
                            ..default()
                        }
                    } else {
                        ColorMaterial::default()
                    };
                    let material_handle = materials.add(material);

                    // バッチエンティティを生成
                    let batched_entity = commands
                        .spawn((
                            Mesh2d(mesh_handle),
                            MeshMaterial2d(material_handle),
                            Transform::from_xyz(0.0, 0.0, 0.0), // デフォルトレイヤー（z=0）に配置
                            BatchedMeshEntity {
                                piece_count: mesh_count, // 実際に結合したメッシュ数を使用
                                last_updated: std::time::Instant::now(),
                            },
                        ))
                        .id();

                    batch_manager.batched_entity = Some(batched_entity);
                    batch_manager.is_rebuilding = false;

                    let rebuild_time = start_time.elapsed();
                    batch_manager.record_rebuild(rebuild_time);

                    // 完了イベントを送信
                    completed_events.write(BatchRebuildCompleted {
                        batched_entity,
                        piece_count: batch_manager.batched_pieces.len(),
                        rebuild_time,
                    });

                    println!(
                        "✅ Batch rebuilt successfully: {} pieces combined into single mesh (entity: {:?}) in {:.2}ms",
                        mesh_count,
                        batched_entity,
                        rebuild_time.as_secs_f64() * 1000.0
                    );
                }
                Err(e) => {
                    println!("❌ Failed to combine meshes: {}", e);
                    batch_manager.is_rebuilding = false;
                }
            }
        } else {
            if matches!(
                perf_monitor.debug_level,
                PerformanceDebugLevel::Medium | PerformanceDebugLevel::High
            ) {
                println!("📝 No pieces to batch, skipping rebuild");
            }
            batch_manager.is_rebuilding = false;
        }
    }
}

/// 定期的なバッチ状態チェックシステム
pub fn monitor_batch_system(
    batch_manager: Res<BatchManager>,
    perf_monitor: Res<PerformanceMonitor>,
) {
    // F11キーでバッチ統計を表示（他のデバッグ機能と同様）
    if matches!(perf_monitor.debug_level, PerformanceDebugLevel::High) {
        static mut LAST_LOG_TIME: Option<std::time::Instant> = None;
        unsafe {
            let now = std::time::Instant::now();
            if LAST_LOG_TIME.map_or(true, |last| now.duration_since(last).as_secs() >= 10) {
                println!("📊 Batch System Status:\n{}", batch_manager.get_stats());
                LAST_LOG_TIME = Some(now);
            }
        }
    }
}

/// 自動バッチ再構築システム（必要時のみ実行）
pub fn auto_batch_rebuild(
    batch_manager: Res<BatchManager>,
    mut rebuild_events: EventWriter<BatchRebuildRequest>,
) {
    if batch_manager.needs_rebuild && !batch_manager.is_rebuilding {
        rebuild_events.write(BatchRebuildRequest {
            reason: BatchRebuildReason::ManualRebuild,
            affected_pieces: vec![],
        });
    }
}

// ==========================================
// Temporary Entity Management
// ==========================================

/// アクティブピース用の一時エンティティ作成システム
pub fn create_temporary_entities(
    mut commands: Commands,
    piece_data_store: Res<PieceDataStore>,
    batch_manager: Res<BatchManager>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
    puzzle_image: Option<Res<PuzzleImage>>,
    temp_entity_query: Query<Entity, With<TemporaryPieceEntity>>,
    perf_monitor: Res<PerformanceMonitor>,
) {
    // 既存の一時エンティティIDを収集
    let existing_temp_entities: std::collections::HashSet<_> = temp_entity_query.iter().collect();

    // 抽出されたピース（選択中/ドラッグ中）用の一時エンティティを作成
    for piece_id in &batch_manager.extracted_pieces {
        // 既に一時エンティティが存在するかチェック
        let already_exists = piece_data_store.temporary_entities.get(piece_id).is_some();
        if already_exists {
            continue;
        }

        // ピースデータを取得
        if let (Some(piece_data), Some(transform)) = (
            piece_data_store.pieces.get(piece_id),
            piece_data_store.transforms.get(piece_id),
        ) {
            // メッシュを作成
            let mut piece_mesh = Mesh::new(
                bevy::render::render_resource::PrimitiveTopology::TriangleList,
                bevy::render::render_asset::RenderAssetUsages::RENDER_WORLD,
            );

            // 頂点を3D座標に変換
            let vertices_3d: Vec<[f32; 3]> = piece_data
                .shape
                .vertices
                .iter()
                .map(|&[x, y]| [x, y, 0.0])
                .collect();

            piece_mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, vertices_3d);

            // UV座標を設定
            let texture_width = piece_data.texture_coords.z - piece_data.texture_coords.x;
            let texture_height = piece_data.texture_coords.w - piece_data.texture_coords.y;

            let uvs: Vec<[f32; 2]> = piece_data
                .shape
                .vertices
                .iter()
                .map(|&[x, y]| {
                    let u = piece_data.texture_coords.x
                        + (x - piece_data.bounds.min.x) / piece_data.bounds.width() * texture_width;
                    // 🔧 FIXED: V座標を反転（テクスチャ座標系とワールド座標系の向きが異なるため）
                    let v_normalized = (y - piece_data.bounds.min.y) / piece_data.bounds.height();
                    let v = piece_data.texture_coords.y + (1.0 - v_normalized) * texture_height;

                    // UV座標を[0,1]範囲にクランプ
                    [u.clamp(0.0, 1.0), v.clamp(0.0, 1.0)]
                })
                .collect();

            // 🔍 DEBUG: 一時エンティティのUV座標も検証（デバッグ用）
            if matches!(perf_monitor.debug_level, PerformanceDebugLevel::High) {
                let (min_u, max_u) = uvs.iter().fold(
                    (f32::INFINITY, f32::NEG_INFINITY),
                    |(min_u, max_u), &[u, _]| (min_u.min(u), max_u.max(u)),
                );
                let (min_v, max_v) = uvs.iter().fold(
                    (f32::INFINITY, f32::NEG_INFINITY),
                    |(min_v, max_v), &[_, v]| (min_v.min(v), max_v.max(v)),
                );

                println!(
                    "🎨 Temporary entity {} UV range: U({:.3}..{:.3}) V({:.3}..{:.3})",
                    piece_id, min_u, max_u, min_v, max_v
                );
            }

            piece_mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
            piece_mesh.insert_indices(bevy::render::mesh::Indices::U32(
                piece_data.shape.indices.clone(),
            ));

            let mesh_handle = meshes.add(piece_mesh);

            // マテリアルを作成
            let material = if let Some(ref puzzle_img) = puzzle_image {
                ColorMaterial {
                    texture: Some(puzzle_img.handle.clone()),
                    ..default()
                }
            } else {
                ColorMaterial::default()
            };
            let material_handle = materials.add(material);

            // 一時エンティティを生成
            let temp_entity = commands
                .spawn((
                    Mesh2d(mesh_handle),
                    MeshMaterial2d(material_handle),
                    *transform,
                    TemporaryPieceEntity {
                        piece_id: *piece_id,
                    },
                    // インタラクション用のコンポーネント
                    PickablePiece {
                        drag_offset: Vec2::ZERO,
                    },
                    PuzzlePiece {
                        id: piece_data.id,
                        original_position: piece_data.original_position,
                        current_position: piece_data.current_position,
                        correct_position: piece_data.correct_position,
                        texture_coords: piece_data.texture_coords,
                        is_placed: piece_data.is_placed,
                        grid_x: piece_data.grid_x,
                        grid_y: piece_data.grid_y,
                        bounds: piece_data.bounds,
                    },
                    PieceShape {
                        vertices: piece_data.shape.vertices.clone(),
                        indices: piece_data.shape.indices.clone(),
                        shape_hash: piece_data.shape.shape_hash.clone(),
                    },
                ))
                .id();

            if matches!(
                perf_monitor.debug_level,
                PerformanceDebugLevel::Medium | PerformanceDebugLevel::High
            ) {
                println!(
                    "🎯 Created temporary entity {:?} for piece {}",
                    temp_entity, piece_id
                );
            }
        }
    }
}

/// 一時エンティティの削除システム
pub fn cleanup_temporary_entities(
    mut commands: Commands,
    piece_data_store: Res<PieceDataStore>,
    batch_manager: Res<BatchManager>,
    temp_entity_query: Query<(Entity, &TemporaryPieceEntity)>,
    perf_monitor: Res<PerformanceMonitor>,
) {
    // バッチに戻されたピース、または配置完了したピースの一時エンティティを削除
    for (entity, temp_piece) in temp_entity_query.iter() {
        let should_remove = !batch_manager
            .extracted_pieces
            .contains(&temp_piece.piece_id);

        if should_remove {
            commands.entity(entity).despawn();

            if matches!(
                perf_monitor.debug_level,
                PerformanceDebugLevel::Medium | PerformanceDebugLevel::High
            ) {
                println!(
                    "🧹 Removed temporary entity {:?} for piece {}",
                    entity, temp_piece.piece_id
                );
            }
        }
    }
}

/// 一時エンティティとデータストア間の同期システム
pub fn sync_temporary_entities_with_data_store(
    mut piece_data_store: ResMut<PieceDataStore>,
    temp_entity_query: Query<
        (Entity, &Transform, &PuzzlePiece, &TemporaryPieceEntity),
        Changed<Transform>,
    >,
    perf_monitor: Res<PerformanceMonitor>,
) {
    let mut updated_count = 0;

    // 一時エンティティのTransform変更をデータストアに反映
    for (entity, transform, piece, temp_piece) in temp_entity_query.iter() {
        // データストアの位置情報を更新
        if let Some(stored_transform) = piece_data_store.transforms.get_mut(&temp_piece.piece_id) {
            *stored_transform = *transform;
            updated_count += 1;
        }

        // ピースデータも更新
        if let Some(stored_piece) = piece_data_store.pieces.get_mut(&temp_piece.piece_id) {
            stored_piece.current_position = transform.translation.truncate();
            stored_piece.is_placed = piece.is_placed;
        }

        // 一時エンティティのマッピングを更新
        piece_data_store
            .temporary_entities
            .insert(temp_piece.piece_id, entity);
    }

    if updated_count > 0 && matches!(perf_monitor.debug_level, PerformanceDebugLevel::High) {
        println!(
            "🔄 Synced {} temporary entity transforms to data store",
            updated_count
        );
    }
}
