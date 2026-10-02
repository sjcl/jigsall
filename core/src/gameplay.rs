//! Input- and rendering-independent definitions and authoritative decisions.
use crate::PieceBitSet;
use bevy_ecs::prelude::{Component, Resource};
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PlayerId(pub u64);
pub const LOCAL_PLAYER: PlayerId = PlayerId(0);
/// Version 5 retains the v4 fillets and decodes distinct macro shape classes.
pub const GENERATOR_VERSION: u16 = 5;

/// Frozen at game start. Image dimensions also participate in reconstruction.
#[derive(Resource, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PuzzleDefinition {
    pub generator_version: u16,
    pub seed: u64,
    pub grid_size: UVec2,
    pub image_size: UVec2,
    pub snap_distance: f32,
}
impl PuzzleDefinition {
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
    #[inline]
    pub fn piece(&self, index: u32, initial_position: Vec2) -> PuzzlePiece {
        let grid_position = UVec2::new(index % self.grid_size.x, index / self.grid_size.x);
        let size = self.image_size.as_vec2() / self.grid_size.as_vec2();
        let offset = grid_position.as_vec2() - (self.grid_size.as_vec2() - Vec2::ONE) * 0.5;
        PuzzlePiece {
            id: PieceId(index),
            grid_position,
            correct_position: Vec2::new(offset.x * size.x, -offset.y * size.y),
            initial_position,
        }
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

#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PuzzlePiece {
    pub id: PieceId,
    pub grid_position: UVec2,
    pub correct_position: Vec2,
    pub initial_position: Vec2,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PieceState {
    pub position: Vec2,
    pub placed: bool,
    pub held_by: Option<PlayerId>,
}
impl PieceState {
    pub fn new(position: Vec2) -> Self {
        Self {
            position,
            placed: false,
            held_by: None,
        }
    }
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
}
impl PieceCommand {
    pub fn piece_id(&self) -> Option<PieceId> {
        match *self {
            Self::Grab(id) | Self::Move { id, .. } | Self::Release(id) => Some(id),
            Self::GrabGroup { .. } | Self::ReleaseGroup { .. } | Self::Rotate { .. } => None,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CommandOutcome {
    Grabbed,
    Moved,
    Released,
}

/// Caller supplies an authenticated identity when a transport is added.
pub fn apply_piece_command(
    state: &mut PieceState,
    player: PlayerId,
    command: &PieceCommand,
) -> Option<CommandOutcome> {
    if state.placed {
        return None;
    }
    match *command {
        PieceCommand::Grab(_) if state.held_by.is_none() => {
            state.held_by = Some(player);
            Some(CommandOutcome::Grabbed)
        }
        PieceCommand::Move { position, .. }
            if state.held_by == Some(player) && position.is_finite() =>
        {
            state.position = position;
            Some(CommandOutcome::Moved)
        }
        PieceCommand::Release(_) if state.held_by == Some(player) => {
            state.held_by = None;
            Some(CommandOutcome::Released)
        }
        _ => None,
    }
}
/// Called after an accepted release; no mouse, UI, or Transform dependency.
pub fn snap_piece(piece: &PuzzlePiece, state: &mut PieceState, distance: f32) -> bool {
    if state.placed
        || state.held_by.is_some()
        || !state.position.is_finite()
        || !distance.is_finite()
        || distance <= 0.0
    {
        return false;
    }
    if state.position.distance(piece.correct_position) < distance {
        state.position = piece.correct_position;
        state.placed = true;
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
            };
            let geometry = d.geometry();
            for id in 0..d.piece_count() as u32 {
                let expected = d.piece(id, Vec2::ZERO).correct_position;
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
        let mut definition = PuzzleDefinition {
            generator_version: GENERATOR_VERSION,
            seed: 42,
            grid_size: UVec2::splat(2),
            image_size: UVec2::splat(100),
            snap_distance: 5.0,
        };
        assert!(definition.validate().is_ok());
        for old_version in [2, 3, 4] {
            definition.generator_version = old_version;
            assert_eq!(
                definition.validate(),
                Err("Unsupported puzzle generator version")
            );
        }
    }
    #[test]
    fn validates_ownership_and_finite_moves() {
        let mut s = PieceState::new(Vec2::ZERO);
        let id = PieceId(7);
        let other = PlayerId(1);
        assert_eq!(
            apply_piece_command(&mut s, LOCAL_PLAYER, &PieceCommand::Grab(id)),
            Some(CommandOutcome::Grabbed)
        );
        assert_eq!(
            apply_piece_command(&mut s, other, &PieceCommand::Grab(id)),
            None
        );
        assert_eq!(
            apply_piece_command(
                &mut s,
                other,
                &PieceCommand::Move {
                    id,
                    position: Vec2::ONE
                }
            ),
            None
        );
        assert_eq!(
            apply_piece_command(
                &mut s,
                LOCAL_PLAYER,
                &PieceCommand::Move {
                    id,
                    position: Vec2::splat(f32::NAN)
                }
            ),
            None
        );
        assert_eq!(
            apply_piece_command(&mut s, other, &PieceCommand::Release(id)),
            None
        );
        assert_eq!(s.position, Vec2::ZERO);
        assert_eq!(
            apply_piece_command(&mut s, LOCAL_PLAYER, &PieceCommand::Release(id)),
            Some(CommandOutcome::Released)
        );
    }
    #[test]
    fn release_snap_locks_piece_and_preserves_threshold() {
        let p = PuzzlePiece {
            id: PieceId(0),
            grid_position: UVec2::ZERO,
            correct_position: Vec2::ZERO,
            initial_position: Vec2::ONE,
        };
        let mut s = PieceState::new(Vec2::new(5.0, 0.0));
        assert!(!snap_piece(&p, &mut s, 5.0));
        s.held_by = Some(LOCAL_PLAYER);
        assert!(!snap_piece(&p, &mut s, 10.0));
        apply_piece_command(&mut s, LOCAL_PLAYER, &PieceCommand::Release(p.id));
        assert!(snap_piece(&p, &mut s, 10.0));
        assert_eq!(s.position, Vec2::ZERO);
        assert_eq!(
            apply_piece_command(&mut s, LOCAL_PLAYER, &PieceCommand::Grab(p.id)),
            None
        );
    }
}
