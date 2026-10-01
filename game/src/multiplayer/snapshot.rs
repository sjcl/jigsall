//! Versioned gameplay snapshots: GPU flags/layout are not the protocol.
use crate::resources::{
    pieces::{ENABLED, MAX_Z, PLACED},
    GpuPieceState, PieceDataStore,
};
use bevy::math::Vec2;
use puzzella_core::{
    session::{AuthorityCursor, ImageHash, SessionDefinition, SessionId},
    PieceId, PuzzleDefinition,
};
use serde::{Deserialize, Serialize};

pub const SNAPSHOT_SCHEMA_VERSION: u16 = 2;
pub const SNAPSHOT_PLACED: u32 = 1;

/// Dense row-major state, 16 bytes per piece. Only PLACED is a snapshot flag.
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
}
impl std::fmt::Display for SnapshotError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Snapshot rejected: {self:?}")
    }
}
impl std::error::Error for SnapshotError {}

impl GameSnapshot {
    /// Explicit checkpoint only: O(N), with no per-piece entity/map allocation.
    pub fn capture(
        store: &PieceDataStore,
        definition: &PuzzleDefinition,
        session: SessionDefinition,
        cursor: AuthorityCursor,
    ) -> Result<Self, SnapshotError> {
        // Validate before counting/indexing an untrusted definition.
        definition
            .validate()
            .map_err(SnapshotError::InvalidDefinition)?;
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
                .map(|state| SnapshotPieceState {
                    position: state.position,
                    z_order: state.z_order,
                    flags: if state.flags & PLACED != 0 {
                        SNAPSHOT_PLACED
                    } else {
                        0
                    },
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
        for (index, state) in self.pieces.iter().enumerate() {
            let id = PieceId(index as u32);
            if !state.position.is_finite() {
                return Err(SnapshotError::NonFinitePosition(id));
            }
            if state.z_order >= self.next_z_order || state.z_order > MAX_Z {
                return Err(SnapshotError::InvalidZOrder(id));
            }
            if state.flags & !SNAPSHOT_PLACED != 0 {
                return Err(SnapshotError::InvalidFlags(id));
            }
        }
        Ok(())
    }

    /// Validate completely before changing any local state. ENABLED is reconstructed.
    /// Resets holds, selections, cached highlights and dirty ranges; advances GPU epoch.
    pub fn install(
        &self,
        store: &mut PieceDataStore,
        expected: SnapshotExpectation<'_>,
    ) -> Result<(), SnapshotError> {
        self.validate(expected)?;
        let states = self
            .pieces
            .iter()
            .map(|state| GpuPieceState {
                position: state.position,
                z_order: state.z_order,
                flags: ENABLED
                    | if state.flags & SNAPSHOT_PLACED != 0 {
                        PLACED
                    } else {
                        0
                    },
            })
            .collect();
        store.replace_snapshot_states(states, self.next_z_order);
        Ok(())
    }
}
