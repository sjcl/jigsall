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
    for entity in batch.batched_entities.drain(..) {
        commands.entity(entity).despawn();
    }
    // A transparent combined mesh is sorted as one draw. Split it wherever an
    // extracted piece occurs, so extraction/return cannot change stacking.
    let mut piece_ids: Vec<_> = store.pieces.keys().copied().collect();
    piece_ids.sort_by(|a, b| {
        store.transforms[a]
            .translation
            .z
            .total_cmp(&store.transforms[b].translation.z)
            .then(a.cmp(b))
    });
    let now = std::time::Instant::now();
    let mut inputs = Vec::new();
    let mut material = None;
    for id in piece_ids {
        if !batch.batched_pieces.contains(&id) {
            if let Some(material) = material.take() {
                append_piece_batch(
                    &mut commands,
                    &mut batch,
                    &mut meshes,
                    std::mem::take(&mut inputs),
                    material,
                    &perf.debug_level,
                );
            }
            continue;
        }
        if let (Some(piece), Some(transform)) = (store.pieces.get(&id), store.transforms.get(&id)) {
            if let Some(mesh) = meshes.get(&piece.render.mesh) {
                inputs.push((mesh.clone(), *transform));
                material = Some(piece.render.material.clone());
            }
        }
    }
    if let Some(material) = material {
        append_piece_batch(
            &mut commands,
            &mut batch,
            &mut meshes,
            inputs,
            material,
            &perf.debug_level,
        );
    }
    batch.record_rebuild(now.elapsed());
    perf.end_system_timing("handle_batch_rebuild_requests", start);
}

fn append_piece_batch(
    commands: &mut Commands,
    batch: &mut BatchManager,
    meshes: &mut Assets<Mesh>,
    mut inputs: Vec<(Mesh, Transform)>,
    material: Handle<ColorMaterial>,
    debug_level: &PerformanceDebugLevel,
) {
    let Some((_, first)) = inputs.first() else {
        return;
    };
    let layer = first.translation.z;
    let count = inputs.len();
    // Keep world-space vertex positions while giving the batch a sortable
    // entity depth between its neighboring extracted pieces.
    for (_, transform) in &mut inputs {
        transform.translation.z -= layer;
    }
    match combine_meshes(inputs, debug_level) {
        Ok(mesh) => {
            batch.batched_entities.push(
                commands
                    .spawn((
                        Mesh2d(meshes.add(mesh)),
                        MeshMaterial2d(material),
                        Transform::from_xyz(0.0, 0.0, layer),
                        BatchedMeshEntity {
                            piece_count: count,
                            last_updated: std::time::Instant::now(),
                        },
                    ))
                    .id(),
            );
        }
        Err(error) => error!("Batch rebuild failed: {error}"),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gameplay::*;

    #[test]
    fn extraction_splits_depth_ranges_and_return_preserves_order_uvs_and_alpha() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<PieceDataStore>()
            .init_resource::<PieceIdManager>()
            .init_resource::<BatchManager>()
            .init_resource::<PerformanceMonitor>()
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<ColorMaterial>>()
            .add_message::<PiecePlacedEvent>()
            .add_message::<BatchRebuildRequest>()
            .add_systems(
                Update,
                (
                    reconcile_piece_rendering,
                    cleanup_temporary_entities,
                    create_temporary_entities,
                    handle_batch_rebuild_requests,
                )
                    .chain(),
            );
        let mesh = app
            .world_mut()
            .resource_mut::<Assets<Mesh>>()
            .add(Rectangle::new(10.0, 10.0));
        let material = app
            .world_mut()
            .resource_mut::<Assets<ColorMaterial>>()
            .add(ColorMaterial::default());
        for (index, z) in [4.0, 0.0, 2.0, 1.0, 3.0].into_iter().enumerate() {
            let id = PieceId(index as u32);
            let position = Vec2::new(index as f32 * 100.0, 0.0);
            let mut store = app.world_mut().resource_mut::<PieceDataStore>();
            store.add_piece(StoredPieceData {
                definition: PuzzlePiece {
                    id,
                    grid_position: UVec2::ZERO,
                    correct_position: Vec2::ZERO,
                    initial_position: position,
                },
                state: PieceState::new(position),
                render: PieceRenderData {
                    bounds: Rect::new(-5.0, -5.0, 5.0, 5.0),
                    shape: PieceShapeData {
                        vertices: vec![],
                        indices: vec![],
                        shape_hash: String::new(),
                    },
                    mesh: mesh.clone(),
                    material: material.clone(),
                },
            });
            store.transforms.get_mut(&id).unwrap().translation.z = z;
            app.world_mut().resource_mut::<BatchManager>().add_piece(id);
        }
        app.world_mut()
            .resource_mut::<PieceDataStore>()
            .selected_pieces
            .insert(PieceId(2));
        app.update();
        let batch = app.world().resource::<BatchManager>();
        assert_eq!(batch.batched_entities.len(), 2);
        assert_eq!(batch.extracted_pieces.len(), 1);
        assert_eq!(batch.total_pieces(), 5);
        let store = app.world().resource::<PieceDataStore>();
        let extracted = store.temporary_entities[&PieceId(2)];
        assert_eq!(
            app.world()
                .get::<Transform>(extracted)
                .unwrap()
                .translation
                .z,
            2.0
        );
        let layers: Vec<_> = batch
            .batched_entities
            .iter()
            .map(|&entity| {
                let layer = app.world().get::<Transform>(entity).unwrap().translation.z;
                let count = app
                    .world()
                    .get::<BatchedMeshEntity>(entity)
                    .unwrap()
                    .piece_count;
                (layer, count)
            })
            .collect();
        assert_eq!(layers, vec![(0.0, 2), (3.0, 2)]);
        app.world_mut()
            .resource_mut::<PieceDataStore>()
            .selected_pieces
            .clear();
        app.update();
        let batch = app.world().resource::<BatchManager>();
        assert_eq!(batch.batched_entities.len(), 1);
        assert_eq!(batch.total_pieces(), 5);
        assert!(app
            .world()
            .resource::<PieceDataStore>()
            .temporary_entities
            .is_empty());
        let entity = batch.batched_entities[0];
        let meshes = app.world().resource::<Assets<Mesh>>();
        let combined = meshes
            .get(&app.world().get::<Mesh2d>(entity).unwrap().0)
            .unwrap();
        let Some(bevy::mesh::VertexAttributeValues::Float32x3(positions)) =
            combined.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!("missing positions");
        };
        let Some(bevy::mesh::VertexAttributeValues::Float32x2(uvs)) =
            combined.attribute(Mesh::ATTRIBUTE_UV_0)
        else {
            panic!("missing UVs");
        };
        let original = meshes.get(&mesh).unwrap();
        let Some(bevy::mesh::VertexAttributeValues::Float32x3(original_positions)) =
            original.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!("missing original positions");
        };
        let Some(bevy::mesh::VertexAttributeValues::Float32x2(original_uvs)) =
            original.attribute(Mesh::ATTRIBUTE_UV_0)
        else {
            panic!("missing original UVs");
        };
        let layer = app.world().get::<Transform>(entity).unwrap().translation.z;
        for (rank, &id) in [1, 3, 2, 4, 0].iter().enumerate() {
            let start = rank * original_positions.len();
            for (index, position) in original_positions.iter().enumerate() {
                assert_eq!(positions[start + index][0], position[0] + id as f32 * 100.0);
                assert_eq!(positions[start + index][2] + layer, rank as f32);
            }
            assert_eq!(
                &uvs[start..start + original_uvs.len()],
                original_uvs.as_slice()
            );
        }
        assert_eq!(
            app.world()
                .resource::<Assets<ColorMaterial>>()
                .get(&material)
                .unwrap()
                .alpha_mode,
            bevy::sprite_render::AlphaMode2d::Blend
        );
    }
}
