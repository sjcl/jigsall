//! Release schema 1 preserves the pre-release schema 5 layout and 16-byte pieces.
pub use crate::checkpoint::{
    CheckpointError as SnapshotError, SnapshotPieceState, SNAPSHOT_CONNECTED_DOWN,
    SNAPSHOT_CONNECTED_RIGHT, SNAPSHOT_PLACED,
};
use crate::{
    checkpoint::{CheckpointView, PuzzleCheckpoint},
    resources::PieceDataStore,
};
use jigsall_core::{
    session::{AuthorityCursor, ImageHash, SessionDefinition, SessionId},
    PieceConnectivity, PuzzleDefinition, MAX_PIECES,
};
use serde::{Deserialize, Serialize};
pub const SNAPSHOT_SCHEMA_VERSION: u16 = 1;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GameSnapshot {
    pub schema_version: u16,
    pub session: SessionId,
    pub image_hash: ImageHash,
    pub cursor: AuthorityCursor,
    pub definition: PuzzleDefinition,
    pub next_z_order: u32,
    #[serde(deserialize_with = "deserialize_snapshot_pieces")]
    pub pieces: Vec<SnapshotPieceState>,
}
fn deserialize_snapshot_pieces<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<SnapshotPieceState>, D::Error> {
    struct Pieces;
    impl<'de> serde::de::Visitor<'de> for Pieces {
        type Value = Vec<SnapshotPieceState>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "at most {MAX_PIECES} snapshot pieces")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            // Never reserve from an untrusted size_hint; grow only after real decodes.
            // Exact definition.piece_count() is checked separately by CheckpointView.
            let mut pieces = Vec::new();
            while pieces.len() < MAX_PIECES {
                let Some(piece) = seq.next_element()? else {
                    return Ok(pieces);
                };
                pieces.push(piece);
            }
            // Probe for overflow without decoding/retaining another full piece state.
            if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                return Err(serde::de::Error::custom("Too many snapshot pieces"));
            }
            Ok(pieces)
        }
    }
    deserializer.deserialize_seq(Pieces)
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
    /// Captures committed state even during local or remote drags, without
    /// changing gameplay. Call between complete authority command applications,
    /// pairing this borrowed store with its last applied cursor.
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
        self.validated_connectivity(expected).map(|_| ())
    }
    pub(crate) fn validated_connectivity(
        &self,
        expected: SnapshotExpectation<'_>,
    ) -> Result<PieceConnectivity, SnapshotError> {
        self.check_expectation(expected)?;
        self.view().validated_connectivity()
    }
    /// Commit only after validating this snapshot and any join overlay.
    pub(crate) fn install_with_validated_connectivity(
        &self,
        store: &mut PieceDataStore,
        connectivity: PieceConnectivity,
    ) {
        self.view()
            .install_with_validated_connectivity(store, connectivity);
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
        let connectivity = self.validated_connectivity(expected)?;
        self.install_with_validated_connectivity(store, connectivity);
        Ok(())
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

#[cfg(test)]
#[path = "snapshot_tests.rs"]
mod tests;
