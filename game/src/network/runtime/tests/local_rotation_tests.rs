use super::*;
use crate::resources::pieces::{prepare_piece_upload, HELD};
use puzzella_core::decode_rotation;

fn pose(app: &App, id: PieceId) -> (Vec2, u32) {
    let store = app.world().resource::<PieceDataStore>();
    let state = store.presentation_state(id);
    let delta = if store
        .drag
        .members
        .get(id.0 as usize / 32)
        .is_some_and(|word| word & (1 << (id.0 % 32)) != 0)
    {
        store.drag.delta
    } else {
        Vec2::ZERO
    };
    (state.position + delta, decode_rotation(state.flags))
}

fn displayed(app: &App) -> [(Vec2, u32); 2] {
    [pose(app, PieceId(0)), pose(app, PieceId(1))]
}

fn assert_display(app: &App, expected: [(Vec2, u32); 2]) {
    for (actual, expected) in displayed(app).into_iter().zip(expected) {
        assert_eq!(actual.1, expected.1, "orientation rolled back");
        assert!(
            actual.0.distance(expected.0) < 0.0001,
            "position {actual:?} != {expected:?}"
        );
    }
    // Last/upload sees the final reconciled pose, never canonical-only state.
    let upload = app.world().resource::<PieceUpload>();
    for range in upload.ranges.iter() {
        for (index, state) in range.states.iter().enumerate() {
            let id = PieceId(range.start + index as u32);
            assert_eq!(
                *state,
                app.world()
                    .resource::<PieceDataStore>()
                    .presentation_state(id)
            );
        }
    }
}

fn deliver_one(pair: &mut Pair) {
    let mut bus = pair.bus.lock().unwrap();
    assert!(!bus.delayed.is_empty());
    let (id, event) = bus.delayed.remove(0);
    bus.inbox.entry(id).or_default().push_back(event);
}

fn uploads(pair: &mut Pair) {
    pair.client
        .init_resource::<PieceUpload>()
        .add_systems(Last, prepare_piece_upload);
    pair.client.update();
    pair.client.update(); // Retire the initial canonical Arc before mutations.
}

fn connected_pair() -> Pair {
    let mut pair = Pair::new();
    {
        let mut store = pair.host.world_mut().resource_mut::<PieceDataStore>();
        let def = definition();
        for id in [PieceId(0), PieceId(1)] {
            store.states[id.0 as usize].position =
                def.correct_position(id) + Vec2::new(140.0, 60.0);
        }
        store.snap_unheld_component(PieceId(0), &def);
        assert!(store.connectivity.same_component(PieceId(0), PieceId(1)));
        for id in [PieceId(2), PieceId(3)] {
            store.states[id.0 as usize].position =
                def.correct_position(id) + Vec2::new(-140.0, -60.0);
        }
        store.snap_unheld_component(PieceId(2), &def);
        assert!(store.connectivity.same_component(PieceId(2), PieceId(3)));
    }
    pair.ready();
    uploads(&mut pair);
    pair
}

fn rotate(pair: &mut Pair, turns: i8) {
    let command = pair
        .client
        .world()
        .resource::<PieceInteraction>()
        .rotation_command(pair.client.world().resource::<PieceDataStore>(), turns)
        .unwrap();
    send(&mut pair.client, command);
    pair.client.update();
}

#[test]
fn local_rotate_multiple_inputs_retire_only_the_acknowledged_prefix() {
    for turns in [vec![1, 1], vec![1, 1, -1]] {
        let mut pair = connected_pair();
        pair.client
            .world_mut()
            .resource_mut::<PieceDataStore>()
            .selected_pieces = members();
        pair.bus.lock().unwrap().hold_authority_control = true;
        let before_cursor = cursor(&pair.client);
        let before = pair
            .client
            .world()
            .resource::<PieceDataStore>()
            .states
            .to_vec();
        for (index, &turn) in turns.iter().enumerate() {
            rotate(&mut pair, turn);
            let expected = turns[..=index]
                .iter()
                .map(|&t| i32::from(t))
                .sum::<i32>()
                .rem_euclid(4) as u32;
            assert_eq!(pose(&pair.client, PieceId(0)).1, expected);
            assert_eq!(
                &*pair.client.world().resource::<PieceDataStore>().states,
                before.as_slice()
            );
            assert_eq!(cursor(&pair.client), before_cursor);
        }
        let expected = displayed(&pair.client);
        for index in 0..turns.len() {
            pair.host.update();
            deliver_one(&mut pair);
            pair.client.update();
            assert_display(&pair.client, expected);
            let committed = turns[..=index]
                .iter()
                .map(|&t| i32::from(t))
                .sum::<i32>()
                .rem_euclid(4) as u32;
            assert_eq!(
                decode_rotation(pair.client.world().resource::<PieceDataStore>().states[0].flags),
                committed
            );
        }
        assert!(pair
            .client
            .world()
            .resource::<PieceDataStore>()
            .local_rotation
            .poses
            .is_empty());
    }
}

#[test]
fn local_rotate_drag_partial_ack_chain_preserves_rotation_pointer_and_pending_release() {
    for move_between in [false, true] {
        let mut pair = connected_pair();
        begin_gesture(&mut pair.client);
        pair.converge();
        pair.bus.lock().unwrap().hold_authority_control = true;
        let a = Vec2::new(13.0, 17.0);
        let b = Vec2::new(27.0, 23.0);
        let c = Vec2::new(39.0, 31.0);
        pointer(&mut pair.client, a, true, false);
        rotate(&mut pair, 1);
        let first = displayed(&pair.client);
        if move_between {
            pointer(&mut pair.client, b, true, false);
        }
        rotate(&mut pair, 1);
        let second = displayed(&pair.client);
        assert_eq!(second[0].1, 2);
        pointer(&mut pair.client, c, true, false);
        pair.client.update();
        let final_pose = displayed(&pair.client);
        let last_rotation_pointer = if move_between { b } else { a };
        for index in 0..2 {
            assert!(
                final_pose[index]
                    .0
                    .distance(second[index].0 + c - last_rotation_pointer)
                    < 0.0001
            );
            assert_eq!(first[index].1, 1);
        }
        pointer(&mut pair.client, c, false, false);
        pair.client.update();
        assert_display(&pair.client, final_pose);
        let canonical = pair
            .client
            .world()
            .resource::<PieceDataStore>()
            .states
            .to_vec();
        let revision = pair.client.world().resource::<PieceUpload>().revision;
        for _ in 0..4 {
            pair.client.update();
            assert_eq!(
                pair.client.world().resource::<PieceUpload>().revision,
                revision
            );
            assert_eq!(
                &*pair.client.world().resource::<PieceDataStore>().states,
                canonical.as_slice()
            );
            assert_display(&pair.client, final_pose);
        }
        // Deliver rotation #1, rotation #2 and Release in three separate frames.
        for stage in 1..=3 {
            pair.host.update();
            deliver_one(&mut pair);
            pair.client.update();
            assert_display(&pair.client, final_pose);
            let store = pair.client.world().resource::<PieceDataStore>();
            assert_eq!(decode_rotation(store.states[0].flags), stage.min(2));
            if stage == 1 {
                assert!(!store.local_rotation.poses.is_empty());
            }
            if stage == 3 {
                assert!(store.local_rotation.poses.is_empty());
                assert!(store.drag.members.is_empty());
                assert_eq!(store.states[0].flags & HELD, 0);
            }
        }
    }
}

#[test]
fn local_rotation_partial_grab_replaces_optimistic_members_before_replay() {
    for early_release in [false, true] {
        let mut pair = Pair::new();
        pair.ready();
        uploads(&mut pair);
        pair.bus.lock().unwrap().hold_authority_control = true;
        send(&mut pair.host, PieceCommand::Grab(PieceId(1)));
        pair.host.update(); // Host wins; the local view still sees both as unheld.
        begin_gesture(&mut pair.client);
        pair.client.update(); // Grab is in flight, authority has not accepted it.
        pointer(&mut pair.client, Vec2::new(9.0, 11.0), true, false);
        rotate(&mut pair, 1);
        let accepted_pose = pose(&pair.client, PieceId(0));
        assert_eq!(pose(&pair.client, PieceId(1)).1, 1);
        if early_release {
            pointer(&mut pair.client, Vec2::new(9.0, 11.0), false, false);
            pair.client.update();
        }
        pair.host.update(); // Partial client Grab accepted: only member 0.
        deliver_one(&mut pair); // Competing host ownership.
        pair.client.update();
        assert_eq!(pose(&pair.client, PieceId(0)), accepted_pose);
        assert_eq!(pose(&pair.client, PieceId(1)).1, 0);
        deliver_one(&mut pair);
        pair.client.update();
        assert_eq!(pose(&pair.client, PieceId(0)), accepted_pose);
        assert_eq!(pose(&pair.client, PieceId(1)).1, 0);
        let store = pair.client.world().resource::<PieceDataStore>();
        assert_eq!(store.drag.members[0], 1);
    }
}

#[test]
fn local_rotation_partial_rotate_acceptance_corrects_only_rejected_component() {
    let mut pair = connected_pair();
    pair.client
        .world_mut()
        .resource_mut::<PieceDataStore>()
        .selected_pieces
        .fill();
    pair.bus.lock().unwrap().hold_authority_control = true;
    // Commit a competing host Grab, with its client notification delayed.
    send(&mut pair.host, PieceCommand::Grab(PieceId(2)));
    pair.host.update();
    rotate(&mut pair, 1);
    assert_eq!(pose(&pair.client, PieceId(2)).1, 1);
    let accepted = displayed(&pair.client);
    pair.host.update();
    deliver_one(&mut pair); // Competing ownership becomes canonical first.
    pair.client.update();
    assert_eq!(pose(&pair.client, PieceId(2)).1, 0);
    deliver_one(&mut pair); // Partial RotationCommitted.
    pair.client.update();
    assert_display(&pair.client, accepted);
    assert_eq!(pose(&pair.client, PieceId(2)).1, 0);
    assert!(pair
        .client
        .world()
        .resource::<PieceDataStore>()
        .local_rotation
        .poses
        .is_empty());
}

#[test]
fn local_rotation_checkpoint_and_snapshot_capture_only_canonical_pose() {
    let mut pair = connected_pair();
    pair.client
        .world_mut()
        .resource_mut::<PieceDataStore>()
        .selected_pieces = members();
    pair.bus.lock().unwrap().hold_authority_control = true;
    rotate(&mut pair, 1);
    let network = pair
        .client
        .world()
        .get_non_send::<NetworkSession>()
        .unwrap();
    let authority = network.authority().unwrap();
    let store = pair.client.world().resource::<PieceDataStore>();
    let snapshot = crate::multiplayer::snapshot::GameSnapshot::capture(
        store,
        &definition(),
        authority.session_definition(),
        authority.cursor(),
    )
    .unwrap();
    let checkpoint = crate::checkpoint::PuzzleCheckpoint::capture(
        store,
        &definition(),
        authority.session_definition().image_hash,
    )
    .unwrap();
    for id in 0..2 {
        assert_eq!(snapshot.pieces[id].position, store.states[id].position);
        assert_eq!(decode_rotation(snapshot.pieces[id].flags), 0);
        assert_eq!(checkpoint.pieces[id].position, store.states[id].position);
        assert_eq!(decode_rotation(checkpoint.pieces[id].flags), 0);
    }
    assert_eq!(pose(&pair.client, PieceId(0)).1, 1);
}

#[test]
fn local_rotation_disconnect_stop_and_menu_restore_canonical_upload() {
    for cleanup in 0..3 {
        let mut pair = connected_pair();
        pair.client
            .world_mut()
            .resource_mut::<PieceDataStore>()
            .selected_pieces = members();
        pair.bus.lock().unwrap().hold_authority_control = true;
        rotate(&mut pair, 1);
        assert!(!pair
            .client
            .world()
            .resource::<PieceDataStore>()
            .local_rotation
            .poses
            .is_empty());
        match cleanup {
            0 => {
                stop_session(pair.host.world_mut());
                pair.client.update();
            }
            1 => {
                stop_session(pair.client.world_mut());
                pair.client.update();
            }
            _ => {
                pair.client
                    .world_mut()
                    .resource_mut::<NextState<AppState>>()
                    .set(AppState::Menu);
                pair.client.update();
            }
        }
        assert!(pair
            .client
            .world()
            .resource::<PieceDataStore>()
            .local_rotation
            .poses
            .is_empty());
    }
}

#[test]
fn local_rotation_protocol_and_send_failure_discard_pending_suffix() {
    for send_failure in [false, true] {
        let mut pair = connected_pair();
        begin_gesture(&mut pair.client);
        pair.converge();
        pair.bus.lock().unwrap().hold_authority_control = true;
        pointer(&mut pair.client, Vec2::new(9.0, 13.0), true, false);
        rotate(&mut pair, 1);
        rotate(&mut pair, 1);
        let connection = pair
            .bus
            .lock()
            .unwrap()
            .routes
            .iter()
            .find_map(|(&local, &(id, _))| (id == 0).then_some(local))
            .unwrap();
        if send_failure {
            pair.host.update();
            deliver_one(&mut pair);
            pair.bus.lock().unwrap().fail.insert(connection);
        } else {
            pair.bus
                .lock()
                .unwrap()
                .inbox
                .entry(1)
                .or_default()
                .push_back(TransportEvent::Message {
                    connection,
                    class: MessageClass::Control,
                    payload: vec![1, 2, 3],
                });
        }
        pair.client.update();
        assert!(matches!(
            pair.client.world().resource::<NetworkStatus>().phase,
            RuntimePhase::Failed | RuntimePhase::Disconnected
        ));
        let store = pair.client.world().resource::<PieceDataStore>();
        assert!(store.local_rotation.poses.is_empty());
        assert!(store.drag.members.is_empty());
        assert_eq!(
            decode_rotation(store.states[0].flags),
            u32::from(send_failure)
        );
    }
}

#[test]
fn local_rotation_host_and_offline_commit_without_pending_override() {
    let mut pair = connected_pair();
    for network in [false, true] {
        let mut offline = app();
        host_world(&mut offline);
        let app = if network {
            &mut pair.host
        } else {
            &mut offline
        };
        let target = PieceTarget::from_selection(
            &app.world().resource::<PieceDataStore>().connectivity,
            &members(),
        )
        .unwrap();
        send(
            app,
            PieceCommand::Rotate {
                target,
                quarter_turns: 1,
            },
        );
        app.update();
        let store = app.world().resource::<PieceDataStore>();
        assert_eq!(decode_rotation(store.states[0].flags), 1);
        assert!(store.local_rotation.poses.is_empty());
    }
}
