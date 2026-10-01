//! Versioned gameplay snapshots: GPU flags/layout are not the protocol.
use crate::resources::{
    pieces::{CONNECTED_EDGE_PAIRS, ENABLED, MAX_Z, PLACED},
    GpuPieceState, PieceDataStore,
};
use bevy::math::Vec2;
use puzzella_core::{
    matches_translation,
    session::{AuthorityCursor, ImageHash, SessionDefinition, SessionId},
    PieceConnectivity, PieceId, PuzzleDefinition,
};
use serde::{Deserialize, Serialize};

pub const SNAPSHOT_SCHEMA_VERSION: u16 = 3;
pub const SNAPSHOT_PLACED: u32 = 1;
pub const SNAPSHOT_CONNECTED_RIGHT: u32 = 1 << 1;
pub const SNAPSHOT_CONNECTED_DOWN: u32 = 1 << 2;
const SNAPSHOT_FLAGS: u32 = SNAPSHOT_PLACED | SNAPSHOT_CONNECTED_RIGHT | SNAPSHOT_CONNECTED_DOWN;

/// Dense row-major state, 16 bytes per piece; undirected edges are saved right/down.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SnapshotPieceState {
    pub position: Vec2,
    pub z_order: u32,
    pub flags: u32,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GameSnapshot {
    pub schema_version: u16,
    pub session: SessionId,
    pub image_hash: ImageHash,
    pub cursor: AuthorityCursor,
    pub definition: PuzzleDefinition,
    pub next_z_order: u32,
    pub pieces: Vec<SnapshotPieceState>,
}
/// Trusted context from the active session/recovery negotiation, never the packet.
#[derive(Clone, Copy, Debug)]
pub struct SnapshotExpectation<'a> {
    pub session: SessionId,
    pub image_hash: ImageHash,
    pub cursor: AuthorityCursor,
    pub definition: &'a PuzzleDefinition,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SnapshotError {
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
impl std::fmt::Display for SnapshotError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Snapshot rejected: {self:?}")
    }
}
impl std::error::Error for SnapshotError {}

impl GameSnapshot {
    /// Explicit checkpoint only: O(N), with no per-piece entity/map allocation.
    /// The adapter must finish or discard local presentation drag before capture.
    pub fn capture(
        store: &PieceDataStore,
        definition: &PuzzleDefinition,
        session: SessionDefinition,
        cursor: AuthorityCursor,
    ) -> Result<Self, SnapshotError> {
        if !store.drag.members.is_empty() {
            return Err(SnapshotError::ActiveLocalDrag);
        }
        // Validate before counting/indexing an untrusted definition.
        definition
            .validate()
            .map_err(SnapshotError::InvalidDefinition)?;
        if store.connectivity.len() != store.len() {
            return Err(SnapshotError::WrongConnectivitySize);
        }
        let snapshot = Self {
            schema_version: SNAPSHOT_SCHEMA_VERSION,
            session: session.id,
            image_hash: session.image_hash,
            cursor,
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
                        flags: if state.flags & PLACED != 0 {
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
                    }
                })
                .collect(),
        };
        snapshot.validate(SnapshotExpectation {
            session: session.id,
            image_hash: session.image_hash,
            cursor,
            definition,
        })?;
        Ok(snapshot)
    }

    /// Exact cursor match prevents installing a different checkpoint than negotiated.
    pub fn validate(&self, expected: SnapshotExpectation<'_>) -> Result<(), SnapshotError> {
        self.validated_connectivity(expected).map(|_| ())
    }

    fn validated_connectivity(
        &self,
        expected: SnapshotExpectation<'_>,
    ) -> Result<PieceConnectivity, SnapshotError> {
        if self.schema_version != SNAPSHOT_SCHEMA_VERSION {
            return Err(SnapshotError::UnsupportedSchema(self.schema_version));
        }
        if self.session != expected.session {
            return Err(SnapshotError::WrongSession);
        }
        if self.image_hash != expected.image_hash {
            return Err(SnapshotError::WrongImageHash);
        }
        if self.cursor != expected.cursor {
            return Err(SnapshotError::WrongCursor);
        }
        self.definition
            .validate()
            .map_err(SnapshotError::InvalidDefinition)?;
        if self.definition != *expected.definition {
            return Err(SnapshotError::WrongDefinition);
        }
        let count = self.definition.piece_count();
        if self.pieces.len() != count {
            return Err(SnapshotError::WrongPieceCount {
                expected: count,
                actual: self.pieces.len(),
            });
        }
        if self.next_z_order < count as u32 || self.next_z_order > MAX_Z {
            return Err(SnapshotError::InvalidNextZOrder);
        }
        let mut connectivity = PieceConnectivity::new(count);
        for (index, state) in self.pieces.iter().enumerate() {
            let id = PieceId(index as u32);
            if !state.position.is_finite() {
                return Err(SnapshotError::NonFinitePosition(id));
            }
            if state.z_order >= self.next_z_order || state.z_order > MAX_Z {
                return Err(SnapshotError::InvalidZOrder(id));
            }
            if state.flags & !SNAPSHOT_FLAGS != 0 {
                return Err(SnapshotError::InvalidFlags(id));
            }
            if state.flags & SNAPSHOT_PLACED != 0
                && state.position != self.definition.correct_position(id)
            {
                return Err(SnapshotError::InvalidPlacedPosition(id));
            }
            let neighbors = self.definition.neighbors(id);
            for (flag, neighbor) in [
                (SNAPSHOT_CONNECTED_RIGHT, neighbors[1]),
                (SNAPSHOT_CONNECTED_DOWN, neighbors[3]),
            ] {
                if state.flags & flag != 0 {
                    let neighbor = neighbor.ok_or(SnapshotError::InvalidBorderConnection(id))?;
                    connectivity.union(id, neighbor);
                }
            }
        }
        for (index, state) in self.pieces.iter().enumerate() {
            let id = PieceId(index as u32);
            let root = connectivity.minimum_member(id);
            let representative = &self.pieces[root.0 as usize];
            let offset = representative.position - self.definition.correct_position(root);
            if (state.flags & SNAPSHOT_PLACED) != (representative.flags & SNAPSHOT_PLACED)
                || !matches_translation(
                    state.position,
                    self.definition.correct_position(id),
                    offset,
                )
            {
                return Err(SnapshotError::InconsistentComponent(id));
            }
        }
        Ok(connectivity)
    }

    /// Validate completely before changing any local state. ENABLED is reconstructed.
    /// Resets holds, selections, cached highlights and dirty ranges; advances GPU epoch.
    pub fn install(
        &self,
        store: &mut PieceDataStore,
        expected: SnapshotExpectation<'_>,
    ) -> Result<(), SnapshotError> {
        let connectivity = self.validated_connectivity(expected)?;
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
                    flags: ENABLED
                        | edges
                        | if state.flags & SNAPSHOT_PLACED != 0 {
                            PLACED
                        } else {
                            0
                        },
                }
            })
            .collect();
        store.replace_snapshot_states(states, self.next_z_order, connectivity);
        Ok(())
    }
}
