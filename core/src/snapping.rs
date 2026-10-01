//! Pure translation candidates; the authority decides ownership and unions.
use crate::PieceId;
use bevy_math::Vec2;

#[inline]
pub fn offset_distance_squared(a: Vec2, b: Vec2) -> f64 {
    let x = f64::from(a.x) - f64::from(b.x);
    let y = f64::from(a.y) - f64::from(b.y);
    x * x + y * y
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnapCandidate {
    pub offset: Vec2,
    /// None is the board; ties prefer board, then the smaller boundary PieceId.
    pub target: Option<PieceId>,
    pub distance_squared: f64,
}

impl SnapCandidate {
    #[inline]
    pub fn new(
        moving_offset: Vec2,
        offset: Vec2,
        target: Option<PieceId>,
        snap_distance: f32,
    ) -> Option<Self> {
        let distance_squared = offset_distance_squared(moving_offset, offset);
        (moving_offset.is_finite()
            && offset.is_finite()
            && snap_distance.is_finite()
            && snap_distance > 0.0
            && distance_squared < f64::from(snap_distance).powi(2))
        .then_some(Self {
            offset,
            target,
            distance_squared,
        })
    }

    #[inline]
    pub fn precedes(&self, other: &Self) -> bool {
        self.distance_squared
            .total_cmp(&other.distance_squared)
            .then_with(|| self.target.cmp(&other.target))
            .is_lt()
    }
}

/// f32 addition/subtraction cannot preserve an arbitrary offset bit-for-bit.
/// Check against ONE representative, allowing only arithmetic rounding, not edge drift.
pub fn matches_translation(position: Vec2, correct: Vec2, offset: Vec2) -> bool {
    let reconstructed = correct + offset;
    position.is_finite()
        && reconstructed.is_finite()
        && (0..2).all(|axis| {
            let scale = position[axis]
                .abs()
                .max(correct[axis].abs())
                .max(offset[axis].abs())
                .max(1.0);
            (position[axis] - reconstructed[axis]).abs() <= 4.0 * f32::EPSILON * scale
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PuzzleDefinition, GENERATOR_VERSION};
    use bevy_math::UVec2;

    #[test]
    fn strict_threshold_and_stable_ties() {
        assert!(SnapCandidate::new(Vec2::new(5.0, 0.0), Vec2::ZERO, None, 5.0).is_none());
        assert!(SnapCandidate::new(Vec2::new(4.99, 0.0), Vec2::ZERO, None, 5.0).is_some());
        let board = SnapCandidate::new(Vec2::X, Vec2::ZERO, None, 5.0).unwrap();
        let piece =
            SnapCandidate::new(Vec2::X, Vec2::new(2.0, 0.0), Some(PieceId(0)), 5.0).unwrap();
        assert!(board.precedes(&piece));
        let lower = SnapCandidate {
            target: Some(PieceId(1)),
            ..piece
        };
        let higher = SnapCandidate {
            target: Some(PieceId(2)),
            ..piece
        };
        assert!(lower.precedes(&higher));
    }

    #[test]
    fn row_major_neighbors_never_wrap_at_borders() {
        let d = PuzzleDefinition {
            generator_version: GENERATOR_VERSION,
            seed: 1,
            grid_size: UVec2::new(3, 2),
            image_size: UVec2::splat(100),
            snap_distance: 5.0,
        };
        assert_eq!(
            d.neighbors(PieceId(2)),
            [Some(PieceId(1)), None, None, Some(PieceId(5))]
        );
        assert_eq!(
            d.neighbors(PieceId(3)),
            [None, Some(PieceId(4)), Some(PieceId(0)), None]
        );
        assert_eq!(d.neighbors(PieceId(6)), [None; 4]);
    }
}
