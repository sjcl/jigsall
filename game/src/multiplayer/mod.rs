//! Opt-in multiplayer foundation. No systems are added to the single-player schedule.
pub mod snapshot;
pub use snapshot::{
    GameSnapshot, SnapshotError, SnapshotExpectation, SnapshotPieceState, SNAPSHOT_PLACED,
    SNAPSHOT_SCHEMA_VERSION,
};

use crate::resources::{pieces::HELD, PieceDataStore};
use puzzella_core::{
    session::{AuthorityCursor, AuthoritySession, ProtocolError},
    PieceId, PlayerId, PuzzleDefinition,
};

/// Scans sparse holds and sorts released IDs. Does not snap or move.
/// Touches only this player's holds; an ordinary disconnect keeps other players' holds.
pub fn release_player_holds(store: &mut PieceDataStore, player: PlayerId) -> Vec<PieceId> {
    let mut ids: Vec<_> = store
        .held_by
        .iter()
        .filter_map(|(&id, &holder)| (holder == player).then_some(id))
        .collect();
    ids.sort_unstable();
    for &id in &ids {
        store.held_by.remove(&id);
        if let Some(state) = store.states.get_mut(id.0 as usize) {
            state.flags &= !HELD;
            store.dirty_pieces.insert(id);
        }
    }
    ids
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
                cursor: snapshot.cursor, // Already matched the trusted recovery source above.
                definition,
            },
        )
        .map_err(MigrationInstallError::Snapshot)?;
    *session = resumed;
    Ok(cursor)
}

#[cfg(test)]
mod tests;
