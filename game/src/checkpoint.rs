//! Transport-independent capture, validation and transactional restore.
use crate::resources::{
    pieces::{CONNECTED_EDGE_PAIRS, ENABLED, MAX_Z, PLACED},
    GpuPieceState, PieceDataStore,
};
use bevy::math::Vec2;
use puzzella_core::{
    decode_rotation, matches_transform, rotate_quarter, session::ImageHash, with_rotation,
    PieceConnectivity, PieceId, PuzzleDefinition, ROTATION_MASK,
};
use serde::{Deserialize, Serialize};

pub const SNAPSHOT_PLACED: u32 = 1;
pub const SNAPSHOT_CONNECTED_RIGHT: u32 = 1 << 1;
pub const SNAPSHOT_CONNECTED_DOWN: u32 = 1 << 2;
const SNAPSHOT_FLAGS: u32 =
    SNAPSHOT_PLACED | SNAPSHOT_CONNECTED_RIGHT | SNAPSHOT_CONNECTED_DOWN | ROTATION_MASK;

/// Dense row-major state, 16 bytes per piece; undirected edges are saved right/down.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SnapshotPieceState {
    pub position: Vec2,
    pub z_order: u32,
    pub flags: u32,
}
const _: () = assert!(std::mem::size_of::<SnapshotPieceState>() == 16);
#[derive(Clone, Debug, PartialEq)]
pub struct PuzzleCheckpoint {
    pub image_hash: ImageHash,
    pub definition: PuzzleDefinition,
    pub next_z_order: u32,
    pub pieces: Vec<SnapshotPieceState>,
}

/// A borrowed view lets snapshots reuse the same algorithms without cloning states.
pub(crate) struct CheckpointView<'a> {
    pub definition: &'a PuzzleDefinition,
    pub next_z_order: u32,
    pub pieces: &'a [SnapshotPieceState],
}
impl PuzzleCheckpoint {
    /// Explicit checkpoint only: O(N), with no per-piece entity/map allocation.
    /// The adapter must finish or discard local presentation drag before capture.
    pub fn capture(
        store: &PieceDataStore,
        definition: &PuzzleDefinition,
        image_hash: ImageHash,
    ) -> Result<Self, CheckpointError> {
        if !store.drag.members.is_empty() {
            return Err(CheckpointError::ActiveLocalDrag);
        }
        // Validate before counting/indexing an untrusted definition.
        definition
            .validate()
            .map_err(CheckpointError::InvalidDefinition)?;
        if store.connectivity.len() != store.len() {
            return Err(CheckpointError::WrongConnectivitySize);
        }
        let snapshot = Self {
            image_hash,
            definition: definition.clone(),
            next_z_order: store.next_z_order,
            pieces: store
                .states
                .iter()
                .enumerate()
                .map(|(index, state)| {
                    let id = PieceId(index as u32);
                    let neighbors = definition.neighbors(id);
                    let connected = |neighbor: Option<PieceId>| {
                        neighbor.is_some_and(|n| {
                            store.contains(n) && store.connectivity.same_component(id, n)
                        })
                    };
                    SnapshotPieceState {
                        position: state.position,
                        z_order: state.z_order,
                        flags: with_rotation(
                            if state.flags & PLACED != 0 {
                                SNAPSHOT_PLACED
                            } else {
                                0
                            } | if connected(neighbors[1]) {
                                SNAPSHOT_CONNECTED_RIGHT
                            } else {
                                0
                            } | if connected(neighbors[3]) {
                                SNAPSHOT_CONNECTED_DOWN
                            } else {
                                0
                            },
                            decode_rotation(state.flags),
                        ),
                    }
                })
                .collect(),
        };
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn validate(&self) -> Result<(), CheckpointError> {
        self.view().validated_connectivity().map(|_| ())
    }
    pub fn install(&self, store: &mut PieceDataStore) -> Result<(), CheckpointError> {
        self.view().install(store)
    }
    fn view(&self) -> CheckpointView<'_> {
        CheckpointView {
            definition: &self.definition,
            next_z_order: self.next_z_order,
            pieces: &self.pieces,
        }
    }
}
impl CheckpointView<'_> {
    pub(crate) fn validated_connectivity(&self) -> Result<PieceConnectivity, CheckpointError> {
        self.definition
            .validate()
            .map_err(CheckpointError::InvalidDefinition)?;
        let count = self.definition.piece_count();
        if self.pieces.len() != count {
            return Err(CheckpointError::WrongPieceCount {
                expected: count,
                actual: self.pieces.len(),
            });
        }
        if self.next_z_order < count as u32 || self.next_z_order > MAX_Z {
            return Err(CheckpointError::InvalidNextZOrder);
        }
        let mut connectivity = PieceConnectivity::new(count);
        for (index, state) in self.pieces.iter().enumerate() {
            let id = PieceId(index as u32);
            if !state.position.is_finite() {
                return Err(CheckpointError::NonFinitePosition(id));
            }
            if state.z_order >= self.next_z_order || state.z_order > MAX_Z {
                return Err(CheckpointError::InvalidZOrder(id));
            }
            if state.flags & !SNAPSHOT_FLAGS != 0 {
                return Err(CheckpointError::InvalidFlags(id));
            }
            if state.flags & SNAPSHOT_PLACED != 0
                && (decode_rotation(state.flags) != 0
                    || state.position != self.definition.correct_position(id))
            {
                return Err(CheckpointError::InvalidPlacedPosition(id));
            }
            let neighbors = self.definition.neighbors(id);
            for (flag, neighbor) in [
                (SNAPSHOT_CONNECTED_RIGHT, neighbors[1]),
                (SNAPSHOT_CONNECTED_DOWN, neighbors[3]),
            ] {
                if state.flags & flag != 0 {
                    let neighbor = neighbor.ok_or(CheckpointError::InvalidBorderConnection(id))?;
                    connectivity.union(id, neighbor);
                }
            }
        }
        for (index, state) in self.pieces.iter().enumerate() {
            let id = PieceId(index as u32);
            let root = connectivity.minimum_member(id);
            let representative = &self.pieces[root.0 as usize];
            let rotation = decode_rotation(representative.flags);
            let offset = representative.position
                - rotate_quarter(self.definition.correct_position(root), rotation);
            if (state.flags & SNAPSHOT_PLACED) != (representative.flags & SNAPSHOT_PLACED)
                || decode_rotation(state.flags) != rotation
                || !matches_transform(
                    state.position,
                    self.definition.correct_position(id),
                    rotation,
                    offset,
                )
            {
                return Err(CheckpointError::InconsistentComponent(id));
            }
        }
        Ok(connectivity)
    }

    /// Validate completely before changing any local state. ENABLED is reconstructed.
    /// Resets holds, selections, cached highlights and dirty ranges; advances GPU epoch.
    pub fn install(&self, store: &mut PieceDataStore) -> Result<(), CheckpointError> {
        let connectivity = self.validated_connectivity()?;
        let states = self
            .pieces
            .iter()
            .enumerate()
            .map(|(index, state)| {
                let id = PieceId(index as u32);
                // Derive from the restored DSU, including shared cycle edges
                // implied by a sparse right/down snapshot. Never trust GPU flags.
                let edges = self
                    .definition
                    .neighbors(id)
                    .into_iter()
                    .zip(CONNECTED_EDGE_PAIRS)
                    .filter_map(|(neighbor, (edge, _))| {
                        neighbor
                            .filter(|&n| connectivity.same_component(id, n))
                            .map(|_| edge)
                    })
                    .fold(0, |flags, edge| flags | edge);
                GpuPieceState {
                    position: state.position,
                    z_order: state.z_order,
                    flags: with_rotation(
                        ENABLED
                            | edges
                            | if state.flags & SNAPSHOT_PLACED != 0 {
                                PLACED
                            } else {
                                0
                            },
                        decode_rotation(state.flags),
                    ),
                }
            })
            .collect();
        store.replace_snapshot_states(states, self.next_z_order, connectivity);
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckpointError {
    ActiveLocalDrag,
    UnsupportedSchema(u16),
    InvalidDefinition(&'static str),
    WrongSession,
    WrongImageHash,
    WrongCursor,
    WrongDefinition,
    WrongPieceCount { expected: usize, actual: usize },
    InvalidNextZOrder,
    NonFinitePosition(PieceId),
    InvalidZOrder(PieceId),
    InvalidFlags(PieceId),
    InvalidBorderConnection(PieceId),
    InconsistentComponent(PieceId),
    InvalidPlacedPosition(PieceId),
    WrongConnectivitySize,
}
impl std::fmt::Display for CheckpointError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Checkpoint rejected: {self:?}")
    }
}
impl std::error::Error for CheckpointError {}
