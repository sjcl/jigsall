use super::*;
use crate::{interaction::PointerFrame, resources::pieces::*, selection::PuzzleSelection};
use bevy::prelude::*;
use puzzella_core::PieceId;

fn fixture(count: usize) -> (CommandBridge, PieceInteraction, PieceDataStore) {
    let interaction = PieceInteraction::default();
    let token = interaction.network_gesture_token();
    let mut members = PieceBitSet::new(count);
    members.fill();
    let mut store = PieceDataStore::default();
    store.initialize(vec![Vec2::new(100.0, 100.0); count]);
    store.drag.members = members.words().clone();
    let mut bridge = CommandBridge {
        active: Some(LocalDrag {
            grab: 0,
            basis: 0,
            last_tick: Some(7),
            members,
            last_delta: Some(Vec2::ZERO),
            scalar_delta: Vec2::ZERO,
            token: token.clone(),
        }),
        next_control: Some(1),
        started: true,
        ..Default::default()
    };
    bridge
        .enqueue(
            PieceCommand::ReleaseGroup {
                members: PieceBitSet::new(count), // Request mask must never control presentation.
                delta: Vec2::new(31.0, -13.0),
            },
            None,
            token,
        )
        .unwrap();
    (bridge, interaction, store)
}

fn session() -> AuthoritySession {
    AuthoritySession::new(
        SessionDefinition {
            id: SessionId(1),
            image_hash: ImageHash([0; 32]),
        },
        PlayerId(37),
        AuthorityCursor::new(0, 0),
    )
}

#[test]
fn pending_release_million_members_idle_frames_share_mask_without_piece_access_or_state_upload() {
    let (mut bridge, mut interaction, store) = fixture(1_000_000);
    let player = PlayerId(37);
    let session = session();
    let mut app = App::new();
    app.insert_resource(store)
        .init_resource::<PieceUpload>()
        .add_systems(Update, prepare_piece_upload);
    app.update();
    app.update(); // Finish the existing initial state/root upload lifecycle.
    let revision = app.world().resource::<PieceUpload>().revision;
    let root_revision = app.world().resource::<PieceUpload>().root_revision;
    let states = app.world().resource::<PieceDataStore>().states.as_ptr();
    let frozen = bridge.active.as_ref().unwrap().members.words().clone();
    assert_eq!(frozen.len() * 4, 125_000);
    let command = bridge
        .next(&session, player, app.world().resource::<PieceDataStore>())
        .unwrap()
        .unwrap();
    assert!(
        matches!(command.command, ProtocolPieceCommand::Release { final_delta, .. }
        if final_delta == Vec2::new(31.0, -13.0))
    );
    // A queued control must neither overwrite the old membership nor send yet.
    bridge
        .enqueue(PieceCommand::Grab(PieceId(999_999)), None, Arc::new(()))
        .unwrap();
    let mut selection = PuzzleSelection::default();
    bridge.present_release(
        &mut interaction,
        &mut app.world_mut().resource_mut::<PieceDataStore>(),
    );
    for frame in 0..12 {
        without_piece_state_access(|| {
            let mut store = app.world_mut().resource_mut::<PieceDataStore>();
            bridge.synchronize(&session, &mut interaction, &mut store);
            assert!(interaction
                .update(
                    PointerFrame {
                        position: Some(Vec2::splat(frame as f32 * 100.0)),
                        screen_position: Some(Vec2::ZERO),
                        pressed: true,
                        just_pressed: true,
                        ctrl: true,
                        over_ui: false,
                        focused: frame % 2 == 0,
                    },
                    &mut store,
                    &mut selection,
                    player
                )
                .is_empty());
            assert!(interaction
                .cancel(&mut store, &mut selection, player)
                .is_empty());
            assert!(interaction
                .update_rotation(&store, &mut selection, Some(Vec2::ZERO), 1)
                .is_none());
            bridge.present_release(&mut interaction, &mut store);
            assert!(bridge.next(&session, player, &store).unwrap().is_none());
            assert!(bridge
                .drag_update(&session, player, &store)
                .unwrap()
                .is_none());
        });
        without_piece_state_access(|| app.update());
        let store = app.world().resource::<PieceDataStore>();
        assert!(Arc::ptr_eq(&store.drag.members, &frozen));
        assert_eq!(store.drag.delta, Vec2::new(31.0, -13.0));
        assert_eq!(store.states.as_ptr(), states);
        assert!(store.dirty_pieces.is_empty());
        let upload = app.world().resource::<PieceUpload>();
        assert!(Arc::ptr_eq(&upload.drag.members, &frozen));
        assert_eq!(upload.revision, revision);
        assert_eq!(upload.root_revision, root_revision);
        assert!(upload.initial.is_none());
        assert!(upload.ranges.is_empty());
    }
}

#[test]
fn pending_release_cancel_reject_scope_and_stale_token_cannot_leave_or_restore_offset() {
    for cleanup in 0..7 {
        let (mut bridge, mut interaction, mut store) = fixture(4);
        let player = PlayerId(37);
        let session = session();
        bridge.synchronize(&session, &mut interaction, &mut store);
        bridge.next(&session, player, &store).unwrap().unwrap();
        bridge.present_release(&mut interaction, &mut store);
        let canonical = store.states.clone();
        let remote = ProtocolAuthorityEvent::DragCancelled(DragCancelled {
            player: PlayerId(99),
            grab_sequence: 0,
        });
        bridge
            .reconcile(player, &remote, &mut interaction, &mut store)
            .unwrap();
        assert!(
            !store.drag.members.is_empty(),
            "remote lifecycle cannot clear local offset"
        );
        bridge
            .reconcile(
                player,
                &ProtocolAuthorityEvent::ReleaseCommitted(ReleaseCommitted {
                    player: PlayerId(99),
                    grab_sequence: 0,
                    final_delta: Vec2::ZERO,
                    result: ReleaseResultFingerprint(0),
                }),
                &mut interaction,
                &mut store,
            )
            .unwrap();
        assert_eq!(store.drag.delta, Vec2::new(31.0, -13.0));
        match cleanup {
            0 => bridge
                .reconcile(
                    player,
                    &ProtocolAuthorityEvent::DragCancelled(DragCancelled {
                        player,
                        grab_sequence: 0,
                    }),
                    &mut interaction,
                    &mut store,
                )
                .unwrap(),
            1 => bridge.reject(&mut interaction, &mut store),
            2 => {
                store.epoch += 1;
                bridge.synchronize(&session, &mut interaction, &mut store);
            }
            3 => bridge.synchronize(
                &AuthoritySession::new(
                    SessionDefinition {
                        id: SessionId(2),
                        image_hash: ImageHash([0; 32]),
                    },
                    player,
                    AuthorityCursor::new(1, 0),
                ),
                &mut interaction,
                &mut store,
            ),
            4 => {
                // A stale presentation/ACK must not erase a replacement gesture.
                interaction = PieceInteraction::default();
                store.drag = DragTransform::default();
                bridge.present_release(&mut interaction, &mut store);
                assert!(store.drag.members.is_empty());
                store.drag.delta = Vec2::splat(9.0);
                bridge
                    .reconcile(
                        player,
                        &ProtocolAuthorityEvent::ReleaseCommitted(ReleaseCommitted {
                            player,
                            grab_sequence: 0,
                            final_delta: Vec2::new(31.0, -13.0),
                            result: ReleaseResultFingerprint(0),
                        }),
                        &mut interaction,
                        &mut store,
                    )
                    .unwrap();
                assert_eq!(store.drag.delta, Vec2::splat(9.0));
            }
            5 => {
                let mut frozen = session;
                frozen.host_lost().unwrap();
                bridge.synchronize(&frozen, &mut interaction, &mut store);
            }
            _ => bridge.synchronize(
                &AuthoritySession::new(
                    session.session_definition(),
                    player,
                    AuthorityCursor::new(1, 0),
                ),
                &mut interaction,
                &mut store,
            ),
        }
        bridge.present_release(&mut interaction, &mut store);
        assert!(store.drag.members.is_empty());
        assert_eq!(store.states, canonical);
        assert!(bridge.release.is_none());
        if cleanup == 2 {
            assert_eq!(
                bridge.next_control,
                Some(2),
                "store reinstall preserves consumed Control history"
            );
        }
    }
}

#[test]
fn pending_release_without_an_accepted_context_is_a_noop() {
    let (mut bridge, mut interaction, mut store) = fixture(4);
    bridge.active = None;
    bridge.present_release(&mut interaction, &mut store);
    assert!(bridge
        .next(&session(), PlayerId(37), &store)
        .unwrap()
        .is_none());
    bridge.present_release(&mut interaction, &mut store);
    assert!(bridge.release.is_none());
    assert!(store.drag.members.is_empty());
    let mut selection = PuzzleSelection::default();
    interaction.update(
        PointerFrame {
            position: Some(Vec2::ZERO),
            screen_position: Some(Vec2::ZERO),
            pressed: true,
            just_pressed: true,
            ctrl: false,
            over_ui: false,
            focused: true,
        },
        &mut store,
        &mut selection,
        PlayerId(37),
    );
    assert!(selection.latest.is_some());
}

#[test]
fn pending_release_reliable_cancellation_and_late_transient_keep_canonical_position() {
    use crate::multiplayer::{protocol::ProtocolDragContexts, replication::PeerReplicationState};
    let player = PlayerId(37);
    let definition = puzzella_core::PuzzleDefinition {
        generator_version: puzzella_core::GENERATOR_VERSION,
        seed: 1,
        grid_size: UVec2::new(2, 1),
        image_size: UVec2::new(128, 64),
        snap_distance: 0.01,
        rotation_enabled: true,
    };
    let mut authority = session();
    let mut peer = session();
    let mut host_store = PieceDataStore::default();
    let mut store = PieceDataStore::default();
    for s in [&mut host_store, &mut store] {
        s.initialize(vec![Vec2::new(100.0, 100.0), Vec2::new(200.0, 100.0)]);
    }
    let mut interaction = PieceInteraction::default();
    let token = interaction.network_gesture_token();
    let mut bridge = CommandBridge::default();
    let mut contexts = ProtocolDragContexts::default();
    let mut replica = PeerReplicationState::default();
    bridge
        .enqueue(PieceCommand::Grab(PieceId(0)), None, token.clone())
        .unwrap();
    let grab = bridge.next(&peer, player, &store).unwrap().unwrap();
    let ack = contexts
        .apply_replicated(
            &mut authority,
            &mut host_store,
            player,
            &grab,
            Some(&definition),
            player,
        )
        .unwrap()
        .authority_event
        .unwrap();
    replica
        .apply_event(
            &mut peer,
            &mut store,
            player,
            &ack,
            Some(&definition),
            player,
        )
        .unwrap();
    bridge
        .reconcile(player, &ack.event, &mut interaction, &mut store)
        .unwrap();
    let late = contexts
        .apply_replicated(
            &mut authority,
            &mut host_store,
            player,
            &ProtocolCommandEnvelope {
                command: ProtocolPieceCommand::DragUpdate {
                    delta: Vec2::splat(5.0),
                },
                sequence: ClientCommandSequence::Move {
                    after_control_sequence: 0,
                    tick: 0,
                },
                ..grab
            },
            Some(&definition),
            player,
        )
        .unwrap()
        .drag_update
        .unwrap();
    bridge
        .enqueue(
            PieceCommand::ReleaseGroup {
                members: PieceBitSet::new(2),
                delta: Vec2::new(31.0, -13.0),
            },
            None,
            token,
        )
        .unwrap();
    bridge.next(&peer, player, &store).unwrap().unwrap();
    bridge.present_release(&mut interaction, &mut store);
    assert_eq!(store.drag.delta, Vec2::new(31.0, -13.0));
    let cancel = contexts
        .cancel_replicated(&mut authority, &mut host_store, player)
        .unwrap()
        .unwrap()
        .authority_event;
    replica
        .apply_event(
            &mut peer,
            &mut store,
            player,
            &cancel,
            Some(&definition),
            player,
        )
        .unwrap();
    bridge
        .reconcile(player, &cancel.event, &mut interaction, &mut store)
        .unwrap();
    assert!(store.drag.members.is_empty());
    assert!(store.held_by.is_empty());
    let dropped = replica
        .apply_drag_update(&peer, &store, player, &late)
        .unwrap_err();
    assert_eq!(
        crate::network::transient::replication_drop(&dropped),
        Some(crate::network::transient::TransientDrop::MissingDragContext)
    );
    bridge.present_release(&mut interaction, &mut store);
    assert!(store.drag.members.is_empty());
    assert_eq!(store.states[0].position, Vec2::new(100.0, 100.0));
}
