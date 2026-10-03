use super::*;
use puzzella_core::protocol::DragCancelled;

#[test]
fn existing_peers_replay_sparse_and_dense_cancel_without_delta_commit() {
    for dense in [false, true] {
        let mut s = Simulation::new(&[Vec2::splat(100.); 64], &[(0, 1)]);
        let ids: Vec<_> = if dense { (0..40).collect() } else { vec![0, 2] };
        s.grab(A, 0, &ids);
        s.grab(B, 0, &[63]);
        let update = s.update(A, 0, 0, Vec2::new(80., 30.));
        let b_update = s.update(B, 0, 0, Vec2::new(20., 50.));
        for peer in &mut s.peers {
            peer.replica
                .apply_drag_update(&peer.session, &peer.store, HOST, &update)
                .unwrap();
            peer.replica
                .apply_drag_update(&peer.session, &peer.store, HOST, &b_update)
                .unwrap();
            peer.store.dirty_pieces.clear();
        }
        let states = s.store.states.clone();
        s.store.dirty_pieces.clear();
        let outcome = s
            .contexts
            .cancel_replicated(&mut s.session, &mut s.store, A)
            .unwrap()
            .unwrap();
        s.deliver(&outcome.authority_event, true);
        s.assert_equal();
        for peer in &mut s.peers {
            assert!(peer
                .replica
                .remote_drag(&peer.session, &peer.store, A)
                .is_none());
            let b = peer
                .replica
                .remote_drag(&peer.session, &peer.store, B)
                .unwrap();
            assert_eq!(b.delta, b_update.delta);
            assert_eq!(b.last_tick, Some(0));
            assert_eq!(b.basis_sequence, 0);
            let cursor = peer.session.cursor();
            assert_eq!(
                peer.replica
                    .apply_drag_update(&peer.session, &peer.store, HOST, &update),
                Err(ReplicationError::MissingDragContext)
            );
            assert_eq!(peer.session.cursor(), cursor);
            assert_eq!(peer.store.dirty_pieces.count(), if dense { 40 } else { 3 });
        }
        for (old, new) in states.iter().zip(s.store.states.iter()) {
            assert_eq!(old.position, new.position);
            assert_eq!(old.z_order, new.z_order);
            assert_eq!(old.flags & !HELD, new.flags & !HELD);
        }
    }
}

#[test]
fn peer_cancel_missing_wrong_and_inconsistent_context_latch_without_mutation() {
    for dense in [false, true] {
        for failure in 0..9 {
            let mut s = Simulation::new(&[Vec2::splat(100.); 64], &[(0, 1)]);
            let ids: Vec<_> = if dense {
                (0..40).collect()
            } else {
                vec![0, 39]
            };
            if failure != 0 {
                s.grab(A, 0, &ids);
            }
            let peer = &mut s.peers[0];
            let expected = match failure {
                0 => ReplicationError::MissingDragContext,
                1 => ReplicationError::WrongDragContext,
                2 => {
                    peer.store.states[39].flags &= !HELD;
                    ReplicationError::Diverged
                }
                3 => {
                    peer.store.held_by.insert(PieceId(39), B);
                    ReplicationError::Diverged
                }
                4 => {
                    peer.store.states[39].flags |= PLACED;
                    ReplicationError::Diverged
                }
                5 => {
                    peer.store.states[39].flags &= !ENABLED;
                    ReplicationError::Diverged
                }
                6 => {
                    peer.store.held_by.insert(PieceId(63), A);
                    peer.store.states[63].flags |= HELD;
                    ReplicationError::Diverged
                }
                7 => {
                    peer.store.connectivity.union(PieceId(39), PieceId(40));
                    ReplicationError::Diverged
                }
                8 => {
                    let drag = peer.replica.remote_drags.get_mut(&A).unwrap();
                    match &mut drag.target {
                        ActiveDragTarget::Sparse(refs) => refs.clear(),
                        ActiveDragTarget::Dense(d) => {
                            d.members.remove(&PieceId(1));
                        }
                    }
                    ReplicationError::Diverged
                }
                _ => unreachable!(),
            };
            let cursor = peer.session.cursor();
            let event = ProtocolAuthorityEventEnvelope {
                session: SESSION.id,
                host: HOST,
                cursor: AuthorityCursor::new(cursor.epoch.0, cursor.sequence.0 + 1),
                event: ProtocolAuthorityEvent::DragCancelled(DragCancelled {
                    player: A,
                    grab_sequence: u64::from(failure == 1),
                }),
            };
            let states = peer.store.states.clone();
            let owners = peer.store.held_by.clone();
            let contexts = peer.replica.remote_drags.clone();
            let dirty = peer.store.dirty_pieces.clone();
            assert_eq!(
                peer.replica.apply_event(
                    &mut peer.session,
                    &mut peer.store,
                    HOST,
                    &event,
                    Some(&s.definition),
                    puzzella_core::LOCAL_PLAYER
                ),
                Err(expected)
            );
            assert!(peer.replica.needs_resync(&peer.session, &peer.store));
            assert_eq!(peer.session.cursor(), cursor);
            assert_eq!(peer.store.states, states);
            assert_eq!(peer.store.held_by, owners);
            assert_eq!(peer.replica.remote_drags, contexts);
            assert_eq!(peer.store.dirty_pieces, dirty);
        }
    }
}
