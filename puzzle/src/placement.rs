use bevy_math::Vec2;
use jigsall_core::{PieceId, PuzzleDefinition};
use rand::prelude::*;
use rand_chacha::ChaCha8Rng;

/// Rotation has its own integer hash domain, independent of the position shuffle.
/// Both halves of the seed and the row-major piece identity participate.
pub fn initial_rotation(definition: &PuzzleDefinition, id: PieceId) -> u32 {
    if !definition.rotation_enabled {
        return 0;
    }
    let mix = crate::procedural::mix32;
    mix(mix(definition.seed as u32 ^ 0x726f_7461) ^ mix((definition.seed >> 32) as u32) ^ mix(id.0))
        & 3
}

/// Square slots leave room for either orientation of non-square pieces.
pub fn placement_piece_size(definition: &PuzzleDefinition) -> Vec2 {
    let size = definition.image_size.as_vec2() / definition.grid_size.as_vec2();
    if definition.rotation_enabled {
        Vec2::splat(size.max_element())
    } else {
        size
    }
}

/// Deterministic workspace for component pivots, independent of the camera,
/// texture allocation and window. Geometry may extend beyond this area.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LogicalPlayArea {
    pub half_extents: bevy_math::DVec2,
}
impl LogicalPlayArea {
    pub fn from_definition(definition: &PuzzleDefinition) -> Result<Self, &'static str> {
        definition.validate()?;
        let image = definition.image_size.as_vec2();
        let scatter = placement_half_extents(
            definition.piece_count(),
            placement_piece_size(definition),
            image,
        );
        Ok(Self {
            half_extents: scatter.as_dvec2()
                + bevy_math::DVec2::splat(f64::from(image.max_element()) * 2.0),
        })
    }

    pub fn contains(self, pivot: bevy_math::DVec2) -> bool {
        pivot.is_finite() && pivot.abs().cmple(self.half_extents).all()
    }
}

/// Fill and shuffle caller-owned storage without allocating a position Vec.
/// The mapped values move with their positions during the seeded shuffle.
pub fn fill_placement_grid<T>(
    placements: &mut [T],
    piece_size: Vec2,
    display_size: Vec2,
    seed: u64,
    mut from_position: impl FnMut(Vec2) -> T,
) {
    for (slot, position) in placements
        .iter_mut()
        .zip(placement_slots(piece_size, display_size))
    {
        *slot = from_position(position);
    }
    placements.shuffle(&mut ChaCha8Rng::seed_from_u64(seed));
}

fn placement_layout(piece_size: Vec2, display_size: Vec2) -> (Vec2, i32, i32) {
    let spacing = piece_size * 1.5;
    let half_x = (display_size.x * 0.5 / spacing.x).ceil() as i32 + 1;
    let half_y = (display_size.y * 0.5 / spacing.y).ceil() as i32 + 1;
    (spacing, half_x, half_y)
}

/// Conservative, origin-centered bounds of the initial piece centers, in O(1).
/// Includes the entire last ring even when only some of its slots are occupied.
/// Piece geometry must be added by the caller. Uses positive dimensions and
/// counts from supported puzzle grids (at most 1000 pieces per axis).
pub fn placement_half_extents(count: usize, piece_size: Vec2, display_size: Vec2) -> Vec2 {
    if count == 0 {
        return Vec2::ZERO;
    }
    let (spacing, half_x, half_y) = placement_layout(piece_size, display_size);
    // k rings contain 4 * k * (half_x + half_y + k - 1) slots.
    // Invert that count with an integer square root, then round up exactly.
    let base = (half_x + half_y - 1) as usize;
    let quarter_count = count.div_ceil(4);
    let mut rings = ((base * base + 4 * quarter_count).isqrt() - base) / 2;
    if rings * (base + rings) < quarter_count {
        rings += 1;
    }
    let last_ring = (rings - 1) as f32;
    Vec2::new(half_x as f32 + last_ring, half_y as f32 + last_ring) * spacing
}

fn placement_slots(piece_size: Vec2, display_size: Vec2) -> impl Iterator<Item = Vec2> {
    let (spacing, half_x, half_y) = placement_layout(piece_size, display_size);
    (0..).flat_map(move |ring| {
        let x = half_x + ring;
        let y = half_y + ring;
        (-x..x)
            .map(move |a| (a, y))
            .chain((-y..y).rev().map(move |b| (x, b + 1)))
            .chain((-x..x).rev().map(move |a| (a + 1, -y)))
            .chain((-y..y).map(move |b| (-x, b)))
            .map(move |(x, y)| Vec2::new(x as f32, y as f32) * spacing)
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn placements(count: usize, piece_size: Vec2, display_size: Vec2, seed: u64) -> Vec<Vec2> {
        let mut positions = vec![Vec2::ZERO; count];
        fill_placement_grid(&mut positions, piece_size, display_size, seed, |p| p);
        positions
    }

    #[test]
    fn initial_rotations_have_stable_vectors_for_both_seed_halves_and_disabled_mode() {
        let mut definition = PuzzleDefinition {
            generator_version: jigsall_core::GENERATOR_VERSION,
            seed: 42,
            grid_size: bevy_math::UVec2::new(4, 3),
            image_size: bevy_math::UVec2::new(400, 90),
            snap_distance: 5.0,
            rotation_enabled: true,
        };
        let rotations = |d: &PuzzleDefinition| {
            (0..12)
                .map(|id| initial_rotation(d, PieceId(id)))
                .collect::<Vec<_>>()
        };
        assert_eq!(rotations(&definition), [2, 2, 1, 0, 2, 1, 3, 3, 2, 0, 3, 2]);
        definition.seed |= 1 << 63;
        assert_eq!(rotations(&definition), [2, 2, 2, 0, 2, 2, 0, 3, 1, 2, 3, 3]);
        definition.seed += 1;
        assert_ne!(rotations(&definition), [2, 2, 2, 0, 2, 2, 0, 3, 1, 2, 3, 3]);
        definition.rotation_enabled = false;
        assert_eq!(rotations(&definition), [0; 12]);
    }

    #[test]
    fn logical_play_area_contains_all_scatter_centers_and_image_scaled_workspace() {
        for (grid, image) in [
            (bevy_math::UVec2::splat(1), bevy_math::UVec2::splat(2)),
            (
                bevy_math::UVec2::new(40, 25),
                bevy_math::UVec2::new(4096, 123),
            ),
            (
                bevy_math::UVec2::new(1000, 1),
                bevy_math::UVec2::new(16384, 1),
            ),
            (
                bevy_math::UVec2::splat(1000),
                bevy_math::UVec2::splat(16384),
            ),
        ] {
            let definition = PuzzleDefinition {
                generator_version: jigsall_core::GENERATOR_VERSION,
                seed: 42,
                grid_size: grid,
                image_size: image,
                snap_distance: 5.0,
                rotation_enabled: false,
            };
            let area = LogicalPlayArea::from_definition(&definition).unwrap();
            let centers = placement_half_extents(
                definition.piece_count(),
                image.as_vec2() / grid.as_vec2(),
                image.as_vec2(),
            );
            assert_eq!(
                area.half_extents - centers.as_dvec2(),
                bevy_math::DVec2::splat(f64::from(image.max_element()) * 2.0)
            );
            let positions = placements(
                definition.piece_count(),
                image.as_vec2() / grid.as_vec2(),
                image.as_vec2(),
                definition.seed,
            );
            assert!(positions.iter().all(|p| area.contains(p.as_dvec2())));
            let mut another_seed = definition.clone();
            another_seed.seed += 1;
            assert_eq!(
                LogicalPlayArea::from_definition(&another_seed).unwrap(),
                area
            );
        }
    }

    #[test]
    fn placement_bounds_include_first_partial_and_million_piece_rings() {
        for (w, h) in [(1, 1), (4, 4), (40, 25), (1000, 1000), (1000, 1), (1, 1000)] {
            let piece_size = Vec2::new(10.0, 7.0);
            let display_size = Vec2::new(w as f32 * 10.0, h as f32 * 7.0);
            let (_, half_x, half_y) = placement_layout(piece_size, display_size);
            let first_ring_count = 4 * (half_x + half_y) as usize;
            for count in [1, first_ring_count, first_ring_count + 1, w * h] {
                let half = placement_half_extents(count, piece_size, display_size);
                let mut actual_half = Vec2::ZERO;
                for position in placement_slots(piece_size, display_size).take(count) {
                    actual_half = actual_half.max(position.abs());
                }
                assert!(actual_half.cmple(half).all(), "{w}x{h}, {count}");
                // Complete rings reach the bound on both axes.
                if count == first_ring_count {
                    assert_eq!(actual_half, half);
                }
            }
        }
        assert_eq!(placement_half_extents(0, Vec2::ONE, Vec2::ONE), Vec2::ZERO);
    }

    #[test]
    fn million_slots_are_unique_outside_and_reproducible() {
        for (w, h) in [(40, 25), (1000, 1000), (1000, 1), (1, 1000)] {
            let a = placements(
                w * h,
                Vec2::new(10.0, 7.0),
                Vec2::new(w as f32 * 10.0, h as f32 * 7.0),
                42,
            );
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
                placements(
                    w * h,
                    Vec2::new(10.0, 7.0),
                    Vec2::new(w as f32 * 10.0, h as f32 * 7.0),
                    42
                )
            );
        }
    }
}
