use bevy_math::Vec2;
use rand::prelude::*;
use rand_chacha::ChaCha8Rng;

/// Disjoint lattice slots on rectangular rings, then seeded Fisher-Yates. O(N).
pub fn generate_placement_grid(
    grid_width: usize,
    grid_height: usize,
    piece_width: f32,
    piece_height: f32,
    display_width: f32,
    display_height: f32,
    seed: u64,
) -> Vec<Vec2> {
    let count = grid_width * grid_height;
    let spacing = Vec2::new(piece_width, piece_height) * 1.5;
    let half_x = (display_width * 0.5 / spacing.x).ceil() as i32 + 1;
    let half_y = (display_height * 0.5 / spacing.y).ceil() as i32 + 1;
    let mut positions = Vec::with_capacity(count);
    let mut ring = 0;
    while positions.len() < count {
        let x = half_x + ring;
        let y = half_y + ring;
        let slots = (-x..x)
            .map(|a| (a, y))
            .chain((-y..y).rev().map(|b| (x, b + 1)))
            .chain((-x..x).rev().map(|a| (a + 1, -y)))
            .chain((-y..y).map(|b| (-x, b)));
        for (x, y) in slots {
            positions.push(Vec2::new(x as f32, y as f32) * spacing);
            if positions.len() == count {
                break;
            }
        }
        ring += 1;
    }
    positions.shuffle(&mut ChaCha8Rng::seed_from_u64(seed));
    positions
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    #[test]
    fn million_slots_are_unique_outside_and_reproducible() {
        for (w, h) in [(40, 25), (1000, 1000), (1000, 1), (1, 1000)] {
            let a = generate_placement_grid(w, h, 10.0, 7.0, w as f32 * 10.0, h as f32 * 7.0, 42);
            assert_eq!(a.len(), w * h);
            let slots: HashSet<_> = a
                .iter()
                .map(|p| ((p.x / 15.0).round() as i32, (p.y / 10.5).round() as i32))
                .collect();
            assert_eq!(slots.len(), a.len());
            assert!(a.iter().all(|p| p.is_finite()
                && (p.x.abs() > w as f32 * 5.0 + 7.2 || p.y.abs() > h as f32 * 3.5 + 5.04)));
            assert_eq!(
                a,
                generate_placement_grid(w, h, 10.0, 7.0, w as f32 * 10.0, h as f32 * 7.0, 42)
            );
        }
    }
}
