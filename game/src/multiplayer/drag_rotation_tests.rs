use super::*;
use crate::multiplayer::protocol::ProtocolCommandResult;
use crate::resources::pieces::{ENABLED, HELD};
use bevy::math::Vec2;
use puzzella_core::protocol::*;
use puzzella_core::session::*;

#[test]
fn snapshots_during_protocol_drag_preserve_rotation_rebase_without_committing_later_updates() {
    let mut s = Simulation::new(&[Vec2::splat(1000.); 2], &[(0, 1)]);
    let initial = s.store.states.clone();
    s.grab(A, 0, &[0]);
    s.update(A, 0, 0, Vec2::new(100., 0.));
    let rotation = s.rotate_drag(1, Some(0), Vec2::new(100., 0.), 1);
    s.deliver(&rotation, true);
    let rebased = s.store.states.clone();
    assert_ne!(rebased[0].position, initial[0].position);
    assert_eq!(puzzella_core::decode_rotation(rebased[0].flags), 1);
    let update = s.update(A, 1, 1, Vec2::new(50., 0.));
    let active = s
        .contexts
        .active_drag(&s.session, &s.store, A)
        .unwrap()
        .clone();
    let snapshot =
        GameSnapshot::capture(&s.store, &s.definition, SESSION, s.session.cursor()).unwrap();
    assert_eq!(s.store.states, rebased);
    assert_eq!(
        s.contexts.active_drag(&s.session, &s.store, A),
        Some(&active)
    );
    for (piece, canonical) in snapshot.pieces.iter().zip(rebased.iter()) {
        assert_eq!(piece.position, canonical.position);
        assert_eq!(piece.z_order, canonical.z_order);
        assert_eq!(puzzella_core::decode_rotation(piece.flags), 1);
    }
    for peer in &mut s.peers {
        peer.replica
            .apply_drag_update(&peer.session, &peer.store, HOST, &update)
            .unwrap();
        let remote = peer
            .replica
            .remote_drag(&peer.session, &peer.store, A)
            .unwrap()
            .clone();
        assert_eq!(remote.delta, Vec2::new(50., 0.));
        let peer_snapshot =
            GameSnapshot::capture(&peer.store, &s.definition, SESSION, peer.session.cursor())
                .unwrap();
        assert_eq!(peer_snapshot, snapshot);
        assert_eq!(
            peer.replica.remote_drag(&peer.session, &peer.store, A),
            Some(&remote)
        );
        peer.replica
            .install_snapshot(
                &mut peer.session,
                &mut peer.store,
                &snapshot,
                SnapshotExpectation {
                    session: SESSION.id,
                    image_hash: SESSION.image_hash,
                    cursor: snapshot.cursor,
                    definition: &s.definition,
                },
            )
            .unwrap();
        assert!(peer
            .replica
            .remote_drag(&peer.session, &peer.store, A)
            .is_none());
        assert!(peer.store.held_by.is_empty());
        assert!(peer.store.drag.members.is_empty());
        assert_eq!(peer.store.drag.delta, Vec2::ZERO);
        assert!(peer
            .store
            .connectivity
            .same_component(PieceId(0), PieceId(1)));
        for (piece, canonical) in peer.store.states.iter().zip(rebased.iter()) {
            assert_eq!(piece.position, canonical.position);
            assert_eq!(piece.z_order, canonical.z_order);
            assert_eq!(puzzella_core::decode_rotation(piece.flags), 1);
            assert_eq!(piece.flags & HELD, 0);
        }
        assert_eq!(
            peer.replica
                .apply_drag_update(&peer.session, &peer.store, HOST, &update),
            Err(ReplicationError::MissingDragContext)
        );
    }
}

#[test]
fn drag_rebases_keep_one_grab_and_reject_old_or_future_basis_transients_on_host_and_peers() {
    let mut s = Simulation::new(&[Vec2::splat(1000.); 5], &[(0, 1), (0, 2), (3, 4)]);
    s.peers[0].store.connectivity = puzzella_core::PieceConnectivity::new(5);
    s.peers[0].store.connectivity.union(PieceId(1), PieceId(2));
    s.peers[0].store.connectivity.union(PieceId(1), PieceId(0));
    s.peers[0].store.connectivity.union(PieceId(3), PieceId(4));
    assert_ne!(
        s.store.connectivity.find_root(PieceId(0)),
        s.peers[0].store.connectivity.find_root(PieceId(0))
    );
    s.grab(A, 0, &[0, 3]);
    let old = s.update(A, 0, 10, Vec2::new(20., 30.));
    let future_request = ProtocolCommandEnvelope {
        session: SESSION.id,
        authority_epoch: AuthorityEpoch(3),
        player: A,
        sequence: ClientCommandSequence::Move {
            after_control_sequence: 1,
            tick: 12,
        },
        command: ProtocolPieceCommand::DragUpdate { delta: Vec2::ONE },
    };
    assert_eq!(
        s.contexts.apply(
            &mut s.session,
            &mut s.store,
            A,
            &future_request,
            Some(&s.definition),
            puzzella_core::LOCAL_PLAYER
        ),
        Err(ProtocolCommandError::Sequence(
            ProtocolError::ControlNotProcessed { required: 1 }
        ))
    );
    let peer = &mut s.peers[0];
    peer.replica
        .apply_drag_update(&peer.session, &peer.store, HOST, &old)
        .unwrap();
    let event = s.rotate_drag(1, Some(11), Vec2::new(40., 50.), 1);
    // New-basis packets may overtake reliable events on separate transport lanes.
    let future = RemoteDragUpdate {
        basis_sequence: 1,
        tick: 12,
        delta: Vec2::new(10., 0.),
        ..old.clone()
    };
    for peer in &mut s.peers {
        assert_eq!(
            peer.replica
                .apply_drag_update(&peer.session, &peer.store, HOST, &future),
            Err(ReplicationError::WrongDragContext)
        );
        assert!(!peer.replica.needs_resync(&peer.session, &peer.store));
    }
    s.deliver(&event, true);
    s.assert_equal();
    let target = s
        .contexts
        .active_drag(&s.session, &s.store, A)
        .unwrap()
        .target
        .clone();
    assert_eq!(
        s.contexts
            .active_drag(&s.session, &s.store, A)
            .unwrap()
            .delta,
        Vec2::ZERO
    );
    for peer in &mut s.peers {
        for late in [
            old.clone(),
            RemoteDragUpdate {
                tick: 11,
                ..old.clone()
            },
            RemoteDragUpdate {
                basis_sequence: 1,
                tick: 11,
                ..old.clone()
            },
        ] {
            assert!(peer
                .replica
                .apply_drag_update(&peer.session, &peer.store, HOST, &late)
                .is_err());
            assert_eq!(
                peer.replica
                    .remote_drag(&peer.session, &peer.store, A)
                    .unwrap()
                    .delta,
                Vec2::ZERO
            );
        }
    }
    let request = |basis, tick| ProtocolCommandEnvelope {
        session: SESSION.id,
        authority_epoch: AuthorityEpoch(3),
        player: A,
        sequence: ClientCommandSequence::Move {
            after_control_sequence: basis,
            tick,
        },
        command: ProtocolPieceCommand::DragUpdate {
            delta: Vec2::new(10., 0.),
        },
    };
    assert_eq!(
        s.contexts.apply(
            &mut s.session,
            &mut s.store,
            A,
            &request(0, 11),
            Some(&s.definition),
            puzzella_core::LOCAL_PLAYER
        ),
        Err(ProtocolCommandError::Sequence(
            ProtocolError::StaleMoveContext
        ))
    );
    assert_eq!(
        s.contexts.apply(
            &mut s.session,
            &mut s.store,
            A,
            &request(1, 11),
            Some(&s.definition),
            puzzella_core::LOCAL_PLAYER
        ),
        Err(ProtocolCommandError::Sequence(
            ProtocolError::DuplicateCommand
        ))
    );
    let update = s
        .command(A, request(1, 12).sequence, request(1, 12).command, true)
        .drag_update
        .unwrap();
    for peer in &mut s.peers {
        assert_eq!(
            peer.replica
                .apply_drag_update(&peer.session, &peer.store, HOST, &update)
                .unwrap(),
            CommandSequenceStatus::InOrder
        );
    }
    let event = s.rotate_drag(2, Some(12), Vec2::new(10., 0.), -1);
    s.deliver(&event, true);
    let event = s.rotate_drag(3, Some(12), Vec2::ZERO, 1);
    s.deliver(&event, true);
    let drag = s.contexts.active_drag(&s.session, &s.store, A).unwrap();
    assert_eq!(drag.grab_sequence, 0);
    assert_eq!(drag.target, target);
    assert_eq!(drag.basis_sequence, 3);
    assert_eq!(drag.last_tick, Some(12));
    s.assert_equal();
    s.release(A, 4, 0, Vec2::new(0., 5.), true);
    s.assert_equal();
    assert!(s.store.held_by.is_empty());
}

#[test]
fn dense_drag_rebase_shares_membership_and_rejects_stale_topology_atomically() {
    let mut s = Simulation::new(&[Vec2::splat(1000.); 40], &[]);
    s.grab(A, 0, &(0..40).collect::<Vec<_>>());
    let ActiveDragTarget::Dense(dense) = &s
        .contexts
        .active_drag(&s.session, &s.store, A)
        .unwrap()
        .target
    else {
        panic!("dense context")
    };
    let mask = dense.members.words().clone();
    let event = s.rotate_drag(1, None, Vec2::new(10., 20.), 1);
    s.deliver(&event, true);
    s.assert_equal();
    let ActiveDragTarget::Dense(dense) = &s
        .contexts
        .active_drag(&s.session, &s.store, A)
        .unwrap()
        .target
    else {
        unreachable!()
    };
    assert!(std::sync::Arc::ptr_eq(&mask, dense.members.words()));
    s.store.connectivity.union(PieceId(0), PieceId(1));
    let before = s.store.states.to_vec();
    let drag = s
        .contexts
        .active_drag(&s.session, &s.store, A)
        .unwrap()
        .clone();
    let request = ProtocolCommandEnvelope {
        session: SESSION.id,
        authority_epoch: AuthorityEpoch(3),
        player: A,
        sequence: ClientCommandSequence::Control(2),
        command: ProtocolPieceCommand::RotateDrag {
            grab_sequence: 0,
            final_delta: Vec2::ONE,
            through_tick: None,
            quarter_turns: 1,
        },
    };
    assert_eq!(
        s.contexts.apply(
            &mut s.session,
            &mut s.store,
            A,
            &request,
            Some(&s.definition),
            puzzella_core::LOCAL_PLAYER
        ),
        Err(ProtocolCommandError::InconsistentDragTarget)
    );
    assert_eq!(s.store.states.to_vec(), before);
    assert_eq!(s.contexts.active_drag(&s.session, &s.store, A), Some(&drag));
}

#[test]
fn no_transient_before_first_rotation_and_later_rotations_is_valid() {
    let mut s = Simulation::new(&[Vec2::splat(1000.); 2], &[(0, 1)]);
    s.grab(A, 0, &[0]);
    for sequence in 1..=4 {
        let event = s.rotate_drag(sequence, None, Vec2::ZERO, 1);
        s.deliver(&event, true);
        s.assert_equal();
    }
    let update = s
        .command(
            A,
            ClientCommandSequence::Move {
                after_control_sequence: 4,
                tick: 0,
            },
            ProtocolPieceCommand::DragUpdate { delta: Vec2::ONE },
            true,
        )
        .drag_update
        .unwrap();
    for peer in &mut s.peers {
        assert_eq!(
            peer.replica
                .apply_drag_update(&peer.session, &peer.store, HOST, &update)
                .unwrap(),
            CommandSequenceStatus::InOrder
        );
    }
    s.release(A, 5, 0, Vec2::new(10., 5.), true);
    s.assert_equal();
}

#[test]
fn invalid_drag_rotation_is_atomic_and_rejected_control_keeps_previous_move_basis_usable() {
    for case in 0..11 {
        let mut s = Simulation::new(&[Vec2::splat(1000.); 4], &[(0, 1)]);
        s.grab(A, 0, &[0, 2]);
        s.update(A, 0, 10, Vec2::new(20., 30.));
        let mut command = ProtocolPieceCommand::RotateDrag {
            grab_sequence: 0,
            final_delta: Vec2::new(40., 50.),
            through_tick: Some(10),
            quarter_turns: 1,
        };
        match case {
            0 => {
                command = ProtocolPieceCommand::RotateDrag {
                    grab_sequence: 99,
                    final_delta: Vec2::ZERO,
                    through_tick: Some(10),
                    quarter_turns: 1,
                }
            }
            1 => {
                command = ProtocolPieceCommand::RotateDrag {
                    grab_sequence: 0,
                    final_delta: Vec2::NAN,
                    through_tick: Some(10),
                    quarter_turns: 1,
                }
            }
            2 => {
                command = ProtocolPieceCommand::RotateDrag {
                    grab_sequence: 0,
                    final_delta: Vec2::ZERO,
                    through_tick: Some(9),
                    quarter_turns: 1,
                }
            }
            3 => {
                s.store.connectivity.union(PieceId(2), PieceId(3));
            }
            4 => {
                s.store.held_by.insert(PieceId(1), B);
            }
            5 => {
                s.store.states[1].flags &= !HELD;
            }
            6 => {
                s.store.states[1].flags &= !ENABLED;
            }
            7 => {
                s.store.states[1].flags = puzzella_core::with_rotation(s.store.states[1].flags, 1);
            }
            8 => {
                s.store.states[1].position.x += 1.;
            }
            9 => {
                command = ProtocolPieceCommand::RotateDrag {
                    grab_sequence: 0,
                    final_delta: Vec2::ZERO,
                    through_tick: None,
                    quarter_turns: 1,
                };
            }
            _ => {}
        };
        let before = s.store.states.to_vec();
        let drag = s
            .contexts
            .active_drag(&s.session, &s.store, A)
            .unwrap()
            .clone();
        let root_dirty = s.store.component_root_dirty.clone();
        let dirty = s.store.dirty_pieces.clone();
        let cursor = s.session.cursor();
        let envelope = ProtocolCommandEnvelope {
            session: SESSION.id,
            authority_epoch: AuthorityEpoch(3),
            player: if case == 10 { B } else { A },
            sequence: ClientCommandSequence::Control(1),
            command,
        };
        assert!(
            s.contexts
                .apply_replicated(
                    &mut s.session,
                    &mut s.store,
                    A,
                    &envelope,
                    Some(&s.definition),
                    puzzella_core::LOCAL_PLAYER
                )
                .is_err(),
            "case {case}"
        );
        assert_eq!(s.store.states.to_vec(), before);
        assert_eq!(s.contexts.active_drag(&s.session, &s.store, A), Some(&drag));
        assert_eq!(s.store.component_root_dirty, root_dirty);
        assert_eq!(s.store.dirty_pieces, dirty);
        assert_eq!(s.session.cursor(), cursor);
        let outcome = s.command(
            A,
            ClientCommandSequence::Move {
                after_control_sequence: 0,
                tick: 11,
            },
            ProtocolPieceCommand::DragUpdate { delta: Vec2::ONE },
            true,
        );
        assert!(matches!(
            outcome.result,
            ProtocolCommandResult::DragUpdated { .. }
        ));
    }
}

#[test]
fn replica_preflight_rejects_inconsistent_sibling_and_bad_context_without_partial_rebase() {
    for case in 0..9 {
        let mut s = Simulation::new(&[Vec2::splat(1000.); 4], &[(0, 1)]);
        s.grab(A, 0, &[0, 2]);
        let update = s.update(A, 0, 10, Vec2::ONE);
        let peer = &mut s.peers[0];
        peer.replica
            .apply_drag_update(&peer.session, &peer.store, HOST, &update)
            .unwrap();
        let mut event = s.rotate_drag(1, Some(11), Vec2::new(40., 50.), 1);
        let peer = &mut s.peers[0];
        match case {
            0 => peer.store.states[1].flags &= !HELD,
            1 => peer.store.states[1].position.x += 1.,
            2 => {
                peer.store.connectivity.union(PieceId(2), PieceId(3));
            }
            3 => {
                if let ProtocolAuthorityEvent::DragRotationCommitted(commit) = &mut event.event {
                    commit.grab_sequence = 99;
                }
            }
            4 => {
                if let ProtocolAuthorityEvent::DragRotationCommitted(commit) = &mut event.event {
                    commit.basis_sequence = 0;
                }
            }
            5 => {
                if let ProtocolAuthorityEvent::DragRotationCommitted(commit) = &mut event.event {
                    commit.final_delta = Vec2::NAN;
                }
            }
            6 => {
                if let ProtocolAuthorityEvent::DragRotationCommitted(commit) = &mut event.event {
                    commit.quarter_turns = 4;
                }
            }
            7 => {
                if let ProtocolAuthorityEvent::DragRotationCommitted(commit) = &mut event.event {
                    commit.through_tick = Some(9);
                }
            }
            _ => {
                if let ProtocolAuthorityEvent::DragRotationCommitted(commit) = &mut event.event {
                    commit.player = B;
                }
            }
        }
        let before = peer.store.states.to_vec();
        let drag = peer.replica.remote_drags[&A].clone();
        assert!(peer
            .replica
            .apply_event(
                &mut peer.session,
                &mut peer.store,
                HOST,
                &event,
                Some(&s.definition),
                puzzella_core::LOCAL_PLAYER
            )
            .is_err());
        assert_eq!(peer.store.states.to_vec(), before);
        assert_eq!(peer.replica.remote_drags[&A], drag);
        assert!(peer.replica.needs_resync(&peer.session, &peer.store));
    }
}

#[test]
fn drag_rotation_fingerprint_mismatch_diverges_and_freezes_presentation() {
    let mut s = Simulation::new(&[Vec2::splat(1000.); 2], &[(0, 1)]);
    s.grab(A, 0, &[0]);
    let mut event = s.rotate_drag(1, None, Vec2::ONE, 1);
    if let ProtocolAuthorityEvent::DragRotationCommitted(commit) = &mut event.event {
        commit.result.0 ^= 1;
    }
    let peer = &mut s.peers[0];
    assert_eq!(
        peer.replica.apply_event(
            &mut peer.session,
            &mut peer.store,
            HOST,
            &event,
            Some(&s.definition),
            puzzella_core::LOCAL_PLAYER
        ),
        Err(ReplicationError::Diverged)
    );
    assert!(peer.replica.needs_resync(&peer.session, &peer.store));
    assert!(peer
        .replica
        .remote_drag(&peer.session, &peer.store, A)
        .is_none());
}
