use super::*;
use crate::resources::pieces::HELD;

fn cancel(f: &mut Fixture) -> Result<Option<HostCancellationOutcome>, ProtocolCommandError> {
    f.contexts
        .cancel_replicated(&mut f.session, &mut f.store, A)
}

fn assert_atomic_failure(f: &mut Fixture, expected: ProtocolCommandError) {
    let states = f.store.states.clone();
    let owners = f.store.held_by.clone();
    let dirty = f.store.dirty_pieces.clone();
    let presentation = f.store.drag.clone();
    let selection = f.store.selected_pieces.clone();
    let contexts = f.contexts.players.clone();
    let scope = f.contexts.scope;
    let cursor = f.session.cursor();
    let epoch = f.store.epoch;
    let next_z = f.store.next_z_order;
    let placed = f.store.placed_count;
    assert_eq!(cancel(f).unwrap_err(), expected);
    assert_eq!(f.store.states, states);
    assert_eq!(f.store.held_by, owners);
    assert_eq!(f.store.dirty_pieces, dirty);
    assert_eq!(f.store.drag.members, presentation.members);
    assert_eq!(f.store.drag.delta, presentation.delta);
    assert_eq!(f.store.selected_pieces, selection);
    assert_eq!(f.contexts.players, contexts);
    assert_eq!(f.contexts.scope, scope);
    assert_eq!(f.session.cursor(), cursor);
    assert_eq!(f.store.epoch, epoch);
    assert_eq!(f.store.next_z_order, next_z);
    assert_eq!(f.store.placed_count, placed);
}

#[test]
fn replicated_cancel_strict_validation_is_atomic_for_sparse_and_dense() {
    for dense in [false, true] {
        for corruption in 0..11 {
            let mut f = Fixture::new(64);
            f.store.connectivity.union(PieceId(0), PieceId(1));
            let ids: Vec<_> = if dense {
                (0..40).collect()
            } else {
                vec![0, 39]
            };
            f.grab(0, f.target(&ids));
            f.apply(&Fixture::update(0, 0, Vec2::new(80., 30.)))
                .unwrap();
            f.store.dirty_pieces.clear();
            // Last component/member corruption must not partially clear the first.
            match corruption {
                0 => {
                    f.store.connectivity.union(PieceId(39), PieceId(40));
                }
                1 => f.store.states[39].flags &= !HELD,
                2 => f.store.held_by.insert(PieceId(39), B),
                3 => f.store.states[39].flags |= PLACED,
                4 => f.store.states[39].flags &= !ENABLED,
                5 => {
                    f.store.held_by.insert(PieceId(63), A);
                    f.store.states[63].flags |= HELD;
                }
                6 => {
                    f.contexts.players.remove(&A);
                }
                7 => {
                    let target = &mut f.contexts.players.get_mut(&A).unwrap().target;
                    match target {
                        ActiveDragTarget::Sparse(refs) => refs.clear(),
                        ActiveDragTarget::Dense(d) => {
                            let empty = PieceBitSet::new(64);
                            *d =
                                DenseTarget::from_selection(&f.store.connectivity, &empty).unwrap();
                        }
                    }
                }
                8 => {
                    let target = &mut f.contexts.players.get_mut(&A).unwrap().target;
                    match target {
                        ActiveDragTarget::Sparse(refs) => refs.push(refs[0]),
                        // Fingerprint still expands to the original full component.
                        ActiveDragTarget::Dense(d) => {
                            d.members.remove(&PieceId(1));
                        }
                    }
                }
                9 => {
                    let target = &mut f.contexts.players.get_mut(&A).unwrap().target;
                    match target {
                        ActiveDragTarget::Sparse(refs) => refs[1].expected_size += 1,
                        ActiveDragTarget::Dense(d) => d.topology_digest ^= 1,
                    }
                }
                10 => {
                    let target = &mut f.contexts.players.get_mut(&A).unwrap().target;
                    match target {
                        ActiveDragTarget::Sparse(refs) => refs[1].member = PieceId(64),
                        ActiveDragTarget::Dense(d) => d.members = PieceBitSet::new(63),
                    }
                }
                _ => unreachable!(),
            }
            assert_atomic_failure(&mut f, ProtocolCommandError::InconsistentDragTarget);
        }
    }
}

#[test]
fn replicated_cancel_preflights_frozen_and_exhausted_cursor() {
    for frozen in [false, true] {
        let mut f = Fixture::new(4);
        f.grab(0, f.target(&[0, 3]));
        let error = if frozen {
            f.session.begin_graceful(B).unwrap();
            ProtocolError::Frozen
        } else {
            f.session = AuthoritySession::new(SESSION, A, AuthorityCursor::new(3, u64::MAX));
            ProtocolError::CounterExhausted
        };
        assert_atomic_failure(&mut f, ProtocolCommandError::Sequence(error));
    }
}

#[test]
fn replicated_cancel_never_uses_old_session_epoch_or_store_scope() {
    for changed in 0..3 {
        let mut f = Fixture::new(4);
        f.grab(0, f.target(&[0]));
        match changed {
            0 => {
                f.session = AuthoritySession::new(
                    SessionDefinition {
                        id: SessionId(999),
                        ..SESSION
                    },
                    A,
                    AuthorityCursor::new(3, 0),
                )
            }
            1 => f.session = AuthoritySession::new(SESSION, A, AuthorityCursor::new(4, 0)),
            2 => f.store.epoch += 1,
            _ => unreachable!(),
        }
        assert_atomic_failure(&mut f, ProtocolCommandError::InconsistentDragTarget);
        // Even with no ownership, the obsolete context is never converted or removed.
        f.store.clear_player_holds(A);
        let contexts = f.contexts.players.clone();
        let cursor = f.session.cursor();
        assert!(cancel(&mut f).unwrap().is_none());
        assert_eq!(f.session.cursor(), cursor);
        assert_eq!(f.contexts.players, contexts);
    }
}

#[test]
fn replicated_cancel_no_context_no_hold_is_noop_and_keeps_command_history() {
    let mut f = Fixture::new(4);
    let states = f.store.states.clone();
    let cursor = f.session.cursor();
    assert!(cancel(&mut f).unwrap().is_none());
    assert_eq!(f.session.cursor(), cursor);
    assert_eq!(f.store.states, states);
    assert!(f.store.held_by.is_empty());
    assert!(f.contexts.players.is_empty());
    let target = f.target(&[0]);
    f.grab(0, target.clone());
    let event = cancel(&mut f).unwrap().unwrap();
    assert_eq!(event.applied.released, 1);
    assert_eq!(event.authority_event.cursor, AuthorityCursor::new(3, 1));
    assert!(cancel(&mut f).unwrap().is_none());
    // Cancellation is not a client command and must not reset the tracker.
    assert_eq!(
        f.apply(&Fixture::envelope(
            A,
            ClientCommandSequence::Control(0),
            ProtocolPieceCommand::Grab {
                target: target.clone()
            }
        )),
        Err(ProtocolCommandError::Sequence(
            ProtocolError::DuplicateCommand
        ))
    );
    f.grab(1, target);
}
