use super::*;
use crate::resources::pieces::prepare_piece_upload;

fn flush_control(pair: &mut Pair) {
    let mut bus = pair.bus.lock().unwrap();
    bus.hold_authority_control = false;
    for (id, event) in std::mem::take(&mut bus.delayed) {
        bus.inbox.entry(id).or_default().push_back(event);
    }
}

fn visual(app: &App, id: PieceId) -> Vec2 {
    let store = app.world().resource::<PieceDataStore>();
    let held = store
        .drag
        .members
        .get(id.0 as usize / 32)
        .is_some_and(|word| word & (1 << (id.0 % 32)) != 0);
    store.state(id).unwrap().position + if held { store.drag.delta } else { Vec2::ZERO }
}

fn release(pair: &mut Pair, delta: Vec2) {
    pair.bus.lock().unwrap().hold_authority_control = true;
    pointer(&mut pair.client, delta, false, false);
    pair.client.update();
}

#[test]
fn pending_release_delayed_control_freezes_visual_and_commits_before_upload_with_lost_transients() {
    for drop_transient in [false, true] {
        let mut pair = Pair::new();
        pair.ready();
        pair.client
            .init_resource::<PieceUpload>()
            .add_systems(Last, prepare_piece_upload);
        begin_gesture(&mut pair.client);
        pair.converge();
        pair.bus.lock().unwrap().drop_transient = drop_transient;
        pointer(&mut pair.client, Vec2::new(10.0, 12.0), true, false);
        pair.converge();
        let canonical = pair
            .client
            .world()
            .resource::<PieceDataStore>()
            .states
            .clone();
        let delta = Vec2::new(31.0, -13.0);
        release(&mut pair, delta);
        let frozen = pair
            .client
            .world()
            .resource::<PieceDataStore>()
            .drag
            .members
            .clone();
        let revision = pair.client.world().resource::<PieceUpload>().revision;
        let visible = [
            visual(&pair.client, PieceId(0)),
            visual(&pair.client, PieceId(1)),
        ];
        let network = pair
            .client
            .world()
            .get_non_send::<NetworkSession>()
            .unwrap();
        let authority = network.authority().unwrap();
        let snapshot = crate::multiplayer::snapshot::GameSnapshot::capture(
            pair.client.world().resource::<PieceDataStore>(),
            &definition(),
            authority.session_definition(),
            authority.cursor(),
        )
        .unwrap();
        assert_eq!(snapshot.pieces[0].position, canonical[0].position);
        assert_eq!(snapshot.pieces[1].position, canonical[1].position);
        pair.host.update(); // Apply Release, but hold its encrypted Control ACK.
        assert!(!pair.bus.lock().unwrap().delayed.is_empty());
        let sent = pair.bus.lock().unwrap().sent.len();
        for frame in 0..8 {
            pointer(
                &mut pair.client,
                Vec2::splat(100.0 + frame as f32),
                true,
                true,
            );
            pair.client.update();
            let store = pair.client.world().resource::<PieceDataStore>();
            assert_eq!(store.states, canonical);
            assert_eq!(store.drag.delta, delta);
            assert!(Arc::ptr_eq(&store.drag.members, &frozen));
            assert_eq!(visual(&pair.client, PieceId(0)), visible[0]);
            assert_eq!(visual(&pair.client, PieceId(1)), visible[1]);
            assert!(!pair
                .client
                .world()
                .resource::<PieceInteraction>()
                .is_dragging());
            assert!(pair
                .client
                .world()
                .resource::<crate::selection::PuzzleSelection>()
                .latest
                .is_none());
            assert_eq!(
                pair.client.world().resource::<PieceUpload>().revision,
                revision
            );
            assert_eq!(
                pair.bus.lock().unwrap().sent.len(),
                sent,
                "no new Control or Transient"
            );
        }
        flush_control(&mut pair);
        pair.client.update();
        let store = pair.client.world().resource::<PieceDataStore>();
        assert!(store.drag.members.is_empty());
        assert_eq!(
            store.states,
            pair.host.world().resource::<PieceDataStore>().states
        );
        assert_eq!(visual(&pair.client, PieceId(0)), visible[0]);
        assert_eq!(visual(&pair.client, PieceId(1)), visible[1]);
        assert!(
            pair.client
                .world()
                .resource::<PieceUpload>()
                .drag
                .members
                .is_empty(),
            "Last observes canonical commit with pending offset already removed"
        );
        // A new gesture gets its own token only after the old result resolves.
        begin_gesture(&mut pair.client);
        pair.converge();
        assert!(pair
            .client
            .world()
            .resource::<PieceInteraction>()
            .is_dragging());
        assert_eq!(
            pair.client.world().resource::<PieceDataStore>().drag.delta,
            Vec2::ZERO
        );
    }
}

#[test]
fn pending_release_uses_partial_acceptance_even_when_release_precedes_grab_ack() {
    for early_release in [false, true] {
        let mut pair = Pair::new();
        pair.ready();
        begin_gesture(&mut pair.client);
        send(&mut pair.host, PieceCommand::Grab(PieceId(1)));
        pair.host.update();
        if early_release {
            pair.client.update(); // Send Grab, before the pointer is released.
            pair.bus.lock().unwrap().hold_authority_control = true;
            pair.host.update(); // Accept Grab, hold its ACK.
        } else {
            pair.converge();
        }
        let delta = Vec2::new(30.0, 20.0);
        release(&mut pair, delta);
        if early_release {
            // Resolve the queued Release's accepted context, then hold its commit.
            flush_control(&mut pair);
            pair.bus.lock().unwrap().hold_authority_control = true;
            pair.client.update();
        }
        pair.host.update();
        for _ in 0..4 {
            pair.client.update();
        }
        let store = pair.client.world().resource::<PieceDataStore>();
        let mut accepted = PieceBitSet::new(4);
        accepted.insert(PieceId(0));
        assert_eq!(store.drag.members, *accepted.words());
        assert_eq!(store.drag.delta, delta);
        assert_eq!(visual(&pair.client, PieceId(0)), Vec2::new(130.0, 120.0));
        assert_eq!(visual(&pair.client, PieceId(1)), Vec2::new(200.0, 100.0));
        flush_control(&mut pair);
        pair.converge();
        assert!(pair
            .client
            .world()
            .resource::<PieceDataStore>()
            .drag
            .members
            .is_empty());
    }
}

#[test]
fn pending_release_rotation_rebases_queued_and_subsequent_release_without_visual_jump() {
    for queued in [false, true] {
        let mut pair = Pair::new();
        pair.ready();
        begin_gesture(&mut pair.client);
        pair.converge();
        pointer(&mut pair.client, Vec2::new(20.0, 30.0), true, false);
        pair.client.update();
        let rotation = pair
            .client
            .world()
            .resource::<PieceInteraction>()
            .rotation_command(pair.client.world().resource::<PieceDataStore>(), 1)
            .unwrap();
        send(&mut pair.client, rotation);
        pair.client.update();
        let final_pointer = Vec2::new(35.0, 47.0);
        if queued {
            release(&mut pair, final_pointer);
            assert_eq!(visual(&pair.client, PieceId(0)), Vec2::new(135.0, 147.0));
            pair.host.update(); // Hold rotation ACK.
            flush_control(&mut pair);
            // Delay only the next authority result, the Release commit.
            pair.bus.lock().unwrap().hold_authority_control = true;
            pair.client.update();
        } else {
            pointer(&mut pair.client, final_pointer, true, false);
            pair.host.update();
            pair.client.update();
            release(&mut pair, final_pointer);
        }
        let residual = Vec2::new(15.0, 17.0);
        assert_eq!(
            pair.client.world().resource::<PieceDataStore>().drag.delta,
            residual
        );
        let visible = visual(&pair.client, PieceId(0));
        let canonical = pair
            .client
            .world()
            .resource::<PieceDataStore>()
            .states
            .clone();
        pair.host.update();
        for _ in 0..5 {
            pair.client.update();
            assert_eq!(
                pair.client.world().resource::<PieceDataStore>().states,
                canonical
            );
            assert_eq!(visual(&pair.client, PieceId(0)), visible);
        }
        assert_eq!(
            pair.host.world().resource::<PieceDataStore>().states[0].position,
            visible
        );
        flush_control(&mut pair);
        pair.converge();
        assert!(pair
            .client
            .world()
            .resource::<PieceDataStore>()
            .drag
            .members
            .is_empty());
        assert_eq!(visual(&pair.client, PieceId(0)), visible);
    }
}

#[test]
fn pending_release_disconnect_stop_and_menu_clear_presentation_without_predicting() {
    for cleanup in 0..3 {
        let mut pair = Pair::new();
        pair.ready();
        begin_gesture(&mut pair.client);
        pair.converge();
        let canonical = pair
            .client
            .world()
            .resource::<PieceDataStore>()
            .states
            .clone();
        release(&mut pair, Vec2::new(31.0, -13.0));
        match cleanup {
            0 => {
                stop_session(pair.host.world_mut());
                pair.client.update();
            }
            1 => stop_session(pair.client.world_mut()),
            _ => {
                pair.client
                    .world_mut()
                    .resource_mut::<NextState<AppState>>()
                    .set(AppState::Menu);
                pair.client.update();
            }
        }
        let store = pair.client.world().resource::<PieceDataStore>();
        assert!(store.drag.members.is_empty());
        assert_eq!(store.states[0].position, canonical[0].position);
        assert!(!pair
            .client
            .world()
            .resource::<PieceInteraction>()
            .is_dragging());
    }
}

#[test]
fn pending_release_host_success_and_semantic_rejection_leave_no_pending_presentation() {
    for rejected in [false, true] {
        let mut pair = Pair::new();
        pair.ready();
        begin_gesture(&mut pair.host);
        pair.converge();
        pointer(&mut pair.host, Vec2::new(31.0, -13.0), false, false);
        if rejected {
            pair.host
                .world_mut()
                .resource_mut::<Messages<ClientCommand>>()
                .clear();
            send(
                &mut pair.host,
                PieceCommand::ReleaseGroup {
                    members: members(),
                    delta: Vec2::splat(f32::NAN),
                },
            );
        }
        pair.host.update();
        assert!(pair
            .host
            .world()
            .resource::<PieceDataStore>()
            .drag
            .members
            .is_empty());
        let expected = if rejected {
            Vec2::splat(100.0)
        } else {
            Vec2::new(131.0, 87.0)
        };
        assert_eq!(
            pair.host.world().resource::<PieceDataStore>().states[0].position,
            expected
        );
        pointer(&mut pair.host, Vec2::ZERO, true, true);
        assert!(pair
            .host
            .world()
            .resource::<crate::selection::PuzzleSelection>()
            .latest
            .is_some());
    }
}

#[test]
fn pending_release_offline_gesture_still_commits_immediately() {
    let mut app = app();
    host_world(&mut app);
    begin_gesture(&mut app);
    app.update();
    pointer(&mut app, Vec2::new(31.0, -13.0), false, false);
    app.update();
    assert_eq!(
        app.world().resource::<PieceDataStore>().states[0].position,
        Vec2::new(131.0, 87.0)
    );
    assert!(app
        .world()
        .resource::<PieceDataStore>()
        .drag
        .members
        .is_empty());
    assert!(!app.world().contains_non_send::<NetworkSession>());
    pointer(&mut app, Vec2::ZERO, true, true);
    assert!(app
        .world()
        .resource::<crate::selection::PuzzleSelection>()
        .latest
        .is_some());
}

#[test]
fn pending_release_authority_rejection_protocol_error_and_send_failure_clear_offset() {
    for failure in 0..3 {
        let mut pair = Pair::new();
        pair.ready();
        begin_gesture(&mut pair.client);
        pair.converge();
        let canonical = pair
            .client
            .world()
            .resource::<PieceDataStore>()
            .states
            .clone();
        let connection = pair
            .bus
            .lock()
            .unwrap()
            .routes
            .iter()
            .find_map(|(&local, &(id, _))| (id == 0).then_some(local))
            .unwrap();
        pointer(&mut pair.client, Vec2::new(31.0, -13.0), false, false);
        if failure == 0 {
            // Finite wire input, semantically outside the authority play area.
            pair.client
                .world_mut()
                .resource_mut::<Messages<ClientCommand>>()
                .clear();
            send(
                &mut pair.client,
                PieceCommand::ReleaseGroup {
                    members: members(),
                    delta: Vec2::splat(1_000_000.0),
                },
            );
        } else if failure == 2 {
            pair.bus.lock().unwrap().fail.insert(connection);
        }
        pair.client.update();
        if failure == 1 {
            assert!(!pair
                .client
                .world()
                .resource::<PieceDataStore>()
                .drag
                .members
                .is_empty());
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
        } else if failure == 0 {
            pair.host.update();
        }
        pair.client.update();
        assert!(matches!(
            pair.client.world().resource::<NetworkStatus>().phase,
            RuntimePhase::Failed | RuntimePhase::Disconnected
        ));
        let store = pair.client.world().resource::<PieceDataStore>();
        assert!(store.drag.members.is_empty());
        assert_eq!(store.states[0].position, canonical[0].position);
    }
}
