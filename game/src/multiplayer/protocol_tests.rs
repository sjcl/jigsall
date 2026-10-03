use super::*;
#[path = "authority_cancellation_tests.rs"]
mod cancellation;
use crate::multiplayer::{GameSnapshot, SnapshotExpectation};
use crate::resources::pieces::{ENABLED, PLACED};
use bevy::math::{UVec2, Vec2};
use puzzella_core::{
    protocol::{ComponentRef, DenseTarget, PieceTarget, MAX_COMPONENT_REFS},
    session::{AuthorityCursor, ImageHash, SessionDefinition},
    PieceBitSet, PieceCommand, PieceId, GENERATOR_VERSION,
};

const A: PlayerId = PlayerId(10);
const B: PlayerId = PlayerId(20);
const SESSION: SessionDefinition = SessionDefinition {
    id: SessionId(123),
    image_hash: ImageHash([7; 32]),
};

#[test]
fn join_capture_checks_authority_context_scalar_membership_and_overlap_invariants() {
    use crate::multiplayer::{JoinBaseline, JoinBaselineError};
    let mut fixture = Fixture::new(6);
    let definition = PuzzleDefinition {
        generator_version: GENERATOR_VERSION,
        seed: 42,
        grid_size: UVec2::new(6, 1),
        image_size: UVec2::new(600, 100),
        snap_distance: 5.,
    };
    fixture.grab(0, fixture.target(&[0]));
    let capture =
        |f: &Fixture| JoinBaseline::capture(&f.session, &f.store, &f.contexts, &definition);
    let original = fixture.contexts.players[&A].clone();
    fixture.contexts.players.get_mut(&A).unwrap().delta.x = f32::INFINITY;
    assert_eq!(capture(&fixture), Err(JoinBaselineError::InvalidDelta(A)));
    fixture.contexts.players.insert(A, original.clone());
    fixture.contexts.players.get_mut(&A).unwrap().grab_sequence = 1;
    assert_eq!(
        capture(&fixture),
        Err(JoinBaselineError::InvalidSequence(A))
    );
    fixture.contexts.players.insert(A, original.clone());
    fixture.contexts.players.get_mut(&A).unwrap().target = ActiveDragTarget::Sparse(vec![]);
    assert_eq!(capture(&fixture), Err(JoinBaselineError::EmptyTarget(A)));
    fixture.contexts.players.insert(A, original.clone());
    fixture.contexts.players.insert(B, original);
    assert_eq!(
        capture(&fixture),
        Err(JoinBaselineError::OverlappingTarget(PieceId(0)))
    );
    fixture.contexts.players.remove(&B);
    fixture.store.states[0].flags |= PLACED;
    fixture.store.states[0].position = definition.correct_position(PieceId(0));
    assert_eq!(
        capture(&fixture),
        Err(JoinBaselineError::PlacedTarget(PieceId(0)))
    );
}

struct Fixture {
    store: PieceDataStore,
    session: AuthoritySession,
    contexts: ProtocolDragContexts,
}
impl Fixture {
    fn new(count: usize) -> Self {
        let mut store = PieceDataStore::default();
        store.initialize(
            (0..count)
                .map(|id| Vec2::new(1000.0 + id as f32 * 100.0, 1000.0))
                .collect(),
        );
        Self {
            store,
            session: AuthoritySession::new(SESSION, A, AuthorityCursor::new(3, 0)),
            contexts: ProtocolDragContexts::default(),
        }
    }
    fn target(&self, ids: &[u32]) -> PieceTarget {
        let mut mask = PieceBitSet::new(self.store.len());
        mask.extend(ids.iter().copied().map(PieceId));
        PieceTarget::from_selection(&self.store.connectivity, &mask).unwrap()
    }
    fn envelope(
        player: PlayerId,
        sequence: ClientCommandSequence,
        command: ProtocolPieceCommand,
    ) -> ProtocolCommandEnvelope {
        ProtocolCommandEnvelope {
            session: SESSION.id,
            authority_epoch: AuthorityEpoch(3),
            player,
            sequence,
            command,
        }
    }
    fn apply(
        &mut self,
        envelope: &ProtocolCommandEnvelope,
    ) -> Result<ProtocolCommandResult, ProtocolCommandError> {
        self.contexts.apply(
            &mut self.session,
            &mut self.store,
            envelope.player,
            envelope,
            None,
            puzzella_core::LOCAL_PLAYER,
        )
    }
    fn grab(&mut self, sequence: u64, target: PieceTarget) -> ProtocolCommandResult {
        self.apply(&Self::envelope(
            A,
            ClientCommandSequence::Control(sequence),
            ProtocolPieceCommand::Grab { target },
        ))
        .unwrap()
    }
    fn update(grab: u64, tick: u64, delta: Vec2) -> ProtocolCommandEnvelope {
        Self::envelope(
            A,
            ClientCommandSequence::Move {
                after_control_sequence: grab,
                tick,
            },
            ProtocolPieceCommand::DragUpdate { delta },
        )
    }
    fn release(sequence: u64, grab: u64, final_delta: Vec2) -> ProtocolCommandEnvelope {
        Self::envelope(
            A,
            ClientCommandSequence::Control(sequence),
            ProtocolPieceCommand::Release {
                grab_sequence: grab,
                final_delta,
            },
        )
    }
    fn drag(&self) -> Option<&ActiveDrag> {
        self.contexts.active_drag(&self.session, &self.store, A)
    }

    fn refs(&self) -> &[ComponentRef] {
        let ActiveDragTarget::Sparse(refs) = &self.drag().unwrap().target else {
            panic!("Expected sparse context")
        };
        refs
    }
}

#[test]
fn sparse_grab_is_component_atomic_and_rejects_only_bad_entries() {
    let mut f = Fixture::new(16);
    for id in (0..16).step_by(2) {
        f.store.connectivity.union(PieceId(id), PieceId(id + 1));
    }
    let refs: Vec<_> = (0..16)
        .step_by(2)
        .map(|id| ComponentRef::from_member(&f.store.connectivity, PieceId(id)).unwrap())
        .collect();
    let mut stale = refs[1];
    stale.expected_size = 99;
    f.store.apply_command(
        B,
        &PieceCommand::Grab(PieceId(4)),
        None,
        puzzella_core::LOCAL_PLAYER,
    );
    let mut input = refs.clone();
    input[1] = stale;
    input.extend([refs[0], refs[0]]);
    input.reverse();
    let ProtocolCommandResult::Grabbed { applied, ack } = f.grab(0, PieceTarget::Components(input))
    else {
        panic!()
    };
    assert_eq!(applied.grabbed, 12);
    assert_eq!(
        ack.rejected,
        [RejectedComponentRef {
            reference: stale,
            reason: TargetError::StaleComponent
        }]
    );
    assert_eq!(
        f.refs(),
        refs.into_iter()
            .filter(|r| r.member != PieceId(2) && r.member != PieceId(4))
            .collect::<Vec<_>>()
    );
    for id in [2, 3] {
        assert!(f.store.held_by.get(&PieceId(id)).is_none());
    }
    for id in [4, 5] {
        assert_eq!(f.store.held_by.get(&PieceId(id)), Some(&B));
    }
}

#[test]
fn dense_grab_expands_partial_components_and_rechecks_all_member_flags() {
    let mut f = Fixture::new(4096);
    for id in (0..8).step_by(2) {
        f.store.connectivity.union(PieceId(id), PieceId(id + 1));
    }
    f.store.apply_command(
        B,
        &PieceCommand::Grab(PieceId(2)),
        None,
        puzzella_core::LOCAL_PLAYER,
    );
    f.store.states[5].flags |= PLACED;
    f.store.states[7].flags &= !ENABLED;
    let mut mask = PieceBitSet::new(4096);
    mask.extend([1, 3, 4, 6].map(PieceId));
    mask.extend((8..3500).map(PieceId));
    let target = PieceTarget::from_selection(&f.store.connectivity, &mask).unwrap();
    assert!(matches!(target, PieceTarget::Dense(_)));
    let ProtocolCommandResult::Grabbed { applied, .. } = f.grab(0, target) else {
        panic!()
    };
    assert_eq!(applied.grabbed, 3494);
    for id in [0, 1] {
        assert_eq!(f.store.held_by.get(&PieceId(id)), Some(&A));
    }
    for id in [4, 5, 6, 7] {
        assert!(f.store.held_by.get(&PieceId(id)).is_none());
    }
    assert_eq!(f.store.held_by.get(&PieceId(2)), Some(&B));
    let ActiveDragTarget::Dense(accepted) = &f.drag().unwrap().target else {
        panic!()
    };
    assert_eq!(accepted.component_count, 3493);
    assert_eq!(accepted.members.count(), 3494);
}

#[test]
fn malicious_component_claims_never_grab_or_expand_stale_targets() {
    let mut f = Fixture::new(8);
    f.store.connectivity.union(PieceId(2), PieceId(3));
    let old = ComponentRef::from_member(&f.store.connectivity, PieceId(2)).unwrap();
    f.store.connectivity.union(PieceId(2), PieceId(4));
    let target = PieceTarget::Components(vec![
        old,
        ComponentRef {
            member: PieceId(3),
            expected_size: 3,
        },
        ComponentRef {
            member: PieceId(2),
            expected_size: u32::MAX,
        },
        ComponentRef {
            member: PieceId(u32::MAX),
            expected_size: 1,
        },
        ComponentRef {
            member: PieceId(0),
            expected_size: 1,
        },
    ]);
    let ProtocolCommandResult::Grabbed { applied, ack } = f.grab(0, target) else {
        panic!()
    };
    assert_eq!(applied.grabbed, 1);
    assert_eq!(ack.rejected.len(), 4);
    assert_eq!(f.refs()[0].member, PieceId(0));
    for id in 2..5 {
        assert!(f.store.held_by.get(&PieceId(id)).is_none());
    }
}

#[test]
fn transient_stream_accepts_gaps_and_rejects_duplicates_old_ticks_and_contexts() {
    let mut f = Fixture::new(4);
    f.grab(0, f.target(&[0, 2]));
    let base = f.store.states.to_vec();
    let membership = f.refs().as_ptr();
    assert_eq!(
        f.apply(&Fixture::update(0, 10, Vec2::splat(10.0))),
        Ok(ProtocolCommandResult::DragUpdated {
            sequence: CommandSequenceStatus::Gap { expected: 0 }
        })
    );
    assert_eq!(
        f.apply(&Fixture::update(0, 12, Vec2::splat(12.0))),
        Ok(ProtocolCommandResult::DragUpdated {
            sequence: CommandSequenceStatus::Gap { expected: 11 }
        })
    );
    assert_eq!(
        f.apply(&Fixture::update(0, 12, Vec2::ZERO)),
        Err(ProtocolCommandError::Sequence(
            ProtocolError::DuplicateCommand
        ))
    );
    assert_eq!(
        f.apply(&Fixture::update(0, 11, Vec2::ZERO)),
        Err(ProtocolCommandError::Sequence(ProtocolError::StaleCommand))
    );
    assert_eq!(
        f.apply(&Fixture::update(1, 13, Vec2::ZERO)),
        Err(ProtocolCommandError::Sequence(
            ProtocolError::ControlNotProcessed { required: 1 }
        ))
    );
    assert_eq!(f.drag().unwrap().delta, Vec2::splat(12.0));
    assert_eq!(f.refs().as_ptr(), membership);
    assert_eq!(&*f.store.states, base);
    f.apply(&Fixture::release(1, 0, Vec2::ONE)).unwrap();
    assert!(f.drag().is_none());
    assert_eq!(
        f.apply(&Fixture::update(0, 14, Vec2::ZERO)),
        Err(ProtocolCommandError::Sequence(
            ProtocolError::StaleMoveContext
        ))
    );
    assert_eq!(
        f.apply(&Fixture::update(1, 0, Vec2::ZERO)),
        Err(ProtocolCommandError::NoActiveDrag)
    );
    f.grab(2, f.target(&[0]));
    assert_eq!(
        f.apply(&Fixture::update(0, 15, Vec2::ZERO)),
        Err(ProtocolCommandError::Sequence(
            ProtocolError::StaleMoveContext
        ))
    );
    f.apply(&Fixture::update(2, 0, Vec2::ONE)).unwrap();
}

#[test]
fn identity_epoch_session_and_wrong_stream_rejections_do_not_consume_valid_ticks() {
    let mut f = Fixture::new(2);
    f.grab(0, f.target(&[0]));
    let update = Fixture::update(0, 0, Vec2::ONE);
    assert_eq!(
        f.contexts.apply(
            &mut f.session,
            &mut f.store,
            B,
            &update,
            None,
            puzzella_core::LOCAL_PLAYER
        ),
        Err(ProtocolCommandError::WrongPlayer)
    );
    let mut bad = update.clone();
    bad.player = B;
    assert_eq!(
        f.apply(&bad),
        Err(ProtocolCommandError::Sequence(
            ProtocolError::ControlNotProcessed { required: 0 }
        ))
    );
    for (epoch, session, reason) in [
        (2, SESSION.id, ProtocolError::WrongEpoch),
        (4, SESSION.id, ProtocolError::WrongEpoch),
        (3, SessionId(999), ProtocolError::WrongSession),
    ] {
        bad = update.clone();
        bad.authority_epoch = AuthorityEpoch(epoch);
        bad.session = session;
        assert_eq!(f.apply(&bad), Err(ProtocolCommandError::Sequence(reason)));
    }
    bad = update.clone();
    bad.sequence = ClientCommandSequence::Control(1);
    assert_eq!(
        f.apply(&bad),
        Err(ProtocolCommandError::Sequence(
            ProtocolError::WrongCommandStream
        ))
    );
    let mut release = Fixture::release(1, 0, Vec2::ONE);
    release.sequence = update.sequence;
    assert_eq!(
        f.apply(&release),
        Err(ProtocolCommandError::Sequence(
            ProtocolError::WrongCommandStream
        ))
    );
    f.apply(&update).unwrap();
}

#[test]
fn reliable_controls_are_contiguous_and_rejected_gameplay_consumes_the_control() {
    let mut f = Fixture::new(4);
    let grab = Fixture::envelope(
        A,
        ClientCommandSequence::Control(0),
        ProtocolPieceCommand::Grab {
            target: f.target(&[0]),
        },
    );
    let mut gap = grab.clone();
    gap.sequence = ClientCommandSequence::Control(2);
    assert_eq!(
        f.apply(&gap),
        Err(ProtocolCommandError::Sequence(ProtocolError::ControlGap {
            expected: 0
        }))
    );
    f.apply(&grab).unwrap();
    assert_eq!(
        f.apply(&grab),
        Err(ProtocolCommandError::Sequence(
            ProtocolError::DuplicateCommand
        ))
    );
    assert_eq!(
        f.apply(&Fixture::release(2, 0, Vec2::ONE)),
        Err(ProtocolCommandError::Sequence(ProtocolError::ControlGap {
            expected: 1
        }))
    );
    assert_eq!(
        f.apply(&Fixture::release(1, 999, Vec2::ONE)),
        Err(ProtocolCommandError::WrongDragContext)
    );
    assert!(f.drag().is_some());
    assert_eq!(
        f.apply(&Fixture::update(1, 0, Vec2::ONE)),
        Err(ProtocolCommandError::WrongDragContext)
    );
    let release = Fixture::release(2, 0, Vec2::ONE);
    f.apply(&release).unwrap();
    assert_eq!(
        f.apply(&release),
        Err(ProtocolCommandError::Sequence(
            ProtocolError::DuplicateCommand
        ))
    );
    assert_eq!(
        f.apply(&grab),
        Err(ProtocolCommandError::Sequence(ProtocolError::StaleCommand))
    );
}

#[test]
fn final_release_is_identical_with_no_lost_or_reordered_updates() {
    let streams: &[&[u64]] = &[&[], &[0, 1, 2, 3], &[0, 3], &[10, 12, 11, 12]];
    let mut expected = None;
    for ticks in streams {
        let mut f = Fixture::new(4);
        f.store.connectivity.union(PieceId(0), PieceId(1));
        f.grab(0, f.target(&[1, 3]));
        for &tick in *ticks {
            let _ = f.apply(&Fixture::update(0, tick, Vec2::splat(tick as f32 * 100.0)));
        }
        let ProtocolCommandResult::Released { applied, .. } = f
            .apply(&Fixture::release(1, 0, Vec2::new(12.0, -7.0)))
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(applied.released, 3);
        assert_eq!(f.store.states[0].position, Vec2::new(1012.0, 993.0));
        assert_eq!(f.store.states[2].position, Vec2::new(1200.0, 1000.0));
        assert!(f.store.held_by.is_empty());
        if let Some(expected) = &expected {
            assert_eq!(&f.store.states.to_vec(), expected);
        } else {
            expected = Some(f.store.states.to_vec());
        }
    }
}

#[test]
fn release_revalidates_stale_membership_ownership_placed_and_enabled() {
    for kind in 0..4 {
        let mut f = Fixture::new(8);
        f.grab(0, f.target(&[0, 2]));
        let base = f.store.states[0].position;
        match kind {
            0 => {
                f.store.connectivity.union(PieceId(0), PieceId(1));
            }
            1 => {
                f.store.held_by.insert(PieceId(0), B);
            }
            2 => {
                f.store.states[0].flags |= PLACED;
            }
            _ => {
                f.store.states[0].flags &= !ENABLED;
            }
        }
        let ProtocolCommandResult::Released {
            applied, rejected, ..
        } = f.apply(&Fixture::release(1, 0, Vec2::ONE)).unwrap()
        else {
            panic!()
        };
        assert_eq!(applied.released, 1);
        assert_eq!(f.store.states[0].position, base);
        assert_eq!(rejected.len(), usize::from(kind == 0));
        assert!(f.drag().is_none());
        f.contexts.cancel_player(&mut f.store, A);
        assert!(!f.store.held_by.has_player(A));
    }
}

#[test]
fn nonfinite_payloads_and_empty_or_oversized_targets_are_rejected_safely() {
    let mut f = Fixture::new(4);
    assert!(matches!(
        f.grab(0, PieceTarget::Components(vec![])),
        ProtocolCommandResult::Grabbed {
            applied: AppliedCommand { grabbed: 0, .. },
            ..
        }
    ));
    assert!(f.drag().is_none());
    assert_eq!(
        f.apply(&Fixture::update(0, 0, Vec2::ONE)),
        Err(ProtocolCommandError::NoActiveDrag)
    );
    let reference = ComponentRef::from_member(&f.store.connectivity, PieceId(0)).unwrap();
    let oversized = Fixture::envelope(
        A,
        ClientCommandSequence::Control(1),
        ProtocolPieceCommand::Grab {
            target: PieceTarget::Components(vec![reference; MAX_COMPONENT_REFS + 1]),
        },
    );
    assert_eq!(
        f.apply(&oversized),
        Err(ProtocolCommandError::Target(TargetError::TooManyComponents))
    );
    let malformed = Fixture::envelope(
        A,
        ClientCommandSequence::Control(2),
        ProtocolPieceCommand::Grab {
            target: PieceTarget::Dense(DenseTarget {
                members: PieceBitSet::new(3),
                component_count: 0,
                topology_digest: 0,
            }),
        },
    );
    assert_eq!(
        f.apply(&malformed),
        Err(ProtocolCommandError::Target(
            TargetError::InvalidMaskDimensions
        ))
    );
    f.grab(3, f.target(&[0]));
    for (tick, delta) in [Vec2::splat(f32::NAN), Vec2::splat(f32::INFINITY)]
        .into_iter()
        .enumerate()
    {
        assert_eq!(
            f.apply(&Fixture::update(3, tick as u64, delta)),
            Err(ProtocolCommandError::InvalidDelta)
        );
    }
    assert_eq!(f.drag().unwrap().delta, Vec2::ZERO);
    assert_eq!(
        f.apply(&Fixture::release(4, 3, Vec2::splat(f32::NEG_INFINITY))),
        Err(ProtocolCommandError::InvalidDelta)
    );
    assert!(f.drag().is_some());
    f.apply(&Fixture::release(5, 3, Vec2::ONE)).unwrap();
}

#[test]
fn independent_players_and_cancel_do_not_cross_contexts() {
    let mut f = Fixture::new(4);
    let ProtocolCommandResult::Grabbed { ack: ack_a, .. } = f.grab(0, f.target(&[0, 1])) else {
        panic!()
    };
    let grab_b = Fixture::envelope(
        B,
        ClientCommandSequence::Control(0),
        ProtocolPieceCommand::Grab {
            target: f.target(&[2, 3]),
        },
    );
    let ProtocolCommandResult::Grabbed { ack: ack_b, .. } = f.apply(&grab_b).unwrap() else {
        panic!()
    };
    assert_eq!((ack_a.player, ack_a.grab_sequence), (A, 0));
    assert_eq!((ack_b.player, ack_b.grab_sequence), (B, 0));
    f.contexts.cancel_player(&mut f.store, A);
    assert!(f.drag().is_none());
    assert_eq!(f.store.held_by.len(), 2);
    let mut update = Fixture::update(0, 0, Vec2::ONE);
    update.player = B;
    f.apply(&update).unwrap();
    assert_eq!(
        f.apply(&Fixture::update(0, 0, Vec2::ONE)),
        Err(ProtocolCommandError::NoActiveDrag)
    );
    let mut release = Fixture::release(1, 0, Vec2::ONE);
    release.player = B;
    f.apply(&release).unwrap();
    assert!(f.store.held_by.is_empty());
}

#[test]
fn snapshot_restore_preserves_stable_refs_and_invalidates_active_contexts() {
    let mut f = Fixture::new(8);
    let d = PuzzleDefinition {
        generator_version: GENERATOR_VERSION,
        seed: 7,
        grid_size: UVec2::new(4, 2),
        image_size: UVec2::new(80, 40),
        snap_distance: 5.0,
    };
    for id in 0..8 {
        f.store.states[id].position = d.correct_position(PieceId(id as u32)) + Vec2::splat(1000.0);
    }
    f.store.connectivity.union(PieceId(2), PieceId(3));
    f.store.connectivity.union(PieceId(2), PieceId(1));
    let r = ComponentRef::from_member(&f.store.connectivity, PieceId(3)).unwrap();
    let old_root = f.store.connectivity.find_root(PieceId(3));
    let mut partial = PieceBitSet::new(8);
    partial.insert(PieceId(3));
    let dense_before_restore =
        DenseTarget::from_selection(&f.store.connectivity, &partial).unwrap();
    f.grab(0, PieceTarget::Component(r));
    let snapshot = GameSnapshot::capture(&f.store, &d, SESSION, f.session.cursor()).unwrap();
    snapshot
        .install(
            &mut f.store,
            SnapshotExpectation {
                session: SESSION.id,
                image_hash: SESSION.image_hash,
                cursor: f.session.cursor(),
                definition: &d,
            },
        )
        .unwrap();
    assert_eq!(
        snapshot.schema_version,
        crate::multiplayer::SNAPSHOT_SCHEMA_VERSION
    );
    assert_eq!(
        std::mem::size_of::<crate::multiplayer::SnapshotPieceState>(),
        16
    );
    assert_ne!(old_root, f.store.connectivity.find_root(PieceId(3)));
    assert_eq!(r.resolve(&f.store.connectivity), Ok(PieceId(1)));
    let restored_dense = dense_before_restore.resolve(&f.store.connectivity).unwrap();
    assert_eq!(
        restored_dense.iter().collect::<Vec<_>>(),
        [PieceId(1), PieceId(2), PieceId(3)]
    );
    assert!(f.drag().is_none());
    assert_eq!(
        f.apply(&Fixture::release(1, 0, Vec2::ONE)),
        Err(ProtocolCommandError::NoActiveDrag)
    );
    assert!(f.store.held_by.is_empty());
    f.grab(2, PieceTarget::Component(r));
}

#[test]
fn migration_freezes_updates_and_new_epoch_cannot_reuse_a_drag() {
    let mut f = Fixture::new(4);
    let d = PuzzleDefinition {
        generator_version: GENERATOR_VERSION,
        seed: 7,
        grid_size: UVec2::splat(2),
        image_size: UVec2::splat(40),
        snap_distance: 5.0,
    };
    f.grab(0, f.target(&[0, 1]));
    let snapshot = GameSnapshot::capture(&f.store, &d, SESSION, f.session.cursor()).unwrap();
    f.session.begin_graceful(B).unwrap();
    assert!(f.drag().is_none());
    assert_eq!(
        f.apply(&Fixture::update(0, 0, Vec2::ONE)),
        Err(ProtocolCommandError::Sequence(ProtocolError::Frozen))
    );
    f.session
        .acknowledge_snapshot(SESSION.id, B, snapshot.cursor)
        .unwrap();
    f.session.host_changed(B).unwrap();
    crate::multiplayer::install_migration_snapshot(&mut f.session, &mut f.store, &d, &snapshot)
        .unwrap();
    assert_eq!(f.session.cursor(), AuthorityCursor::new(4, 0));
    assert!(f.drag().is_none());
    assert_eq!(
        f.apply(&Fixture::release(1, 0, Vec2::ONE)),
        Err(ProtocolCommandError::Sequence(ProtocolError::WrongEpoch))
    );
    let mut release = Fixture::release(0, 0, Vec2::ONE);
    release.authority_epoch = AuthorityEpoch(4);
    assert_eq!(f.apply(&release), Err(ProtocolCommandError::NoActiveDrag));
    let mut grab = Fixture::envelope(
        A,
        ClientCommandSequence::Control(1),
        ProtocolPieceCommand::Grab {
            target: f.target(&[0, 1]),
        },
    );
    grab.authority_epoch = AuthorityEpoch(4);
    f.apply(&grab).unwrap();
    assert_eq!(f.drag().unwrap().grab_sequence, 1);
}

#[test]
fn protocol_dispatch_preserves_relative_z_compaction_and_final_board_snap() {
    let mut f = Fixture::new(4);
    let d = PuzzleDefinition {
        generator_version: GENERATOR_VERSION,
        seed: 7,
        grid_size: UVec2::splat(2),
        image_size: UVec2::splat(40),
        snap_distance: 5.0,
    };
    for id in 0..4 {
        f.store.states[id].position = d.correct_position(PieceId(id as u32)) + Vec2::splat(100.0);
    }
    f.store.connectivity.union(PieceId(0), PieceId(1));
    f.store.connectivity.union(PieceId(2), PieceId(3));
    f.store.states[0].z_order = 9;
    f.store.states[1].z_order = 3;
    f.store.states[2].z_order = 7;
    f.store.states[3].z_order = 1;
    f.store.next_z_order = crate::resources::pieces::MAX_Z;
    f.grab(0, f.target(&[1, 3]));
    let z = |id: usize| f.store.states[id].z_order;
    assert!(z(3) < z(1) && z(1) < z(2) && z(2) < z(0));
    f.apply(&Fixture::update(0, 0, Vec2::splat(9999.0)))
        .unwrap();
    let release = Fixture::release(1, 0, Vec2::splat(-100.0));
    let result = f
        .contexts
        .apply(
            &mut f.session,
            &mut f.store,
            A,
            &release,
            Some(&d),
            puzzella_core::LOCAL_PLAYER,
        )
        .unwrap();
    let ProtocolCommandResult::Released { applied, .. } = result else {
        panic!()
    };
    assert_eq!(applied.released, 4);
    assert_eq!(applied.placed, 4);
    for id in 0..4 {
        assert_eq!(
            f.store.states[id].position,
            d.correct_position(PieceId(id as u32))
        );
        assert_ne!(f.store.states[id].flags & PLACED, 0);
    }
}

#[test]
fn serialized_targets_round_trip_with_small_payload_bounds_and_bounded_decode() {
    let c = puzzella_core::PieceConnectivity::new(1_000_000);
    let refs: Vec<_> = (0..32)
        .map(|id| ComponentRef::from_member(&c, PieceId(id)).unwrap())
        .collect();
    let mut dense = PieceBitSet::new(c.len());
    dense.fill();
    let targets = [
        PieceTarget::Component(refs[0]),
        PieceTarget::Components(refs[..8].to_vec()),
        PieceTarget::Components(refs.clone()),
        PieceTarget::Dense(DenseTarget::from_selection(&c, &dense).unwrap()),
    ];
    let sizes: Vec<_> = targets
        .iter()
        .map(|target| {
            let bytes = serde_json::to_vec(target).unwrap();
            assert_eq!(
                &serde_json::from_slice::<PieceTarget>(&bytes).unwrap(),
                target
            );
            bytes.len()
        })
        .collect();
    assert!(sizes[0] < 128);
    assert!(sizes[1] < 1024);
    assert!(sizes[2] < 4096);
    assert!(sizes[3] < 500_000);
    assert!(sizes[0] * 100 < sizes[3]);
    assert!(sizes[0] < sizes[1] && sizes[1] < sizes[2] && sizes[2] < sizes[3]);
    let million_component = PieceTarget::Component(ComponentRef {
        member: PieceId(0),
        expected_size: 1_000_000,
    });
    let bytes = serde_json::to_vec(&million_component).unwrap();
    assert!(bytes.len() < 128 && bytes.len() * 100 < sizes[3]);
    assert_eq!(
        serde_json::from_slice::<PieceTarget>(&bytes).unwrap(),
        million_component
    );
    let oversized = serde_json::to_vec(&PieceTarget::Components(vec![
        refs[0];
        MAX_COMPONENT_REFS + 1
    ]))
    .unwrap();
    assert!(serde_json::from_slice::<PieceTarget>(&oversized).is_err());
    for command in [
        ProtocolPieceCommand::Grab {
            target: targets[2].clone(),
        },
        ProtocolPieceCommand::DragUpdate { delta: Vec2::ONE },
        ProtocolPieceCommand::Release {
            grab_sequence: 99,
            final_delta: Vec2::ONE,
        },
    ] {
        let sequence = if matches!(command, ProtocolPieceCommand::DragUpdate { .. }) {
            ClientCommandSequence::Move {
                after_control_sequence: 99,
                tick: 42,
            }
        } else {
            ClientCommandSequence::Control(100)
        };
        let envelope = Fixture::envelope(A, sequence, command);
        let bytes = serde_json::to_vec(&envelope).unwrap();
        assert_eq!(
            serde_json::from_slice::<ProtocolCommandEnvelope>(&bytes).unwrap(),
            envelope
        );
        if !matches!(envelope.command, ProtocolPieceCommand::Grab { .. }) {
            assert!(bytes.len() < 512);
        }
    }
}

#[test]
fn stale_dense_grab_cannot_silently_expand_and_unrelated_unions_are_allowed() {
    for merge in [(0, 1), (0, 100)] {
        let mut f = Fixture::new(128);
        let target = f.target(&(0..80).collect::<Vec<_>>());
        assert!(matches!(target, PieceTarget::Dense(_)));
        f.store.connectivity.union(PieceId(110), PieceId(111));
        // The unrelated union does not invalidate this target.
        target.resolve(&f.store.connectivity).unwrap();
        f.store
            .connectivity
            .union(PieceId(merge.0), PieceId(merge.1));
        let before = f.store.states.to_vec();
        let grab = Fixture::envelope(
            A,
            ClientCommandSequence::Control(0),
            ProtocolPieceCommand::Grab { target },
        );
        assert_eq!(
            f.apply(&grab),
            Err(ProtocolCommandError::Target(TargetError::StaleTopology))
        );
        assert_eq!(&*f.store.states, before);
        assert!(f.store.held_by.is_empty());
        assert!(f.drag().is_none());
        f.grab(1, f.target(&[120])); // Rejected gameplay still consumes the control.
    }
}

#[test]
fn accepted_dense_membership_is_canonical_shared_with_ack_and_compacts_small_results() {
    let mut f = Fixture::new(128);
    f.store.connectivity.union(PieceId(0), PieceId(1));
    let input = f.target(&(1..96).collect::<Vec<_>>());
    f.store.apply_command(
        B,
        &PieceCommand::Grab(PieceId(5)),
        None,
        puzzella_core::LOCAL_PLAYER,
    );
    let ProtocolCommandResult::Grabbed { applied, ack } = f.grab(0, input) else {
        panic!()
    };
    assert_eq!(applied.grabbed, 95);
    let ActiveDragTarget::Dense(active) = &f.drag().unwrap().target else {
        panic!()
    };
    let PieceTarget::Dense(accepted) = &ack.accepted else {
        panic!()
    };
    assert_eq!(accepted.members.count(), 95);
    assert!(accepted.members.contains(&PieceId(0))); // Expanded by authority.
    assert!(!accepted.members.contains(&PieceId(5))); // Remote hold rejected.
    assert!(std::sync::Arc::ptr_eq(
        active.members.words(),
        accepted.members.words()
    ));
    assert_eq!(
        accepted.resolve(&f.store.connectivity).unwrap(),
        accepted.members
    );
    let ack_bytes = serde_json::to_vec(&ack).unwrap();
    assert_eq!(
        serde_json::from_slice::<GrabAccepted>(&ack_bytes).unwrap(),
        ack
    );

    let mut small = Fixture::new(128);
    let input = small.target(&(0..96).collect::<Vec<_>>());
    for id in 2..96 {
        small.store.states[id].flags &= !ENABLED;
    }
    let ProtocolCommandResult::Grabbed { applied, ack } = small.grab(0, input) else {
        panic!()
    };
    assert_eq!(applied.grabbed, 2);
    assert_eq!(small.refs().len(), 2);
    assert!(matches!(ack.accepted, PieceTarget::Components(refs) if refs.len() == 2));
}

#[test]
fn million_singleton_context_uses_125kb_membership_and_no_component_list() {
    let mut f = Fixture::new(1_000_000);
    let mut mask = PieceBitSet::new(f.store.len());
    mask.fill();
    let input = PieceTarget::from_selection(&f.store.connectivity, &mask).unwrap();
    let ProtocolCommandResult::Grabbed { applied, ack } = f.grab(0, input) else {
        panic!()
    };
    assert_eq!(applied.grabbed, 1_000_000);
    let ActiveDragTarget::Dense(active) = &f.drag().unwrap().target else {
        panic!()
    };
    assert_eq!(active.component_count, 1_000_000);
    assert_eq!(
        std::mem::size_of_val(active.members.words().as_ref()),
        125_000
    );
    let PieceTarget::Dense(accepted) = &ack.accepted else {
        panic!()
    };
    assert!(std::sync::Arc::ptr_eq(
        active.members.words(),
        accepted.members.words()
    ));
    let allocation = active.members.words().as_ptr();
    f.apply(&Fixture::update(0, 100, Vec2::splat(999.0)))
        .unwrap();
    let ActiveDragTarget::Dense(active) = &f.drag().unwrap().target else {
        panic!()
    };
    assert_eq!(active.members.words().as_ptr(), allocation);
    let ProtocolCommandResult::Released { applied, .. } =
        f.apply(&Fixture::release(1, 0, Vec2::ONE)).unwrap()
    else {
        panic!()
    };
    assert_eq!(applied.released, 1_000_000);
    assert_eq!(f.store.states[0].position, Vec2::splat(1001.0));
    assert!(f.store.held_by.is_empty());
    assert!(f.drag().is_none());
}

#[test]
fn dense_release_revalidates_topology_and_each_components_gameplay_state() {
    let mut stale = Fixture::new(128);
    stale.grab(0, stale.target(&(0..96).collect::<Vec<_>>()));
    stale.store.connectivity.union(PieceId(0), PieceId(100));
    let base = stale.store.states.to_vec();
    assert_eq!(
        stale.apply(&Fixture::release(1, 0, Vec2::ONE)),
        Err(ProtocolCommandError::Target(TargetError::StaleTopology))
    );
    assert_eq!(&*stale.store.states, base);
    assert_eq!(stale.store.held_by.len(), 96);
    assert!(stale.drag().is_some());
    stale.contexts.cancel_player(&mut stale.store, A);

    for kind in 0..3 {
        let mut f = Fixture::new(128);
        f.store.connectivity.union(PieceId(0), PieceId(1));
        f.grab(0, f.target(&(0..96).collect::<Vec<_>>()));
        let base = f.store.states[0].position;
        match kind {
            0 => {
                f.store.held_by.insert(PieceId(1), B);
            }
            1 => {
                f.store.states[1].flags |= PLACED;
            }
            _ => {
                f.store.states[1].flags &= !ENABLED;
            }
        }
        let ProtocolCommandResult::Released { applied, .. } =
            f.apply(&Fixture::release(1, 0, Vec2::ONE)).unwrap()
        else {
            panic!()
        };
        assert_eq!(applied.released, 94);
        assert_eq!(f.store.states[0].position, base);
        assert_eq!(f.store.held_by.get(&PieceId(0)), Some(&A));
        assert!(f.drag().is_none());
    }
}

#[test]
fn grab_ack_identifies_exact_partial_acceptance_and_round_trips_with_authority_cursor() {
    use puzzella_core::protocol::{ProtocolAuthorityEvent, ProtocolAuthorityEventEnvelope};
    let mut f = Fixture::new(12);
    for (a, b) in [(0, 1), (1, 2), (3, 4), (5, 6), (6, 7), (7, 8)] {
        f.store.connectivity.union(PieceId(a), PieceId(b));
    }
    f.store.apply_command(
        B,
        &PieceCommand::Grab(PieceId(3)),
        None,
        puzzella_core::LOCAL_PLAYER,
    );
    let ProtocolCommandResult::Grabbed { applied, ack } = f.grab(0, f.target(&[2, 4, 8])) else {
        panic!()
    };
    assert_eq!(applied.grabbed, 7);
    assert_eq!(ack.player, A);
    assert_eq!(ack.grab_sequence, 0);
    let PieceTarget::Components(refs) = &ack.accepted else {
        panic!()
    };
    assert_eq!(
        refs.iter()
            .map(|r| (r.member.0, r.expected_size))
            .collect::<Vec<_>>(),
        [(0, 3), (5, 4)]
    );
    assert_eq!(refs, f.refs());
    let cursor = f.session.advance_authority().unwrap();
    let event = ProtocolAuthorityEventEnvelope {
        session: SESSION.id,
        host: A,
        cursor,
        event: ProtocolAuthorityEvent::GrabAccepted(ack.clone()),
    };
    let bytes = serde_json::to_vec(&event).unwrap();
    assert_eq!(
        serde_json::from_slice::<ProtocolAuthorityEventEnvelope>(&bytes).unwrap(),
        event
    );
    let reference = refs[0];
    let invalid = GrabAccepted {
        rejected: vec![
            RejectedComponentRef {
                reference,
                reason: TargetError::StaleComponent
            };
            MAX_COMPONENT_REFS + 1
        ],
        ..ack
    };
    assert!(
        serde_json::from_slice::<GrabAccepted>(&serde_json::to_vec(&invalid).unwrap()).is_err()
    );

    // Naked dense masks from the superseded command shape cannot bypass topology validation.
    let old_dense = serde_json::json!({ "Dense": PieceBitSet::new(12) });
    assert!(serde_json::from_value::<PieceTarget>(old_dense).is_err());
}
