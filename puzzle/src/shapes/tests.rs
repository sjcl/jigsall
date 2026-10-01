use super::*;

fn points(curve: CubicBezier) -> [Vec2; 4] {
    [curve.from, curve.control1, curve.control2, curve.to]
}
fn sample(curve: CubicBezier, t: f32) -> Vec2 {
    let s = 1.0 - t;
    curve.from * s * s * s
        + curve.control1 * 3.0 * s * s * t
        + curve.control2 * 3.0 * s * t * t
        + curve.to * t * t * t
}

#[test]
fn shared_edges_reverse_the_same_control_points_on_both_axes() {
    let grid = UVec2::new(7, 5);
    for size in [
        Vec2::new(128.0, 96.0),
        Vec2::new(800.0 / 7.0, 600.0 / 5.0),
        Vec2::new(0.25, 0.5),
    ] {
        for seed in [0, 42, u64::MAX] {
            for y in 0..grid.y {
                for x in 0..grid.x {
                    let a = piece_edges(seed, grid, UVec2::new(x, y), size);
                    for (neighbor, first, second, offset) in [
                        (UVec2::new(x + 1, y), 1, 3, Vec2::new(size.x, 0.0)),
                        (UVec2::new(x, y + 1), 2, 0, Vec2::new(0.0, -size.y)),
                    ] {
                        if neighbor.x >= grid.x || neighbor.y >= grid.y {
                            continue;
                        }
                        let b = piece_edges(seed, grid, neighbor, size);
                        let PieceEdge::Curved(ca) = &a[first] else {
                            panic!("internal edge straight")
                        };
                        let PieceEdge::Curved(cb) = b[second].clone().reversed() else {
                            panic!("internal edge straight")
                        };
                        for (ca, cb) in ca.iter().zip(cb) {
                            for (pa, pb) in points(*ca).into_iter().zip(points(cb)) {
                                // Local centers differ by one cell; rounding of
                                // translation is bounded to two f32 ulps.
                                assert!(
                                    (pa - (pb + offset)).abs().max_element()
                                        <= size.max_element() * f32::EPSILON * 2.0
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn profiles_vary_dimensions_and_use_all_seed_bits() {
    let mut styles = [false; 6];
    for orientation in [EdgeOrientation::Horizontal, EdgeOrientation::Vertical] {
        let mut changed = 0;
        for x in 0..64 {
            let edge = EdgeId {
                orientation,
                x,
                y: 5,
            };
            let a = EdgeProfile::from_seed(42, edge);
            let b = EdgeProfile::from_seed(42 | (1 << 63), edge);
            assert_eq!(a, EdgeProfile::from_seed(42, edge));
            styles[a.style as usize] = true;
            if a.width != b.width && a.depth != b.depth && a.head_width != b.head_width {
                changed += 1;
            }
            assert!((0.475..=0.525).contains(&a.center));
            assert!(a.asymmetry.abs() <= 0.025);
            assert!(a.neck_width >= 0.115 && a.neck_width < a.head_width);
            assert!(a.depth < 0.22);
        }
        assert!(changed > 60);
    }
    assert!(styles.into_iter().all(|found| found));
}

#[test]
fn outer_edges_are_exact_lines_and_cubics_are_c1() {
    let grid = UVec2::new(3, 3);
    for seed in 0..128 {
        for y in 0..3 {
            for x in 0..3 {
                let size = Vec2::new(160.0, 80.0);
                let edges = piece_edges(seed, grid, UVec2::new(x, y), size);
                for (i, edge) in edges.iter().enumerate() {
                    let outer = [y == 0, x == 2, y == 2, x == 0][i];
                    assert_eq!(matches!(edge, PieceEdge::Straight { .. }), outer);
                    match edge {
                        PieceEdge::Straight { from, to } => {
                            assert!(from.x == to.x || from.y == to.y)
                        }
                        PieceEdge::Curved(curves) => {
                            for pair in curves.windows(2) {
                                assert_eq!(pair[0].to, pair[1].from);
                                let left = pair[0].to - pair[0].control2;
                                let right = pair[1].control1 - pair[1].from;
                                assert!((left - right).length() < 0.00002);
                            }
                        }
                    }
                }
            }
        }
    }
}

fn crossing(a: Vec2, b: Vec2, c: Vec2, d: Vec2) -> bool {
    let ab = b - a;
    let cd = d - c;
    let side_c = ab.perp_dot(c - a);
    let side_d = ab.perp_dot(d - a);
    let side_a = cd.perp_dot(a - c);
    let side_b = cd.perp_dot(b - c);
    side_c * side_d < 0.0 && side_a * side_b < 0.0
}

#[test]
fn sampled_contours_do_not_self_intersect_or_invade_adjacent_edges() {
    for size in [
        Vec2::new(100.0, 100.0),
        Vec2::new(200.0, 10.0),
        Vec2::new(0.001, 0.002),
    ] {
        for seed in 0..64 {
            let edges = piece_edges(seed, UVec2::splat(3), UVec2::ONE, size);
            let mut vertices = Vec::new();
            for edge in &edges {
                let PieceEdge::Curved(curves) = edge else {
                    unreachable!()
                };
                for &curve in curves {
                    vertices.extend((0..12).map(|i| sample(curve, i as f32 / 12.0)));
                }
            }
            for i in 0..vertices.len() {
                let next_i = (i + 1) % vertices.len();
                assert!(vertices[i].is_finite());
                for j in i + 2..vertices.len() {
                    let next_j = (j + 1) % vertices.len();
                    if next_j == i {
                        continue;
                    }
                    assert!(
                        !crossing(vertices[i], vertices[next_i], vertices[j], vertices[next_j]),
                        "seed {seed}, size {size}, segments {i}/{j}"
                    );
                }
            }
        }
    }
}
