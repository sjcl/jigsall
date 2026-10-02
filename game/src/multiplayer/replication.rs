//! Explicit transport-independent peer application. No scheduled/idle work.
use crate::{
    multiplayer::{GameSnapshot, SnapshotError, SnapshotExpectation},
    resources::{pieces::AppliedCommand, PieceDataStore},
};
use bevy::math::Vec2;
use puzzella_core::{
    protocol::{
        ActiveDragTarget, ProtocolAuthorityEvent, ProtocolAuthorityEventEnvelope, RemoteDragUpdate,
        ResolvedPieceTarget,
    },
    session::{AuthorityEpoch, AuthoritySession, CommandSequenceStatus, ProtocolError, SessionId},
    PlayerId, PuzzleDefinition,
};
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplicationError {
    Protocol(ProtocolError),
    InvalidDelta,
    MissingDragContext,
    WrongDragContext,
    DuplicateUpdate,
    StaleUpdate,
    /// Gameplay may already have replayed; stop until a trusted snapshot restores
    /// the store. The failed reliable cursor was NOT recorded. No full-state clone.
    Diverged,
    Snapshot(SnapshotError),
}

#[derive(Clone, Debug, PartialEq)]
pub struct RemoteDrag {
    pub grab_sequence: u64,
    pub target: ActiveDragTarget,
    pub delta: Vec2,
    pub last_tick: Option<u64>,
}

#[derive(Default, Debug)]
pub struct PeerReplicationState {
    scope: Option<(SessionId, AuthorityEpoch, u64)>,
    remote_drags: HashMap<PlayerId, RemoteDrag>,
    diverged: bool,
}

impl PeerReplicationState {
    fn scope(
        session: &AuthoritySession,
        store: &PieceDataStore,
    ) -> (SessionId, AuthorityEpoch, u64) {
        (session.session_id(), session.cursor().epoch, store.epoch)
    }

    fn synchronize(&mut self, session: &AuthoritySession, store: &PieceDataStore) {
        let scope = Self::scope(session, store);
        if self.scope != Some(scope) {
            self.remote_drags.clear();
            self.diverged = false;
            self.scope = Some(scope);
        }
    }

    /// Scope/freeze checks are O(1); reading presentation never visits pieces.
    pub fn remote_drag(
        &self,
        session: &AuthoritySession,
        store: &PieceDataStore,
        player: PlayerId,
    ) -> Option<&RemoteDrag> {
        if session.is_active() && !self.diverged && self.scope == Some(Self::scope(session, store))
        {
            self.remote_drags.get(&player)
        } else {
            None
        }
    }

    pub fn remote_drags<'a>(
        &'a self,
        session: &AuthoritySession,
        store: &PieceDataStore,
    ) -> impl Iterator<Item = (PlayerId, &'a RemoteDrag)> {
        let visible = session.is_active()
            && !self.diverged
            && self.scope == Some(Self::scope(session, store));
        self.remote_drags
            .iter()
            .filter(move |_| visible)
            .map(|(&player, drag)| (player, drag))
    }

    pub fn needs_resync(&self, session: &AuthoritySession, store: &PieceDataStore) -> bool {
        self.scope == Some(Self::scope(session, store)) && self.diverged
    }

    /// Explicit externally coordinated disconnect/timeout, without translation or
    /// snap. The authority and all replicas must cancel the same player.
    pub fn cancel_player(&mut self, store: &mut PieceDataStore, player: PlayerId) {
        self.remote_drags.remove(&player);
        store.clear_player_holds(player);
    }

    /// Authenticate the backend sender separately from the wire host claim.
    /// Validate session/epoch/host/cursor BEFORE gameplay; record only on success.
    pub fn apply_event(
        &mut self,
        session: &mut AuthoritySession,
        store: &mut PieceDataStore,
        authenticated_host: PlayerId,
        envelope: &ProtocolAuthorityEventEnvelope,
        definition: Option<&PuzzleDefinition>,
    ) -> Result<AppliedCommand, ReplicationError> {
        self.synchronize(session, store);
        session
            .validate_event(envelope)
            .map_err(ReplicationError::Protocol)?;
        if authenticated_host != session.host() {
            return Err(ReplicationError::Protocol(ProtocolError::WrongHost));
        }
        if self.diverged {
            return Err(ReplicationError::Diverged);
        }
        match self.apply_gameplay(store, &envelope.event, definition) {
            Ok(applied) => {
                session
                    .record_applied_event(envelope)
                    .map_err(ReplicationError::Protocol)?;
                Ok(applied)
            }
            Err(error) => {
                self.diverged = true;
                Err(error)
            }
        }
    }

    fn apply_gameplay(
        &mut self,
        store: &mut PieceDataStore,
        event: &ProtocolAuthorityEvent,
        definition: Option<&PuzzleDefinition>,
    ) -> Result<AppliedCommand, ReplicationError> {
        match event {
            ProtocolAuthorityEvent::RotationCommitted(commit) => {
                let definition = definition.ok_or(ReplicationError::Diverged)?;
                if !(0..4).contains(&commit.quarter_turns)
                    || !store.can_rotate_target(&commit.accepted, commit.quarter_turns, definition)
                {
                    return Err(ReplicationError::Diverged);
                }
                let rotation = store
                    .rotate_target(&commit.accepted, commit.quarter_turns, definition)
                    .map_err(|_| ReplicationError::Diverged)?;
                let result = super::release::result_fingerprint(
                    store,
                    &rotation.roots,
                    Some(definition),
                    &rotation.applied,
                );
                if result != commit.result {
                    return Err(ReplicationError::Diverged);
                }
                Ok(rotation.applied)
            }
            ProtocolAuthorityEvent::GrabAccepted(ack) => {
                if self.remote_drags.contains_key(&ack.player) {
                    return Err(ReplicationError::Diverged);
                }
                let resolved = ack
                    .accepted
                    .resolve(&store.connectivity)
                    .map_err(|_| ReplicationError::Diverged)?;
                if !resolved.rejected.is_empty() {
                    return Err(ReplicationError::Diverged);
                }
                let (target, applied) = match resolved.target {
                    ResolvedPieceTarget::Sparse(refs) => {
                        // This is a consistency assertion, never partial acceptance.
                        // If even one accepted member contradicts the event, abort
                        // the entire operation BEFORE mutating ownership/Z/selection.
                        if !refs.iter().all(|r| {
                            store
                                .connectivity
                                .iter_component(r.member)
                                .all(|id| store.is_selectable(id))
                        }) {
                            return Err(ReplicationError::Diverged);
                        }
                        let applied = store
                            .grab_authority_components(ack.player, refs.iter().map(|r| r.member));
                        (ActiveDragTarget::Sparse(refs), applied)
                    }
                    ResolvedPieceTarget::Dense(members) => {
                        if !members.iter().all(|id| store.is_selectable(id)) {
                            return Err(ReplicationError::Diverged);
                        }
                        let target =
                            ActiveDragTarget::from_accepted_members(&store.connectivity, &members)
                                .map_err(|_| ReplicationError::Diverged)?;
                        let applied = store.grab_accepted_members(ack.player, &members);
                        (target, applied)
                    }
                };
                if applied.grabbed != 0 {
                    self.remote_drags.insert(
                        ack.player,
                        RemoteDrag {
                            grab_sequence: ack.grab_sequence,
                            target,
                            delta: Vec2::ZERO,
                            last_tick: None,
                        },
                    );
                }
                Ok(applied)
            }
            ProtocolAuthorityEvent::ReleaseCommitted(commit) => {
                if !commit.final_delta.is_finite() {
                    return Err(ReplicationError::Diverged);
                }
                let drag = self
                    .remote_drags
                    .get(&commit.player)
                    .ok_or(ReplicationError::MissingDragContext)?;
                if drag.grab_sequence != commit.grab_sequence {
                    return Err(ReplicationError::WrongDragContext);
                }
                // Same target resolution, release_roots, connected snap, board
                // priority and rounded closure used by the authority adapter.
                let (applied, _, result) = super::release::release_drag(
                    store,
                    commit.player,
                    &drag.target,
                    commit.final_delta,
                    definition,
                )
                .map_err(|_| ReplicationError::Diverged)?;
                if result != commit.result {
                    return Err(ReplicationError::Diverged);
                }
                self.remote_drags.remove(&commit.player);
                Ok(applied)
            }
        }
    }

    /// Absolute presentation delta with independent ticks. No membership resolve,
    /// masks, piece-state writes, dirty flags or authority cursor mutation.
    pub fn apply_drag_update(
        &mut self,
        session: &AuthoritySession,
        store: &PieceDataStore,
        authenticated_host: PlayerId,
        update: &RemoteDragUpdate,
    ) -> Result<CommandSequenceStatus, ReplicationError> {
        self.synchronize(session, store);
        if !session.is_active() {
            return Err(ReplicationError::Protocol(ProtocolError::Frozen));
        }
        if update.session != session.session_id() {
            return Err(ReplicationError::Protocol(ProtocolError::WrongSession));
        }
        if update.authority_epoch != session.cursor().epoch {
            return Err(ReplicationError::Protocol(ProtocolError::WrongEpoch));
        }
        if authenticated_host != session.host() {
            return Err(ReplicationError::Protocol(ProtocolError::WrongHost));
        }
        if self.diverged {
            return Err(ReplicationError::Diverged);
        }
        if !update.delta.is_finite() {
            return Err(ReplicationError::InvalidDelta);
        }
        let drag = self
            .remote_drags
            .get_mut(&update.player)
            .ok_or(ReplicationError::MissingDragContext)?;
        if drag.grab_sequence != update.grab_sequence {
            return Err(ReplicationError::WrongDragContext);
        }
        if let Some(last) = drag.last_tick {
            if update.tick == last {
                return Err(ReplicationError::DuplicateUpdate);
            }
            if update.tick < last {
                return Err(ReplicationError::StaleUpdate);
            }
        }
        let expected = drag
            .last_tick
            .map_or(Some(0), |tick| tick.checked_add(1))
            .ok_or(ReplicationError::Protocol(ProtocolError::CounterExhausted))?;
        drag.last_tick = Some(update.tick);
        drag.delta = update.delta;
        Ok(if update.tick == expected {
            CommandSequenceStatus::InOrder
        } else {
            CommandSequenceStatus::Gap { expected }
        })
    }

    /// Trusted resync baseline. Snapshots omit holds/drag contexts; the
    /// backend must coordinate cancellation or a new epoch before continuing an
    /// in-flight drag. Snapshot validation completes before changing session/context.
    pub fn install_snapshot(
        &mut self,
        session: &mut AuthoritySession,
        store: &mut PieceDataStore,
        snapshot: &GameSnapshot,
        expected: SnapshotExpectation<'_>,
    ) -> Result<(), ReplicationError> {
        if !session.is_active() {
            return Err(ReplicationError::Protocol(ProtocolError::Frozen));
        }
        if expected.session != session.session_id() || expected.image_hash != session.image_hash() {
            return Err(ReplicationError::Protocol(ProtocolError::WrongSession));
        }
        if expected.cursor.epoch != session.cursor().epoch || expected.cursor < session.cursor() {
            return Err(ReplicationError::Protocol(
                ProtocolError::WrongSnapshotCursor,
            ));
        }
        snapshot
            .install(store, expected)
            .map_err(ReplicationError::Snapshot)?;
        *session = AuthoritySession::new(
            session.session_definition(),
            session.host(),
            expected.cursor,
        );
        self.synchronize(session, store);
        Ok(())
    }
}

#[cfg(test)]
#[path = "replication_tests.rs"]
mod tests;
