//! Opt-in multiplayer foundation. No systems are added to the single-player schedule.
pub mod baseline;
pub mod catch_up;
pub mod finalization;
pub mod protocol;
mod release;
pub mod replication;
pub mod snapshot;
pub use baseline::{
    BaselineDrag, JoinBaseline, JoinBaselineError, JOIN_BASELINE_SCHEMA_VERSION, MAX_BASELINE_DRAGS,
};
pub use catch_up::{
    CatchUpError, CatchUpLimits, CatchUpRestartReason, JoinCatchUpCoordinator, JoinCatchUpPhase,
    JoinCatchUpStart, JoinCatchUpStatus, MAX_CATCH_UP_EVENTS, MAX_CATCH_UP_RETAINED_BYTES,
    MAX_PENDING_JOIN_SYNCS,
};
pub use snapshot::{
    GameSnapshot, SnapshotError, SnapshotExpectation, SnapshotPieceState, SNAPSHOT_CONNECTED_DOWN,
    SNAPSHOT_CONNECTED_RIGHT, SNAPSHOT_PLACED, SNAPSHOT_SCHEMA_VERSION,
};

use crate::resources::PieceDataStore;
use jigsall_core::{
    session::{AuthorityCursor, AuthoritySession, ProtocolError},
    PieceId, PlayerId, PuzzleDefinition,
};

/// Expands this player's holds into complete components. Does not snap or move.
/// Unreplicated emergency migration/repair cleanup; normal active-session
/// disconnects use ProtocolDragContexts::cancel_replicated and peer event replay.
/// Touches only this player's holds.
pub fn release_player_holds(store: &mut PieceDataStore, player: PlayerId) -> Vec<PieceId> {
    store.clear_player_holds(player)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MigrationInstallError {
    Protocol(ProtocolError),
    Snapshot(SnapshotError),
}
impl std::fmt::Display for MigrationInstallError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Migration rejected: {self:?}")
    }
}
impl std::error::Error for MigrationInstallError {}

/// Transactional activation: validate transition and snapshot, restore all pieces,
/// then publish the new host/epoch. Any rejection leaves both session and store intact.
/// The backend must authenticate snapshot delivery as the selected recovery source.
/// Trusted peers supply the cursor/content; this does not prove old-host provenance.
pub fn install_migration_snapshot(
    session: &mut AuthoritySession,
    store: &mut PieceDataStore,
    definition: &PuzzleDefinition,
    snapshot: &GameSnapshot,
) -> Result<AuthorityCursor, MigrationInstallError> {
    let mut resumed = session.clone();
    let cursor = resumed
        .complete_migration(snapshot.session, snapshot.cursor)
        .map_err(MigrationInstallError::Protocol)?;
    snapshot
        .install(
            store,
            SnapshotExpectation {
                session: session.session_id(),
                image_hash: session.image_hash(),
                cursor: snapshot.cursor, // Already matched the trusted recovery source above.
                definition,
            },
        )
        .map_err(MigrationInstallError::Snapshot)?;
    *session = resumed;
    Ok(cursor)
}

#[cfg(test)]
mod connected_tests;
#[cfg(test)]
mod tests;
