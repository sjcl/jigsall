use super::*;
use crate::resources::pieces::{prepare_piece_upload, without_piece_state_access, PieceUpload};
use bevy::prelude::*;
use jigsall_core::{PieceId, PuzzleDefinition, GENERATOR_VERSION};

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
fn local_rotation_million_store_small_and_dense_boundaries_and_idle_zero_uploads() {
    let def = PuzzleDefinition {
        generator_version: GENERATOR_VERSION,
        seed: 37,
        grid_size: UVec2::splat(1000),
        image_size: UVec2::splat(1000),
        snap_distance: 0.01,
        rotation_enabled: true,
    };
    let mut app = App::new();
    app.init_resource::<PieceDataStore>()
        .init_resource::<PieceUpload>()
        .insert_resource(def.clone())
        .add_systems(Update, prepare_piece_upload);
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .initialize(vec![Vec2::splat(100.0); 1_000_000]);
    app.update();
    app.update();
    let canonical: Arc<[_]> = app
        .world()
        .resource::<PieceDataStore>()
        .states
        .clone()
        .into();
    let mut interaction = PieceInteraction::default();
    let mut bridge = CommandBridge {
        prediction_enabled: true,
        ..Default::default()
    };
    let session = session();
    let player = PlayerId(37);
    for dense in [false, true] {
        let mut store = app.world_mut().remove_resource::<PieceDataStore>().unwrap();
        bridge.synchronize(&session, &mut interaction, &mut store);
        let target = if dense {
            let mut members = PieceBitSet::new(store.len());
            members.fill();
            PieceTarget::from_selection(&store.connectivity, &members).unwrap()
        } else {
            PieceTarget::Component(
                ComponentRef::from_member(&store.connectivity, PieceId(999_999)).unwrap(),
            )
        };
        bridge
            .enqueue(
                PieceCommand::Rotate {
                    target,
                    quarter_turns: 1,
                },
                None,
                interaction.network_gesture_token(),
            )
            .unwrap();
        bridge.next(&session, player, &store).unwrap().unwrap();
        bridge.refresh_prediction(player, Some(&def), &interaction, &mut store);
        assert_eq!(
            store.local_rotation.poses.len(),
            if dense { 1_000_000 } else { 1 }
        );
        assert!(
            Arc::ptr_eq(&canonical, &store.states.clone().into()),
            "prediction copied canonical Arc"
        );
        assert_eq!(store.placed_count, 0);
        app.world_mut().insert_resource(store);
        app.update();
        let upload = app.world().resource::<PieceUpload>();
        assert_eq!(upload.ranges.len(), 1);
        assert_eq!(
            upload.ranges[0].states.len(),
            if dense { 1_000_000 } else { 1 }
        );
        let revision = upload.revision;
        let roots = upload.root_revision;
        let capacity = app
            .world()
            .resource::<PieceDataStore>()
            .local_rotation
            .poses
            .capacity();
        for _ in 0..8 {
            let mut store = app.world_mut().remove_resource::<PieceDataStore>().unwrap();
            without_piece_state_access(|| {
                bridge.synchronize(&session, &mut interaction, &mut store);
                bridge.present_release(&mut interaction, &mut store);
                assert!(bridge.next(&session, player, &store).unwrap().is_none());
                assert!(bridge
                    .drag_update(&session, player, &store)
                    .unwrap()
                    .is_none());
            });
            assert_eq!(store.local_rotation.poses.capacity(), capacity);
            app.world_mut().insert_resource(store);
            without_piece_state_access(|| app.update());
            let upload = app.world().resource::<PieceUpload>();
            assert_eq!(upload.revision, revision);
            assert_eq!(upload.root_revision, roots);
        }
        let mut store = app.world_mut().remove_resource::<PieceDataStore>().unwrap();
        bridge.reject(&mut interaction, &mut store);
        bridge.refresh_prediction(player, Some(&def), &interaction, &mut store);
        assert!(store.local_rotation.poses.is_empty());
        app.world_mut().insert_resource(store);
        app.update();
        assert_eq!(
            app.world().resource::<PieceUpload>().ranges[0].states[0].flags,
            crate::resources::pieces::ENABLED
        );
    }
}

#[test]
fn local_rotation_scope_epoch_inactive_and_rejection_restore_overrides() {
    for cleanup in 0..5 {
        let mut store = PieceDataStore::default();
        store.initialize(vec![Vec2::splat(100.0)]);
        let def = PuzzleDefinition {
            generator_version: GENERATOR_VERSION,
            seed: 1,
            grid_size: UVec2::ONE,
            image_size: UVec2::splat(128),
            snap_distance: 0.01,
            rotation_enabled: true,
        };
        let mut session = session();
        let mut interaction = PieceInteraction::default();
        let mut bridge = CommandBridge {
            prediction_enabled: true,
            ..Default::default()
        };
        bridge.synchronize(&session, &mut interaction, &mut store);
        bridge
            .enqueue(
                PieceCommand::Rotate {
                    target: PieceTarget::Component(
                        ComponentRef::from_member(&store.connectivity, PieceId(0)).unwrap(),
                    ),
                    quarter_turns: 1,
                },
                None,
                interaction.network_gesture_token(),
            )
            .unwrap();
        bridge.next(&session, PlayerId(37), &store).unwrap();
        bridge.refresh_prediction(PlayerId(37), Some(&def), &interaction, &mut store);
        assert_eq!(store.local_rotation.poses.len(), 1);
        store.dirty_pieces.clear();
        match cleanup {
            0 => bridge.reject(&mut interaction, &mut store),
            1 => {
                store.epoch += 1;
                bridge.synchronize(&session, &mut interaction, &mut store);
            }
            2 => {
                session = AuthoritySession::new(
                    session.session_definition(),
                    PlayerId(37),
                    AuthorityCursor::new(1, 0),
                );
                bridge.synchronize(&session, &mut interaction, &mut store);
            }
            3 => {
                session.begin_graceful(PlayerId(38)).unwrap();
                bridge.synchronize(&session, &mut interaction, &mut store);
            }
            _ => {
                let mut members = PieceBitSet::new(1);
                members.insert(PieceId(0));
                bridge.active = Some(LocalDrag {
                    grab: 0,
                    basis: 0,
                    last_tick: None,
                    members,
                    last_delta: None,
                    scalar_delta: Vec2::ZERO,
                    token: interaction.network_gesture_token(),
                });
                bridge
                    .reconcile(
                        PlayerId(37),
                        &ProtocolAuthorityEvent::DragCancelled(DragCancelled {
                            player: PlayerId(37),
                            grab_sequence: 0,
                        }),
                        &mut interaction,
                        &mut store,
                    )
                    .unwrap();
            }
        }
        assert!(store.local_rotation.poses.is_empty());
        assert!(store.dirty_pieces.contains(&PieceId(0)));
        assert_eq!(jigsall_core::decode_rotation(store.states[0].flags), 0);
    }
}

#[test]
fn local_rotation_old_gesture_ack_and_cancel_do_not_rebase_new_prediction() {
    let player = PlayerId(37);
    let def = PuzzleDefinition {
        generator_version: GENERATOR_VERSION,
        seed: 1,
        grid_size: UVec2::new(2, 1),
        image_size: UVec2::splat(128),
        snap_distance: 0.01,
        rotation_enabled: true,
    };
    let mut store = PieceDataStore::default();
    store.initialize(vec![Vec2::splat(100.0), Vec2::new(200.0, 100.0)]);
    let old_token = PieceInteraction::default().network_gesture_token();
    let mut interaction = PieceInteraction::default();
    let new_token = interaction.network_gesture_token();
    let mut old_members = PieceBitSet::new(2);
    old_members.insert(PieceId(0));
    let mut new_members = PieceBitSet::new(2);
    new_members.insert(PieceId(1));
    store.drag.members = new_members.words().clone();
    store.drag.delta = Vec2::new(9.0, 13.0);
    store.selected_pieces = new_members.clone();
    let old_command = PieceCommand::RotateDrag {
        members: old_members.clone(),
        delta: Vec2::splat(5.0),
        quarter_turns: 1,
    };
    let mut bridge = CommandBridge {
        prediction_enabled: true,
        active: Some(LocalDrag {
            grab: 0,
            basis: 0,
            last_tick: None,
            members: old_members,
            last_delta: None,
            scalar_delta: Vec2::ZERO,
            token: old_token.clone(),
        }),
        pending: Some(PendingControl {
            envelope: CommandBridge::envelope(
                &session(),
                player,
                ClientCommandSequence::Control(1),
                ProtocolPieceCommand::RotateDrag {
                    grab_sequence: 0,
                    final_delta: Vec2::splat(5.0),
                    through_tick: None,
                    quarter_turns: 1,
                },
            ),
            requested: None,
            pointer: Some(Vec2::splat(5.0)),
            token: old_token,
            predicted: Some(old_command),
        }),
        ..Default::default()
    };
    bridge
        .enqueue(PieceCommand::Grab(PieceId(1)), None, new_token.clone())
        .unwrap();
    bridge
        .enqueue(
            PieceCommand::RotateDrag {
                members: new_members.clone(),
                delta: store.drag.delta,
                quarter_turns: 1,
            },
            None,
            new_token.clone(),
        )
        .unwrap();
    bridge.refresh_prediction(player, Some(&def), &interaction, &mut store);
    let before = store.presentation_state(PieceId(1));
    assert_eq!(jigsall_core::decode_rotation(before.flags), 1);
    bridge
        .reconcile(
            player,
            &ProtocolAuthorityEvent::DragRotationCommitted(DragRotationCommitted {
                player,
                grab_sequence: 0,
                basis_sequence: 1,
                through_tick: None,
                final_delta: Vec2::splat(5.0),
                quarter_turns: 1,
                result: ReleaseResultFingerprint(0),
            }),
            &mut interaction,
            &mut store,
        )
        .unwrap();
    bridge.refresh_prediction(player, Some(&def), &interaction, &mut store);
    assert_eq!(store.presentation_state(PieceId(1)), before);
    assert_eq!(store.drag.delta, Vec2::new(9.0, 13.0));
    assert_eq!(store.selected_pieces, new_members);
    assert!(Arc::ptr_eq(
        &interaction.network_gesture_token(),
        &new_token
    ));
    // An obsolete cancellation cannot discard an unrelated newer drag context.
    bridge.active.as_mut().unwrap().grab = 10;
    bridge
        .reconcile(
            player,
            &ProtocolAuthorityEvent::DragCancelled(DragCancelled {
                player,
                grab_sequence: 0,
            }),
            &mut interaction,
            &mut store,
        )
        .unwrap();
    assert_eq!(store.presentation_state(PieceId(1)), before);
    assert_eq!(bridge.active.as_ref().unwrap().grab, 10);
}
