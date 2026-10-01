use super::*;
use bevy_math::UVec2;
use puzzella_core::GENERATOR_VERSION;

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
fn generation_reproduces_ids_shapes_uvs_bounds_and_positions() {
    let def = definition(42);
    let a = generate_pieces(&def).unwrap();
    let b = generate_pieces(&def).unwrap();
    assert_eq!(a.pieces.len(), def.piece_count());
    assert_eq!(a.pieces, b.pieces);
    for (index, piece) in a.pieces.iter().enumerate() {
        assert_eq!(piece.piece_component.id, PieceId(index as u32));
    }
    let c = generate_pieces(&definition(43)).unwrap();
    assert_ne!(a.pieces[0].state.position, c.pieces[0].state.position);
    assert!(a
        .pieces
        .iter()
        .zip(&c.pieces)
        .any(|(a, b)| a.geometry.fill != b.geometry.fill));
}

#[test]
fn worker_counts_and_arbitrary_generation_order_are_identical() {
    let mut def = definition(u64::MAX);
    def.grid_size = UVec2::new(10, 10);
    let baseline = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap()
        .install(|| generate_pieces(&def).unwrap());
    for count in [2, 4] {
        let result = rayon::ThreadPoolBuilder::new()
            .num_threads(count)
            .build()
            .unwrap()
            .install(|| generate_pieces(&def).unwrap());
        assert_eq!(baseline.pieces, result.pieces);
    }
    let mut worker = TessellationWorker::default();
    // Coprime stride visits every ID once, in a different order.
    for index in (0..100).map(|i| (i * 37 + 13) % 100) {
        let expected = &baseline.pieces[index];
        let piece = worker
            .generate_piece(&def, PieceId(index as u32), expected.state.position)
            .unwrap();
        assert_eq!(&piece, expected);
    }
}

#[test]
fn meshes_uvs_and_bounds_are_valid_including_small_and_nonsquare_pieces() {
    for image in [
        UVec2::new(800, 600),
        UVec2::new(13, 7),
        UVec2::new(2048, 20),
        UVec2::new(1, 1),
    ] {
        for seed in [0, 42, 43, u64::MAX] {
            let mut def = definition(seed);
            def.image_size = image;
            let size = image.as_vec2() / def.grid_size.as_vec2();
            let pieces = generate_pieces(&def).unwrap().pieces;
            for data in pieces {
                let geometry = &data.geometry;
                assert!(geometry.bounds.min.is_finite() && geometry.bounds.max.is_finite());
                for mesh in [&geometry.fill, &geometry.stroke] {
                    assert!(!mesh.positions.is_empty());
                    assert_eq!(mesh.indices.len() % 3, 0);
                    assert!(mesh.positions.iter().flatten().all(|v| v.is_finite()));
                    assert!(mesh
                        .indices
                        .iter()
                        .all(|&i| usize::from(i) < mesh.positions.len()));
                    for triangle in mesh.indices.chunks_exact(3) {
                        let [a, b, c] = [triangle[0], triangle[1], triangle[2]].map(|i| {
                            let p = mesh.positions[usize::from(i)];
                            Vec2::new(p[0], p[1])
                        });
                        assert_ne!((b - a).perp_dot(c - a), 0.0);
                    }
                }
                assert_eq!(geometry.fill.positions.len(), geometry.fill.uvs.len());
                let center =
                    (data.piece_component.grid_position.as_vec2() + Vec2::splat(0.5)) * size;
                for (p, uv) in geometry.fill.positions.iter().zip(&geometry.fill.uvs) {
                    let p = Vec2::new(p[0], p[1]);
                    assert!(
                        p.cmpge(geometry.bounds.min).all() && p.cmple(geometry.bounds.max).all()
                    );
                    let expected = (center + Vec2::new(p.x, -p.y)) / image.as_vec2();
                    assert_eq!(*uv, expected.to_array());
                    assert!(uv.iter().all(|v| v.is_finite()));
                    assert!(uv.iter().all(|&v| (-0.000001..=1.000001).contains(&v)));
                }
                assert_eq!(geometry.shape.vertices.len(), geometry.fill.positions.len());
                for (&index, &debug_index) in
                    geometry.fill.indices.iter().zip(&geometry.shape.indices)
                {
                    assert_eq!(u32::from(index), debug_index);
                }
                let mesh = data.geometry.fill.into_mesh();
                assert!(matches!(mesh.indices(), Some(Indices::U16(_))));
            }
        }
    }
}

#[test]
fn complementary_meshes_cover_the_image_without_area_loss() {
    for seed in 0..16 {
        let def = definition(seed);
        let pieces = generate_pieces(&def).unwrap().pieces;
        let mut area = 0.0_f64;
        for piece in pieces {
            let mesh = piece.geometry.fill;
            for triangle in mesh.indices.chunks_exact(3) {
                let [a, b, c] = [triangle[0], triangle[1], triangle[2]].map(|i| {
                    let p = mesh.positions[usize::from(i)];
                    Vec2::new(p[0], p[1])
                });
                let triangle_area = (b - a).perp_dot(c - a).abs() * 0.5;
                assert!(
                    triangle_area > 0.0,
                    "degenerate triangle at seed {seed}: {a} {b} {c}"
                );
                area += f64::from(triangle_area);
            }
        }
        let expected = f64::from(def.image_size.x) * f64::from(def.image_size.y);
        assert!(
            (area - expected).abs() / expected < 0.0001,
            "{area} != {expected}"
        );
    }
}

#[test]
fn unsupported_version_and_invalid_ids_are_rejected() {
    let mut def = definition(42);
    def.generator_version = 1;
    assert!(matches!(
        generate_pieces(&def),
        Err(GenerationError::InvalidDefinition(_))
    ));
    def.generator_version = GENERATOR_VERSION;
    assert!(matches!(
        TessellationWorker::default().generate_piece(&def, PieceId(12), Vec2::ZERO),
        Err(GenerationError::InvalidPiece(_))
    ));
}
