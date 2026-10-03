//! Same-epoch join state. Saves and migration continue to use canonical snapshots.
use super::{
    protocol::ProtocolDragContexts, GameSnapshot, SnapshotError, SnapshotExpectation,
    SNAPSHOT_PLACED,
};
use crate::resources::{
    pieces::{HELD, PLACED},
    PieceDataStore,
};
use bevy::math::Vec2;
use puzzella_core::{
    protocol::{
        ActiveDrag, ActiveDragTarget, DenseTarget, PieceTarget, ResolvedPieceTarget, TargetError,
    },
    session::{AuthoritySession, ProtocolError},
    PieceBitSet, PieceConnectivity, PieceId, PlayerId, PuzzleDefinition,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const JOIN_BASELINE_SCHEMA_VERSION: u16 = 1;
/// Matches the current session/transport connection cap. Checked while decoding
/// and again for direct Rust callers, before allocating validation scratch.
pub const MAX_BASELINE_DRAGS: usize = 64;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JoinBaseline {
    pub schema_version: u16,
    pub snapshot: GameSnapshot,
    #[serde(deserialize_with = "deserialize_drags")]
    pub active_drags: Vec<BaselineDrag>,
}

/// Wire/storage representation, separate from authority-created ActiveDrag.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BaselineDrag {
    pub player: PlayerId,
    pub grab_sequence: u64,
    pub basis_sequence: u64,
    pub last_tick: Option<u64>,
    pub target: PieceTarget,
    pub delta: Vec2,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JoinBaselineError {
    UnsupportedSchema(u16),
    Protocol(ProtocolError),
    Snapshot(SnapshotError),
    TooManyActiveDrags,
    DuplicatePlayer(PlayerId),
    InvalidDelta(PlayerId),
    InvalidSequence(PlayerId),
    EmptyTarget(PlayerId),
    InvalidTarget {
        player: PlayerId,
        reason: TargetError,
    },
    /// A dense overlay must name whole components, never silently grow a mask.
    NonCanonicalTarget(PlayerId),
    OverlappingTarget(PieceId),
    PlacedTarget(PieceId),
    HoldMismatch(PieceId),
    OwnerMismatch {
        piece: PieceId,
        player: PlayerId,
    },
    MissingDragContext(PieceId),
}
impl std::fmt::Display for JoinBaselineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Join baseline rejected: {self:?}")
    }
}
impl std::error::Error for JoinBaselineError {}

fn deserialize_drags<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<BaselineDrag>, D::Error> {
    struct Drags;
    impl<'de> serde::de::Visitor<'de> for Drags {
        type Value = Vec<BaselineDrag>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "at most {MAX_BASELINE_DRAGS} baseline drags")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut drags =
                Vec::with_capacity(seq.size_hint().unwrap_or(0).min(MAX_BASELINE_DRAGS));
            // At the cap probe without decoding/allocating another full drag target.
            while drags.len() < MAX_BASELINE_DRAGS {
                let Some(drag) = seq.next_element()? else {
                    return Ok(drags);
                };
                drags.push(drag);
            }
            if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                return Err(serde::de::Error::custom("Too many baseline drags"));
            }
            Ok(drags)
        }
    }
    deserializer.deserialize_seq(Drags)
}

/// Operation-local, validated representation. Dense masks stay COW; sparse
/// targets retain <=32 refs, even when a component contains a million pieces.
pub(crate) struct PreparedJoinBaseline {
    pub connectivity: PieceConnectivity,
    pub drags: Vec<(PlayerId, ActiveDrag)>,
}

impl JoinBaseline {
    /// Borrow all authority state together between complete apply_replicated calls.
    /// Captures the current cursor without waiting, releasing, cancelling or rebasing.
    pub fn capture(
        session: &AuthoritySession,
        store: &PieceDataStore,
        contexts: &ProtocolDragContexts,
        definition: &PuzzleDefinition,
    ) -> Result<Self, JoinBaselineError> {
        if !session.is_active() {
            return Err(JoinBaselineError::Protocol(ProtocolError::Frozen));
        }
        let mut active_drags = Vec::new();
        for (player, drag) in contexts.active_drags(session, store) {
            if active_drags.len() == MAX_BASELINE_DRAGS {
                return Err(JoinBaselineError::TooManyActiveDrags);
            }
            active_drags.push(BaselineDrag {
                player,
                grab_sequence: drag.grab_sequence,
                basis_sequence: drag.basis_sequence,
                last_tick: drag.last_tick,
                target: drag.target.to_piece_target(),
                delta: drag.delta,
            });
        }
        active_drags.sort_unstable_by_key(|drag| drag.player.0);
        let snapshot = GameSnapshot::capture(
            store,
            definition,
            session.session_definition(),
            session.cursor(),
        )
        .map_err(JoinBaselineError::Snapshot)?;
        let (_, covered) = prepare_drags(&active_drags, &store.connectivity, |player, id| {
            if store.states[id.0 as usize].flags & PLACED != 0 {
                return Err(JoinBaselineError::PlacedTarget(id));
            }
            if store.states[id.0 as usize].flags & HELD == 0 {
                return Err(JoinBaselineError::HoldMismatch(id));
            }
            if store.held_by.get(&id) != Some(&player) {
                return Err(JoinBaselineError::OwnerMismatch { piece: id, player });
            }
            Ok(())
        })?;
        // Rare O(N) audit of flag/owner/context mirrors, including orphan holds.
        for (index, state) in store.states.iter().enumerate() {
            let id = PieceId(index as u32);
            let held = state.flags & HELD != 0;
            if held != store.held_by.get(&id).is_some() {
                return Err(JoinBaselineError::HoldMismatch(id));
            }
            if held && !covered.contains(&id) {
                return Err(JoinBaselineError::MissingDragContext(id));
            }
        }
        // Public PieceOwners mutation can also introduce occupancy outside the store.
        if let Some((id, _)) = store.held_by.iter().find(|(id, _)| !store.contains(*id)) {
            return Err(JoinBaselineError::HoldMismatch(id));
        }
        Ok(Self {
            schema_version: JOIN_BASELINE_SCHEMA_VERSION,
            snapshot,
            active_drags,
        })
    }

    pub(crate) fn prepare(
        &self,
        expected: SnapshotExpectation<'_>,
    ) -> Result<PreparedJoinBaseline, JoinBaselineError> {
        if self.schema_version != JOIN_BASELINE_SCHEMA_VERSION {
            return Err(JoinBaselineError::UnsupportedSchema(self.schema_version));
        }
        if self.active_drags.len() > MAX_BASELINE_DRAGS {
            return Err(JoinBaselineError::TooManyActiveDrags);
        }
        // One DSU, kept through overlay validation and moved into the store at commit.
        let connectivity = self
            .snapshot
            .validated_connectivity(expected)
            .map_err(JoinBaselineError::Snapshot)?;
        let (drags, _) = prepare_drags(&self.active_drags, &connectivity, |_, id| {
            if self.snapshot.pieces[id.0 as usize].flags & SNAPSHOT_PLACED != 0 {
                Err(JoinBaselineError::PlacedTarget(id))
            } else {
                Ok(())
            }
        })?;
        Ok(PreparedJoinBaseline {
            connectivity,
            drags,
        })
    }
}

fn prepare_drags(
    drags: &[BaselineDrag],
    connectivity: &PieceConnectivity,
    mut check_piece: impl FnMut(PlayerId, PieceId) -> Result<(), JoinBaselineError>,
) -> Result<(Vec<(PlayerId, ActiveDrag)>, PieceBitSet), JoinBaselineError> {
    let mut players = BTreeSet::new();
    let mut covered = PieceBitSet::new(connectivity.len());
    let mut prepared = Vec::with_capacity(drags.len());
    for drag in drags {
        if !players.insert(drag.player.0) {
            return Err(JoinBaselineError::DuplicatePlayer(drag.player));
        }
        if !drag.delta.is_finite() {
            return Err(JoinBaselineError::InvalidDelta(drag.player));
        }
        if drag.basis_sequence < drag.grab_sequence {
            return Err(JoinBaselineError::InvalidSequence(drag.player));
        }
        let invalid = |reason| JoinBaselineError::InvalidTarget {
            player: drag.player,
            reason,
        };
        let resolved = drag.target.resolve(connectivity).map_err(invalid)?;
        if let Some(rejected) = resolved.rejected.first() {
            return Err(invalid(rejected.reason));
        }
        let target = match resolved.target {
            ResolvedPieceTarget::Sparse(refs) => ActiveDragTarget::Sparse(refs),
            ResolvedPieceTarget::Dense(members) => {
                let PieceTarget::Dense(dense) = &drag.target else {
                    unreachable!()
                };
                if members != dense.members {
                    return Err(JoinBaselineError::NonCanonicalTarget(drag.player));
                }
                ActiveDragTarget::Dense(DenseTarget {
                    members,
                    component_count: dense.component_count,
                    topology_digest: dense.topology_digest,
                })
            }
        };
        let mut count = 0;
        let mut visit = |id| {
            if !covered.insert(id) {
                return Err(JoinBaselineError::OverlappingTarget(id));
            }
            check_piece(drag.player, id)?;
            count += 1;
            Ok(())
        };
        match &target {
            ActiveDragTarget::Sparse(refs) => {
                for reference in refs {
                    for id in connectivity.iter_component(reference.member) {
                        visit(id)?;
                    }
                }
            }
            ActiveDragTarget::Dense(dense) => {
                for id in dense.members.iter() {
                    visit(id)?;
                }
            }
        }
        if count == 0 {
            return Err(JoinBaselineError::EmptyTarget(drag.player));
        }
        prepared.push((
            drag.player,
            ActiveDrag {
                grab_sequence: drag.grab_sequence,
                basis_sequence: drag.basis_sequence,
                last_tick: drag.last_tick,
                target,
                delta: drag.delta,
            },
        ));
    }
    Ok((prepared, covered))
}

#[cfg(test)]
#[path = "baseline_tests.rs"]
mod tests;
