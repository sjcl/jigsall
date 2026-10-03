use super::*;
#[path = "baseline_cancellation_tests.rs"]
mod cancellation;
use crate::{
    multiplayer::{
        replication::{PeerReplicationState, ReplicationError},
        SNAPSHOT_SCHEMA_VERSION,
    },
    resources::pieces::{prepare_piece_upload, PieceUpload},
};
use bevy::{
    math::UVec2,
    prelude::{App, Update},
};
use puzzella_core::protocol::{
    ProtocolAuthorityEventEnvelope, ProtocolCommandEnvelope, ProtocolPieceCommand, RemoteDragUpdate,
};
use puzzella_core::{
    protocol::ComponentRef,
    session::{
        AuthorityCursor, ClientCommandSequence, CommandSequenceStatus, ImageHash,
        SessionDefinition, SessionId,
    },
    GENERATOR_VERSION,
};
use std::sync::Arc;

const HOST: PlayerId = PlayerId(7);
const JOINER: PlayerId = PlayerId(99);
const SESSION: SessionDefinition = SessionDefinition {
    id: SessionId(123),
    image_hash: ImageHash([7; 32]),
};

struct Host {
    definition: PuzzleDefinition,
    session: AuthoritySession,
    store: PieceDataStore,
    contexts: ProtocolDragContexts,
    next_control: std::collections::HashMap<PlayerId, u64>,
}
impl Host {
    fn new(count: usize, links: &[(u32, u32)]) -> Self {
        let grid_size = if count == puzzella_core::MAX_PIECES {
            UVec2::splat(1000)
        } else {
            UVec2::new(count as u32, 1)
        };
        let definition = PuzzleDefinition {
            generator_version: GENERATOR_VERSION,
            seed: 42,
            grid_size,
            image_size: puzzella_core::fit_image_size(
                grid_size * 200,
                puzzella_core::MAX_PUZZLE_IMAGE_DIMENSION,
            ),
            snap_distance: 5.,
        };
        definition.validate().unwrap();
        let mut store = PieceDataStore::default();
        store.initialize(
            (0..count as u32)
                .map(|id| definition.correct_position(PieceId(id)) + Vec2::splat(1000.))
                .collect(),
        );
        for &(a, b) in links {
            store.connectivity.union(PieceId(a), PieceId(b));
        }
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
            store,
            session: AuthoritySession::new(SESSION, HOST, AuthorityCursor::new(3, 0)),
            contexts: ProtocolDragContexts::default(),
            next_control: std::collections::HashMap::new(),
        }
    }
    fn command(
        &mut self,
        player: PlayerId,
        sequence: ClientCommandSequence,
        command: ProtocolPieceCommand,
    ) -> super::super::protocol::HostCommandOutcome {
        let authority_epoch = self.session.cursor().epoch;
        if let ClientCommandSequence::Control(control) = sequence {
            let next = self.next_control.entry(player).or_default();
            // Model earlier consumed controls without producing gameplay/events,
            // so tests can use the requested grab=40 / rotation basis=42 examples.
            for previous in *next..control {
                self.session
                    .accept_command(&ProtocolCommandEnvelope {
                        session: SESSION.id,
                        authority_epoch,
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
                    authority_epoch,
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
    ) -> ProtocolAuthorityEventEnvelope {
        let mut mask = PieceBitSet::new(self.store.len());
        mask.extend(ids.into_iter().map(PieceId));
        let target = PieceTarget::from_selection(&self.store.connectivity, &mask).unwrap();
        self.command(
            player,
            ClientCommandSequence::Control(sequence),
            ProtocolPieceCommand::Grab { target },
        )
        .authority_event
        .unwrap()
    }
    fn update(&mut self, player: PlayerId, basis: u64, tick: u64, delta: Vec2) -> RemoteDragUpdate {
        self.command(
            player,
            ClientCommandSequence::Move {
                after_control_sequence: basis,
                tick,
            },
            ProtocolPieceCommand::DragUpdate { delta },
        )
        .drag_update
        .unwrap()
    }
    fn release(
        &mut self,
        player: PlayerId,
        sequence: u64,
        grab_sequence: u64,
        final_delta: Vec2,
    ) -> ProtocolAuthorityEventEnvelope {
        self.command(
            player,
            ClientCommandSequence::Control(sequence),
            ProtocolPieceCommand::Release {
                grab_sequence,
                final_delta,
            },
        )
        .authority_event
        .unwrap()
    }
    fn rotate(
        &mut self,
        sequence: u64,
        grab_sequence: u64,
        tick: Option<u64>,
        delta: Vec2,
    ) -> ProtocolAuthorityEventEnvelope {
        self.command(
            HOST,
            ClientCommandSequence::Control(sequence),
            ProtocolPieceCommand::RotateDrag {
                grab_sequence,
                through_tick: tick,
                final_delta: delta,
                quarter_turns: 1,
            },
        )
        .authority_event
        .unwrap()
    }
    fn capture(&self) -> JoinBaseline {
        JoinBaseline::capture(&self.session, &self.store, &self.contexts, &self.definition).unwrap()
    }
    fn expected<'a>(&'a self, baseline: &JoinBaseline) -> SnapshotExpectation<'a> {
        SnapshotExpectation {
            session: SESSION.id,
            image_hash: SESSION.image_hash,
            cursor: baseline.snapshot.cursor,
            definition: &self.definition,
        }
    }
}
struct Peer {
    session: AuthoritySession,
    store: PieceDataStore,
    replica: PeerReplicationState,
}
impl Peer {
    fn new() -> Self {
        Self {
            session: AuthoritySession::new(SESSION, HOST, AuthorityCursor::new(3, 0)),
            store: PieceDataStore::default(),
            replica: PeerReplicationState::default(),
        }
    }
    fn install(&mut self, host: &Host, baseline: &JoinBaseline) -> Result<(), JoinBaselineError> {
        self.replica.install_join_baseline(
            &mut self.session,
            &mut self.store,
            baseline,
            host.expected(baseline),
        )
    }
    fn event(&mut self, host: &Host, event: &ProtocolAuthorityEventEnvelope) {
        self.replica
            .apply_event(
                &mut self.session,
                &mut self.store,
                HOST,
                event,
                Some(&host.definition),
                JOINER,
            )
            .unwrap();
    }
    fn update(
        &mut self,
        update: &RemoteDragUpdate,
    ) -> Result<CommandSequenceStatus, ReplicationError> {
        self.replica
            .apply_drag_update(&self.session, &self.store, HOST, update)
    }
    fn assert_matches(&self, host: &Host) {
        assert_eq!(self.session.cursor(), host.session.cursor());
        assert_eq!(self.store.next_z_order, host.store.next_z_order);
        assert_eq!(self.store.held_by, host.store.held_by);
        assert_eq!(self.store.placed_count, host.store.placed_count);
        assert_eq!(self.store.states, host.store.states);
        for id in 0..self.store.len() as u32 {
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
        for (player, drag) in host.contexts.active_drags(&host.session, &host.store) {
            assert_eq!(
                self.replica.remote_drag(&self.session, &self.store, player),
                Some(drag)
            );
        }
    }
}

#[test]
fn sparse_and_dense_join_continue_with_update_and_release_without_replaying_grab() {
    for dense in [false, true] {
        let mut host = Host::new(64, &[(0, 1)]);
        host.grab(
            HOST,
            40,
            if dense {
                (0..40).collect::<Vec<_>>()
            } else {
                vec![0]
            },
        );
        host.update(HOST, 40, 100, Vec2::new(70., 20.));
        let mut local_members = PieceBitSet::new(host.store.len());
        local_members.extend(host.store.held_by.iter().map(|(id, _)| id));
        host.store.drag.members = local_members.words().clone();
        host.store.drag.delta = Vec2::new(70., 20.);
        host.store.selected_pieces = local_members;
        let presentation = host.store.drag.clone();
        let selection = host.store.selected_pieces.clone();
        let states = host.store.states.clone();
        let baseline = host.capture();
        assert_eq!(host.store.states, states);
        assert!(Arc::ptr_eq(&host.store.drag.members, &presentation.members));
        assert_eq!(host.store.drag.delta, presentation.delta);
        assert_eq!(host.store.selected_pieces, selection);
        assert_eq!(baseline.snapshot.cursor, host.session.cursor());
        assert_eq!(baseline.snapshot.schema_version, SNAPSHOT_SCHEMA_VERSION);
        if dense {
            let PieceTarget::Dense(target) = &baseline.active_drags[0].target else {
                panic!()
            };
            let ActiveDragTarget::Dense(authority) = &host
                .contexts
                .active_drag(&host.session, &host.store, HOST)
                .unwrap()
                .target
            else {
                panic!()
            };
            assert!(Arc::ptr_eq(
                target.members.words(),
                authority.members.words()
            ));
        }
        // These are the only events the new peer receives: no GrabAccepted.
        let update = host.update(HOST, 40, 101, Vec2::new(90., 30.));
        let mut peer = Peer::new();
        peer.install(&host, &baseline).unwrap();
        assert_eq!(peer.store.states, states);
        assert_eq!(peer.store.next_z_order, baseline.snapshot.next_z_order);
        assert_eq!(peer.session.cursor(), baseline.snapshot.cursor);
        assert!(peer.store.drag.members.is_empty());
        assert_eq!(peer.store.drag.delta, Vec2::ZERO);
        assert!(peer.store.selected_pieces.is_empty());
        assert_eq!(
            peer.replica
                .remote_drag(&peer.session, &peer.store, HOST)
                .unwrap()
                .delta,
            Vec2::new(70., 20.)
        );
        assert_eq!(peer.update(&update), Ok(CommandSequenceStatus::InOrder));
        peer.assert_matches(&host);
        let release = host.release(HOST, 41, 40, Vec2::new(120., 40.));
        peer.event(&host, &release);
        peer.assert_matches(&host);
        assert!(peer.store.held_by.is_empty());
        assert_eq!(
            peer.replica
                .remote_drags(&peer.session, &peer.store)
                .count(),
            0
        );
    }
}

#[test]
fn baseline_before_and_after_rotation_preserves_basis_tick_and_fingerprint() {
    for dense in [false, true] {
        let mut host = Host::new(64, &[(0, 1)]);
        host.grab(
            HOST,
            40,
            if dense {
                (0..40).collect::<Vec<_>>()
            } else {
                vec![0]
            },
        );
        host.update(HOST, 40, 100, Vec2::new(70., 20.));
        let before = host.capture();
        let rotation = host.rotate(42, 40, Some(100), Vec2::new(70., 20.));
        let mut peer = Peer::new();
        peer.install(&host, &before).unwrap();
        peer.event(&host, &rotation); // fingerprint checked by existing peer apply
        peer.assert_matches(&host);
        host.update(HOST, 42, 101, Vec2::new(5., 8.));
        let after = host.capture();
        let mut later = Peer::new();
        later.install(&host, &after).unwrap();
        later.assert_matches(&host);
        let update = host.update(HOST, 42, 102, Vec2::new(10., 12.));
        assert_eq!(
            later.update(&RemoteDragUpdate {
                basis_sequence: 40,
                ..update.clone()
            }),
            Err(ReplicationError::WrongDragContext)
        );
        assert_eq!(later.update(&update), Ok(CommandSequenceStatus::InOrder));
        let release = host.release(HOST, 43, 40, Vec2::new(20., 25.));
        later.event(&host, &release);
        later.assert_matches(&host);
    }
}

#[test]
fn restored_last_tick_rejects_stale_duplicate_and_tracks_gaps() {
    let mut host = Host::new(2, &[]);
    host.grab(HOST, 40, [0]);
    let update = host.update(HOST, 40, 100, Vec2::ONE);
    let baseline = host.capture();
    let mut peer = Peer::new();
    peer.install(&host, &baseline).unwrap();
    for (tick, result) in [
        (99, Err(ReplicationError::StaleUpdate)),
        (100, Err(ReplicationError::DuplicateUpdate)),
        (101, Ok(CommandSequenceStatus::InOrder)),
    ] {
        assert_eq!(
            peer.update(&RemoteDragUpdate {
                tick,
                ..update.clone()
            }),
            result
        );
    }
    peer.install(&host, &baseline).unwrap();
    assert_eq!(
        peer.update(&RemoteDragUpdate {
            tick: 105,
            ..update
        }),
        Ok(CommandSequenceStatus::Gap { expected: 101 })
    );
    assert_eq!(peer.session.cursor(), baseline.snapshot.cursor);
}

#[test]
fn multiple_players_restore_in_stable_order_and_release_independently() {
    let mut host = Host::new(6, &[(0, 1), (3, 4)]);
    let other = PlayerId(2);
    host.grab(HOST, 40, [0]);
    host.grab(other, 8, [3]);
    host.update(HOST, 40, 100, Vec2::ONE);
    host.update(other, 8, 17, Vec2::splat(20.));
    let baseline = host.capture();
    assert_eq!(
        baseline
            .active_drags
            .iter()
            .map(|drag| drag.player)
            .collect::<Vec<_>>(),
        [other, HOST]
    );
    assert_eq!(
        serde_json::to_vec(&baseline).unwrap(),
        serde_json::to_vec(&host.capture()).unwrap()
    );
    let mut peer = Peer::new();
    peer.install(&host, &baseline).unwrap();
    peer.assert_matches(&host);
    assert_eq!(peer.store.held_by.get(&PieceId(1)), Some(&HOST));
    assert_eq!(peer.store.held_by.get(&PieceId(4)), Some(&other));
    let remaining = peer
        .replica
        .remote_drag(&peer.session, &peer.store, other)
        .unwrap()
        .clone();
    let release = host.release(HOST, 41, 40, Vec2::splat(30.));
    peer.event(&host, &release);
    peer.assert_matches(&host);
    assert_eq!(
        peer.replica.remote_drag(&peer.session, &peer.store, other),
        Some(&remaining)
    );
    assert_eq!(peer.store.held_by.get(&PieceId(4)), Some(&other));
}

#[test]
fn million_piece_dense_capture_and_restore_share_masks_without_member_refs() {
    let mut host = Host::new(puzzella_core::MAX_PIECES, &[]);
    host.grab(HOST, 40, 0..puzzella_core::MAX_PIECES as u32);
    host.update(HOST, 40, 91, Vec2::new(70., 20.));
    let baseline = host.capture();
    let PieceTarget::Dense(dense) = &baseline.active_drags[0].target else {
        panic!()
    };
    let ActiveDragTarget::Dense(authority) = &host
        .contexts
        .active_drag(&host.session, &host.store, HOST)
        .unwrap()
        .target
    else {
        panic!()
    };
    assert_eq!(dense.members.words().len() * 4, 125_000);
    assert!(Arc::ptr_eq(
        dense.members.words(),
        authority.members.words()
    ));
    let mut peer = Peer::new();
    peer.install(&host, &baseline).unwrap();
    let ActiveDragTarget::Dense(remote) = &peer
        .replica
        .remote_drag(&peer.session, &peer.store, HOST)
        .unwrap()
        .target
    else {
        panic!()
    };
    assert!(Arc::ptr_eq(remote.members.words(), dense.members.words()));
    assert_eq!(peer.store.held_by.len(), puzzella_core::MAX_PIECES);
    assert!(peer.store.held_by.has_player(HOST));
    assert!(peer.store.dirty_pieces.is_empty());
    assert!(peer.store.drag.members.is_empty());
    peer.assert_matches(&host);
}

#[test]
fn full_upload_contains_restored_holds_and_retired_upload_stays_immutable() {
    let mut host = Host::new(40, &[(0, 1)]);
    let mut peer = Peer::new();
    peer.store.initialize(vec![Vec2::ZERO; 40]);
    let mut app = App::new();
    app.insert_resource(peer.store)
        .init_resource::<PieceUpload>()
        .add_systems(Update, prepare_piece_upload);
    app.update();
    let old_upload = app.world().resource::<PieceUpload>().clone();
    host.grab(HOST, 40, 0..40);
    let baseline = host.capture();
    {
        let mut store = app.world_mut().resource_mut::<PieceDataStore>();
        peer.replica
            .install_join_baseline(
                &mut peer.session,
                &mut store,
                &baseline,
                host.expected(&baseline),
            )
            .unwrap();
        assert!(store.dirty_pieces.is_empty());
        assert!(store.component_root_dirty.is_empty());
    }
    app.update();
    let upload = app.world().resource::<PieceUpload>();
    assert_ne!(upload.epoch, old_upload.epoch);
    assert!(upload
        .initial
        .as_ref()
        .unwrap()
        .iter()
        .all(|state| state.flags & HELD != 0));
    assert_eq!(
        upload.initial.as_ref().unwrap().as_ref(),
        &*host.store.states
    );
    assert!(old_upload
        .initial
        .as_ref()
        .unwrap()
        .iter()
        .all(|state| state.flags & HELD == 0));
    assert!(upload.ranges.is_empty());
    assert!(upload.initial_roots.is_some());
    app.update();
    assert!(app.world().resource::<PieceUpload>().initial.is_none());
    assert!(app.world().resource::<PieceUpload>().ranges.is_empty());
}

fn assert_transactional_rejection(
    host: &Host,
    baseline: &JoinBaseline,
    expected: SnapshotExpectation<'_>,
    error: JoinBaselineError,
) {
    // A receiver already has live contexts, owner state, cursor and presentation.
    let mut existing = Host::new(host.store.len(), &[(0, 1)]);
    existing.grab(JOINER, 10, [10]);
    existing.update(JOINER, 10, 5, Vec2::splat(12.));
    let mut peer = Peer::new();
    peer.install(&existing, &existing.capture()).unwrap();
    peer.store.selected_pieces.fill();
    peer.store.drag.members = peer.store.selected_pieces.words().clone();
    peer.store.drag.delta = Vec2::splat(321.);
    peer.store.highlights_dirty = true;
    let states = peer.store.states.clone();
    let allocation = peer.store.states.as_ptr();
    let connectivity = peer.store.connectivity.clone();
    let owners = peer.store.held_by.clone();
    let selected = peer.store.selected_pieces.clone();
    let dirty = peer.store.dirty_pieces.clone();
    let roots = peer.store.component_root_dirty.clone();
    let drag = peer.store.drag.clone();
    let epoch = peer.store.epoch;
    let next_z = peer.store.next_z_order;
    let placed = peer.store.placed_count;
    let session = format!("{:?}", peer.session);
    let replica = format!("{:?}", peer.replica);
    assert_eq!(
        peer.replica
            .install_join_baseline(&mut peer.session, &mut peer.store, baseline, expected),
        Err(error)
    );
    assert_eq!(peer.store.states, states);
    assert_eq!(peer.store.states.as_ptr(), allocation);
    assert_eq!(peer.store.connectivity, connectivity);
    assert_eq!(peer.store.held_by, owners);
    assert_eq!(peer.store.selected_pieces, selected);
    assert_eq!(peer.store.dirty_pieces, dirty);
    assert_eq!(peer.store.component_root_dirty, roots);
    assert!(Arc::ptr_eq(&peer.store.drag.members, &drag.members));
    assert_eq!(peer.store.drag.delta, drag.delta);
    assert_eq!(peer.store.epoch, epoch);
    assert_eq!(peer.store.next_z_order, next_z);
    assert_eq!(peer.store.placed_count, placed);
    assert!(peer.store.highlights_dirty);
    assert_eq!(format!("{:?}", peer.session), session);
    assert_eq!(format!("{:?}", peer.replica), replica);
}

#[test]
fn invalid_baselines_reject_before_any_store_session_or_replica_changes() {
    let mut host = Host::new(64, &[(0, 1)]);
    host.grab(HOST, 40, [0]);
    host.grab(PlayerId(8), 50, [3]);
    let valid = host.capture();
    let invalid_target = |reason| JoinBaselineError::InvalidTarget {
        player: HOST,
        reason,
    };
    let mut cases = Vec::new();
    let mut bad = valid.clone();
    bad.schema_version = 2;
    cases.push((bad, JoinBaselineError::UnsupportedSchema(2)));
    let mut bad = valid.clone();
    bad.active_drags = vec![bad.active_drags[0].clone(); MAX_BASELINE_DRAGS + 1];
    cases.push((bad, JoinBaselineError::TooManyActiveDrags));
    let mut bad = valid.clone();
    bad.active_drags.push(bad.active_drags[0].clone());
    cases.push((bad, JoinBaselineError::DuplicatePlayer(HOST)));
    for delta in [
        Vec2::new(f32::NAN, 0.),
        Vec2::new(0., f32::INFINITY),
        Vec2::splat(f32::NEG_INFINITY),
    ] {
        let mut bad = valid.clone();
        bad.active_drags[0].delta = delta;
        cases.push((bad, JoinBaselineError::InvalidDelta(HOST)));
    }
    let mut bad = valid.clone();
    bad.active_drags[0].basis_sequence = 39;
    cases.push((bad, JoinBaselineError::InvalidSequence(HOST)));
    let mut bad = valid.clone();
    bad.active_drags[0].target = PieceTarget::Components(vec![]);
    cases.push((bad, JoinBaselineError::EmptyTarget(HOST)));
    for (reference, reason) in [
        (
            ComponentRef {
                member: PieceId(0),
                expected_size: 1,
            },
            TargetError::StaleComponent,
        ),
        (
            ComponentRef {
                member: PieceId(1),
                expected_size: 2,
            },
            TargetError::StaleComponent,
        ),
        (
            ComponentRef {
                member: PieceId(64),
                expected_size: 1,
            },
            TargetError::InvalidPieceId,
        ),
    ] {
        let mut bad = valid.clone();
        bad.active_drags[0].target = PieceTarget::Component(reference);
        cases.push((bad, invalid_target(reason)));
    }
    // Partial sparse acceptance is forbidden: one good and one stale ref fails.
    let mut bad = valid.clone();
    bad.active_drags[0].target = PieceTarget::Components(vec![
        ComponentRef {
            member: PieceId(0),
            expected_size: 2,
        },
        ComponentRef {
            member: PieceId(5),
            expected_size: 2,
        },
    ]);
    cases.push((bad, invalid_target(TargetError::StaleComponent)));
    let mut bad = valid.clone();
    bad.active_drags[1].target = bad.active_drags[0].target.clone();
    cases.push((bad, JoinBaselineError::OverlappingTarget(PieceId(0))));
    let mut bad = valid.clone();
    bad.snapshot.pieces[3].flags = SNAPSHOT_PLACED;
    bad.snapshot.pieces[3].position = host.definition.correct_position(PieceId(3));
    cases.push((bad, JoinBaselineError::PlacedTarget(PieceId(3))));
    let mut bad = valid.clone();
    bad.snapshot.pieces[4].position.x = f32::NAN;
    cases.push((
        bad,
        JoinBaselineError::Snapshot(SnapshotError::NonFinitePosition(PieceId(4))),
    ));
    for (field, error) in [
        (0, SnapshotError::WrongSession),
        (1, SnapshotError::WrongImageHash),
        (2, SnapshotError::WrongCursor),
        (3, SnapshotError::WrongDefinition),
        (4, SnapshotError::UnsupportedSchema(3)),
    ] {
        let mut bad = valid.clone();
        match field {
            0 => bad.snapshot.session = SessionId(456),
            1 => bad.snapshot.image_hash = ImageHash([1; 32]),
            2 => bad.snapshot.cursor.sequence.0 += 1,
            3 => bad.snapshot.definition.seed += 1,
            _ => bad.snapshot.schema_version = 3,
        }
        cases.push((bad, JoinBaselineError::Snapshot(error)));
    }
    for (bad, error) in cases {
        assert_transactional_rejection(&host, &bad, host.expected(&valid), error);
    }
    for cursor in [
        AuthorityCursor::new(3, 0),
        AuthorityCursor::new(4, 2),
        AuthorityCursor::new(2, 2),
    ] {
        let mut bad = valid.clone();
        bad.snapshot.cursor = cursor;
        assert_transactional_rejection(
            &host,
            &bad,
            host.expected(&bad),
            JoinBaselineError::Protocol(ProtocolError::WrongSnapshotCursor),
        );
    }
    let mut expected = host.expected(&valid);
    expected.session = SessionId(456);
    assert_transactional_rejection(
        &host,
        &valid,
        expected,
        JoinBaselineError::Protocol(ProtocolError::WrongSession),
    );
    expected = host.expected(&valid);
    expected.image_hash = ImageHash([9; 32]);
    assert_transactional_rejection(
        &host,
        &valid,
        expected,
        JoinBaselineError::Protocol(ProtocolError::WrongSession),
    );
}

#[test]
fn dense_topology_dimensions_exact_membership_and_mixed_overlap_are_validated() {
    let mut host = Host::new(64, &[(0, 1)]);
    host.grab(HOST, 40, 0..40);
    let valid = host.capture();
    let mut bad = valid.clone();
    let PieceTarget::Dense(target) = &mut bad.active_drags[0].target else {
        panic!()
    };
    target.topology_digest ^= 1;
    assert_transactional_rejection(
        &host,
        &bad,
        host.expected(&valid),
        JoinBaselineError::InvalidTarget {
            player: HOST,
            reason: TargetError::StaleTopology,
        },
    );
    let mut bad = valid.clone();
    let PieceTarget::Dense(target) = &mut bad.active_drags[0].target else {
        panic!()
    };
    target.members = PieceBitSet::new(63);
    assert_transactional_rejection(
        &host,
        &bad,
        host.expected(&valid),
        JoinBaselineError::InvalidTarget {
            player: HOST,
            reason: TargetError::InvalidMaskDimensions,
        },
    );
    let mut bad = valid.clone();
    let PieceTarget::Dense(target) = &mut bad.active_drags[0].target else {
        panic!()
    };
    target.members.remove(&PieceId(1)); // same topology, but not whole membership
    assert_transactional_rejection(
        &host,
        &bad,
        host.expected(&valid),
        JoinBaselineError::NonCanonicalTarget(HOST),
    );
    for target in [
        PieceTarget::Component(ComponentRef {
            member: PieceId(0),
            expected_size: 2,
        }),
        valid.active_drags[0].target.clone(),
    ] {
        let mut bad = valid.clone();
        bad.active_drags.push(BaselineDrag {
            player: PlayerId(8),
            target,
            ..bad.active_drags[0].clone()
        });
        assert_transactional_rejection(
            &host,
            &bad,
            host.expected(&valid),
            JoinBaselineError::OverlappingTarget(PieceId(0)),
        );
    }
    let mut bad = valid.clone();
    bad.active_drags[0].target = PieceTarget::Dense(
        DenseTarget::from_selection(&host.store.connectivity, &PieceBitSet::new(64)).unwrap(),
    );
    assert_transactional_rejection(
        &host,
        &bad,
        host.expected(&valid),
        JoinBaselineError::EmptyTarget(HOST),
    );
}

#[test]
fn fourth_invalid_drag_leaves_existing_receiver_and_diverged_state_intact() {
    let mut host = Host::new(64, &[]);
    for player in 0..5 {
        host.grab(PlayerId(player), 40, [player as u32 + 20]);
    }
    let valid = host.capture();
    let mut bad = valid.clone();
    bad.active_drags[3].delta.x = f32::INFINITY;
    assert_transactional_rejection(
        &host,
        &bad,
        host.expected(&valid),
        JoinBaselineError::InvalidDelta(PlayerId(3)),
    );
    let mut peer = Peer::new();
    peer.install(&host, &valid).unwrap();
    let mut event = host.release(PlayerId(0), 41, 40, Vec2::ONE);
    let puzzella_core::protocol::ProtocolAuthorityEvent::ReleaseCommitted(commit) =
        &mut event.event
    else {
        panic!()
    };
    commit.result.0 ^= 1;
    assert_eq!(
        peer.replica.apply_event(
            &mut peer.session,
            &mut peer.store,
            HOST,
            &event,
            Some(&host.definition),
            JOINER
        ),
        Err(ReplicationError::Diverged)
    );
    assert!(peer.replica.needs_resync(&peer.session, &peer.store));
    let epoch = peer.store.epoch;
    assert!(peer.install(&host, &bad).is_err());
    assert_eq!(peer.store.epoch, epoch);
    assert!(peer.replica.needs_resync(&peer.session, &peer.store));
    let latest = host.capture();
    peer.install(&host, &latest).unwrap();
    assert!(!peer.replica.needs_resync(&peer.session, &peer.store));
    peer.assert_matches(&host);
}

#[test]
fn capture_rejects_orphan_owner_wrong_owner_held_mirrors_and_stale_targets() {
    let mut host = Host::new(6, &[(0, 1)]);
    host.store.held_by.insert(PieceId(5), HOST);
    host.store.states[5].flags |= HELD;
    assert_eq!(
        JoinBaseline::capture(&host.session, &host.store, &host.contexts, &host.definition),
        Err(JoinBaselineError::MissingDragContext(PieceId(5)))
    );
    host.store.states[5].flags &= !HELD;
    assert_eq!(
        JoinBaseline::capture(&host.session, &host.store, &host.contexts, &host.definition),
        Err(JoinBaselineError::HoldMismatch(PieceId(5)))
    );
    host.store.held_by.remove(&PieceId(5));
    host.grab(HOST, 40, [0]);
    host.store.held_by.insert(PieceId(1), PlayerId(8));
    assert_eq!(
        JoinBaseline::capture(&host.session, &host.store, &host.contexts, &host.definition),
        Err(JoinBaselineError::OwnerMismatch {
            piece: PieceId(1),
            player: HOST
        })
    );
    host.store.held_by.insert(PieceId(1), HOST);
    host.store.states[1].flags &= !HELD;
    assert_eq!(
        JoinBaseline::capture(&host.session, &host.store, &host.contexts, &host.definition),
        Err(JoinBaselineError::HoldMismatch(PieceId(1)))
    );
    host.store.states[1].flags |= HELD;
    host.store.held_by.remove(&PieceId(1));
    assert_eq!(
        JoinBaseline::capture(&host.session, &host.store, &host.contexts, &host.definition),
        Err(JoinBaselineError::OwnerMismatch {
            piece: PieceId(1),
            player: HOST
        })
    );
    host.store.held_by.insert(PieceId(1), HOST);
    host.store.connectivity.union(PieceId(1), PieceId(2));
    assert_eq!(
        JoinBaseline::capture(&host.session, &host.store, &host.contexts, &host.definition),
        Err(JoinBaselineError::InvalidTarget {
            player: HOST,
            reason: TargetError::StaleComponent
        })
    );
}

#[test]
fn capture_rejects_unowned_held_flag_and_out_of_store_occupancy() {
    let mut host = Host::new(6, &[]);
    host.store.states[5].flags |= HELD;
    assert_eq!(
        JoinBaseline::capture(&host.session, &host.store, &host.contexts, &host.definition),
        Err(JoinBaselineError::HoldMismatch(PieceId(5)))
    );
    host.store.states[5].flags &= !HELD;
    host.store.held_by.insert(PieceId(6), HOST);
    assert_eq!(
        JoinBaseline::capture(&host.session, &host.store, &host.contexts, &host.definition),
        Err(JoinBaselineError::HoldMismatch(PieceId(6)))
    );
}

#[test]
fn old_scope_is_hidden_or_rejected_and_frozen_sessions_cannot_capture_or_install() {
    let mut host = Host::new(6, &[]);
    host.grab(HOST, 40, [0]);
    let baseline = host.capture();
    for session in [
        AuthoritySession::new(
            SessionDefinition {
                id: SessionId(456),
                ..SESSION
            },
            HOST,
            host.session.cursor(),
        ),
        AuthoritySession::new(SESSION, HOST, AuthorityCursor::new(4, 1)),
    ] {
        assert_eq!(host.contexts.active_drags(&session, &host.store).count(), 0);
        assert_eq!(
            JoinBaseline::capture(&session, &host.store, &host.contexts, &host.definition),
            Err(JoinBaselineError::MissingDragContext(PieceId(0)))
        );
    }
    let mut other_store = PieceDataStore::default();
    baseline
        .snapshot
        .install(&mut other_store, host.expected(&baseline))
        .unwrap();
    assert_eq!(
        host.contexts
            .active_drags(&host.session, &other_store)
            .count(),
        0
    );
    assert!(JoinBaseline::capture(
        &host.session,
        &other_store,
        &host.contexts,
        &host.definition
    )
    .unwrap()
    .active_drags
    .is_empty());
    other_store.held_by.insert(PieceId(0), HOST);
    other_store.states[0].flags |= HELD;
    assert_eq!(
        JoinBaseline::capture(
            &host.session,
            &other_store,
            &host.contexts,
            &host.definition
        ),
        Err(JoinBaselineError::MissingDragContext(PieceId(0)))
    );
    host.session.host_lost().unwrap();
    assert_eq!(
        JoinBaseline::capture(&host.session, &host.store, &host.contexts, &host.definition),
        Err(JoinBaselineError::Protocol(ProtocolError::Frozen))
    );
    let epoch = other_store.epoch;
    let expected = SnapshotExpectation {
        session: SESSION.id,
        image_hash: SESSION.image_hash,
        cursor: baseline.snapshot.cursor,
        definition: &host.definition,
    };
    assert_eq!(
        PeerReplicationState::default().install_join_baseline(
            &mut host.session,
            &mut other_store,
            &baseline,
            expected
        ),
        Err(JoinBaselineError::Protocol(ProtocolError::Frozen))
    );
    assert_eq!(other_store.epoch, epoch);
}

#[test]
fn baseline_decoding_is_bounded_and_roundtrips_sparse_dense_and_limit() {
    let mut host = Host::new(100, &[]);
    host.grab(HOST, 40, 0..40);
    host.grab(PlayerId(8), 50, [50]);
    let baseline = host.capture();
    assert_eq!(
        serde_json::from_slice::<JoinBaseline>(&serde_json::to_vec(&baseline).unwrap()).unwrap(),
        baseline
    );
    assert_eq!(
        postcard::from_bytes::<JoinBaseline>(&postcard::to_allocvec(&baseline).unwrap()).unwrap(),
        baseline
    );
    let mut limit = baseline.clone();
    limit.active_drags = vec![baseline.active_drags[0].clone(); MAX_BASELINE_DRAGS];
    assert_eq!(
        serde_json::from_slice::<JoinBaseline>(&serde_json::to_vec(&limit).unwrap()).unwrap(),
        limit
    );
    assert_eq!(
        postcard::from_bytes::<JoinBaseline>(&postcard::to_allocvec(&limit).unwrap()).unwrap(),
        limit
    );
    limit.active_drags.push(baseline.active_drags[0].clone());
    assert!(serde_json::from_slice::<JoinBaseline>(&serde_json::to_vec(&limit).unwrap()).is_err());
    assert!(postcard::from_bytes::<JoinBaseline>(&postcard::to_allocvec(&limit).unwrap()).is_err());

    // A lying gigantic size_hint cannot cause an unbounded reservation.
    struct HugeHint;
    impl<'de> serde::de::SeqAccess<'de> for HugeHint {
        type Error = serde::de::value::Error;
        fn next_element_seed<T: serde::de::DeserializeSeed<'de>>(
            &mut self,
            _: T,
        ) -> Result<Option<T::Value>, Self::Error> {
            Ok(None)
        }
        fn size_hint(&self) -> Option<usize> {
            Some(usize::MAX)
        }
    }
    let decoded =
        deserialize_drags(serde::de::value::SeqAccessDeserializer::new(HugeHint)).unwrap();
    assert!(decoded.is_empty());
    assert!(decoded.capacity() <= MAX_BASELINE_DRAGS);
}

#[test]
fn authority_capture_and_install_accept_exactly_64_players_then_reject_65() {
    let mut host = Host::new(100, &[]);
    for player in (0..MAX_BASELINE_DRAGS as u64).rev() {
        host.grab(PlayerId(player), 0, [player as u32]);
    }
    let baseline = host.capture();
    assert_eq!(baseline.active_drags.len(), MAX_BASELINE_DRAGS);
    assert!(baseline
        .active_drags
        .iter()
        .enumerate()
        .all(|(i, drag)| drag.player.0 == i as u64));
    let mut peer = Peer::new();
    peer.install(&host, &baseline).unwrap();
    peer.assert_matches(&host);
    host.grab(PlayerId(64), 0, [64]);
    assert_eq!(
        JoinBaseline::capture(&host.session, &host.store, &host.contexts, &host.definition),
        Err(JoinBaselineError::TooManyActiveDrags)
    );
}
