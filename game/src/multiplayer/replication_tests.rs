use super::*;
#[path = "drag_rotation_tests.rs"]
mod drag_rotation_tests;
use crate::{
    multiplayer::{
        install_migration_snapshot,
        protocol::{HostCommandOutcome, ProtocolCommandError, ProtocolDragContexts},
    },
    resources::pieces::{CONNECTED_EDGES, ENABLED, HELD, MAX_Z, PLACED},
};
use bevy::math::UVec2;
use puzzella_core::{
    protocol::{
        DenseTarget, PieceTarget, ProtocolCommandEnvelope, ProtocolPieceCommand, ReleaseCommitted,
        ReleaseResultFingerprint,
    },
    session::{
        AuthorityCursor, AuthoritySequence, ClientCommandSequence, ImageHash, SessionDefinition,
    },
    PieceBitSet, PieceId, GENERATOR_VERSION, ROTATION_MASK,
};

const HOST: PlayerId = PlayerId(9);
const A: PlayerId = PlayerId(10);
const B: PlayerId = PlayerId(20);
const SESSION: SessionDefinition = SessionDefinition {
    id: SessionId(123),
    image_hash: ImageHash([7; 32]),
};

struct Peer {
    store: PieceDataStore,
    session: AuthoritySession,
    replica: PeerReplicationState,
}
struct Simulation {
    definition: PuzzleDefinition,
    store: PieceDataStore,
    session: AuthoritySession,
    contexts: ProtocolDragContexts,
    peers: [Peer; 2],
}
impl Simulation {
    fn rotate_drag(
        &mut self,
        sequence: u64,
        through_tick: Option<u64>,
        final_delta: Vec2,
        turns: i8,
    ) -> ProtocolAuthorityEventEnvelope {
        self.command(
            A,
            ClientCommandSequence::Control(sequence),
            ProtocolPieceCommand::RotateDrag {
                grab_sequence: 0,
                final_delta,
                through_tick,
                quarter_turns: turns,
            },
            true,
        )
        .authority_event
        .unwrap()
    }
    fn new(offsets: &[Vec2], links: &[(u32, u32)]) -> Self {
        Self::with_definition(
            PuzzleDefinition {
                generator_version: GENERATOR_VERSION,
                seed: 42,
                grid_size: UVec2::new(offsets.len() as u32, 1),
                image_size: UVec2::new(offsets.len() as u32 * 20, 20),
                snap_distance: 5.0,
            },
            offsets,
            links,
        )
    }
    fn with_definition(
        definition: PuzzleDefinition,
        offsets: &[Vec2],
        links: &[(u32, u32)],
    ) -> Self {
        let mut store = PieceDataStore::default();
        store.initialize(
            offsets
                .iter()
                .enumerate()
                .map(|(id, offset)| definition.correct_position(PieceId(id as u32)) + *offset)
                .collect(),
        );
        for &(a, b) in links {
            store.connectivity.union(PieceId(a), PieceId(b));
        }
        let cursor = AuthorityCursor::new(3, 0);
        let snapshot = GameSnapshot::capture(&store, &definition, SESSION, cursor).unwrap();
        let expected = SnapshotExpectation {
            session: SESSION.id,
            image_hash: SESSION.image_hash,
            cursor,
            definition: &definition,
        };
        snapshot.install(&mut store, expected).unwrap();
        let peers = std::array::from_fn(|_| {
            let mut store = PieceDataStore::default();
            snapshot.install(&mut store, expected).unwrap();
            Peer {
                store,
                session: AuthoritySession::new(SESSION, HOST, cursor),
                replica: PeerReplicationState::default(),
            }
        });
        Self {
            definition,
            store,
            session: AuthoritySession::new(SESSION, HOST, cursor),
            contexts: ProtocolDragContexts::default(),
            peers,
        }
    }
    fn command(
        &mut self,
        player: PlayerId,
        sequence: ClientCommandSequence,
        command: ProtocolPieceCommand,
        snap: bool,
    ) -> HostCommandOutcome {
        self.contexts
            .apply_replicated(
                &mut self.session,
                &mut self.store,
                player,
                &ProtocolCommandEnvelope {
                    session: SESSION.id,
                    authority_epoch: self.peers[0].session.cursor().epoch,
                    player,
                    sequence,
                    command,
                },
                snap.then_some(&self.definition),
                puzzella_core::LOCAL_PLAYER,
            )
            .unwrap()
    }
    fn deliver(&mut self, event: &ProtocolAuthorityEventEnvelope, snap: bool) {
        for peer in &mut self.peers {
            peer.replica
                .apply_event(
                    &mut peer.session,
                    &mut peer.store,
                    self.session.host(),
                    event,
                    snap.then_some(&self.definition),
                    puzzella_core::LOCAL_PLAYER,
                )
                .unwrap();
        }
    }
    fn grab(
        &mut self,
        player: PlayerId,
        sequence: u64,
        ids: &[u32],
    ) -> ProtocolAuthorityEventEnvelope {
        let mut mask = PieceBitSet::new(self.store.len());
        mask.extend(ids.iter().copied().map(PieceId));
        let target = PieceTarget::from_selection(&self.store.connectivity, &mask).unwrap();
        let event = self
            .command(
                player,
                ClientCommandSequence::Control(sequence),
                ProtocolPieceCommand::Grab { target },
                true,
            )
            .authority_event
            .unwrap();
        self.deliver(&event, true);
        event
    }
    fn update(
        &mut self,
        player: PlayerId,
        grab_sequence: u64,
        tick: u64,
        delta: Vec2,
    ) -> RemoteDragUpdate {
        let cursor = self.session.cursor();
        let outcome = self.command(
            player,
            ClientCommandSequence::Move {
                after_control_sequence: grab_sequence,
                tick,
            },
            ProtocolPieceCommand::DragUpdate { delta },
            true,
        );
        assert!(outcome.authority_event.is_none());
        assert_eq!(self.session.cursor(), cursor);
        outcome.drag_update.unwrap()
    }
    fn release(
        &mut self,
        player: PlayerId,
        sequence: u64,
        grab_sequence: u64,
        final_delta: Vec2,
        snap: bool,
    ) -> ProtocolAuthorityEventEnvelope {
        let event = self
            .command(
                player,
                ClientCommandSequence::Control(sequence),
                ProtocolPieceCommand::Release {
                    grab_sequence,
                    final_delta,
                },
                snap,
            )
            .authority_event
            .unwrap();
        self.deliver(&event, snap);
        event
    }
    fn assert_equal(&self) {
        for peer in &self.peers {
            assert_authority_equal(&self.store, &peer.store);
            assert_eq!(peer.session.cursor(), self.session.cursor());
        }
    }
}
fn assert_authority_equal(host: &PieceDataStore, peer: &PieceDataStore) {
    assert_eq!(host.len(), peer.len());
    assert_eq!(host.held_by, peer.held_by);
    assert_eq!(host.placed_count, peer.placed_count);
    assert_eq!(host.next_z_order, peer.next_z_order);
    for (id, (a, b)) in host.states.iter().zip(peer.states.iter()).enumerate() {
        assert_eq!(
            a.position.to_array().map(f32::to_bits),
            b.position.to_array().map(f32::to_bits),
            "position {id}"
        );
        assert_eq!(a.z_order, b.z_order, "Z {id}");
        assert_eq!(
            a.flags & (PLACED | HELD | ENABLED | CONNECTED_EDGES | ROTATION_MASK),
            b.flags & (PLACED | HELD | ENABLED | CONNECTED_EDGES | ROTATION_MASK),
            "flags {id}"
        );
        let id = PieceId(id as u32);
        assert_eq!(
            host.connectivity.minimum_member(id),
            peer.connectivity.minimum_member(id)
        );
        assert_eq!(
            host.connectivity.component_size(id),
            peer.connectivity.component_size(id)
        );
    }
}

#[test]
fn simple_grab_release_and_lost_or_reordered_presentation_converge() {
    for delivery in 0..3 {
        let mut s = Simulation::new(&[Vec2::splat(100.0), Vec2::splat(700.0)], &[]);
        s.peers[0].store.selected_pieces.fill();
        s.grab(A, 0, &[0]);
        assert!(!s.peers[0].store.selected_pieces.contains(&PieceId(0)));
        s.assert_equal(); // Ownership, HELD and deterministic Z already match.
        let updates: Vec<_> = [1, 2, 3, 10, 11, 12]
            .into_iter()
            .map(|tick| s.update(A, 0, tick, Vec2::splat(tick as f32)))
            .collect();
        for peer in &mut s.peers {
            let states = peer.store.states.clone();
            let dirty = peer.store.dirty_pieces.clone();
            let roots = peer.store.component_root_dirty.clone();
            let connectivity = peer.store.connectivity.clone();
            let selection = peer.store.selected_pieces.clone();
            let cursor = peer.session.cursor();
            match delivery {
                0 => {
                    for update in &updates {
                        peer.replica
                            .apply_drag_update(&peer.session, &peer.store, HOST, update)
                            .unwrap();
                    }
                }
                1 => {
                    peer.replica
                        .apply_drag_update(&peer.session, &peer.store, HOST, &updates[0])
                        .unwrap();
                }
                _ => {
                    peer.replica
                        .apply_drag_update(&peer.session, &peer.store, HOST, &updates[3])
                        .unwrap();
                    peer.replica
                        .apply_drag_update(&peer.session, &peer.store, HOST, &updates[5])
                        .unwrap();
                    assert_eq!(
                        peer.replica.apply_drag_update(
                            &peer.session,
                            &peer.store,
                            HOST,
                            &updates[4]
                        ),
                        Err(ReplicationError::StaleUpdate)
                    );
                }
            }
            assert_eq!(peer.store.states, states);
            assert_eq!(peer.store.dirty_pieces, dirty);
            assert_eq!(peer.store.component_root_dirty, roots);
            assert_eq!(peer.store.connectivity, connectivity);
            assert_eq!(peer.store.selected_pieces, selection);
            assert_eq!(peer.session.cursor(), cursor);
            let expected = if delivery == 1 { 1.0 } else { 12.0 };
            assert_eq!(
                peer.replica
                    .remote_drag(&peer.session, &peer.store, A)
                    .unwrap()
                    .delta,
                Vec2::splat(expected)
            );
        }
        let event = s.release(A, 1, 0, Vec2::new(30.0, -7.0), true);
        s.assert_equal();
        for peer in &mut s.peers {
            assert!(peer
                .replica
                .remote_drag(&peer.session, &peer.store, A)
                .is_none());
            assert_eq!(
                peer.replica
                    .apply_drag_update(&peer.session, &peer.store, HOST, &updates[5]),
                Err(ReplicationError::MissingDragContext)
            );
        }
        assert_eq!(s.session.cursor(), AuthorityCursor::new(3, 2));
        let bytes = serde_json::to_vec(&event).unwrap();
        assert!(bytes.len() < 400);
        assert_eq!(
            serde_json::from_slice::<ProtocolAuthorityEventEnvelope>(&bytes).unwrap(),
            event
        );
    }
}

#[test]
fn connected_snap_board_snap_closure_and_no_chained_translation_replay() {
    for (offsets, links, delta, size, placed) in [
        (
            vec![Vec2::new(100.0, 100.0), Vec2::new(104.0, 100.0)],
            vec![],
            Vec2::ZERO,
            2,
            0,
        ),
        (
            vec![Vec2::splat(100.0); 2],
            vec![(0, 1)],
            Vec2::new(-97.0, -100.0),
            2,
            2,
        ),
        (
            [0.0, 4.0, 4.0, 4.0].map(|x| Vec2::new(x, 100.0)).to_vec(),
            vec![],
            Vec2::ZERO,
            4,
            0,
        ),
        (
            [0.0, 4.0, 8.0].map(|x| Vec2::new(x, 100.0)).to_vec(),
            vec![],
            Vec2::ZERO,
            2,
            0,
        ),
        (
            [3.0, 4.0].map(|x| Vec2::new(x, 0.0)).to_vec(),
            vec![],
            Vec2::ZERO,
            1,
            1,
        ),
    ] {
        let mut s = Simulation::new(&offsets, &links);
        s.grab(A, 0, &[0]);
        s.release(A, 1, 0, delta, true);
        assert_eq!(s.store.connectivity.component_size(PieceId(0)), size);
        assert_eq!(s.store.placed_count, placed);
        s.assert_equal();
    }
}

#[test]
fn rounded_closure_and_multi_root_release_use_the_same_fixed_translation() {
    for grid in [UVec2::new(3, 1), UVec2::new(1, 3)] {
        let d = PuzzleDefinition {
            generator_version: GENERATOR_VERSION,
            seed: 42,
            grid_size: grid,
            image_size: if grid.x == 3 {
                UVec2::new(4096, 20)
            } else {
                UVec2::new(20, 4096)
            },
            snap_distance: 5.0,
        };
        let offset = if grid.x == 3 {
            Vec2::new(100.37, 0.0)
        } else {
            Vec2::new(0.0, 100.37)
        };
        let delta = if grid.x == 3 {
            Vec2::new(4.0, 0.0)
        } else {
            Vec2::new(0.0, 4.0)
        };
        let mut s = Simulation::with_definition(d, &[offset - delta, offset, offset], &[]);
        s.grab(A, 0, &[0]);
        s.release(A, 1, 0, Vec2::ZERO, true);
        assert_eq!(s.store.connectivity.component_size(PieceId(0)), 3);
        s.assert_equal();
    }
    let mut s = Simulation::new(
        &[100.0, 104.0, 108.0, 112.0].map(|x| Vec2::new(x, 100.0)),
        &[],
    );
    s.grab(A, 0, &[0, 1, 2, 3]);
    s.release(A, 1, 0, Vec2::ZERO, true);
    assert_eq!(s.store.connectivity.component_size(PieceId(0)), 3);
    assert_eq!(s.store.connectivity.component_size(PieceId(3)), 1);
    s.assert_equal();
}

#[test]
fn partial_acceptance_uses_only_authority_membership_and_preserves_other_player() {
    let mut s = Simulation::new(
        &[Vec2::splat(100.0), Vec2::splat(500.0), Vec2::splat(900.0)],
        &[],
    );
    s.grab(B, 0, &[1]);
    let event = s.grab(A, 0, &[0, 1, 2]);
    let ProtocolAuthorityEvent::GrabAccepted(ack) = event.event else {
        panic!()
    };
    assert!(
        matches!(ack.accepted, PieceTarget::Components(ref refs) if refs.iter().map(|r| r.member).collect::<Vec<_>>() == [PieceId(0), PieceId(2)])
    );
    for peer in &s.peers {
        assert_eq!(peer.store.held_by.get(&PieceId(1)), Some(&B));
        assert_eq!(
            peer.replica
                .remote_drags(&peer.session, &peer.store)
                .count(),
            2
        );
    }
    s.assert_equal();
    s.release(A, 1, 0, Vec2::splat(10.0), true);
    s.release(B, 1, 0, Vec2::splat(-10.0), true);
    s.assert_equal();
}

#[test]
fn grab_z_order_matches_even_with_ties_and_max_z_compaction() {
    let mut s = Simulation::new(&[Vec2::splat(100.0); 4], &[(0, 1)]);
    for store in std::iter::once(&mut s.store).chain(s.peers.iter_mut().map(|p| &mut p.store)) {
        store.next_z_order = MAX_Z - 1;
        for (state, z) in store.states.iter_mut().zip([7, 2, 2, 99]) {
            state.z_order = z;
        }
    }
    s.grab(A, 0, &[0, 2]);
    s.assert_equal();
    assert_eq!(
        [
            s.store.states[1].z_order,
            s.store.states[2].z_order,
            s.store.states[0].z_order
        ],
        [4, 5, 6]
    );
    s.release(A, 1, 0, Vec2::splat(50.0), false);
    s.assert_equal();
}

#[test]
fn transient_validation_never_poison_ticks_or_change_authority() {
    let mut s = Simulation::new(&[Vec2::splat(100.0)], &[]);
    s.grab(A, 0, &[0]);
    let update = s.update(A, 0, 10, Vec2::splat(10.0));
    let peer = &mut s.peers[0];
    for (invalid, host, error) in [
        (
            RemoteDragUpdate {
                session: SessionId(456),
                ..update.clone()
            },
            HOST,
            ReplicationError::Protocol(ProtocolError::WrongSession),
        ),
        (
            RemoteDragUpdate {
                authority_epoch: AuthorityEpoch(4),
                ..update.clone()
            },
            HOST,
            ReplicationError::Protocol(ProtocolError::WrongEpoch),
        ),
        (
            update.clone(),
            B,
            ReplicationError::Protocol(ProtocolError::WrongHost),
        ),
        (
            RemoteDragUpdate {
                grab_sequence: 99,
                ..update.clone()
            },
            HOST,
            ReplicationError::WrongDragContext,
        ),
        (
            RemoteDragUpdate {
                player: B,
                ..update.clone()
            },
            HOST,
            ReplicationError::MissingDragContext,
        ),
        (
            RemoteDragUpdate {
                delta: Vec2::NAN,
                ..update.clone()
            },
            HOST,
            ReplicationError::InvalidDelta,
        ),
    ] {
        assert_eq!(
            peer.replica
                .apply_drag_update(&peer.session, &peer.store, host, &invalid),
            Err(error)
        );
    }
    assert_eq!(
        peer.replica
            .apply_drag_update(&peer.session, &peer.store, HOST, &update),
        Ok(CommandSequenceStatus::Gap { expected: 0 })
    );
    assert_eq!(
        peer.replica
            .apply_drag_update(&peer.session, &peer.store, HOST, &update),
        Err(ReplicationError::DuplicateUpdate)
    );
    assert_eq!(
        peer.replica.apply_drag_update(
            &peer.session,
            &peer.store,
            HOST,
            &RemoteDragUpdate {
                tick: 9,
                ..update.clone()
            }
        ),
        Err(ReplicationError::StaleUpdate)
    );
    let last = RemoteDragUpdate {
        tick: u64::MAX,
        delta: Vec2::splat(100.0),
        ..update
    };
    peer.replica
        .apply_drag_update(&peer.session, &peer.store, HOST, &last)
        .unwrap();
    assert_eq!(
        peer.replica
            .apply_drag_update(&peer.session, &peer.store, HOST, &last),
        Err(ReplicationError::DuplicateUpdate)
    );
    assert_eq!(peer.session.cursor(), AuthorityCursor::new(3, 1));
    s.release(A, 1, 0, Vec2::ZERO, false);
    s.grab(A, 2, &[0]);
    let fresh = s.update(A, 2, 0, Vec2::ONE);
    for peer in &mut s.peers {
        assert_eq!(
            peer.replica
                .apply_drag_update(&peer.session, &peer.store, HOST, &last),
            Err(ReplicationError::WrongDragContext)
        );
        assert_eq!(
            peer.replica
                .apply_drag_update(&peer.session, &peer.store, HOST, &fresh),
            Ok(CommandSequenceStatus::InOrder)
        );
    }
}

#[test]
fn event_authentication_gap_and_duplicate_are_checked_before_gameplay() {
    let mut s = Simulation::new(&[Vec2::splat(100.0)], &[]);
    let target = PieceTarget::from_selection(&s.store.connectivity, &{
        let mut m = PieceBitSet::new(1);
        m.fill();
        m
    })
    .unwrap();
    let grab = s
        .command(
            A,
            ClientCommandSequence::Control(0),
            ProtocolPieceCommand::Grab { target },
            false,
        )
        .authority_event
        .unwrap();
    let release = s
        .command(
            A,
            ClientCommandSequence::Control(1),
            ProtocolPieceCommand::Release {
                grab_sequence: 0,
                final_delta: Vec2::ONE,
            },
            false,
        )
        .authority_event
        .unwrap();
    let peer = &mut s.peers[0];
    let states = peer.store.states.clone();
    for (invalid, host, error) in [
        (
            ProtocolAuthorityEventEnvelope {
                session: SessionId(456),
                ..grab.clone()
            },
            HOST,
            ProtocolError::WrongSession,
        ),
        (
            ProtocolAuthorityEventEnvelope {
                cursor: AuthorityCursor::new(2, 1),
                ..grab.clone()
            },
            HOST,
            ProtocolError::WrongEpoch,
        ),
        (
            ProtocolAuthorityEventEnvelope {
                host: B,
                ..grab.clone()
            },
            HOST,
            ProtocolError::WrongHost,
        ),
        (grab.clone(), B, ProtocolError::WrongHost),
        (
            release.clone(),
            HOST,
            ProtocolError::EventGap {
                expected: AuthoritySequence(1),
            },
        ),
    ] {
        assert_eq!(
            peer.replica.apply_event(
                &mut peer.session,
                &mut peer.store,
                host,
                &invalid,
                None,
                puzzella_core::LOCAL_PLAYER
            ),
            Err(ReplicationError::Protocol(error))
        );
        assert_eq!(peer.session.cursor(), AuthorityCursor::new(3, 0));
        assert_eq!(peer.store.states, states);
        assert!(!peer.replica.needs_resync(&peer.session, &peer.store));
    }
    s.deliver(&grab, false);
    s.assert_equal_after_grab_for_duplicate(&grab);
    s.deliver(&release, false);
    s.assert_equal();
    for peer in &mut s.peers {
        let states = peer.store.states.clone();
        for stale in [&grab, &release] {
            assert_eq!(
                peer.replica.apply_event(
                    &mut peer.session,
                    &mut peer.store,
                    HOST,
                    stale,
                    None,
                    puzzella_core::LOCAL_PLAYER
                ),
                Err(ReplicationError::Protocol(ProtocolError::StaleEvent))
            );
            assert_eq!(peer.store.states, states);
        }
    }
}
impl Simulation {
    fn assert_equal_after_grab_for_duplicate(&mut self, grab: &ProtocolAuthorityEventEnvelope) {
        for peer in &mut self.peers {
            let states = peer.store.states.clone();
            assert_eq!(
                peer.replica.apply_event(
                    &mut peer.session,
                    &mut peer.store,
                    HOST,
                    grab,
                    None,
                    puzzella_core::LOCAL_PLAYER
                ),
                Err(ReplicationError::Protocol(ProtocolError::StaleEvent))
            );
            assert_eq!(peer.store.states, states);
            assert_eq!(peer.session.cursor(), grab.cursor);
        }
    }
}

#[test]
fn divergence_does_not_record_cursor_or_allow_replay_until_snapshot_resync() {
    for corruption in ["position", "z", "edges", "topology", "fingerprint"] {
        let mut s = Simulation::new(&[Vec2::splat(100.0), Vec2::splat(700.0)], &[]);
        s.grab(A, 0, &[0]);
        let update = s.update(A, 0, 0, Vec2::ONE);
        let mut release = s
            .command(
                A,
                ClientCommandSequence::Control(1),
                ProtocolPieceCommand::Release {
                    grab_sequence: 0,
                    final_delta: Vec2::splat(30.0),
                },
                false,
            )
            .authority_event
            .unwrap();
        let peer = &mut s.peers[0];
        match corruption {
            "position" => peer.store.states[0].position.x += 100.0,
            "z" => peer.store.states[0].z_order += 1,
            "edges" => peer.store.states[0].flags ^= crate::resources::pieces::CONNECTED_RIGHT,
            "topology" => {
                peer.store.connectivity.union(PieceId(0), PieceId(1));
            }
            "fingerprint" => {
                let ProtocolAuthorityEvent::ReleaseCommitted(commit) = &mut release.event else {
                    panic!()
                };
                commit.result.0 ^= 1;
            }
            _ => unreachable!(),
        }
        assert_eq!(
            peer.replica.apply_event(
                &mut peer.session,
                &mut peer.store,
                HOST,
                &release,
                None,
                puzzella_core::LOCAL_PLAYER
            ),
            Err(ReplicationError::Diverged),
            "{corruption}"
        );
        assert_eq!(peer.session.cursor(), AuthorityCursor::new(3, 1));
        assert!(peer.replica.needs_resync(&peer.session, &peer.store));
        assert!(peer
            .replica
            .remote_drag(&peer.session, &peer.store, A)
            .is_none());
        let after_failure = peer.store.states.clone();
        assert_eq!(
            peer.replica.apply_event(
                &mut peer.session,
                &mut peer.store,
                HOST,
                &release,
                None,
                puzzella_core::LOCAL_PLAYER
            ),
            Err(ReplicationError::Diverged)
        );
        assert_eq!(peer.store.states, after_failure); // No doubled final delta.
        assert_eq!(
            peer.replica
                .apply_drag_update(&peer.session, &peer.store, HOST, &update),
            Err(ReplicationError::Diverged)
        );
        let snapshot =
            GameSnapshot::capture(&s.store, &s.definition, SESSION, s.session.cursor()).unwrap();
        peer.replica
            .install_snapshot(
                &mut peer.session,
                &mut peer.store,
                &snapshot,
                SnapshotExpectation {
                    session: SESSION.id,
                    image_hash: SESSION.image_hash,
                    cursor: s.session.cursor(),
                    definition: &s.definition,
                },
            )
            .unwrap();
        assert!(!peer.replica.needs_resync(&peer.session, &peer.store));
        assert!(peer
            .replica
            .remote_drag(&peer.session, &peer.store, A)
            .is_none());
        assert_eq!(
            peer.replica
                .apply_drag_update(&peer.session, &peer.store, HOST, &update),
            Err(ReplicationError::MissingDragContext)
        );
        assert_eq!(
            peer.replica.apply_event(
                &mut peer.session,
                &mut peer.store,
                HOST,
                &release,
                None,
                puzzella_core::LOCAL_PLAYER
            ),
            Err(ReplicationError::Protocol(ProtocolError::StaleEvent))
        );
        // The second peer gets the actual host event, even in the tampered-wire case.
        if corruption == "fingerprint" {
            let ProtocolAuthorityEvent::ReleaseCommitted(commit) = &mut release.event else {
                panic!()
            };
            commit.result.0 ^= 1;
        }
        let peer = &mut s.peers[1];
        peer.replica
            .apply_event(
                &mut peer.session,
                &mut peer.store,
                HOST,
                &release,
                None,
                puzzella_core::LOCAL_PLAYER,
            )
            .unwrap();
        s.grab(A, 2, &[0]);
        s.release(A, 3, 2, Vec2::splat(10.0), false);
        s.assert_equal();
    }
}

#[test]
fn contradictory_grab_is_atomic_including_components_after_a_valid_sibling() {
    for corruption in ["owner", "held", "placed", "disabled", "topology"] {
        let mut s = Simulation::new(&[Vec2::splat(100.0); 3], &[]);
        let mut mask = PieceBitSet::new(3);
        mask.fill();
        let target = PieceTarget::from_selection(&s.store.connectivity, &mask).unwrap();
        let event = s
            .command(
                A,
                ClientCommandSequence::Control(0),
                ProtocolPieceCommand::Grab { target },
                false,
            )
            .authority_event
            .unwrap();
        let peer = &mut s.peers[0];
        match corruption {
            "owner" => peer.store.held_by.insert(PieceId(1), B),
            "held" => peer.store.states[1].flags |= HELD,
            "placed" => peer.store.states[1].flags |= PLACED,
            "disabled" => peer.store.states[1].flags &= !ENABLED,
            "topology" => {
                peer.store.connectivity.union(PieceId(1), PieceId(2));
            }
            _ => unreachable!(),
        }
        let states = peer.store.states.clone();
        let owners = peer.store.held_by.clone();
        let next_z = peer.store.next_z_order;
        assert_eq!(
            peer.replica.apply_event(
                &mut peer.session,
                &mut peer.store,
                HOST,
                &event,
                None,
                puzzella_core::LOCAL_PLAYER
            ),
            Err(ReplicationError::Diverged)
        );
        assert_eq!(peer.store.states, states);
        assert_eq!(peer.store.held_by, owners);
        assert_eq!(peer.store.next_z_order, next_z);
        assert_eq!(peer.session.cursor(), AuthorityCursor::new(3, 0));
    }
}

#[test]
fn missing_or_wrong_release_context_does_not_mutate_or_record() {
    for with_grab in [false, true] {
        let mut s = Simulation::new(&[Vec2::splat(100.0)], &[]);
        if with_grab {
            s.grab(A, 0, &[0]);
        }
        let peer = &mut s.peers[0];
        let cursor = peer.session.cursor();
        let event = ProtocolAuthorityEventEnvelope {
            session: SESSION.id,
            host: HOST,
            cursor: AuthorityCursor::new(3, cursor.sequence.0 + 1),
            event: ProtocolAuthorityEvent::ReleaseCommitted(ReleaseCommitted {
                player: A,
                grab_sequence: 99,
                final_delta: Vec2::ONE,
                result: ReleaseResultFingerprint(0),
            }),
        };
        let states = peer.store.states.clone();
        assert_eq!(
            peer.replica.apply_event(
                &mut peer.session,
                &mut peer.store,
                HOST,
                &event,
                None,
                puzzella_core::LOCAL_PLAYER
            ),
            Err(if with_grab {
                ReplicationError::WrongDragContext
            } else {
                ReplicationError::MissingDragContext
            })
        );
        assert_eq!(peer.session.cursor(), cursor);
        assert_eq!(peer.store.states, states);
    }
}

#[test]
fn dense_million_piece_remote_context_retains_mask_and_scalar_presentation() {
    let count = puzzella_core::MAX_PIECES;
    let d = PuzzleDefinition {
        generator_version: GENERATOR_VERSION,
        seed: 42,
        grid_size: UVec2::splat(1000),
        image_size: UVec2::splat(20000),
        snap_distance: 5.0,
    };
    let mut s = Simulation::with_definition(d, &vec![Vec2::splat(1000.0); count], &[]);
    let mut members = PieceBitSet::new(count);
    members.fill();
    let target =
        PieceTarget::Dense(DenseTarget::from_selection(&s.store.connectivity, &members).unwrap());
    let event = s
        .command(
            A,
            ClientCommandSequence::Control(0),
            ProtocolPieceCommand::Grab { target },
            false,
        )
        .authority_event
        .unwrap();
    s.deliver(&event, false);
    let ProtocolAuthorityEvent::GrabAccepted(ack) = &event.event else {
        panic!()
    };
    let PieceTarget::Dense(accepted) = &ack.accepted else {
        panic!()
    };
    assert_eq!(accepted.members.words().len() * 4, 125_000);
    for peer in &mut s.peers {
        let drag = peer
            .replica
            .remote_drag(&peer.session, &peer.store, A)
            .unwrap();
        let ActiveDragTarget::Dense(dense) = &drag.target else {
            panic!("Dense was expanded into refs")
        };
        assert_eq!(dense.component_count, count as u32);
        assert!(std::sync::Arc::ptr_eq(
            dense.members.words(),
            accepted.members.words()
        ));
        peer.store.dirty_pieces.clear();
        let dirty = peer.store.component_root_dirty.clone();
        let update = RemoteDragUpdate {
            session: SESSION.id,
            authority_epoch: AuthorityEpoch(3),
            player: A,
            grab_sequence: 0,
            basis_sequence: 0,
            tick: 100,
            delta: Vec2::new(20.0, -30.0),
        };
        peer.replica
            .apply_drag_update(&peer.session, &peer.store, HOST, &update)
            .unwrap();
        assert!(peer.store.dirty_pieces.is_empty());
        assert_eq!(peer.store.component_root_dirty, dirty);
    }
    s.assert_equal();
    // Disconnect cancellation clears presentation/holds without any snap.
    let positions: Vec<_> = s.store.states.iter().map(|state| state.position).collect();
    s.contexts.cancel_player(&mut s.store, A);
    for peer in &mut s.peers {
        peer.replica.cancel_player(&mut peer.store, A);
    }
    s.assert_equal();
    assert!(s.store.held_by.is_empty());
    assert_eq!(
        s.store
            .states
            .iter()
            .map(|state| state.position)
            .collect::<Vec<_>>(),
        positions
    );
    assert_eq!(s.store.placed_count, 0);
}

#[test]
fn dense_release_replay_and_stale_topology_preserve_target_specific_validation() {
    let count = 4096;
    let d = PuzzleDefinition {
        generator_version: GENERATOR_VERSION,
        seed: 42,
        grid_size: UVec2::splat(64),
        image_size: UVec2::splat(1280),
        snap_distance: 5.0,
    };
    let offsets: Vec<_> = (0..count)
        .map(|id| Vec2::splat(1000.0 + id as f32 * 100.0))
        .collect();
    let mut s = Simulation::with_definition(d, &offsets, &[]);
    let ids: Vec<_> = (0..4000).collect();
    s.grab(A, 0, &ids);
    // An unrelated merge never invalidates the active Dense fingerprint.
    for store in std::iter::once(&mut s.store).chain(s.peers.iter_mut().map(|peer| &mut peer.store))
    {
        store.connectivity.union(PieceId(4094), PieceId(4095));
    }
    s.release(A, 1, 0, Vec2::new(10.0, -5.0), false);
    s.assert_equal();
    s.grab(A, 2, &ids);
    let peer = &mut s.peers[0];
    peer.store.connectivity.union(PieceId(0), PieceId(4000));
    let event = s
        .command(
            A,
            ClientCommandSequence::Control(3),
            ProtocolPieceCommand::Release {
                grab_sequence: 2,
                final_delta: Vec2::ONE,
            },
            false,
        )
        .authority_event
        .unwrap();
    let peer = &mut s.peers[0];
    let states = peer.store.states.clone();
    assert_eq!(
        peer.replica.apply_event(
            &mut peer.session,
            &mut peer.store,
            HOST,
            &event,
            None,
            puzzella_core::LOCAL_PLAYER
        ),
        Err(ReplicationError::Diverged)
    );
    assert_eq!(peer.store.states, states);
    assert_eq!(peer.session.cursor(), AuthorityCursor::new(3, 3));
}

#[test]
fn snapshot_install_invalidates_in_flight_context_and_rejects_old_updates() {
    let mut s = Simulation::new(&[Vec2::splat(100.0)], &[]);
    let grab = s.grab(A, 0, &[0]);
    let update = s.update(A, 0, 0, Vec2::ONE);
    let snapshot =
        GameSnapshot::capture(&s.store, &s.definition, SESSION, s.session.cursor()).unwrap();
    for peer in &mut s.peers {
        let expected = SnapshotExpectation {
            session: SESSION.id,
            image_hash: SESSION.image_hash,
            cursor: snapshot.cursor,
            definition: &s.definition,
        };
        // Failed install leaves active context, session and store unchanged.
        let mut invalid = snapshot.clone();
        invalid.schema_version = 99;
        assert!(peer
            .replica
            .install_snapshot(&mut peer.session, &mut peer.store, &invalid, expected)
            .is_err());
        assert!(peer
            .replica
            .remote_drag(&peer.session, &peer.store, A)
            .is_some());
        snapshot.install(&mut peer.store, expected).unwrap(); // External restore works too.
        assert!(peer
            .replica
            .remote_drag(&peer.session, &peer.store, A)
            .is_none());
        assert_eq!(
            peer.replica
                .apply_drag_update(&peer.session, &peer.store, HOST, &update),
            Err(ReplicationError::MissingDragContext)
        );
        assert_eq!(
            peer.replica.apply_event(
                &mut peer.session,
                &mut peer.store,
                HOST,
                &grab,
                Some(&s.definition),
                puzzella_core::LOCAL_PLAYER
            ),
            Err(ReplicationError::Protocol(ProtocolError::StaleEvent))
        );
    }
    s.contexts.cancel_player(&mut s.store, A); // Snapshots omit holds.
    s.grab(A, 1, &[0]);
    s.release(A, 2, 1, Vec2::ONE, false);
    s.assert_equal();
}

#[test]
fn migration_freezes_apply_then_restarts_with_new_host_epoch_and_no_old_drags() {
    let mut s = Simulation::new(&[Vec2::splat(100.0)], &[]);
    s.grab(A, 0, &[0]);
    let update = s.update(A, 0, 0, Vec2::ONE);
    let cursor = s.session.cursor();
    let snapshot = GameSnapshot::capture(&s.store, &s.definition, SESSION, cursor).unwrap();
    let pending = ProtocolAuthorityEventEnvelope {
        session: SESSION.id,
        host: HOST,
        cursor: AuthorityCursor::new(3, 2),
        event: ProtocolAuthorityEvent::ReleaseCommitted(ReleaseCommitted {
            player: A,
            grab_sequence: 0,
            final_delta: Vec2::ONE,
            result: ReleaseResultFingerprint(0),
        }),
    };
    s.session.begin_graceful(B).unwrap();
    s.session
        .acknowledge_snapshot(SESSION.id, B, cursor)
        .unwrap();
    s.session.host_changed(B).unwrap();
    for peer in &mut s.peers {
        peer.session.begin_graceful(B).unwrap();
        assert!(peer
            .replica
            .remote_drag(&peer.session, &peer.store, A)
            .is_none());
        let states = peer.store.states.clone();
        assert_eq!(
            peer.replica.apply_event(
                &mut peer.session,
                &mut peer.store,
                HOST,
                &pending,
                Some(&s.definition),
                puzzella_core::LOCAL_PLAYER
            ),
            Err(ReplicationError::Protocol(ProtocolError::Frozen))
        );
        assert_eq!(
            peer.replica
                .apply_drag_update(&peer.session, &peer.store, HOST, &update),
            Err(ReplicationError::Protocol(ProtocolError::Frozen))
        );
        assert_eq!(peer.store.states, states);
        assert_eq!(peer.session.cursor(), cursor);
        peer.session
            .acknowledge_snapshot(SESSION.id, B, cursor)
            .unwrap();
        peer.session.host_changed(B).unwrap();
        install_migration_snapshot(&mut peer.session, &mut peer.store, &s.definition, &snapshot)
            .unwrap();
        assert_eq!(
            peer.replica
                .apply_drag_update(&peer.session, &peer.store, B, &update),
            Err(ReplicationError::Protocol(ProtocolError::WrongEpoch))
        );
        assert_eq!(
            peer.replica.apply_event(
                &mut peer.session,
                &mut peer.store,
                HOST,
                &pending,
                Some(&s.definition),
                puzzella_core::LOCAL_PLAYER
            ),
            Err(ReplicationError::Protocol(ProtocolError::WrongEpoch))
        );
        assert_eq!(
            peer.replica.apply_event(
                &mut peer.session,
                &mut peer.store,
                HOST,
                &ProtocolAuthorityEventEnvelope {
                    host: B,
                    cursor: AuthorityCursor::new(4, 1),
                    ..pending.clone()
                },
                Some(&s.definition),
                puzzella_core::LOCAL_PLAYER
            ),
            Err(ReplicationError::Protocol(ProtocolError::WrongHost))
        );
    }
    install_migration_snapshot(&mut s.session, &mut s.store, &s.definition, &snapshot).unwrap();
    s.grab(A, 0, &[0]);
    s.release(A, 1, 0, Vec2::ONE, false);
    s.assert_equal();
    assert_eq!(s.session.host(), B);
    assert_eq!(s.session.cursor(), AuthorityCursor::new(4, 2));
}

#[test]
fn publication_counter_exhaustion_is_rejected_before_mutation() {
    let mut s = Simulation::new(&[Vec2::splat(100.0)], &[]);
    s.session = AuthoritySession::new(SESSION, HOST, AuthorityCursor::new(3, u64::MAX));
    let target = PieceTarget::Component(
        puzzella_core::protocol::ComponentRef::from_member(&s.store.connectivity, PieceId(0))
            .unwrap(),
    );
    let envelope = ProtocolCommandEnvelope {
        session: SESSION.id,
        authority_epoch: AuthorityEpoch(3),
        player: A,
        sequence: ClientCommandSequence::Control(0),
        command: ProtocolPieceCommand::Grab { target },
    };
    let states = s.store.states.clone();
    assert!(matches!(
        s.contexts.apply_replicated(
            &mut s.session,
            &mut s.store,
            A,
            &envelope,
            None,
            puzzella_core::LOCAL_PLAYER
        ),
        Err(ProtocolCommandError::Sequence(
            ProtocolError::CounterExhausted
        ))
    ));
    assert_eq!(s.store.states, states);
    assert!(s.store.held_by.is_empty());
    // The preflight did not consume control 0.
    s.contexts
        .apply(
            &mut s.session,
            &mut s.store,
            A,
            &envelope,
            None,
            puzzella_core::LOCAL_PLAYER,
        )
        .unwrap();
}

#[test]
fn result_fingerprint_has_fixed_bytes_root_history_independence_and_local_scope() {
    use crate::multiplayer::release::result_fingerprint;
    let mut store = PieceDataStore::default();
    store.initialize(vec![Vec2::new(10.0, 20.0), Vec2::splat(100.0)]);
    store.states[0].z_order = 7;
    store.next_z_order = 4;
    let applied = AppliedCommand {
        released: 1,
        ..Default::default()
    };
    let fingerprint = result_fingerprint(&store, &[PieceId(0)], None, &applied);
    // Independently encoded with Python struct.pack and hashlib SHA-256.
    assert_eq!(
        fingerprint,
        ReleaseResultFingerprint(0xcf436034aec7d6a808950565e1824159)
    );
    store.states[1].position += Vec2::splat(500.0);
    assert_eq!(
        result_fingerprint(&store, &[PieceId(0)], None, &applied),
        fingerprint
    );
    let mut a = PieceDataStore::default();
    a.initialize(vec![Vec2::splat(10.0); 4]);
    a.connectivity.union(PieceId(3), PieceId(1));
    a.connectivity.union(PieceId(3), PieceId(0));
    let mut b = PieceDataStore::default();
    b.initialize(vec![Vec2::splat(10.0); 4]);
    b.connectivity.union(PieceId(0), PieceId(1));
    b.connectivity.union(PieceId(0), PieceId(3));
    assert_ne!(
        a.connectivity.find_root(PieceId(0)),
        b.connectivity.find_root(PieceId(0))
    );
    assert_eq!(
        result_fingerprint(&a, &[PieceId(3), PieceId(1)], None, &applied),
        result_fingerprint(&b, &[PieceId(0)], None, &applied)
    );
    // Every member is checked, rather than trusting a representative offset alone.
    b.states[3].position.x += 1.0;
    assert_ne!(
        result_fingerprint(&a, &[PieceId(0)], None, &applied),
        result_fingerprint(&b, &[PieceId(0)], None, &applied)
    );
}

#[test]
fn restored_replica_with_different_dsu_root_replays_connected_snap_identically() {
    let definition = PuzzleDefinition {
        generator_version: GENERATOR_VERSION,
        seed: 42,
        grid_size: UVec2::new(3, 2),
        image_size: UVec2::new(60, 40),
        snap_distance: 5.0,
    };
    let offsets = [100.0, 100.0, 104.0, 800.0, 100.0, 104.0].map(|x| Vec2::new(x, 100.0));
    let mut s = Simulation::with_definition(definition, &offsets, &[(1, 4), (1, 0)]);
    let mut connectivity = puzzella_core::PieceConnectivity::new(6);
    connectivity.union(PieceId(1), PieceId(4));
    connectivity.union(PieceId(1), PieceId(0));
    s.store.connectivity = connectivity;
    assert_ne!(
        s.store.connectivity.find_root(PieceId(0)),
        s.peers[0].store.connectivity.find_root(PieceId(0))
    );
    s.grab(A, 0, &[0]);
    s.release(A, 1, 0, Vec2::ZERO, true);
    assert_eq!(s.store.connectivity.component_size(PieceId(0)), 5);
    s.assert_equal();
}

#[test]
fn empty_acceptance_is_reliable_but_rejected_commands_publish_nothing() {
    let mut s = Simulation::new(&[Vec2::splat(100.0)], &[]);
    s.grab(B, 0, &[0]);
    let empty = s.grab(A, 0, &[0]);
    let ProtocolAuthorityEvent::GrabAccepted(ack) = empty.event else {
        panic!()
    };
    assert_eq!(ack.accepted, PieceTarget::Components(vec![]));
    assert_eq!(s.session.cursor(), AuthorityCursor::new(3, 2));
    for peer in &s.peers {
        assert!(peer
            .replica
            .remote_drag(&peer.session, &peer.store, A)
            .is_none());
        assert_eq!(peer.store.held_by.get(&PieceId(0)), Some(&B));
    }
    let cursor = s.session.cursor();
    let envelope = ProtocolCommandEnvelope {
        session: SESSION.id,
        authority_epoch: cursor.epoch,
        player: A,
        sequence: ClientCommandSequence::Control(1),
        command: ProtocolPieceCommand::Release {
            grab_sequence: 0,
            final_delta: Vec2::ONE,
        },
    };
    assert!(matches!(
        s.contexts.apply_replicated(
            &mut s.session,
            &mut s.store,
            A,
            &envelope,
            Some(&s.definition),
            puzzella_core::LOCAL_PLAYER
        ),
        Err(ProtocolCommandError::NoActiveDrag)
    ));
    assert_eq!(s.session.cursor(), cursor);
    s.release(B, 1, 0, Vec2::ZERO, false);
    s.assert_equal();
}

fn rotate_event(
    s: &mut Simulation,
    sequence: u64,
    ids: &[u32],
    turns: i8,
) -> ProtocolAuthorityEventEnvelope {
    let mut mask = PieceBitSet::new(s.store.len());
    mask.extend(ids.iter().copied().map(PieceId));
    let target = PieceTarget::from_selection(&s.store.connectivity, &mask).unwrap();
    let outcome = s.command(
        A,
        ClientCommandSequence::Control(sequence),
        ProtocolPieceCommand::Rotate {
            target,
            quarter_turns: turns,
        },
        true,
    );
    assert!(outcome.drag_update.is_none());
    let event = outcome.authority_event.unwrap();
    s.deliver(&event, true);
    s.assert_equal();
    event
}

#[test]
fn reliable_rotations_replay_identically_and_snap_rotated_neighbors_after_drag() {
    let mut s = Simulation::new(&[Vec2::splat(100.0); 3], &[(0, 1)]);
    let initial = s.store.states.to_vec();
    for (sequence, turns) in [1, -1, 127, -127].into_iter().enumerate() {
        let event = rotate_event(&mut s, sequence as u64, &[0, 1, 2], turns);
        let ProtocolAuthorityEvent::RotationCommitted(commit) = &event.event else {
            panic!()
        };
        assert_eq!(
            commit.quarter_turns,
            puzzella_core::add_quarter_turns(0, turns) as i8
        );
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
            Err(ReplicationError::Protocol(ProtocolError::StaleEvent))
        );
    }
    assert_eq!(&*s.store.states, &initial);
    // Rotating each body independently means the singleton needs a translation
    // to align with the pair. Release and its fingerprint include rotation bits.
    rotate_event(&mut s, 4, &[0, 2], 1);
    let moving = s.store.states[2];
    let translation = s.store.states[0].position
        - puzzella_core::rotate_quarter(s.definition.correct_position(PieceId(0)), 1);
    let final_delta = puzzella_core::rotate_quarter(s.definition.correct_position(PieceId(2)), 1)
        + translation
        - moving.position;
    s.grab(A, 5, &[2]);
    s.update(A, 5, 0, final_delta * 0.5);
    s.release(A, 6, 5, final_delta, true);
    s.assert_equal();
    assert_eq!(s.store.connectivity.component_size(PieceId(0)), 3);
    rotate_event(&mut s, 7, &[2], 1);
    assert!(s
        .store
        .states
        .iter()
        .all(|state| puzzella_core::decode_rotation(state.flags) == 2));
    let snapshot =
        GameSnapshot::capture(&s.store, &s.definition, SESSION, s.session.cursor()).unwrap();
    let serialized = postcard::to_allocvec(&snapshot).unwrap();
    let decoded: GameSnapshot = postcard::from_bytes(&serialized).unwrap();
    assert_eq!(decoded, snapshot);
    let mut restored = PieceDataStore::default();
    decoded
        .install(
            &mut restored,
            SnapshotExpectation {
                session: SESSION.id,
                image_hash: SESSION.image_hash,
                cursor: s.session.cursor(),
                definition: &s.definition,
            },
        )
        .unwrap();
    assert_eq!(&*s.store.states, &*restored.states);
    assert_eq!(restored.connectivity.component_size(PieceId(0)), 3);
    assert!(restored.held_by.is_empty());
}

#[test]
fn authority_rotation_excludes_remote_holds_and_rejects_active_player_drag() {
    let mut s = Simulation::new(&[Vec2::splat(100.0); 3], &[(0, 1)]);
    s.grab(B, 0, &[0]);
    let before = s.store.states.to_vec();
    rotate_event(&mut s, 0, &[0, 2], 1);
    assert_eq!(s.store.states[0], before[0]);
    assert_eq!(s.store.states[1], before[1]);
    assert_eq!(puzzella_core::decode_rotation(s.store.states[2].flags), 1);
    let target = PieceTarget::Component(
        puzzella_core::protocol::ComponentRef::from_member(&s.store.connectivity, PieceId(0))
            .unwrap(),
    );
    let envelope = ProtocolCommandEnvelope {
        session: SESSION.id,
        authority_epoch: s.session.cursor().epoch,
        player: B,
        sequence: ClientCommandSequence::Control(1),
        command: ProtocolPieceCommand::Rotate {
            target,
            quarter_turns: 1,
        },
    };
    let cursor = s.session.cursor();
    assert!(matches!(
        s.contexts.apply_replicated(
            &mut s.session,
            &mut s.store,
            B,
            &envelope,
            Some(&s.definition),
            puzzella_core::LOCAL_PLAYER
        ),
        Err(ProtocolCommandError::ActiveDragExists)
    ));
    assert_eq!(s.session.cursor(), cursor);
    s.assert_equal();
}

#[test]
fn replica_rotation_preflights_whole_event_and_detects_rotation_divergence() {
    for corrupt in 0..3 {
        let mut s = Simulation::new(&[Vec2::splat(100.0); 3], &[(0, 1)]);
        let target = PieceTarget::Components(vec![
            puzzella_core::protocol::ComponentRef::from_member(&s.store.connectivity, PieceId(0))
                .unwrap(),
            puzzella_core::protocol::ComponentRef::from_member(&s.store.connectivity, PieceId(2))
                .unwrap(),
        ]);
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
        let peer = &mut s.peers[0];
        let ProtocolAuthorityEvent::RotationCommitted(commit) = &mut event.event else {
            panic!()
        };
        match corrupt {
            0 => peer.store.states[2].flags &= !ENABLED,
            1 => commit.quarter_turns = 4,
            2 => commit.result.0 ^= 1,
            _ => unreachable!(),
        }
        let before = peer.store.states.to_vec();
        let cursor = peer.session.cursor();
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
        assert_eq!(peer.session.cursor(), cursor);
        assert!(peer.replica.needs_resync(&peer.session, &peer.store));
        if corrupt < 2 {
            assert_eq!(&*peer.store.states, &before);
        }
    }
}

#[test]
fn reliable_dense_rotation_preserves_topology_and_rejects_stale_targets() {
    use puzzella_core::protocol::TargetError;
    let mut s = Simulation::new(&[Vec2::splat(100.0); 40], &[]);
    let mut members = PieceBitSet::new(40);
    members.fill();
    let target = PieceTarget::from_selection(&s.store.connectivity, &members).unwrap();
    assert!(matches!(target, PieceTarget::Dense(_)));
    let event = s
        .command(
            A,
            ClientCommandSequence::Control(0),
            ProtocolPieceCommand::Rotate {
                target: target.clone(),
                quarter_turns: -1,
            },
            true,
        )
        .authority_event
        .unwrap();
    s.deliver(&event, true);
    s.assert_equal();
    let before = s.store.states.to_vec();
    s.store.connectivity.union(PieceId(0), PieceId(1));
    let envelope = ProtocolCommandEnvelope {
        session: SESSION.id,
        authority_epoch: s.session.cursor().epoch,
        player: A,
        sequence: ClientCommandSequence::Control(1),
        command: ProtocolPieceCommand::Rotate {
            target,
            quarter_turns: 1,
        },
    };
    assert!(matches!(
        s.contexts.apply_replicated(
            &mut s.session,
            &mut s.store,
            A,
            &envelope,
            Some(&s.definition),
            puzzella_core::LOCAL_PLAYER
        ),
        Err(ProtocolCommandError::Target(TargetError::StaleTopology))
    ));
    assert_eq!(&*s.store.states, &before);
}

#[test]
fn rotation_replay_is_independent_of_dsu_root_and_member_list_history() {
    let mut s = Simulation::new(&[Vec2::splat(100.37); 3], &[(0, 1), (1, 2)]);
    let peer = &mut s.peers[1];
    peer.store.connectivity = puzzella_core::PieceConnectivity::new(3);
    peer.store.connectivity.union(PieceId(2), PieceId(1));
    peer.store.connectivity.union(PieceId(2), PieceId(0));
    assert_ne!(
        s.store.connectivity.find_root(PieceId(0)),
        peer.store.connectivity.find_root(PieceId(0))
    );
    for sequence in 0..4 {
        rotate_event(&mut s, sequence, &[1], 1);
    }
}

#[test]
fn peer_nonzero_identity_replays_own_and_remote_zero_authority_events() {
    let local = PlayerId(42);
    let remote = PlayerId(0);
    for dense in [false, true] {
        for player in [local, remote] {
            let mut s = Simulation::new(&[Vec2::splat(1000.0); 96], &[(0, 1)]);
            let count = if dense { 8 } else { 2 };
            let mut members = PieceBitSet::new(96);
            members.extend((0..count).map(PieceId));
            let target = if dense {
                PieceTarget::Dense(
                    DenseTarget::from_selection(&s.store.connectivity, &members).unwrap(),
                )
            } else {
                PieceTarget::from_selection(&s.store.connectivity, &members).unwrap()
            };
            let peer = &mut s.peers[0];
            peer.store.selected_pieces = members.clone();
            peer.store.drag.members = members.words().clone();
            let event = s
                .command(
                    player,
                    ClientCommandSequence::Control(0),
                    ProtocolPieceCommand::Grab { target },
                    false,
                )
                .authority_event
                .unwrap();
            let peer = &mut s.peers[0];
            peer.replica
                .apply_event(
                    &mut peer.session,
                    &mut peer.store,
                    HOST,
                    &event,
                    None,
                    local,
                )
                .unwrap();
            assert_eq!(peer.store.selected_pieces.is_empty(), player != local);
            assert_eq!(
                peer.store.drag.members.iter().all(|&word| word == 0),
                player != local
            );
            for id in members.iter() {
                assert_eq!(peer.store.held_by.get(&id), Some(&player));
            }
            let event = s
                .command(
                    player,
                    ClientCommandSequence::Control(1),
                    ProtocolPieceCommand::Release {
                        grab_sequence: 0,
                        final_delta: Vec2::ONE,
                    },
                    false,
                )
                .authority_event
                .unwrap();
            let peer = &mut s.peers[0];
            peer.replica
                .apply_event(
                    &mut peer.session,
                    &mut peer.store,
                    HOST,
                    &event,
                    None,
                    local,
                )
                .unwrap();
            assert!(peer.store.drag.members.iter().all(|&word| word == 0));
            assert!(peer.store.held_by.is_empty());
            assert_authority_equal(&s.store, &peer.store);
        }
    }
}
