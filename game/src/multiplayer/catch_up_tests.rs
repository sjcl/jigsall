use super::*;
use crate::multiplayer::{replication::PeerReplicationState, GameSnapshot, SnapshotExpectation};
use bevy::math::{UVec2, Vec2};
use puzzella_core::{
    protocol::{
        DenseTarget, DragCancelled, GrabAccepted, ProtocolCommandEnvelope, ProtocolPieceCommand,
        ReleaseResultFingerprint, RotationCommitted, TargetError,
    },
    session::{ClientCommandSequence, ImageHash, SessionDefinition},
    PieceBitSet, PieceId, GENERATOR_VERSION,
};
use std::collections::HashMap;

const HOST: PlayerId = PlayerId(7);
const B: PlayerId = PlayerId(8);
const JOINER: PlayerId = PlayerId(99);
const J2: PlayerId = PlayerId(100);
const SESSION: SessionDefinition = SessionDefinition {
    id: SessionId(123),
    image_hash: ImageHash([7; 32]),
};

struct Host {
    definition: PuzzleDefinition,
    session: AuthoritySession,
    store: PieceDataStore,
    contexts: ProtocolDragContexts,
    next_control: HashMap<PlayerId, u64>,
}
impl Host {
    fn new(count: usize) -> Self {
        let grid_size = if count == 4096 {
            UVec2::splat(64)
        } else {
            UVec2::new(count as u32, 1)
        };
        let definition = PuzzleDefinition {
            generator_version: GENERATOR_VERSION,
            seed: 42,
            grid_size,
            image_size: grid_size * 20,
            snap_distance: 5.,
        };
        let mut store = PieceDataStore::default();
        store.initialize(
            (0..count as u32)
                .map(|id| definition.correct_position(PieceId(id)) + Vec2::splat(1000.))
                .collect(),
        );
        store.connectivity.union(PieceId(0), PieceId(1));
        let cursor = AuthorityCursor::new(3, 0);
        let snapshot = GameSnapshot::capture(&store, &definition, SESSION, cursor).unwrap();
        snapshot
            .install(
                &mut store,
                SnapshotExpectation {
                    session: SESSION.id,
                    image_hash: SESSION.image_hash,
                    cursor,
                    definition: &definition,
                },
            )
            .unwrap();
        Self {
            definition,
            session: AuthoritySession::new(SESSION, HOST, cursor),
            store,
            contexts: ProtocolDragContexts::default(),
            next_control: HashMap::new(),
        }
    }
    fn command(
        &mut self,
        player: PlayerId,
        sequence: ClientCommandSequence,
        command: ProtocolPieceCommand,
    ) -> HostCommandOutcome {
        if let ClientCommandSequence::Control(control) = sequence {
            let next = self.next_control.entry(player).or_default();
            // Model previously consumed rejected controls for the 40/42 examples.
            for previous in *next..control {
                self.session
                    .accept_command(&ProtocolCommandEnvelope {
                        session: self.session.session_id(),
                        authority_epoch: self.session.cursor().epoch,
                        player,
                        sequence: ClientCommandSequence::Control(previous),
                        command: ProtocolPieceCommand::Rotate {
                            target: PieceTarget::Components(vec![]),
                            quarter_turns: 0,
                        },
                    })
                    .unwrap();
            }
            *next = control + 1;
        }
        self.contexts
            .apply_replicated(
                &mut self.session,
                &mut self.store,
                player,
                &ProtocolCommandEnvelope {
                    session: SESSION.id,
                    authority_epoch: AuthorityEpoch(3),
                    player,
                    sequence,
                    command,
                },
                Some(&self.definition),
                HOST,
            )
            .unwrap()
    }
    fn grab(
        &mut self,
        player: PlayerId,
        sequence: u64,
        ids: impl IntoIterator<Item = u32>,
    ) -> HostCommandOutcome {
        let mut mask = PieceBitSet::new(self.store.len());
        mask.extend(ids.into_iter().map(PieceId));
        let target = PieceTarget::from_selection(&self.store.connectivity, &mask).unwrap();
        self.command(
            player,
            ClientCommandSequence::Control(sequence),
            ProtocolPieceCommand::Grab { target },
        )
    }
    fn update(&mut self, player: PlayerId, basis: u64, tick: u64) -> HostCommandOutcome {
        self.command(
            player,
            ClientCommandSequence::Move {
                after_control_sequence: basis,
                tick,
            },
            ProtocolPieceCommand::DragUpdate {
                delta: Vec2::new(tick as f32, 30.),
            },
        )
    }
    fn rotate_drag(&mut self) -> HostCommandOutcome {
        self.command(
            HOST,
            ClientCommandSequence::Control(42),
            ProtocolPieceCommand::RotateDrag {
                grab_sequence: 40,
                final_delta: Vec2::new(80., 30.),
                through_tick: Some(100),
                quarter_turns: 1,
            },
        )
    }
    fn release(
        &mut self,
        player: PlayerId,
        sequence: u64,
        grab_sequence: u64,
    ) -> HostCommandOutcome {
        self.command(
            player,
            ClientCommandSequence::Control(sequence),
            ProtocolPieceCommand::Release {
                grab_sequence,
                final_delta: Vec2::new(80., 30.),
            },
        )
    }
    fn rotate_idle(&mut self, sequence: u64) -> HostCommandOutcome {
        let target = PieceTarget::Component(
            ComponentRef::from_member(
                &self.store.connectivity,
                PieceId(self.store.len() as u32 - 1),
            )
            .unwrap(),
        );
        self.command(
            PlayerId(11),
            ClientCommandSequence::Control(sequence),
            ProtocolPieceCommand::Rotate {
                target,
                quarter_turns: 1,
            },
        )
    }
    fn begin(
        &self,
        coordinator: &mut JoinCatchUpCoordinator,
        player: PlayerId,
    ) -> Result<JoinCatchUpStart, CatchUpError> {
        coordinator.begin_join(
            player,
            &self.session,
            &self.store,
            &self.contexts,
            &self.definition,
        )
    }
    fn restart(
        &self,
        coordinator: &mut JoinCatchUpCoordinator,
    ) -> Result<JoinCatchUpStart, CatchUpError> {
        coordinator.restart_join(
            JOINER,
            &self.session,
            &self.store,
            &self.contexts,
            &self.definition,
        )
    }
    fn record(
        &self,
        coordinator: &mut JoinCatchUpCoordinator,
        outcome: &HostCommandOutcome,
    ) -> Result<(), CatchUpError> {
        coordinator.record_command_outcome(&self.session, &self.store, outcome)
    }
    fn record_event(
        &self,
        coordinator: &mut JoinCatchUpCoordinator,
        event: &ProtocolAuthorityEventEnvelope,
    ) -> Result<(), CatchUpError> {
        coordinator.record_authority_event(&self.session, &self.store, event)
    }
    fn caught_up(
        &self,
        coordinator: &mut JoinCatchUpCoordinator,
        generation: u64,
    ) -> Result<bool, CatchUpError> {
        coordinator.is_reliable_caught_up(JOINER, generation, &self.session, &self.store)
    }
}

struct Peer {
    session: AuthoritySession,
    store: PieceDataStore,
    replica: PeerReplicationState,
}
impl Peer {
    fn install(
        host: &Host,
        start: &JoinCatchUpStart,
        coordinator: &mut JoinCatchUpCoordinator,
    ) -> Self {
        let mut peer = Self {
            session: AuthoritySession::new(SESSION, HOST, AuthorityCursor::new(3, 0)),
            store: PieceDataStore::default(),
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
                    definition: &host.definition,
                },
            )
            .unwrap();
        coordinator
            .mark_baseline_installed(JOINER, start.generation, start.baseline.snapshot.cursor)
            .unwrap();
        peer
    }
    fn catch_up(&mut self, host: &Host, coordinator: &mut JoinCatchUpCoordinator, generation: u64) {
        for event in coordinator
            .pending_events(JOINER, generation, usize::MAX)
            .unwrap()
        {
            self.replica
                .apply_event(
                    &mut self.session,
                    &mut self.store,
                    HOST,
                    &event,
                    Some(&host.definition),
                    JOINER,
                )
                .unwrap();
            coordinator
                .acknowledge_through(JOINER, generation, event.cursor)
                .unwrap();
        }
        for update in coordinator.latest_drag_updates(JOINER, generation).unwrap() {
            self.replica
                .apply_drag_update(&self.session, &self.store, HOST, &update)
                .unwrap();
        }
    }
    fn assert_matches(&self, host: &Host) {
        assert_eq!(self.session.cursor(), host.session.cursor());
        assert_eq!(self.store.states, host.store.states);
        assert_eq!(self.store.held_by, host.store.held_by);
        assert_eq!(self.store.next_z_order, host.store.next_z_order);
        assert_eq!(self.store.placed_count, host.store.placed_count);
        for id in 0..host.store.len() as u32 {
            let id = PieceId(id);
            assert_eq!(
                self.store.connectivity.minimum_member(id),
                host.store.connectivity.minimum_member(id)
            );
            assert_eq!(
                self.store.connectivity.component_size(id),
                host.store.connectivity.component_size(id)
            );
        }
        let host_drags: Vec<_> = host
            .contexts
            .active_drags(&host.session, &host.store)
            .collect();
        let peer_drags: Vec<_> = self
            .replica
            .remote_drags(&self.session, &self.store)
            .collect();
        assert_eq!(host_drags.len(), peer_drags.len());
        for (player, drag) in host_drags {
            assert_eq!(
                self.replica.remote_drag(&self.session, &self.store, player),
                Some(drag)
            );
        }
    }
}

fn count_limits(max_events: usize) -> JoinCatchUpCoordinator {
    JoinCatchUpCoordinator::new(CatchUpLimits {
        max_events,
        max_retained_bytes: usize::MAX,
    })
}
fn assert_discarded(
    coordinator: &JoinCatchUpCoordinator,
    player: PlayerId,
    reason: CatchUpRestartReason,
) {
    let status = coordinator.status(player).unwrap();
    assert_eq!(status.phase, JoinCatchUpPhase::RestartRequired(reason));
    assert_eq!(status.retained_events, 0);
    assert_eq!(status.retained_bytes, 0);
    assert_eq!(status.latest_drag_updates, 0);
    let peer = &coordinator.peers[&player.0];
    assert_eq!(peer.events.capacity(), 0);
    assert!(peer.drag_bases.is_empty());
}

#[test]
fn baseline_pending_retains_gameplay_then_replays_to_exact_host_state() {
    for dense in [false, true] {
        let mut host = Host::new(64);
        let mut coordinator = JoinCatchUpCoordinator::default();
        host.grab(
            HOST,
            40,
            if dense {
                (0..40).collect::<Vec<_>>()
            } else {
                vec![0]
            },
        );
        host.update(HOST, 40, 100);
        let start = host.begin(&mut coordinator, JOINER).unwrap();
        let c = start.baseline.snapshot.cursor;
        assert!(coordinator.peers[&JOINER.0].latest_updates.is_empty());
        let outcome = host.grab(B, 0, [50]);
        host.record(&mut coordinator, &outcome).unwrap();
        let outcome = host.update(B, 0, 10);
        host.record(&mut coordinator, &outcome).unwrap();
        let outcome = host.rotate_drag();
        host.record(&mut coordinator, &outcome).unwrap();
        let outcome = host.update(HOST, 42, 101);
        host.record(&mut coordinator, &outcome).unwrap();
        let cancellation = host
            .contexts
            .cancel_replicated(&mut host.session, &mut host.store, HOST)
            .unwrap()
            .unwrap();
        host.record_event(&mut coordinator, &cancellation.authority_event)
            .unwrap();
        let outcome = host.release(B, 1, 0);
        host.record(&mut coordinator, &outcome).unwrap();
        let status = coordinator.status(JOINER).unwrap();
        assert_eq!(status.phase, JoinCatchUpPhase::BaselinePending);
        assert_eq!(status.retained_events, 4);
        assert_eq!(status.last_recorded_cursor.sequence.0, c.sequence.0 + 4);
        assert_eq!(status.latest_drag_updates, 0);
        assert_eq!(host.caught_up(&mut coordinator, 0), Ok(false));
        let mut peer = Peer::install(&host, &start, &mut coordinator);
        peer.catch_up(&host, &mut coordinator, 0);
        peer.assert_matches(&host);
        assert_eq!(coordinator.status(JOINER).unwrap().retained_bytes, 0);
        assert!(coordinator
            .pending_events(JOINER, 0, usize::MAX)
            .unwrap()
            .is_empty());
        assert_eq!(host.caught_up(&mut coordinator, 0), Ok(true));
    }
}

#[test]
fn multiple_peers_start_at_different_cursors_share_payload_and_prune_independently() {
    let mut host = Host::new(64);
    let mut coordinator = JoinCatchUpCoordinator::default();
    let j1 = host.begin(&mut coordinator, JOINER).unwrap();
    for sequence in 0..2 {
        let outcome = host.rotate_idle(sequence);
        host.record(&mut coordinator, &outcome).unwrap();
    }
    let j2 = host.begin(&mut coordinator, J2).unwrap();
    let event = host.grab(HOST, 0, 0..40).authority_event.unwrap();
    host.record_event(&mut coordinator, &event).unwrap();
    let a = &coordinator.peers[&JOINER.0].events[2];
    let b = &coordinator.peers[&J2.0].events[0];
    assert!(Arc::ptr_eq(a, b));
    let ProtocolAuthorityEvent::GrabAccepted(ack) = &event.event else {
        panic!()
    };
    let PieceTarget::Dense(dense) = &ack.accepted else {
        panic!()
    };
    let ProtocolAuthorityEvent::GrabAccepted(retained) = &a.envelope.event else {
        panic!()
    };
    let PieceTarget::Dense(retained_dense) = &retained.accepted else {
        panic!()
    };
    assert!(Arc::ptr_eq(
        dense.members.words(),
        retained_dense.members.words()
    ));
    coordinator
        .mark_baseline_installed(JOINER, 0, j1.baseline.snapshot.cursor)
        .unwrap();
    coordinator
        .mark_baseline_installed(J2, 0, j2.baseline.snapshot.cursor)
        .unwrap();
    let batch = coordinator.pending_events(JOINER, 0, 2).unwrap();
    assert_eq!(batch.len(), 2);
    assert_eq!(
        batch[0].cursor.sequence.0,
        j1.baseline.snapshot.cursor.sequence.0 + 1
    );
    assert_eq!(coordinator.pending_events(JOINER, 0, 2).unwrap(), batch);
    assert_eq!(coordinator.status(JOINER).unwrap().retained_events, 3);
    let j2_before = coordinator.status(J2).unwrap();
    coordinator
        .acknowledge_through(JOINER, 0, batch[1].cursor)
        .unwrap();
    assert_eq!(coordinator.status(J2).unwrap(), j2_before);
    assert_eq!(
        coordinator.pending_events(JOINER, 0, 1).unwrap().as_slice(),
        std::slice::from_ref(&event)
    );
    assert_eq!(coordinator.pending_events(J2, 0, 10).unwrap(), [event]);
}

#[test]
fn latest_updates_coalesce_in_player_order_without_baseline_duplicates_or_regressions() {
    let mut host = Host::new(64);
    host.grab(B, 0, [50]);
    host.update(B, 0, 9);
    host.grab(HOST, 40, [0]);
    let mut coordinator = JoinCatchUpCoordinator::default();
    let start = host.begin(&mut coordinator, JOINER).unwrap();
    assert!(coordinator.peers[&JOINER.0].latest_updates.is_empty());
    for tick in 10..=12 {
        let outcome = host.update(B, 0, tick);
        host.record(&mut coordinator, &outcome).unwrap();
    }
    let outcome = host.update(HOST, 40, 100);
    host.record(&mut coordinator, &outcome).unwrap();
    coordinator
        .mark_baseline_installed(JOINER, 0, start.baseline.snapshot.cursor)
        .unwrap();
    let latest = coordinator.latest_drag_updates(JOINER, 0).unwrap();
    assert_eq!(
        latest
            .iter()
            .map(|u| (u.player.0, u.tick))
            .collect::<Vec<_>>(),
        [(HOST.0, 100), (B.0, 12)]
    );
    let mut update = latest[1].clone();
    for (tick, error) in [
        (12, CatchUpError::DuplicateUpdate),
        (11, CatchUpError::StaleUpdate),
    ] {
        update.tick = tick;
        assert_eq!(
            coordinator.record_drag_update(&host.session, &host.store, &update),
            Err(error)
        );
    }
    update.tick = 13;
    update.basis_sequence = 1;
    assert_eq!(
        coordinator.record_drag_update(&host.session, &host.store, &update),
        Err(CatchUpError::WrongDragContext)
    );
    assert_eq!(coordinator.latest_drag_updates(JOINER, 0).unwrap(), latest);
}

#[test]
fn reliable_rebase_clears_old_transient_and_only_accepts_new_basis() {
    let mut host = Host::new(64);
    host.grab(HOST, 40, [0]);
    let mut coordinator = JoinCatchUpCoordinator::default();
    let start = host.begin(&mut coordinator, JOINER).unwrap();
    let old = host.update(HOST, 40, 100);
    host.record(&mut coordinator, &old).unwrap();
    let outcome = host.rotate_drag();
    host.record(&mut coordinator, &outcome).unwrap();
    assert!(coordinator.peers[&JOINER.0].latest_updates.is_empty());
    assert_eq!(
        host.record(&mut coordinator, &old),
        Err(CatchUpError::WrongDragContext)
    );
    let outcome = host.update(HOST, 42, 101);
    host.record(&mut coordinator, &outcome).unwrap();
    let mut peer = Peer::install(&host, &start, &mut coordinator);
    let latest = coordinator.latest_drag_updates(JOINER, 0).unwrap();
    assert_eq!((latest[0].basis_sequence, latest[0].tick), (42, 101));
    peer.catch_up(&host, &mut coordinator, 0);
    peer.assert_matches(&host);
    // ACK pruning must not discard the scalar basis/tick guards.
    assert_eq!(
        host.record(&mut coordinator, &outcome),
        Err(CatchUpError::DuplicateUpdate)
    );
}

#[test]
fn release_and_cancellation_remove_latest_and_new_grab_cannot_reuse_old_basis() {
    for cancel in [false, true] {
        let mut host = Host::new(64);
        host.grab(HOST, 0, [0]);
        let mut coordinator = JoinCatchUpCoordinator::default();
        host.begin(&mut coordinator, JOINER).unwrap();
        let old = host.update(HOST, 0, 10);
        host.record(&mut coordinator, &old).unwrap();
        if cancel {
            let cancellation = host
                .contexts
                .cancel_replicated(&mut host.session, &mut host.store, HOST)
                .unwrap()
                .unwrap();
            host.record_event(&mut coordinator, &cancellation.authority_event)
                .unwrap();
        } else {
            let outcome = host.release(HOST, 1, 0);
            host.record(&mut coordinator, &outcome).unwrap();
        }
        assert!(coordinator.peers[&JOINER.0].latest_updates.is_empty());
        assert_eq!(
            host.record(&mut coordinator, &old),
            Err(CatchUpError::MissingDragContext)
        );
        let outcome = host.grab(HOST, 2, [10]);
        host.record(&mut coordinator, &outcome).unwrap();
        assert_eq!(
            host.record(&mut coordinator, &old),
            Err(CatchUpError::WrongDragContext)
        );
    }
}

#[test]
fn event_count_overflow_drops_allocations_without_changing_gameplay() {
    for limit in [0, 2] {
        let mut host = Host::new(64);
        host.grab(HOST, 0, [0]);
        let mut coordinator = count_limits(limit);
        host.begin(&mut coordinator, JOINER).unwrap();
        let outcome = host.update(HOST, 0, 10);
        host.record(&mut coordinator, &outcome).unwrap();
        let mut weak = None;
        for sequence in 0..=limit as u64 {
            let outcome = host.rotate_idle(sequence);
            let cursor = host.session.cursor();
            let states = host.store.states.clone();
            host.record(&mut coordinator, &outcome).unwrap();
            assert_eq!(host.session.cursor(), cursor);
            assert_eq!(host.store.states, states);
            if sequence == 0 && limit > 0 {
                weak = Some(Arc::downgrade(&coordinator.peers[&JOINER.0].events[0]));
            }
        }
        assert_discarded(&coordinator, JOINER, CatchUpRestartReason::EventCountLimit);
        if let Some(weak) = weak {
            assert!(weak.upgrade().is_none());
        }
    }
}

#[test]
fn dense_event_exceeds_byte_budget_before_event_count_limit() {
    let mut host = Host::new(4096);
    let mut coordinator = JoinCatchUpCoordinator::new(CatchUpLimits {
        max_events: 100,
        max_retained_bytes: size_of::<ProtocolAuthorityEventEnvelope>() + 100,
    });
    host.begin(&mut coordinator, JOINER).unwrap();
    let outcome = host.grab(HOST, 0, 0..4000);
    let event = outcome.authority_event.as_ref().unwrap();
    let ProtocolAuthorityEvent::GrabAccepted(ack) = &event.event else {
        panic!()
    };
    assert!(matches!(ack.accepted, PieceTarget::Dense(_)));
    assert_eq!(
        logical_retained_bytes(event),
        size_of::<ProtocolAuthorityEventEnvelope>() + 512
    );
    host.record(&mut coordinator, &outcome).unwrap();
    assert_discarded(
        &coordinator,
        JOINER,
        CatchUpRestartReason::RetainedByteLimit,
    );
}

#[test]
fn logical_bytes_cover_dynamic_targets_rejections_and_fixed_events_without_copying() {
    let reference = ComponentRef {
        member: PieceId(0),
        expected_size: 1,
    };
    let rejected = RejectedComponentRef {
        reference,
        reason: TargetError::StaleComponent,
    };
    let base = size_of::<ProtocolAuthorityEventEnvelope>();
    let mut event = ProtocolAuthorityEventEnvelope {
        session: SESSION.id,
        host: HOST,
        cursor: AuthorityCursor::new(3, 1),
        event: ProtocolAuthorityEvent::GrabAccepted(GrabAccepted {
            player: HOST,
            grab_sequence: 0,
            accepted: PieceTarget::Components(vec![reference; 3]),
            rejected: vec![rejected; 2],
        }),
    };
    assert_eq!(
        logical_retained_bytes(&event),
        base + 3 * size_of::<ComponentRef>() + 2 * size_of::<RejectedComponentRef>()
    );
    let target = PieceTarget::Dense(DenseTarget {
        members: PieceBitSet::new(puzzella_core::MAX_PIECES),
        component_count: 0,
        topology_digest: 0,
    });
    event.event = ProtocolAuthorityEvent::RotationCommitted(RotationCommitted {
        player: HOST,
        accepted: target,
        quarter_turns: 1,
        result: ReleaseResultFingerprint(0),
    });
    let ProtocolAuthorityEvent::RotationCommitted(commit) = &event.event else {
        panic!()
    };
    let PieceTarget::Dense(dense) = &commit.accepted else {
        panic!()
    };
    let count = Arc::strong_count(dense.members.words());
    for _ in 0..100 {
        assert_eq!(logical_retained_bytes(&event), base + 125_000);
    }
    assert_eq!(Arc::strong_count(dense.members.words()), count);
    event.event = ProtocolAuthorityEvent::DragCancelled(DragCancelled {
        player: HOST,
        grab_sequence: 0,
    });
    assert_eq!(logical_retained_bytes(&event), base);
    assert_eq!(target_dynamic_bytes(&PieceTarget::Component(reference)), 0);
}

#[test]
fn slow_peer_overflows_alone_and_restart_recovers_with_stale_generation_protection() {
    let mut host = Host::new(64);
    host.grab(HOST, 0, [0]);
    let mut coordinator = count_limits(2);
    host.begin(&mut coordinator, JOINER).unwrap();
    for sequence in 0..2 {
        let outcome = host.rotate_idle(sequence);
        host.record(&mut coordinator, &outcome).unwrap();
    }
    let j2 = host.begin(&mut coordinator, J2).unwrap();
    coordinator
        .mark_baseline_installed(J2, 0, j2.baseline.snapshot.cursor)
        .unwrap();
    let outcome = host.rotate_idle(2);
    host.record(&mut coordinator, &outcome).unwrap();
    assert_discarded(&coordinator, JOINER, CatchUpRestartReason::EventCountLimit);
    assert_eq!(
        coordinator.status(J2).unwrap().phase,
        JoinCatchUpPhase::CatchingUp
    );
    let j2_before = coordinator.status(J2).unwrap();
    let start = host.restart(&mut coordinator).unwrap();
    assert_eq!(start.generation, 1);
    assert_eq!(start.baseline.snapshot.cursor, host.session.cursor());
    assert_eq!(coordinator.status(J2).unwrap(), j2_before);
    let before = coordinator.status(JOINER).unwrap();
    let mismatch = Err(CatchUpError::GenerationMismatch {
        expected: 1,
        received: 0,
    });
    assert_eq!(
        coordinator.mark_baseline_installed(JOINER, 0, start.baseline.snapshot.cursor),
        mismatch
    );
    assert_eq!(
        coordinator.pending_events(JOINER, 0, 10),
        Err(CatchUpError::GenerationMismatch {
            expected: 1,
            received: 0
        })
    );
    assert_eq!(
        coordinator.acknowledge_through(JOINER, 0, start.baseline.snapshot.cursor),
        mismatch
    );
    assert_eq!(
        coordinator.latest_drag_updates(JOINER, 0),
        Err(CatchUpError::GenerationMismatch {
            expected: 1,
            received: 0
        })
    );
    assert_eq!(coordinator.remove_join(JOINER, 0), mismatch);
    assert_eq!(coordinator.status(JOINER).unwrap(), before);
    let outcome = host.release(HOST, 1, 0);
    host.record(&mut coordinator, &outcome).unwrap();
    let mut peer = Peer::install(&host, &start, &mut coordinator);
    peer.catch_up(&host, &mut coordinator, 1);
    peer.assert_matches(&host);
    assert_eq!(host.caught_up(&mut coordinator, 1), Ok(true));
}

#[test]
fn failed_restart_capture_and_generation_exhaustion_preserve_failed_join() {
    let mut host = Host::new(64);
    let mut coordinator = count_limits(0);
    host.begin(&mut coordinator, JOINER).unwrap();
    let outcome = host.rotate_idle(0);
    host.record(&mut coordinator, &outcome).unwrap();
    let before = coordinator.status(JOINER).unwrap();
    let mut invalid = host.definition.clone();
    invalid.grid_size = UVec2::ZERO;
    assert!(matches!(
        coordinator.restart_join(JOINER, &host.session, &host.store, &host.contexts, &invalid),
        Err(CatchUpError::Baseline(_))
    ));
    assert_eq!(coordinator.status(JOINER).unwrap(), before);
    coordinator.peers.get_mut(&JOINER.0).unwrap().generation = u64::MAX;
    let before = coordinator.status(JOINER).unwrap();
    assert!(matches!(
        host.restart(&mut coordinator),
        Err(CatchUpError::GenerationExhausted)
    ));
    assert_eq!(coordinator.status(JOINER).unwrap(), before);
}

#[test]
fn missing_reliable_hook_invalidates_all_peers_and_new_baseline_recovers_observation() {
    let mut host = Host::new(64);
    let mut coordinator = JoinCatchUpCoordinator::default();
    host.begin(&mut coordinator, JOINER).unwrap();
    host.begin(&mut coordinator, J2).unwrap();
    host.rotate_idle(0);
    let outcome = host.rotate_idle(1);
    assert_eq!(
        host.record(&mut coordinator, &outcome),
        Err(CatchUpError::RestartRequired(
            CatchUpRestartReason::MissedAuthorityEvent
        ))
    );
    for player in [JOINER, J2] {
        assert_discarded(
            &coordinator,
            player,
            CatchUpRestartReason::MissedAuthorityEvent,
        );
    }
    let start = host.restart(&mut coordinator).unwrap();
    let outcome = host.rotate_idle(2);
    host.record(&mut coordinator, &outcome).unwrap();
    let mut peer = Peer::install(&host, &start, &mut coordinator);
    peer.catch_up(&host, &mut coordinator, 1);
    peer.assert_matches(&host);
}

#[test]
fn transient_detects_unrecorded_reliable_event() {
    let mut host = Host::new(64);
    let mut coordinator = JoinCatchUpCoordinator::default();
    host.begin(&mut coordinator, JOINER).unwrap();
    host.begin(&mut coordinator, J2).unwrap();
    host.grab(HOST, 0, [0]);
    let outcome = host.update(HOST, 0, 10);
    assert_eq!(
        host.record(&mut coordinator, &outcome),
        Err(CatchUpError::RestartRequired(
            CatchUpRestartReason::MissedAuthorityEvent
        ))
    );
    for player in [JOINER, J2] {
        assert_discarded(
            &coordinator,
            player,
            CatchUpRestartReason::MissedAuthorityEvent,
        );
    }
}

#[test]
fn caught_up_detects_missing_hook_even_when_queue_is_empty() {
    let mut host = Host::new(64);
    let mut coordinator = JoinCatchUpCoordinator::default();
    let start = host.begin(&mut coordinator, JOINER).unwrap();
    coordinator
        .mark_baseline_installed(JOINER, 0, start.baseline.snapshot.cursor)
        .unwrap();
    assert_eq!(host.caught_up(&mut coordinator, 0), Ok(true));
    host.rotate_idle(0);
    assert_eq!(
        host.caught_up(&mut coordinator, 0),
        Err(CatchUpError::RestartRequired(
            CatchUpRestartReason::MissedAuthorityEvent
        ))
    );
    assert_discarded(
        &coordinator,
        JOINER,
        CatchUpRestartReason::MissedAuthorityEvent,
    );
}

#[test]
fn new_join_cannot_mask_an_existing_missing_hook() {
    let mut host = Host::new(64);
    let mut coordinator = JoinCatchUpCoordinator::default();
    host.begin(&mut coordinator, JOINER).unwrap();
    host.rotate_idle(0);
    let j2 = host.begin(&mut coordinator, J2).unwrap();
    assert_discarded(
        &coordinator,
        JOINER,
        CatchUpRestartReason::MissedAuthorityEvent,
    );
    assert_eq!(j2.baseline.snapshot.cursor, host.session.cursor());
    let outcome = host.rotate_idle(1);
    host.record(&mut coordinator, &outcome).unwrap();
    assert_eq!(coordinator.status(J2).unwrap().retained_events, 1);
}

#[test]
fn record_requires_current_session_cursor_and_rejects_batched_generation() {
    let mut host = Host::new(64);
    let mut coordinator = JoinCatchUpCoordinator::default();
    host.begin(&mut coordinator, JOINER).unwrap();
    let first = host.rotate_idle(0);
    host.rotate_idle(1);
    assert_eq!(
        host.record(&mut coordinator, &first),
        Err(CatchUpError::EventCursorMismatch)
    );
    assert_discarded(
        &coordinator,
        JOINER,
        CatchUpRestartReason::MissedAuthorityEvent,
    );
}

#[test]
fn duplicate_stale_and_wrong_event_identity_do_not_mutate_history() {
    let mut host = Host::new(64);
    let mut coordinator = JoinCatchUpCoordinator::default();
    host.begin(&mut coordinator, JOINER).unwrap();
    let first = host.rotate_idle(0);
    host.record(&mut coordinator, &first).unwrap();
    let second = host.rotate_idle(1);
    host.record(&mut coordinator, &second).unwrap();
    let before = coordinator.status(JOINER).unwrap();
    assert_eq!(
        host.record(&mut coordinator, &second),
        Err(CatchUpError::DuplicateEvent)
    );
    assert_eq!(
        host.record(&mut coordinator, &first),
        Err(CatchUpError::StaleEvent)
    );
    for error in [
        CatchUpError::WrongSession,
        CatchUpError::WrongHost,
        CatchUpError::WrongEpoch,
    ] {
        let mut event = second.authority_event.clone().unwrap();
        match error {
            CatchUpError::WrongSession => event.session = SessionId(999),
            CatchUpError::WrongHost => event.host = B,
            _ => event.cursor.epoch = AuthorityEpoch(999),
        }
        assert_eq!(host.record_event(&mut coordinator, &event), Err(error));
    }
    assert_eq!(coordinator.status(JOINER).unwrap(), before);
}

#[test]
fn baseline_ack_phase_cursor_and_cumulative_event_ack_are_checked() {
    let mut host = Host::new(64);
    let mut coordinator = JoinCatchUpCoordinator::default();
    let start = host.begin(&mut coordinator, JOINER).unwrap();
    let c = start.baseline.snapshot.cursor;
    let wrong = AuthorityCursor::new(c.epoch.0, c.sequence.0 + 1);
    assert_eq!(
        coordinator.mark_baseline_installed(JOINER, 0, wrong),
        Err(CatchUpError::WrongBaselineCursor)
    );
    assert!(matches!(
        coordinator.pending_events(JOINER, 0, 1),
        Err(CatchUpError::WrongPhase(_))
    ));
    assert!(matches!(
        coordinator.acknowledge_through(JOINER, 0, c),
        Err(CatchUpError::WrongPhase(_))
    ));
    coordinator.mark_baseline_installed(JOINER, 0, c).unwrap();
    assert!(matches!(
        coordinator.mark_baseline_installed(JOINER, 0, c),
        Err(CatchUpError::WrongPhase(_))
    ));
    let first = host.rotate_idle(0);
    host.record(&mut coordinator, &first).unwrap();
    let second = host.rotate_idle(1);
    host.record(&mut coordinator, &second).unwrap();
    assert_eq!(coordinator.pending_events(JOINER, 0, 0).unwrap(), []);
    let cursor = first.authority_event.unwrap().cursor;
    let bytes = coordinator.peers[&JOINER.0].events[1].logical_bytes;
    coordinator.acknowledge_through(JOINER, 0, cursor).unwrap();
    let before = coordinator.status(JOINER).unwrap();
    assert_eq!(before.retained_events, 1);
    assert_eq!(before.retained_bytes, bytes);
    coordinator.acknowledge_through(JOINER, 0, cursor).unwrap();
    assert_eq!(
        coordinator.acknowledge_through(JOINER, 0, c),
        Err(CatchUpError::StaleAcknowledgement)
    );
    assert_eq!(
        coordinator.acknowledge_through(JOINER, 0, AuthorityCursor::new(3, 100)),
        Err(CatchUpError::FutureAcknowledgement)
    );
    assert_eq!(
        coordinator.acknowledge_through(JOINER, 0, AuthorityCursor::new(4, 1)),
        Err(CatchUpError::WrongEpoch)
    );
    assert_eq!(coordinator.status(JOINER).unwrap(), before);
    assert_eq!(host.caught_up(&mut coordinator, 0), Ok(false));
    coordinator
        .acknowledge_through(JOINER, 0, second.authority_event.unwrap().cursor)
        .unwrap();
    assert_eq!(coordinator.status(JOINER).unwrap().retained_bytes, 0);
    assert_eq!(host.caught_up(&mut coordinator, 0), Ok(true));
}

#[test]
fn scope_changes_and_migration_freeze_invalidate_all_join_state() {
    for change in 0..5 {
        let mut host = Host::new(64);
        host.grab(HOST, 0, [0]);
        let mut coordinator = JoinCatchUpCoordinator::default();
        let start = host.begin(&mut coordinator, JOINER).unwrap();
        host.begin(&mut coordinator, J2).unwrap();
        coordinator
            .mark_baseline_installed(JOINER, 0, start.baseline.snapshot.cursor)
            .unwrap();
        let outcome = host.update(HOST, 0, 10);
        host.record(&mut coordinator, &outcome).unwrap();
        let outcome = host.rotate_idle(0);
        host.record(&mut coordinator, &outcome).unwrap();
        match change {
            0 => host.session = AuthoritySession::new(SESSION, HOST, AuthorityCursor::new(4, 0)),
            1 => host.store.epoch += 1,
            2 => host.session = AuthoritySession::new(SESSION, B, host.session.cursor()),
            3 => {
                host.session = AuthoritySession::new(
                    SessionDefinition {
                        id: SessionId(999),
                        ..SESSION
                    },
                    HOST,
                    host.session.cursor(),
                )
            }
            _ => host.session.begin_graceful(B).unwrap(),
        }
        let reason = if change == 4 {
            CatchUpRestartReason::AuthorityFrozen
        } else {
            CatchUpRestartReason::ScopeChanged
        };
        assert_eq!(
            host.caught_up(&mut coordinator, 0),
            Err(CatchUpError::RestartRequired(reason))
        );
        for player in [JOINER, J2] {
            assert_discarded(&coordinator, player, reason);
        }
    }
}

#[test]
fn duplicate_host_and_capacity_joins_reject_without_breaking_existing_sync() {
    let host = Host::new(4);
    let mut coordinator = JoinCatchUpCoordinator::default();
    assert!(matches!(
        host.begin(&mut coordinator, HOST),
        Err(CatchUpError::HostCannotJoin)
    ));
    host.begin(&mut coordinator, JOINER).unwrap();
    assert!(matches!(
        host.begin(&mut coordinator, JOINER),
        Err(CatchUpError::AlreadyJoining)
    ));
    assert!(matches!(
        host.restart(&mut coordinator),
        Err(CatchUpError::WrongPhase(_))
    ));
    for i in 1..MAX_PENDING_JOIN_SYNCS {
        host.begin(&mut coordinator, PlayerId(1000 + i as u64))
            .unwrap();
    }
    let before = coordinator.status(JOINER).unwrap();
    assert!(matches!(
        host.begin(&mut coordinator, J2),
        Err(CatchUpError::TooManyPendingJoins)
    ));
    assert_eq!(coordinator.peers.len(), MAX_PENDING_JOIN_SYNCS);
    assert_eq!(coordinator.status(JOINER).unwrap(), before);
}

#[test]
fn idle_outcome_helper_retains_nothing_and_last_join_removal_restores_idle_state() {
    let mut host = Host::new(64);
    let outcome = host.grab(HOST, 0, 0..40);
    let transient = host.update(HOST, 0, 10);
    let ProtocolAuthorityEvent::GrabAccepted(ack) =
        &outcome.authority_event.as_ref().unwrap().event
    else {
        panic!()
    };
    let PieceTarget::Dense(dense) = &ack.accepted else {
        panic!()
    };
    let count = Arc::strong_count(dense.members.words());
    let mut coordinator = JoinCatchUpCoordinator::default();
    for _ in 0..10_000 {
        host.record(&mut coordinator, &outcome).unwrap();
        host.record(&mut coordinator, &transient).unwrap();
    }
    assert!(coordinator.peers.is_empty());
    assert!(coordinator.scope.is_none());
    assert!(coordinator.observed_cursor.is_none());
    assert_eq!(Arc::strong_count(dense.members.words()), count);
    host.begin(&mut coordinator, JOINER).unwrap();
    coordinator.remove_join(JOINER, 0).unwrap();
    assert!(coordinator.peers.is_empty());
    assert!(coordinator.scope.is_none());
    assert!(coordinator.observed_cursor.is_none());
    assert_eq!(coordinator.status(JOINER), Err(CatchUpError::NotJoining));
}

#[test]
fn active_drag_metadata_has_a_defensive_bound() {
    let mut host = Host::new(128);
    let mut coordinator = JoinCatchUpCoordinator::default();
    host.begin(&mut coordinator, JOINER).unwrap();
    for i in 0..=MAX_BASELINE_DRAGS {
        let outcome = host.grab(PlayerId(1000 + i as u64), 0, [i as u32 + 2]);
        host.record(&mut coordinator, &outcome).unwrap();
    }
    assert_discarded(
        &coordinator,
        JOINER,
        CatchUpRestartReason::TooManyActiveDrags,
    );
}

#[test]
fn zero_byte_budget_and_exact_budget_boundary_are_safe_and_ack_reclaims_budget() {
    let mut host = Host::new(64);
    let base = size_of::<ProtocolAuthorityEventEnvelope>();
    let mut zero = JoinCatchUpCoordinator::new(CatchUpLimits {
        max_events: 10,
        max_retained_bytes: 0,
    });
    let mut exact = JoinCatchUpCoordinator::new(CatchUpLimits {
        max_events: 10,
        max_retained_bytes: base + size_of::<ComponentRef>() * 2,
    });
    host.begin(&mut zero, JOINER).unwrap();
    let start = host.begin(&mut exact, JOINER).unwrap();
    exact
        .mark_baseline_installed(JOINER, 0, start.baseline.snapshot.cursor)
        .unwrap();
    let outcome = host.grab(HOST, 0, [0, 2]);
    host.record(&mut zero, &outcome).unwrap();
    host.record(&mut exact, &outcome).unwrap();
    assert_discarded(&zero, JOINER, CatchUpRestartReason::RetainedByteLimit);
    assert_eq!(
        exact.status(JOINER).unwrap().retained_bytes,
        exact.limits.max_retained_bytes
    );
    exact
        .acknowledge_through(JOINER, 0, host.session.cursor())
        .unwrap();
    let outcome = host.release(HOST, 1, 0);
    host.record(&mut exact, &outcome).unwrap();
    assert_eq!(exact.status(JOINER).unwrap().retained_bytes, base);
    let outcome = host.rotate_idle(0);
    host.record(&mut exact, &outcome).unwrap();
    assert_discarded(&exact, JOINER, CatchUpRestartReason::RetainedByteLimit);
}

#[test]
fn transient_validation_includes_baseline_tick_identity_and_finite_delta() {
    let mut host = Host::new(64);
    host.grab(HOST, 0, [0]);
    let outcome = host.update(HOST, 0, 10);
    let mut coordinator = JoinCatchUpCoordinator::default();
    host.begin(&mut coordinator, JOINER).unwrap();
    let before = coordinator.status(JOINER).unwrap();
    assert_eq!(
        host.record(&mut coordinator, &outcome),
        Err(CatchUpError::DuplicateUpdate)
    );
    let update = outcome.drag_update.unwrap();
    for error in [
        CatchUpError::WrongSession,
        CatchUpError::WrongEpoch,
        CatchUpError::InvalidDelta,
        CatchUpError::WrongDragContext,
    ] {
        let mut invalid = update.clone();
        invalid.tick = 11;
        match error {
            CatchUpError::WrongSession => invalid.session = SessionId(999),
            CatchUpError::WrongEpoch => invalid.authority_epoch = AuthorityEpoch(999),
            CatchUpError::InvalidDelta => invalid.delta.x = f32::NAN,
            _ => invalid.grab_sequence = 1,
        }
        assert_eq!(
            coordinator.record_drag_update(&host.session, &host.store, &invalid),
            Err(error)
        );
    }
    assert_eq!(coordinator.status(JOINER).unwrap(), before);
}

#[test]
fn recording_paths_detect_scope_changes_without_a_caught_up_query() {
    for transient in [false, true] {
        let mut host = Host::new(64);
        host.grab(HOST, 0, [0]);
        let mut coordinator = JoinCatchUpCoordinator::default();
        host.begin(&mut coordinator, JOINER).unwrap();
        host.begin(&mut coordinator, J2).unwrap();
        let outcome = if transient {
            host.update(HOST, 0, 10)
        } else {
            host.rotate_idle(0)
        };
        host.store.epoch += 1;
        assert_eq!(
            host.record(&mut coordinator, &outcome),
            Err(CatchUpError::RestartRequired(
                CatchUpRestartReason::ScopeChanged
            ))
        );
        for player in [JOINER, J2] {
            assert_discarded(&coordinator, player, CatchUpRestartReason::ScopeChanged);
        }
    }
}

#[test]
fn failed_begin_capture_and_empty_outcome_do_not_change_existing_sync() {
    let host = Host::new(64);
    let mut coordinator = JoinCatchUpCoordinator::default();
    host.begin(&mut coordinator, JOINER).unwrap();
    let before = coordinator.status(JOINER).unwrap();
    let mut invalid = host.definition.clone();
    invalid.grid_size = UVec2::ZERO;
    assert!(matches!(
        coordinator.begin_join(J2, &host.session, &host.store, &host.contexts, &invalid),
        Err(CatchUpError::Baseline(_))
    ));
    assert_eq!(coordinator.status(J2), Err(CatchUpError::NotJoining));
    assert_eq!(coordinator.status(JOINER).unwrap(), before);
    let result = super::super::protocol::ProtocolCommandResult::DragUpdated {
        sequence: puzzella_core::session::CommandSequenceStatus::InOrder,
    };
    let outcome = HostCommandOutcome {
        result,
        authority_event: None,
        drag_update: None,
    };
    host.record(&mut coordinator, &outcome).unwrap();
    assert_eq!(coordinator.status(JOINER).unwrap(), before);
}
