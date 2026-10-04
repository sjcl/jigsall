//! Input- and rendering-independent definitions and authoritative decisions.
use crate::PieceBitSet;
use bevy_ecs::prelude::Resource;
use bevy_math::{UVec2, Vec2};
use serde::{Deserialize, Serialize};

/// Row-major identity within one puzzle; never a Bevy Entity ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PieceId(pub u32);
impl std::fmt::Display for PieceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PlayerId(pub u64);
pub const LOCAL_PLAYER: PlayerId = PlayerId(0);
/// Release v1 preserves the pre-release v5 shape classes and v4 root fillets.
pub const GENERATOR_VERSION: u16 = 1;

/// Device-independent upper bound for the puzzle's coordinate system.
pub const MAX_PUZZLE_IMAGE_DIMENSION: u32 = 16_384;

/// Fit within a maximum edge without upscaling. Round the shorter edge to the
/// nearest pixel (ties up), retaining at least one pixel for very thin images.
/// u64 intermediates make this identical for all u32 source dimensions.
pub fn fit_image_size(size: UVec2, max_dimension: u32) -> UVec2 {
    if size.x == 0 || size.y == 0 || max_dimension == 0 {
        return UVec2::ZERO;
    }
    let longest = size.max_element();
    if longest <= max_dimension {
        return size;
    }
    let scale = |edge: u32| {
        ((u64::from(edge) * u64::from(max_dimension) + u64::from(longest) / 2) / u64::from(longest))
            .max(1) as u32
    };
    UVec2::new(scale(size.x), scale(size.y))
}

/// Frozen at game start. Logical image dimensions participate in reconstruction;
/// local GPU texture dimensions never affect these coordinates.
#[derive(Resource, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PuzzleDefinition {
    pub generator_version: u16,
    pub seed: u64,
    pub grid_size: UVec2,
    pub image_size: UVec2,
    /// Frozen for saves and multiplayer; new games derive this from nominal cell size.
    pub snap_distance: f32,
    /// Frozen game rule: seeded initial quarter turns and rotation commands.
    pub rotation_enabled: bool,
}
impl PuzzleDefinition {
    /// Create a new puzzle with a snap radius of 20% of the piece's shorter side.
    /// Logical image dimensions keep texture resolution and zoom from affecting it.
    /// Call `validate` before using the definition.
    pub fn new(seed: u64, grid_size: UVec2, image_size: UVec2, rotation_enabled: bool) -> Self {
        let piece_size = image_size.as_vec2() / grid_size.as_vec2();
        Self {
            generator_version: GENERATOR_VERSION,
            seed,
            grid_size,
            image_size,
            snap_distance: piece_size.min_element() / 5.0,
            rotation_enabled,
        }
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.generator_version != GENERATOR_VERSION {
            return Err("Unsupported puzzle generator version");
        }
        if self.grid_size.x == 0
            || self.grid_size.y == 0
            || self.grid_size.x > 1000
            || self.grid_size.y > 1000
        {
            return Err("Grid dimensions must be between 1 and 1000");
        }
        if self.image_size.x == 0 || self.image_size.y == 0 {
            return Err("Image dimensions must be positive");
        }
        if self.image_size.max_element() > MAX_PUZZLE_IMAGE_DIMENSION {
            return Err("Image dimensions exceed the puzzle limit");
        }
        if !self.snap_distance.is_finite() || self.snap_distance <= 0.0 {
            return Err("Snap distance must be finite and positive");
        }
        Ok(())
    }
    #[inline]
    pub fn piece_count(&self) -> usize {
        self.grid_size.x as usize * self.grid_size.y as usize
    }
    #[inline]
    pub fn correct_position(&self, id: PieceId) -> Vec2 {
        self.geometry().correct_position(id)
    }

    /// Precompute coordinate constants once for bulk operations. Arithmetic order
    /// matches the generator, including the final y negation.
    pub fn geometry(&self) -> PuzzleGeometry {
        PuzzleGeometry {
            grid_size: self.grid_size,
            size: self.image_size.as_vec2() / self.grid_size.as_vec2(),
            center: (self.grid_size.as_vec2() - Vec2::ONE) * 0.5,
        }
    }

    /// Correct image neighbors only, in stable left/right/up/down order.
    #[inline]
    pub fn neighbors(&self, id: PieceId) -> [Option<PieceId>; 4] {
        grid_neighbors(self.grid_size, id)
    }
}
/// Release-local coordinate constants; no persistent per-piece allocation.
#[derive(Clone, Copy)]
pub struct PuzzleGeometry {
    grid_size: UVec2,
    size: Vec2,
    center: Vec2,
}
impl PuzzleGeometry {
    #[inline]
    pub fn correct_position(&self, id: PieceId) -> Vec2 {
        let x = (id.0 % self.grid_size.x) as f32 - self.center.x;
        let y = (id.0 / self.grid_size.x) as f32 - self.center.y;
        Vec2::new(x * self.size.x, -(y * self.size.y))
    }

    #[inline]
    pub fn neighbors(&self, id: PieceId) -> [Option<PieceId>; 4] {
        grid_neighbors(self.grid_size, id)
    }
}

#[inline]
fn grid_neighbors(grid: UVec2, id: PieceId) -> [Option<PieceId>; 4] {
    let width = grid.x;
    if width == 0 || id.0 as usize >= grid.x as usize * grid.y as usize {
        return [None; 4];
    }
    let x = id.0 % width;
    let y = id.0 / width;
    [
        (x > 0).then(|| PieceId(id.0 - 1)),
        (x + 1 < width).then(|| PieceId(id.0 + 1)),
        (y > 0).then(|| PieceId(id.0 - width)),
        (y + 1 < grid.y).then(|| PieceId(id.0 + width)),
    ]
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PieceState {
    pub position: Vec2,
    pub placed: bool,
    pub held_by: Option<PlayerId>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PieceCommand {
    Grab(PieceId),
    Move {
        id: PieceId,
        position: Vec2,
    },
    Release(PieceId),
    /// Reliable control: compact membership, authenticated owner supplied by caller.
    GrabGroup {
        members: PieceBitSet,
    },
    /// Commit the displayed delta once, release and resolve each connected component.
    /// This is a reliable control, not a best-effort Move packet.
    ReleaseGroup {
        members: PieceBitSet,
        delta: Vec2,
    },
    /// Reliable discrete operation on complete, unheld components.
    Rotate {
        target: crate::protocol::PieceTarget,
        quarter_turns: i8,
    },
    /// Atomically commit the displayed transform, rotate held components and rebase.
    RotateDrag {
        members: PieceBitSet,
        delta: Vec2,
        quarter_turns: i8,
    },
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn automatic_snap_distance_scales_with_piece_short_side() {
        for (image_size, grid_size, expected) in [
            (UVec2::new(1000, 600), UVec2::splat(10), 12.0),
            (UVec2::new(1000, 600), UVec2::new(20, 5), 10.0),
            (UVec2::new(1000, 600), UVec2::new(5, 20), 6.0),
            (UVec2::new(2000, 1200), UVec2::splat(10), 24.0),
            (UVec2::new(1000, 600), UVec2::splat(20), 6.0),
            (UVec2::ONE, UVec2::splat(1000), 0.0002),
            (UVec2::splat(16384), UVec2::ONE, 3276.8),
        ] {
            let definition = PuzzleDefinition::new(42, grid_size, image_size, false);
            assert!(definition.validate().is_ok());
            assert!(
                (definition.snap_distance - expected).abs() <= expected * f32::EPSILON * 2.0,
                "{image_size:?} / {grid_size:?}: {} != {expected}",
                definition.snap_distance
            );
            for seed in [0, 42, u64::MAX] {
                let repeated = PuzzleDefinition::new(seed, grid_size, image_size, true);
                let transposed = PuzzleDefinition::new(
                    seed,
                    UVec2::new(grid_size.y, grid_size.x),
                    UVec2::new(image_size.y, image_size.x),
                    false,
                );
                assert_eq!(
                    repeated.snap_distance.to_bits(),
                    definition.snap_distance.to_bits()
                );
                assert_eq!(
                    transposed.snap_distance.to_bits(),
                    definition.snap_distance.to_bits()
                );
            }
        }
    }

    #[test]
    fn automatic_snap_distance_preserves_strict_snap_boundary() {
        let definition = PuzzleDefinition::new(42, UVec2::splat(10), UVec2::new(1000, 600), false);
        for (offset, target) in [
            (Vec2::ZERO, None),
            (Vec2::new(100.0, 50.0), Some(PieceId(1))),
        ] {
            assert!(crate::SnapCandidate::new(
                offset + Vec2::new(12.0, 0.0),
                offset,
                target,
                definition.snap_distance
            )
            .is_none());
            assert!(crate::SnapCandidate::new(
                offset + Vec2::new(11.99, 0.0),
                offset,
                target,
                definition.snap_distance
            )
            .is_some());
        }
    }

    #[test]
    fn image_fit_rounds_consistently_without_upscaling_or_overflow() {
        for (source, limit, expected) in [
            (UVec2::new(24000, 16000), 16384, UVec2::new(16384, 10923)),
            (UVec2::new(20000, 12000), 16384, UVec2::new(16384, 9830)),
            (UVec2::new(16384, 9830), 8192, UVec2::new(8192, 4915)),
            (UVec2::new(100, 200), 16384, UVec2::new(100, 200)),
            (
                UVec2::splat(u32::MAX),
                u32::MAX - 1,
                UVec2::splat(u32::MAX - 1),
            ),
            (UVec2::new(u32::MAX, 1), 16384, UVec2::new(16384, 1)),
            (UVec2::new(4, 1), 2, UVec2::new(2, 1)),
            (UVec2::ZERO, 16384, UVec2::ZERO),
            (UVec2::new(100, 0), 16384, UVec2::ZERO),
            (UVec2::ONE, 0, UVec2::ZERO),
        ] {
            assert_eq!(fit_image_size(source, limit), expected);
            assert_eq!(
                fit_image_size(UVec2::new(source.y, source.x), limit),
                UVec2::new(expected.y, expected.x)
            );
        }
    }

    #[test]
    fn definition_rejects_zero_and_oversized_logical_image_dimensions() {
        let mut definition = PuzzleDefinition {
            generator_version: GENERATOR_VERSION,
            seed: 42,
            grid_size: UVec2::ONE,
            image_size: UVec2::splat(MAX_PUZZLE_IMAGE_DIMENSION),
            snap_distance: 5.0,
            rotation_enabled: true,
        };
        assert!(definition.validate().is_ok());
        for image_size in [
            UVec2::ZERO,
            UVec2::new(1, 0),
            UVec2::new(0, 1),
            UVec2::new(MAX_PUZZLE_IMAGE_DIMENSION + 1, 1),
            UVec2::new(1, MAX_PUZZLE_IMAGE_DIMENSION + 1),
            UVec2::splat(u32::MAX),
        ] {
            definition.image_size = image_size;
            assert!(definition.validate().is_err(), "{image_size:?}");
        }
    }

    #[test]
    fn precomputed_coordinates_exactly_match_generator_arithmetic() {
        for (grid_size, image_size) in [
            (UVec2::ONE, UVec2::ONE),
            (UVec2::new(3, 2), UVec2::new(123, 4096)),
            (UVec2::new(123, 79), UVec2::new(4096, 3071)),
            (UVec2::splat(1000), UVec2::new(u32::MAX, 4096)),
        ] {
            let d = PuzzleDefinition {
                generator_version: GENERATOR_VERSION,
                seed: 42,
                grid_size,
                image_size,
                snap_distance: 5.0,
                rotation_enabled: true,
            };
            let geometry = d.geometry();
            for id in 0..d.piece_count() as u32 {
                let cell = UVec2::new(id % grid_size.x, id / grid_size.x);
                let size = image_size.as_vec2() / grid_size.as_vec2();
                let offset = cell.as_vec2() - (grid_size.as_vec2() - Vec2::ONE) * 0.5;
                let expected = Vec2::new(offset.x * size.x, -offset.y * size.y);
                let actual = geometry.correct_position(PieceId(id));
                assert_eq!(
                    actual.to_array().map(f32::to_bits),
                    expected.to_array().map(f32::to_bits)
                );
                assert_eq!(d.correct_position(PieceId(id)), expected);
                assert_eq!(geometry.neighbors(PieceId(id)), d.neighbors(PieceId(id)));
            }
        }
    }

    #[test]
    fn only_current_shape_version_is_accepted() {
        assert_eq!(GENERATOR_VERSION, 1);
        let mut definition = PuzzleDefinition {
            generator_version: GENERATOR_VERSION,
            seed: 42,
            grid_size: UVec2::splat(2),
            image_size: UVec2::splat(100),
            snap_distance: 5.0,
            rotation_enabled: true,
        };
        assert!(definition.validate().is_ok());
        for old_version in [0, 2, 3, 4, 5, 6] {
            definition.generator_version = old_version;
            assert_eq!(
                definition.validate(),
                Err("Unsupported puzzle generator version")
            );
        }
    }
}
