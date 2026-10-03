//! Game-layer adapter: accepted network contexts -> renderer-neutral bounded cache.
use super::*;
use crate::resources::remote_drag::RemoteDragPresentation;
use puzzella_core::PieceBitSet;
use std::collections::HashMap;

#[derive(Default)]
pub(super) struct RemotePresentationBridge {
    pub state: RemoteDragPresentation,
    scope: Option<(SessionId, AuthorityEpoch, u64)>,
    slots: HashMap<PlayerId, u32>,
}
impl RemotePresentationBridge {
    pub fn initialize<'a>(
        &mut self,
        local: PlayerId,
        session: &AuthoritySession,
        store: &PieceDataStore,
        drags: impl Iterator<Item = (PlayerId, &'a ActiveDrag)>,
    ) {
        self.synchronize(session, store);
        for (player, drag) in drags {
            self.grab(local, player, drag, store);
        }
    }
    pub fn synchronize(&mut self, session: &AuthoritySession, store: &PieceDataStore) {
        let scope = (session.session_id(), session.cursor().epoch, store.epoch);
        if self.scope != Some(scope) || self.state.epoch != store.epoch {
            self.state.reset(store.epoch, store.len());
            self.slots.clear();
            self.scope = Some(scope);
        }
    }
    pub fn grab(
        &mut self,
        local: PlayerId,
        player: PlayerId,
        drag: &ActiveDrag,
        store: &PieceDataStore,
    ) {
        if player == local {
            return;
        }
        assert!(!self.slots.contains_key(&player));
        // Retain exact membership for clearing even if Release snaps/merges DSU.
        // Dense authority masks share their Arc; sparse components expand only here.
        let members = match &drag.target {
            ActiveDragTarget::Dense(target) => target.members.clone(),
            ActiveDragTarget::Sparse(refs) => {
                let mut members = PieceBitSet::new(store.len());
                for reference in refs {
                    members.extend(store.connectivity.iter_component(reference.member));
                }
                members
            }
        };
        if let Some(slot) = self.state.allocate(members, drag.delta) {
            self.slots.insert(player, slot);
        }
    }
    pub fn event(
        &mut self,
        local: PlayerId,
        event: &ProtocolAuthorityEvent,
        drag: Option<&ActiveDrag>,
        store: &PieceDataStore,
    ) {
        match event {
            ProtocolAuthorityEvent::GrabAccepted(ack) => {
                if let Some(drag) = drag {
                    self.grab(local, ack.player, drag, store);
                }
            }
            ProtocolAuthorityEvent::DragRotationCommitted(commit) => {
                if let Some(drag) = drag {
                    self.delta(commit.player, drag.delta);
                }
            }
            ProtocolAuthorityEvent::ReleaseCommitted(commit) => self.release(commit.player),
            ProtocolAuthorityEvent::DragCancelled(cancel) => self.release(cancel.player),
            ProtocolAuthorityEvent::RotationCommitted(_) => {}
        }
    }
    pub fn delta(&mut self, player: PlayerId, delta: Vec2) {
        if let Some(&slot) = self.slots.get(&player) {
            self.state.set_delta(slot, delta);
        }
    }
    fn release(&mut self, player: PlayerId) {
        if let Some(slot) = self.slots.remove(&player) {
            self.state.release(slot);
        }
    }
}

pub(super) fn event_player(event: &ProtocolAuthorityEvent) -> PlayerId {
    match event {
        ProtocolAuthorityEvent::GrabAccepted(e) => e.player,
        ProtocolAuthorityEvent::ReleaseCommitted(e) => e.player,
        ProtocolAuthorityEvent::DragRotationCommitted(e) => e.player,
        ProtocolAuthorityEvent::DragCancelled(e) => e.player,
        ProtocolAuthorityEvent::RotationCommitted(e) => e.player,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multiplayer::{finalization::FinalDragSet, replication::ReplicationError};
    use puzzella_core::{PieceId, GENERATOR_VERSION};
    #[test]
    fn remote_presentation_final_reconciliation_rebase_and_late_transients() {
        assert_eq!(
            crate::resources::remote_drag::REMOTE_DRAG_SLOTS,
            crate::multiplayer::MAX_BASELINE_DRAGS
        );
        let host = PlayerId(0);
        let remote = PlayerId(1);
        let local = PlayerId(2);
        let definition = PuzzleDefinition {
            generator_version: GENERATOR_VERSION,
            seed: 42,
            grid_size: UVec2::ONE,
            image_size: UVec2::splat(20),
            snap_distance: 0.01,
        };
        let mut store = PieceDataStore::default();
        store.initialize(vec![Vec2::splat(100.0)]);
        let mut peer_store = PieceDataStore::default();
        peer_store.initialize(vec![Vec2::splat(100.0)]);
        let session_definition = SessionDefinition {
            id: SessionId(10),
            image_hash: ImageHash([0; 32]),
        };
        let mut session =
            AuthoritySession::new(session_definition, host, AuthorityCursor::new(0, 0));
        let mut peer_session =
            AuthoritySession::new(session_definition, host, AuthorityCursor::new(0, 0));
        let mut contexts = ProtocolDragContexts::default();
        let mut replica = PeerReplicationState::default();
        let command = |sequence, command| ProtocolCommandEnvelope {
            session: session_definition.id,
            authority_epoch: AuthorityEpoch(0),
            player: remote,
            sequence,
            command,
        };
        let grab = contexts
            .apply_replicated(
                &mut session,
                &mut store,
                remote,
                &command(
                    ClientCommandSequence::Control(0),
                    ProtocolPieceCommand::Grab {
                        target: PieceTarget::Component(ComponentRef {
                            member: PieceId(0),
                            expected_size: 1,
                        }),
                    },
                ),
                Some(&definition),
                host,
            )
            .unwrap();
        replica
            .apply_event(
                &mut peer_session,
                &mut peer_store,
                host,
                grab.authority_event.as_ref().unwrap(),
                Some(&definition),
                local,
            )
            .unwrap();
        let update = contexts
            .apply_replicated(
                &mut session,
                &mut store,
                remote,
                &command(
                    ClientCommandSequence::Move {
                        after_control_sequence: 0,
                        tick: 3,
                    },
                    ProtocolPieceCommand::DragUpdate {
                        delta: Vec2::new(12.0, 34.0),
                    },
                ),
                Some(&definition),
                host,
            )
            .unwrap()
            .drag_update
            .unwrap();
        let mut ahead = update.clone();
        ahead.tick = 99;
        ahead.delta = Vec2::splat(999.0);
        replica
            .apply_drag_update(&peer_session, &peer_store, host, &ahead)
            .unwrap();
        let final_drags = FinalDragSet::capture(&contexts, &session, &store).unwrap();
        replica
            .reconcile_final_drags(&peer_session, &peer_store, &final_drags)
            .unwrap();
        let mut presentation = RemotePresentationBridge::default();
        presentation.initialize(
            local,
            &peer_session,
            &peer_store,
            replica.remote_drags(&peer_session, &peer_store),
        );
        assert_eq!(presentation.state.offset(PieceId(0)), update.delta);
        let rotated = contexts
            .apply_replicated(
                &mut session,
                &mut store,
                remote,
                &command(
                    ClientCommandSequence::Control(1),
                    ProtocolPieceCommand::RotateDrag {
                        grab_sequence: 0,
                        final_delta: update.delta,
                        through_tick: Some(3),
                        quarter_turns: 1,
                    },
                ),
                Some(&definition),
                host,
            )
            .unwrap();
        let event = &rotated.authority_event.unwrap();
        replica
            .apply_event(
                &mut peer_session,
                &mut peer_store,
                host,
                event,
                Some(&definition),
                local,
            )
            .unwrap();
        presentation.event(
            local,
            &event.event,
            replica.remote_drag(&peer_session, &peer_store, remote),
            &peer_store,
        );
        assert_eq!(presentation.state.offset(PieceId(0)), Vec2::ZERO);
        assert_eq!(
            replica.apply_drag_update(&peer_session, &peer_store, host, &ahead),
            Err(ReplicationError::WrongDragContext)
        );
        assert_eq!(presentation.state.offset(PieceId(0)), Vec2::ZERO);
        let cancel = contexts
            .cancel_replicated(&mut session, &mut store, remote)
            .unwrap()
            .unwrap();
        replica
            .apply_event(
                &mut peer_session,
                &mut peer_store,
                host,
                &cancel.authority_event,
                Some(&definition),
                local,
            )
            .unwrap();
        presentation.event(local, &cancel.authority_event.event, None, &peer_store);
        let mut late = ahead;
        late.basis_sequence = 1;
        assert_eq!(
            replica.apply_drag_update(&peer_session, &peer_store, host, &late),
            Err(ReplicationError::MissingDragContext)
        );
        assert_eq!(presentation.state.offset(PieceId(0)), Vec2::ZERO);
        assert!(presentation.slots.is_empty());
    }
}
