use crate::{components::*, resources::*};
use bevy::prelude::*;
use puzzella_core::*;
use puzzella_puzzle::generate_pieces;

/// CPU shape work and tessellation remain on workers; asset creation stays here.
// CPU results, assets and presentation resources have separate ECS access.
#[allow(clippy::too_many_arguments)]
pub fn spawn_puzzle_pieces_progressive(
    mut commands: Commands,
    definition: Option<Res<PuzzleDefinition>>,
    puzzle_image: Option<Res<PuzzleImage>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
    mut progress: ResMut<PieceGenerationProgress>,
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
        progress.generation_phase = GenerationPhase::GeneratingPieces;
        let definition = definition.clone();
        let (sender, receiver) = crossbeam::channel::bounded(1);
        std::thread::spawn(move || {
            let result = generate_pieces(&definition).map_err(|error| error.to_string());
            let _ = sender.send(result);
        });
        progress.receiver = Some(receiver);
    }

    if let Some(receiver) = &progress.receiver {
        match receiver.try_recv() {
            Ok(Ok(result)) => {
                progress.pending_pieces = result.pieces.into();
                progress.generation_phase = GenerationPhase::SpawningEntities;
                progress.receiver = None;
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
            let id = data.piece_component.id;
            let mut mesh = data.geometry.fill.into_mesh();
            mesh.insert_attribute(
                crate::selection::ATTRIBUTE_PIECE_ID,
                vec![id.0; mesh.count_vertices()],
            );
            let mesh = meshes.add(mesh);
            commands.spawn(crate::selection::PuzzlePieceId(id));
            let stroke = meshes.add(data.geometry.stroke.into_mesh());
            store.add_piece(StoredPieceData {
                definition: data.piece_component,
                state: data.state,
                render: PieceRenderData {
                    bounds: data.geometry.bounds,
                    #[cfg(any(test, feature = "cpu-picking-debug"))]
                    shape: data.geometry.shape,
                    mesh,
                    stroke,
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
    progress.receiver = None;
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
    fn every_generated_triangle_is_pickable_at_its_drawn_position() {
        let def = definition(42);
        let generated = generate_pieces(&def).unwrap();
        for data in generated.pieces {
            let id = data.piece_component.id;
            let position = data.state.position;
            let vertices: Vec<Vec2> = data
                .geometry
                .shape
                .vertices
                .into_iter()
                .map(Vec2::from)
                .collect();
            let indices = data.geometry.shape.indices;
            let mut collision = PieceCollisionSystem::default();
            collision.add_piece(PieceCollisionData {
                piece_id: id,
                position,
                z_order: 0.0,
                bounding_box: Rect {
                    min: data.geometry.bounds.min + position,
                    max: data.geometry.bounds.max + position,
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
