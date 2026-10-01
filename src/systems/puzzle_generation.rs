use crate::jigsaw_shapes::JigsawShapeGenerator;
use crate::puzzle::{extract_shape_data_from_jigsaw_shape, generate_placement_grid};
use crate::{components::*, gameplay::*, resources::*};
use bevy::prelude::*;

/// CPU shape work and tessellation remain on workers; asset creation stays here.
// CPU results, assets and presentation resources have separate ECS access.
#[allow(clippy::too_many_arguments)]
pub fn spawn_puzzle_pieces_progressive(
    definition: Option<Res<PuzzleDefinition>>,
    puzzle_image: Option<Res<PuzzleImage>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
    mut progress: ResMut<PieceGenerationProgress>,
    mut stroke_cache: ResMut<StrokeMeshCache>,
    mut perf: ResMut<PerformanceMonitor>,
    mut next: ResMut<NextState<GameSubState>>,
    mut batch: ResMut<BatchManager>,
    mut store: ResMut<PieceDataStore>,
) {
    let (Some(definition), Some(puzzle_image)) = (definition, puzzle_image) else {
        return;
    };
    let _span = info_span!("spawn_puzzle_pieces_progressive").entered();
    let start = perf.start_system_timing("spawn_puzzle_pieces_progressive");
    if progress.generation_phase == GenerationPhase::NotStarted {
        progress.is_generating = true;
        progress.total_pieces = definition.piece_count();
        progress.grid_size = (
            definition.grid_size.x as usize,
            definition.grid_size.y as usize,
        );
        progress.generation_phase = GenerationPhase::PreparingShapes;
        let definition = definition.clone();
        let (sender, receiver) = crossbeam::channel::bounded(1);
        std::thread::spawn(move || {
            let result = generate_shapes(&definition);
            let _ = sender.send(result);
        });
        progress.bg_thread_receiver = Some(receiver);
    }

    if let Some(receiver) = &progress.bg_thread_receiver {
        match receiver.try_recv() {
            Ok(Ok(result)) => {
                progress.shapes_generated = result.total_pieces;
                progress.generation_phase = GenerationPhase::CreatingPieces;
                let definition = definition.clone();
                let (sender, receiver) = crossbeam::channel::bounded(1);
                std::thread::spawn(move || {
                    let result = create_all_pieces_sync(result, &definition);
                    let _ = sender.send(result);
                });
                progress.piece_thread_receiver = Some(receiver);
                progress.bg_thread_receiver = None;
            }
            Ok(Err(error)) => fail_generation(&mut progress, error),
            Err(crossbeam::channel::TryRecvError::Disconnected) => {
                fail_generation(&mut progress, "Shape worker stopped".into())
            }
            Err(crossbeam::channel::TryRecvError::Empty) => {}
        }
    }
    if let Some(receiver) = &progress.piece_thread_receiver {
        match receiver.try_recv() {
            Ok(Ok(result)) => {
                progress.pending_pieces = result.pieces.into();
                progress.generation_phase = GenerationPhase::SpawningEntities;
                progress.piece_thread_receiver = None;
            }
            Ok(Err(error)) => fail_generation(&mut progress, error),
            Err(crossbeam::channel::TryRecvError::Disconnected) => {
                fail_generation(&mut progress, "Piece worker stopped".into())
            }
            Err(crossbeam::channel::TryRecvError::Empty) => {}
        }
    }
    progress.pieces_spawned_this_frame = 0;
    if progress.generation_phase == GenerationPhase::SpawningEntities {
        // Preserve incremental main-thread asset creation (ten pieces per frame).
        let material = store
            .pieces
            .values()
            .next()
            .map(|piece| piece.render.material.clone())
            .unwrap_or_else(|| {
                materials.add(ColorMaterial {
                    texture: Some(puzzle_image.handle.clone()),
                    ..default()
                })
            });
        for _ in 0..10 {
            let Some(data) = progress.pending_pieces.pop_front() else {
                break;
            };
            let mesh = meshes.add(data.mesh);
            if let Some(stroke) = data.stroke_mesh {
                stroke_cache
                    .stroke_meshes
                    .insert(data.piece_shape.shape_hash.clone(), meshes.add(stroke));
            }
            let id = data.piece_component.id;
            store.add_piece(StoredPieceData {
                definition: data.piece_component,
                state: data.state,
                render: PieceRenderData {
                    bounds: data.bounds,
                    shape: PieceShapeData {
                        vertices: data.piece_shape.vertices,
                        indices: data.piece_shape.indices,
                        shape_hash: data.piece_shape.shape_hash,
                    },
                    mesh,
                    material: material.clone(),
                },
            });
            batch.add_piece(id);
            progress.pieces_created += 1;
            progress.pieces_spawned_this_frame += 1;
        }
        if progress.pending_pieces.is_empty() {
            progress.is_generating = false;
            progress.generation_phase = GenerationPhase::Completed;
            next.set(GameSubState::Playing);
        }
    }
    // CPU mesh values never access a World or GPU resources on worker threads.
    perf.end_system_timing("spawn_puzzle_pieces_progressive", start);
}

fn fail_generation(progress: &mut PieceGenerationProgress, error: String) {
    error!("Puzzle generation failed: {error}");
    progress.is_generating = false;
    progress.generation_phase = GenerationPhase::Failed;
    progress.error = Some(error);
    progress.bg_thread_receiver = None;
    progress.piece_thread_receiver = None;
}

fn generate_shapes(definition: &PuzzleDefinition) -> Result<ShapeGenerationResult, String> {
    definition.validate().map_err(str::to_owned)?;
    let (width, height) = (
        definition.grid_size.x as usize,
        definition.grid_size.y as usize,
    );
    let size = definition.image_size.as_vec2();
    let piece_size = size / definition.grid_size.as_vec2();
    let mut generator = JigsawShapeGenerator::new(
        (piece_size.x, piece_size.y),
        (width, height),
        definition.seed,
    );
    generator
        .generate_jigsaw_template()
        .map_err(|e| e.to_string())?;
    generator.generate_all_shapes().map_err(|e| e.to_string())?;
    let positions = generate_placement_grid(
        width,
        height,
        piece_size.x,
        piece_size.y,
        size.x,
        size.y,
        definition.seed,
    );
    if positions.len() != definition.piece_count() {
        return Err("Could not generate all initial positions".into());
    }
    Ok(ShapeGenerationResult {
        shape_generator: generator,
        placement_positions: positions,
        total_pieces: definition.piece_count(),
    })
}

fn create_all_pieces_sync(
    result: ShapeGenerationResult,
    definition: &PuzzleDefinition,
) -> Result<PieceCreationResult, String> {
    let mut pieces = Vec::with_capacity(result.total_pieces);
    for (index, &position) in result.placement_positions.iter().enumerate() {
        let piece = definition.piece(index as u32, position);
        let shape = result
            .shape_generator
            .get_shape(
                piece.grid_position.x as usize,
                piece.grid_position.y as usize,
            )
            .ok_or_else(|| format!("Missing shape for piece {index}"))?;
        pieces.push(PieceData {
            bounds: shape.bounds,
            mesh: crate::jigsaw_shapes::clone_mesh_from_shape(shape),
            stroke_mesh: shape.stroke_mesh.clone(),
            piece_shape: extract_shape_data_from_jigsaw_shape(shape),
            state: PieceState::new(position),
            piece_component: piece,
        });
    }
    Ok(PieceCreationResult { pieces })
}

pub fn spawn_grid_reference(mut commands: Commands, image: Res<PuzzleImage>) {
    commands.spawn((
        Sprite {
            image: image.handle.clone(),
            custom_size: Some(image.size),
            color: Color::WHITE.with_alpha(0.3),
            ..default()
        },
        Transform::from_xyz(0.0, 0.0, -30.0),
        GridReference,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    fn definition(seed: u64) -> PuzzleDefinition {
        PuzzleDefinition {
            generator_version: GENERATOR_VERSION,
            seed,
            grid_size: UVec2::new(4, 3),
            image_size: UVec2::new(800, 600),
            snap_distance: 50.0,
        }
    }
    #[test]
    fn generation_reproduces_ids_shapes_uvs_and_positions() {
        let def = definition(42);
        let a = create_all_pieces_sync(generate_shapes(&def).unwrap(), &def).unwrap();
        let b = create_all_pieces_sync(generate_shapes(&def).unwrap(), &def).unwrap();
        assert_eq!(a.pieces.len(), def.piece_count());
        for (index, (a, b)) in a.pieces.iter().zip(&b.pieces).enumerate() {
            assert_eq!(a.piece_component.id, PieceId(index as u32));
            assert_eq!(a.piece_component, b.piece_component);
            assert_eq!(a.state, b.state);
            assert_eq!(a.piece_shape.vertices, b.piece_shape.vertices);
            assert_eq!(a.piece_shape.indices, b.piece_shape.indices);
            assert_eq!(
                a.mesh.attribute(Mesh::ATTRIBUTE_UV_0),
                b.mesh.attribute(Mesh::ATTRIBUTE_UV_0)
            );
        }
        let other = definition(43);
        let c = create_all_pieces_sync(generate_shapes(&other).unwrap(), &other).unwrap();
        assert_ne!(a.pieces[0].state.position, c.pieces[0].state.position);
        assert!(a
            .pieces
            .iter()
            .zip(&c.pieces)
            .any(|(a, c)| a.piece_shape.vertices != c.piece_shape.vertices));
    }

    #[test]
    fn every_generated_triangle_is_pickable_at_its_drawn_position() {
        let def = definition(42);
        let generated = create_all_pieces_sync(generate_shapes(&def).unwrap(), &def).unwrap();
        for data in generated.pieces {
            let id = data.piece_component.id;
            let position = data.state.position;
            let vertices: Vec<Vec2> = data
                .piece_shape
                .vertices
                .into_iter()
                .map(Vec2::from)
                .collect();
            let indices = data.piece_shape.indices;
            let mut collision = PieceCollisionSystem::default();
            collision.add_piece(PieceCollisionData {
                piece_id: id,
                position,
                z_order: 0.0,
                bounding_box: Rect {
                    min: data.bounds.min + position,
                    max: data.bounds.max + position,
                },
                vertices: vertices.clone(),
                indices: indices.clone(),
            });
            let mut tested = 0;
            for [a, b, c] in crate::piece_geometry::triangles(&vertices, &indices) {
                // Skip triangles too thin to survive world-coordinate rounding.
                if (b - a).perp_dot(c - a).abs() < 0.01 {
                    continue;
                }
                let centroid = position + (a + b + c) / 3.0;
                assert_eq!(
                    collision.find_piece_at_position(centroid),
                    Some(id),
                    "piece {id} at {centroid}"
                );
                tested += 1;
            }
            assert!(tested > 0);
        }
    }
}
