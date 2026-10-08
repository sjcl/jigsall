use super::*;
use crate::multiplayer::{catch_up::JoinCatchUpCoordinator, protocol::ProtocolCommandResult};
use crate::resources::pieces::AppliedCommand;

fn snap_fixture(moving: u32, stationary: u32) -> (Simulation, PieceTarget) {
    let mut offsets: Vec<_> = (0..40)
        .map(|id| Vec2::new(500.0 + id as f32 * 20.0, 500.0))
        .collect();
    offsets[moving as usize] = offsets[stationary as usize] + Vec2::new(10.0, 0.0);
    let s = Simulation::new(&offsets, &[]);
    let mut members = PieceBitSet::new(40);
    members.extend((0..33).map(PieceId));
    let target = PieceTarget::from_selection(&s.store.connectivity, &members).unwrap();
    assert!(matches!(target, PieceTarget::Dense(_)));
    (s, target)
}

#[test]
fn dense_stale_grab_and_rotate_after_competing_snap_replicate_and_catch_up() {
    // Joining two selected components and absorbing an unselected component
    // both invalidate the original intent, without authorizing any new members.
    for (moving, stationary) in [(1, 0), (33, 32)] {
        for rotating in [false, true] {
            let (mut s, target) = snap_fixture(moving, stationary);
            let joiner = PlayerId(30);
            let mut catch_up = JoinCatchUpCoordinator::default();
            let start = catch_up
                .begin_join(joiner, &s.session, &s.store, &s.contexts, &s.definition)
                .unwrap();
            let record = |s: &Simulation, log: &mut JoinCatchUpCoordinator, event: &_| {
                log.record_authority_event(&s.session, &s.store, event)
                    .unwrap();
            };
            let grab = s.grab(B, 0, &[moving]);
            record(&s, &mut catch_up, &grab);
            let release = s.release(B, 1, 0, Vec2::new(-10.0, 0.0), true);
            record(&s, &mut catch_up, &release);
            assert!(s
                .store
                .connectivity
                .same_component(PieceId(moving), PieceId(stationary)));
            assert_eq!(
                target.resolve(&s.store.connectivity),
                Err(jigsall_core::protocol::TargetError::StaleTopology)
            );
            s.assert_equal();

            let before = s.store.states.clone();
            let z = s.store.next_z_order;
            let placed = s.store.placed_count;
            let command = if rotating {
                ProtocolPieceCommand::Rotate {
                    target,
                    quarter_turns: -1,
                }
            } else {
                ProtocolPieceCommand::Grab { target }
            };
            let outcome = s.command(A, ClientCommandSequence::Control(0), command, true);
            match &outcome.result {
                ProtocolCommandResult::Grabbed { applied, ack } => {
                    assert_eq!(*applied, AppliedCommand::default());
                    assert_eq!(ack.accepted, PieceTarget::Components(vec![]));
                    assert!(ack.rejected.is_empty());
                }
                ProtocolCommandResult::Rotated {
                    applied,
                    rejected,
                    commit,
                } => {
                    assert_eq!(*applied, AppliedCommand::default());
                    assert!(rejected.is_empty());
                    assert_eq!(commit.accepted, PieceTarget::Components(vec![]));
                    assert_eq!(commit.quarter_turns, 3);
                    assert_eq!(
                        commit.result,
                        crate::multiplayer::release::result_fingerprint(
                            &s.store,
                            &[],
                            Some(&s.definition),
                            &AppliedCommand::default()
                        )
                    );
                }
                _ => panic!(),
            }
            let event = outcome.authority_event.unwrap();
            assert_eq!(event.cursor.sequence.0, 3);
            assert_eq!(s.store.states, before);
            assert_eq!(s.store.next_z_order, z);
            assert_eq!(s.store.placed_count, placed);
            assert!(s.store.held_by.is_empty());
            assert!(s.contexts.active_drag(&s.session, &s.store, A).is_none());
            record(&s, &mut catch_up, &event);
            s.deliver(&event, true);
            s.assert_equal();
            for peer in &s.peers {
                assert!(peer
                    .replica
                    .remote_drag(&peer.session, &peer.store, A)
                    .is_none());
                assert!(!peer.replica.needs_resync(&peer.session, &peer.store));
            }

            // Reliable Control(0) was consumed normally. Both following kinds
            // of intent can succeed with a newly sampled target.
            let mut members = PieceBitSet::new(40);
            members.extend((0..33).map(PieceId));
            let target = PieceTarget::from_selection(&s.store.connectivity, &members).unwrap();
            let outcome = s.command(
                A,
                ClientCommandSequence::Control(1),
                ProtocolPieceCommand::Rotate {
                    target: target.clone(),
                    quarter_turns: 1,
                },
                true,
            );
            let ProtocolCommandResult::Rotated { applied, .. } = outcome.result else {
                panic!()
            };
            assert!(applied.rotated >= 33);
            let event = outcome.authority_event.unwrap();
            record(&s, &mut catch_up, &event);
            s.deliver(&event, true);
            let outcome = s.command(
                A,
                ClientCommandSequence::Control(2),
                ProtocolPieceCommand::Grab { target },
                true,
            );
            let ProtocolCommandResult::Grabbed { applied, .. } = outcome.result else {
                panic!()
            };
            assert!(applied.grabbed >= 33);
            let event = outcome.authority_event.unwrap();
            record(&s, &mut catch_up, &event);
            s.deliver(&event, true);
            let event = s.release(A, 3, 2, Vec2::ZERO, true);
            record(&s, &mut catch_up, &event);
            assert_eq!(event.cursor.sequence.0, 6);
            s.assert_equal();

            assert_eq!(catch_up.status(joiner).unwrap().retained_events, 6);
            let mut peer = Peer {
                store: PieceDataStore::default(),
                session: AuthoritySession::new(SESSION, HOST, start.baseline.snapshot.cursor),
                replica: PeerReplicationState::default(),
            };
            peer.replica
                .install_join_baseline(
                    &mut peer.session,
                    &mut peer.store,
                    &start.baseline,
                    SnapshotExpectation {
                        session: SESSION.id,
                        image_hash: SESSION.image_hash,
                        cursor: start.baseline.snapshot.cursor,
                        definition: &s.definition,
                    },
                )
                .unwrap();
            catch_up
                .mark_baseline_installed(joiner, start.generation, start.baseline.snapshot.cursor)
                .unwrap();
            for event in catch_up
                .pending_events(joiner, start.generation, usize::MAX)
                .unwrap()
            {
                peer.replica
                    .apply_event(
                        &mut peer.session,
                        &mut peer.store,
                        HOST,
                        &event,
                        Some(&s.definition),
                        joiner,
                    )
                    .unwrap();
                catch_up
                    .acknowledge_through(joiner, start.generation, event.cursor)
                    .unwrap();
            }
            assert_authority_equal(&s.store, &peer.store);
            assert_eq!(peer.session.cursor(), s.session.cursor());
            assert_eq!(
                catch_up.is_reliable_caught_up(joiner, start.generation, &s.session, &s.store),
                Ok(true)
            );
        }
    }
}

#[test]
fn dense_targets_survive_unrelated_snap_for_grab_and_rotate() {
    let (mut s, target) = snap_fixture(34, 33);
    s.grab(B, 0, &[34]);
    s.release(B, 1, 0, Vec2::new(-10.0, 0.0), true);
    target.resolve(&s.store.connectivity).unwrap();
    let outcome = s.command(
        A,
        ClientCommandSequence::Control(0),
        ProtocolPieceCommand::Rotate {
            target: target.clone(),
            quarter_turns: 1,
        },
        true,
    );
    let ProtocolCommandResult::Rotated { applied, .. } = outcome.result else {
        panic!()
    };
    assert_eq!(applied.rotated, 33);
    s.deliver(&outcome.authority_event.unwrap(), true);
    let outcome = s.command(
        A,
        ClientCommandSequence::Control(1),
        ProtocolPieceCommand::Grab { target },
        true,
    );
    let ProtocolCommandResult::Grabbed { applied, ack } = outcome.result else {
        panic!()
    };
    assert_eq!(applied.grabbed, 33);
    assert!(matches!(ack.accepted, PieceTarget::Dense(_)));
    s.deliver(&outcome.authority_event.unwrap(), true);
    s.assert_equal();
}

#[test]
fn dense_empty_rotation_still_verifies_result_fingerprint() {
    let (mut s, target) = snap_fixture(1, 0);
    s.grab(B, 0, &[1]);
    s.release(B, 1, 0, Vec2::new(-10.0, 0.0), true);
    let mut event = s
        .command(
            A,
            ClientCommandSequence::Control(0),
            ProtocolPieceCommand::Rotate {
                target,
                quarter_turns: 1,
            },
            true,
        )
        .authority_event
        .unwrap();
    let ProtocolAuthorityEvent::RotationCommitted(commit) = &mut event.event else {
        panic!()
    };
    assert_eq!(commit.accepted, PieceTarget::Components(vec![]));
    commit.result.0 ^= 1;
    let peer = &mut s.peers[0];
    let before = peer.store.states.clone();
    let cursor = peer.session.cursor();
    assert_eq!(
        peer.replica.apply_event(
            &mut peer.session,
            &mut peer.store,
            HOST,
            &event,
            Some(&s.definition),
            A
        ),
        Err(ReplicationError::Diverged)
    );
    assert_eq!(peer.store.states, before);
    assert_eq!(peer.session.cursor(), cursor);
    assert!(peer.replica.needs_resync(&peer.session, &peer.store));
}

#[test]
fn sparse_stale_entries_still_reject_individually_after_competing_snap() {
    for rotating in [false, true] {
        let (mut s, _) = snap_fixture(1, 0);
        let mut members = PieceBitSet::new(40);
        members.extend([PieceId(0), PieceId(2)]);
        let target = PieceTarget::from_selection(&s.store.connectivity, &members).unwrap();
        s.grab(B, 0, &[1]);
        s.release(B, 1, 0, Vec2::new(-10.0, 0.0), true);
        let before = s.store.states.clone();
        let command = if rotating {
            ProtocolPieceCommand::Rotate {
                target,
                quarter_turns: 1,
            }
        } else {
            ProtocolPieceCommand::Grab { target }
        };
        let outcome = s.command(A, ClientCommandSequence::Control(0), command, true);
        let (applied, accepted, rejected) = match outcome.result {
            ProtocolCommandResult::Grabbed { applied, ack } => {
                (applied, ack.accepted, ack.rejected)
            }
            ProtocolCommandResult::Rotated {
                applied,
                commit,
                rejected,
            } => (applied, commit.accepted, rejected),
            _ => panic!(),
        };
        assert_eq!(applied.grabbed + applied.rotated, 1);
        assert!(
            matches!(accepted, PieceTarget::Component(reference) if reference.member == PieceId(2))
        );
        assert_eq!(rejected.len(), 1);
        assert_eq!(
            rejected[0].reason,
            jigsall_core::protocol::TargetError::StaleComponent
        );
        assert_eq!(s.store.states[0], before[0]);
        assert_eq!(s.store.states[1], before[1]);
        s.deliver(&outcome.authority_event.unwrap(), true);
        s.assert_equal();
    }
}
