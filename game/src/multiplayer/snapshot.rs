//! Schema 3 keeps its serialized field order and representation unchanged.
pub use crate::checkpoint::{
    CheckpointError as SnapshotError, SnapshotPieceState, SNAPSHOT_CONNECTED_DOWN,
    SNAPSHOT_CONNECTED_RIGHT, SNAPSHOT_PLACED,
};
use crate::{
    checkpoint::{CheckpointView, PuzzleCheckpoint},
    resources::PieceDataStore,
};
use puzzella_core::{
    session::{AuthorityCursor, ImageHash, SessionDefinition, SessionId},
    PuzzleDefinition,
};
use serde::{Deserialize, Serialize};
pub const SNAPSHOT_SCHEMA_VERSION: u16 = 3;
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
impl GameSnapshot {
    pub fn capture(
        store: &PieceDataStore,
        definition: &PuzzleDefinition,
        session: SessionDefinition,
        cursor: AuthorityCursor,
    ) -> Result<Self, SnapshotError> {
        let checkpoint = PuzzleCheckpoint::capture(store, definition, session.image_hash)?;
        Ok(Self {
            schema_version: SNAPSHOT_SCHEMA_VERSION,
            session: session.id,
            image_hash: checkpoint.image_hash,
            cursor,
            definition: checkpoint.definition,
            next_z_order: checkpoint.next_z_order,
            pieces: checkpoint.pieces,
        })
    }
    pub fn validate(&self, expected: SnapshotExpectation<'_>) -> Result<(), SnapshotError> {
        self.check_expectation(expected)?;
        self.view().validated_connectivity().map(|_| ())
    }
    fn check_expectation(&self, expected: SnapshotExpectation<'_>) -> Result<(), SnapshotError> {
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
        Ok(())
    }
    fn view(&self) -> CheckpointView<'_> {
        CheckpointView {
            definition: &self.definition,
            next_z_order: self.next_z_order,
            pieces: &self.pieces,
        }
    }
    pub fn install(
        &self,
        store: &mut PieceDataStore,
        expected: SnapshotExpectation<'_>,
    ) -> Result<(), SnapshotError> {
        self.check_expectation(expected)?;
        self.view().install(store)
    }
    pub fn into_checkpoint(self) -> PuzzleCheckpoint {
        PuzzleCheckpoint {
            image_hash: self.image_hash,
            definition: self.definition,
            next_z_order: self.next_z_order,
            pieces: self.pieces,
        }
    }
}
