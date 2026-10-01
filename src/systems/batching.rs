use bevy::asset::RenderAssetUsages;
use bevy::mesh::Indices;
use bevy::prelude::*;
use bevy::render::render_resource::PrimitiveTopology;

use crate::components::*;
use crate::resources::*;

// ==========================================
// Mesh Combination Functions
// ==========================================

/// 複数のメッシュを1つに結合する
pub fn combine_meshes(
    meshes_with_transforms: Vec<(Mesh, Transform)>,
    _debug_level: &PerformanceDebugLevel,
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
            Some(bevy::mesh::VertexAttributeValues::Float32x3(positions)) => positions,
            _ => return Err("Mesh missing position attribute".to_string()),
        };

        // UV座標を取得
        let uvs = match mesh.attribute(Mesh::ATTRIBUTE_UV_0) {
            Some(bevy::mesh::VertexAttributeValues::Float32x2(uvs)) => uvs,
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
        let transform_matrix = transform.to_matrix();
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

/// Reconcile existing batch extraction with local selection and held states.
pub fn reconcile_piece_rendering(
    store: Res<PieceDataStore>,
    mut batch: ResMut<BatchManager>,
    mut placed: MessageReader<PiecePlacedEvent>,
) {
    let wanted: std::collections::HashSet<_> = store
        .selected_pieces
        .iter()
        .chain(&store.preview_pieces)
        .chain(&store.held_pieces)
        .copied()
        .collect();
    let extracted: Vec<_> = batch.extracted_pieces.iter().copied().collect();
    for id in extracted {
        if !wanted.contains(&id) {
            batch.return_piece(id);
        }
    }
    for id in wanted {
        batch.extract_piece(id);
    }
    for event in placed.read() {
        batch.place_piece(event.id);
    }
}

pub fn create_temporary_entities(
    mut commands: Commands,
    mut store: ResMut<PieceDataStore>,
    batch: Res<BatchManager>,
    mut ids: ResMut<PieceIdManager>,
) {
    for &id in &batch.extracted_pieces {
        if store.temporary_entities.contains_key(&id) {
            continue;
        }
        let Some(piece) = store.pieces.get(&id) else {
            continue;
        };
        let Some(&transform) = store.transforms.get(&id) else {
            continue;
        };
        let entity = commands
            .spawn((
                Mesh2d(piece.render.mesh.clone()),
                MeshMaterial2d(piece.render.material.clone()),
                transform,
                piece.definition.clone(),
                PieceShape {
                    vertices: piece.render.shape.vertices.clone(),
                    indices: piece.render.shape.indices.clone(),
                    shape_hash: piece.render.shape.shape_hash.clone(),
                },
                TemporaryPieceEntity { piece_id: id },
            ))
            .id();
        ids.register_piece(entity, id);
        store.temporary_entities.insert(id, entity);
    }
}

pub fn cleanup_temporary_entities(
    mut commands: Commands,
    mut store: ResMut<PieceDataStore>,
    batch: Res<BatchManager>,
    pieces: Query<(Entity, &TemporaryPieceEntity)>,
    mut ids: ResMut<PieceIdManager>,
) {
    for (entity, piece) in &pieces {
        if !batch.extracted_pieces.contains(&piece.piece_id) {
            commands.entity(entity).despawn();
            store.temporary_entities.remove(&piece.piece_id);
            ids.unregister_entity(entity);
        }
    }
}

/// Coalesce requests into one rebuild and reuse the original tessellated UVs.
pub fn handle_batch_rebuild_requests(
    mut commands: Commands,
    mut batch: ResMut<BatchManager>,
    mut requests: MessageReader<BatchRebuildRequest>,
    mut meshes: ResMut<Assets<Mesh>>,
    store: Res<PieceDataStore>,
    mut perf: ResMut<PerformanceMonitor>,
) {
    for request in requests.read() {
        trace!(reason = ?request.reason, affected = request.affected_pieces.len(), "Batch rebuild requested");
        batch.needs_rebuild = true;
    }
    if !batch.needs_rebuild {
        return;
    }
    let start = perf.start_system_timing("handle_batch_rebuild_requests");
    if let Some(entity) = batch.batched_entity.take() {
        commands.entity(entity).despawn();
    }
    let mut piece_ids: Vec<_> = batch.batched_pieces.iter().copied().collect();
    piece_ids.sort_unstable();
    let mut inputs = Vec::with_capacity(piece_ids.len());
    let mut material = None;
    for id in piece_ids {
        if let (Some(piece), Some(transform)) = (store.pieces.get(&id), store.transforms.get(&id)) {
            if let Some(mesh) = meshes.get(&piece.render.mesh) {
                inputs.push((mesh.clone(), *transform));
                material = Some(piece.render.material.clone());
            }
        }
    }
    let now = std::time::Instant::now();
    let count = inputs.len();
    if !inputs.is_empty() {
        match combine_meshes(inputs, &perf.debug_level) {
            Ok(mesh) => {
                if let Some(material) = material {
                    batch.batched_entity = Some(
                        commands
                            .spawn((
                                Mesh2d(meshes.add(mesh)),
                                MeshMaterial2d(material),
                                Transform::default(),
                                BatchedMeshEntity {
                                    piece_count: count,
                                    last_updated: now,
                                },
                            ))
                            .id(),
                    );
                }
            }
            Err(error) => error!("Batch rebuild failed: {error}"),
        }
    }
    batch.record_rebuild(now.elapsed());
    perf.end_system_timing("handle_batch_rebuild_requests", start);
}

pub fn monitor_batch_system(
    batch: Res<BatchManager>,
    input: Res<ButtonInput<KeyCode>>,
    query: Query<&BatchedMeshEntity>,
    mut rebuild: MessageWriter<BatchRebuildRequest>,
) {
    if input.just_pressed(KeyCode::F9) {
        rebuild.write(BatchRebuildRequest {
            reason: BatchRebuildReason::ManualRebuild,
            affected_pieces: vec![],
        });
    }
    if input.just_pressed(KeyCode::F10) {
        info!("{}", batch.get_stats());
        info!(total = batch.total_pieces(), "Piece count");
        for mesh in &query {
            info!(pieces = mesh.piece_count, elapsed = ?mesh.last_updated.elapsed(), "Batch mesh");
        }
    }
}
