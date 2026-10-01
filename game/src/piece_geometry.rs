//! Collision uses the same indexed triangles as the rendered piece.
use bevy::prelude::*;

pub fn triangles<'a>(
    vertices: &'a [Vec2],
    indices: &'a [u32],
) -> impl Iterator<Item = [Vec2; 3]> + 'a {
    indices.chunks_exact(3).filter_map(|triangle| {
        let a = *vertices.get(triangle[0] as usize)?;
        let b = *vertices.get(triangle[1] as usize)?;
        let c = *vertices.get(triangle[2] as usize)?;
        let area = (b - a).perp_dot(c - a);
        (a.is_finite() && b.is_finite() && c.is_finite() && area.abs() > f32::EPSILON)
            .then_some([a, b, c])
    })
}

pub fn triangle_contains_point([a, b, c]: [Vec2; 3], point: Vec2) -> bool {
    let sides = [
        (b - a).perp_dot(point - a),
        (c - b).perp_dot(point - b),
        (a - c).perp_dot(point - c),
    ];
    sides.iter().all(|&side| side >= 0.0) || sides.iter().all(|&side| side <= 0.0)
}

/// Separating-axis test also catches a thin rectangle crossing a triangle
/// without containing any of its vertices (or vice versa).
pub fn triangle_intersects_rect(triangle: [Vec2; 3], rect: Rect) -> bool {
    let corners = [
        rect.min,
        Vec2::new(rect.max.x, rect.min.y),
        rect.max,
        Vec2::new(rect.min.x, rect.max.y),
    ];
    let edges = [
        triangle[1] - triangle[0],
        triangle[2] - triangle[1],
        triangle[0] - triangle[2],
    ];
    [Vec2::X, Vec2::Y]
        .into_iter()
        .chain(edges.map(Vec2::perp))
        .all(|axis| {
            let (min_t, max_t) = projection_range(&triangle, axis);
            let (min_r, max_r) = projection_range(&corners, axis);
            min_t <= max_r && min_r <= max_t
        })
}

fn projection_range(points: &[Vec2], axis: Vec2) -> (f32, f32) {
    points
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(min, max), point| {
            let value = point.dot(axis);
            (min.min(value), max.max(value))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::*;
    use puzzella_core::PieceId;

    fn rectangle(id: u32, z_order: f32) -> PieceCollisionData {
        PieceCollisionData {
            piece_id: PieceId(id),
            position: Vec2::ZERO,
            z_order,
            bounding_box: Rect::new(-10.0, -10.0, 10.0, 10.0),
            // These are deliberately not in boundary order.
            vertices: vec![
                Vec2::new(-10.0, -10.0),
                Vec2::new(10.0, 10.0),
                Vec2::new(10.0, -10.0),
                Vec2::new(-10.0, 10.0),
            ],
            indices: vec![0, 2, 1, 0, 1, 3],
        }
    }

    #[test]
    fn picking_uses_indexed_triangles_and_the_frontmost_piece() {
        let mut collision = PieceCollisionSystem::default();
        collision.add_piece(rectangle(0, 2.0));
        collision.add_piece(rectangle(1, 1.0));
        for point in [Vec2::new(-8.0, 0.0), Vec2::new(8.0, 0.0), Vec2::ZERO] {
            assert_eq!(collision.find_piece_at_position(point), Some(PieceId(0)));
        }
        collision.update_piece_z_order(PieceId(1), 3.0);
        assert_eq!(
            collision.find_piece_at_position(Vec2::ZERO),
            Some(PieceId(1))
        );
        assert_eq!(collision.rtree.size(), 2);
        assert_eq!(collision.find_piece_at_position(Vec2::new(11.0, 0.0)), None);
        assert_eq!(
            collision.find_piece_at_position(Vec2::splat(f32::NAN)),
            None
        );
    }

    #[test]
    fn rectangle_intersection_handles_crossing_edges_and_empty_corners() {
        let triangle = [
            Vec2::new(-10.0, -1.0),
            Vec2::new(10.0, -1.0),
            Vec2::new(0.0, 1.0),
        ];
        assert!(triangle_intersects_rect(
            triangle,
            Rect::new(3.9, -5.0, 4.1, 5.0)
        ));
        assert!(!triangle_intersects_rect(
            triangle,
            Rect::new(9.9, 0.9, 10.0, 1.0)
        ));
        assert!(triangle_intersects_rect(
            triangle,
            Rect::new(-20.0, -20.0, 20.0, 20.0)
        ));
        assert!(triangle_contains_point(triangle, Vec2::new(0.0, -1.0)));
        assert!(triangle_contains_point(
            [triangle[2], triangle[1], triangle[0]],
            Vec2::ZERO
        ));
    }

    #[test]
    fn rectangle_selection_does_not_fill_empty_mesh_regions() {
        let mut data = rectangle(0, 0.0);
        // One triangle occupies only the lower half of the bounding box.
        data.indices = vec![0, 2, 1];
        let mut collision = PieceCollisionSystem::default();
        collision.add_piece(data);
        assert!(collision
            .find_pieces_with_detailed_rect_intersection(Rect::new(-9.0, 8.0, -8.0, 9.0))
            .is_empty());
        assert_eq!(
            collision.find_pieces_with_detailed_rect_intersection(Rect::new(8.0, -9.0, 9.0, -8.0)),
            vec![PieceId(0)]
        );
    }

    #[test]
    fn repeated_updates_and_drag_rebuilds_leave_one_current_tree_record() {
        let mut collision = PieceCollisionSystem::default();
        collision.add_piece(rectangle(0, 0.0));
        collision.rebuild_rtree();
        for index in 1..1000 {
            let position = Vec2::new(index as f32 * 0.37, index as f32 * -0.29);
            collision.update_piece_position(
                PieceId(0),
                position,
                Rect::new(-10.0, -10.0, 10.0, 10.0),
            );
            assert_eq!(collision.rtree.size(), 1);
        }
        let level = PerformanceDebugLevel::Off;
        collision.start_dragging_piece(PieceId(0), &level);
        collision.start_dragging_piece(PieceId(0), &level);
        collision.need_rebuild = true;
        collision.rebuild_rtree();
        assert_eq!(collision.rtree.size(), 0);
        collision.update_piece_position(
            PieceId(0),
            Vec2::splat(1000.0),
            Rect::new(-10.0, -10.0, 10.0, 10.0),
        );
        collision.stop_dragging_piece(PieceId(0), &level);
        collision.stop_dragging_piece(PieceId(0), &level);
        assert_eq!(collision.rtree.size(), 1);
        assert_eq!(collision.find_piece_at_position(Vec2::ZERO), None);
        assert_eq!(
            collision.find_piece_at_position(Vec2::splat(1000.0)),
            Some(PieceId(0))
        );
        collision.remove_piece(PieceId(0));
        assert_eq!(collision.rtree.size(), 0);
    }
}
