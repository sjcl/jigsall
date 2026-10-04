use super::*;
use crate::resources::pieces::{ENABLED, HELD};
use jigsall_core::protocol::{DragCancelled, ProtocolAuthorityEvent};

#[test]
fn join_at_c_then_cancel_at_c_plus_one_keeps_baseline_position() {
    let mut host = Host::new(4, &[(0, 1)]);
    host.grab(HOST, 0, [0]);
    host.update(HOST, 0, 0, Vec2::new(80., 30.));
    let baseline = host.capture();
    let cancellation = host
        .contexts
        .cancel_replicated(&mut host.session, &mut host.store, HOST)
        .unwrap()
        .unwrap();
    let mut joiner = Peer::new();
    // Cancellation has already happened when the captured baseline is installed.
    joiner.install(&host, &baseline).unwrap();
    assert_eq!(joiner.session.cursor(), baseline.snapshot.cursor);
    assert_eq!(joiner.store.held_by.len(), 2);
    joiner.event(&host, &cancellation.authority_event);
    joiner.assert_matches(&host);
    assert_eq!(
        joiner.session.cursor().sequence.0,
        baseline.snapshot.cursor.sequence.0 + 1
    );
    assert!(joiner.store.held_by.is_empty());
    assert!(joiner
        .replica
        .remote_drags(&joiner.session, &joiner.store)
        .next()
        .is_none());
    for (state, base) in joiner.store.states.iter().zip(&baseline.snapshot.pieces) {
        assert_eq!(state.position, base.position);
        assert_eq!(state.flags & HELD, 0);
    }
}

#[test]
fn join_baseline_then_cancel_preserves_canonical_state_and_other_drag() {
    let b = PlayerId(8);
    for dense in [false, true] {
        for rotate in [false, true] {
            let mut host = Host::new(64, &[(0, 1)]);
            let ids: Vec<_> = if dense { (0..40).collect() } else { vec![0, 2] };
            host.grab(HOST, 40, ids);
            host.update(HOST, 40, 100, Vec2::new(80., 30.));
            if rotate {
                host.rotate(42, 40, Some(100), Vec2::new(80., 30.));
                host.update(HOST, 42, 101, Vec2::new(800., 300.));
                assert_ne!(jigsall_core::decode_rotation(host.store.states[0].flags), 0);
            }
            host.grab(b, 5, [63]);
            host.update(b, 5, 50, Vec2::new(-20., 50.));
            let b_context = host
                .contexts
                .active_drag(&host.session, &host.store, b)
                .unwrap()
                .clone();
            let canonical = host.store.states.clone();
            let baseline = host.capture();
            let old_update = RemoteDragUpdate {
                session: SESSION.id,
                authority_epoch: host.session.cursor().epoch,
                player: HOST,
                grab_sequence: 40,
                basis_sequence: if rotate { 42 } else { 40 },
                tick: 102,
                delta: Vec2::splat(1_000_000.),
            };
            let mut peer = Peer::new();
            peer.install(&host, &baseline).unwrap();
            // Exercise host and peer local presentation cleanup, including B.
            let mut all = PieceBitSet::new(64);
            all.extend(host.store.held_by.iter().map(|(id, _)| id));
            for store in [&mut host.store, &mut peer.store] {
                store.drag.members = all.words().clone();
                store.drag.delta = Vec2::new(80., 30.);
                store.dirty_pieces.clear();
            }
            let host_epoch = host.store.epoch;
            let peer_epoch = peer.store.epoch;
            let outcome = host
                .contexts
                .cancel_replicated(&mut host.session, &mut host.store, HOST)
                .unwrap()
                .unwrap();
            assert_eq!(
                outcome.applied,
                crate::resources::pieces::AppliedCommand {
                    released: if dense { 40 } else { 3 },
                    ..Default::default()
                }
            );
            assert_eq!(
                outcome.authority_event.event,
                ProtocolAuthorityEvent::DragCancelled(DragCancelled {
                    player: HOST,
                    grab_sequence: 40
                })
            );
            assert_eq!(
                outcome.authority_event.cursor.sequence.0,
                baseline.snapshot.cursor.sequence.0 + 1
            );
            // Only the baseline and cancellation reach the joiner, no Grab replay.
            peer.event(&host, &outcome.authority_event);
            peer.assert_matches(&host);
            assert!(host
                .contexts
                .active_drag(&host.session, &host.store, HOST)
                .is_none());
            assert!(peer
                .replica
                .remote_drag(&peer.session, &peer.store, HOST)
                .is_none());
            assert_eq!(
                peer.replica.remote_drag(&peer.session, &peer.store, b),
                Some(&b_context)
            );
            assert_eq!(
                host.contexts.active_drag(&host.session, &host.store, b),
                Some(&b_context)
            );
            assert_eq!(host.store.epoch, host_epoch);
            assert_eq!(peer.store.epoch, peer_epoch);
            for store in [&host.store, &peer.store] {
                for (id, state) in store.states.iter().enumerate() {
                    assert_eq!(state.position, canonical[id].position);
                    assert_eq!(state.z_order, canonical[id].z_order);
                    let cancelled = id < if dense { 40 } else { 3 };
                    assert_eq!(
                        state.flags,
                        canonical[id].flags & if cancelled { !HELD } else { u32::MAX }
                    );
                    assert_eq!(store.dirty_pieces.contains(&PieceId(id as u32)), cancelled);
                }
                let remaining = PieceBitSet::from_words(64, store.drag.members.to_vec()).unwrap();
                assert_eq!(remaining.iter().collect::<Vec<_>>(), [PieceId(63)]);
                assert_eq!(store.drag.delta, Vec2::new(80., 30.));
                assert_eq!(store.states[63].flags & (ENABLED | HELD), ENABLED | HELD);
                assert_eq!(store.held_by.get(&PieceId(63)), Some(&b));
            }
            let cursor = peer.session.cursor();
            assert_eq!(
                peer.update(&old_update),
                Err(ReplicationError::MissingDragContext)
            );
            assert_eq!(peer.session.cursor(), cursor);
            assert!(!peer.replica.needs_resync(&peer.session, &peer.store));
        }
    }
}

#[test]
fn million_piece_dense_baseline_cancellation_uses_compact_membership() {
    let mut host = Host::new(jigsall_core::MAX_PIECES, &[]);
    host.grab(HOST, 0, 0..jigsall_core::MAX_PIECES as u32);
    host.update(HOST, 0, 0, Vec2::new(80., 30.));
    let ActiveDragTarget::Dense(target) = &host
        .contexts
        .active_drag(&host.session, &host.store, HOST)
        .unwrap()
        .target
    else {
        panic!()
    };
    assert_eq!(target.members.words().len(), 31_250);
    let baseline = host.capture();
    let mut peer = Peer::new();
    peer.install(&host, &baseline).unwrap();
    host.store.dirty_pieces.clear();
    peer.store.dirty_pieces.clear();
    let event = host
        .contexts
        .cancel_replicated(&mut host.session, &mut host.store, HOST)
        .unwrap()
        .unwrap();
    assert_eq!(event.applied.released, jigsall_core::MAX_PIECES);
    peer.event(&host, &event.authority_event);
    peer.assert_matches(&host);
    assert!(host.store.held_by.is_empty());
    assert!(peer.store.held_by.is_empty());
    assert_eq!(host.store.dirty_pieces.count(), jigsall_core::MAX_PIECES);
    assert_eq!(peer.store.dirty_pieces.count(), jigsall_core::MAX_PIECES);
    assert!(peer
        .replica
        .remote_drag(&peer.session, &peer.store, HOST)
        .is_none());
    for (state, base) in peer.store.states.iter().zip(&baseline.snapshot.pieces) {
        assert_eq!(state.position, base.position);
        assert_eq!(state.flags & HELD, 0);
    }
}
