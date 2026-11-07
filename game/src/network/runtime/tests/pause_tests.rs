use super::*;

fn request(app: &mut App, paused: bool) {
    app.world_mut()
        .resource_mut::<GameplayPause>()
        .request(paused);
}

fn assert_paused(app: &App, paused: bool) {
    assert_eq!(app.world().resource::<GameplayPause>().paused, paused);
    assert_eq!(
        app.world()
            .get_non_send::<NetworkSession>()
            .unwrap()
            .authority()
            .unwrap()
            .gameplay_paused(),
        paused
    );
}

#[test]
fn host_pause_cancels_active_drags_syncs_late_join_and_only_host_can_resume() {
    let mut pair = Pair::new();
    pair.ready();
    begin_gesture(&mut pair.client);
    pair.converge();
    pointer(&mut pair.client, Vec2::new(10.0, 12.0), true, false);
    send(&mut pair.host, PieceCommand::Grab(PieceId(2)));
    pair.converge();
    assert!(!pair
        .host
        .world()
        .resource::<PieceDataStore>()
        .held_by
        .is_empty());
    let positions: Vec<_> = pair
        .host
        .world()
        .resource::<PieceDataStore>()
        .states
        .iter()
        .map(|state| state.position)
        .collect();
    request(&mut pair.host, true);
    pair.converge();
    for app in [&pair.host, &pair.client] {
        assert_paused(app, true);
        let store = app.world().resource::<PieceDataStore>();
        assert!(store.held_by.is_empty());
        assert!(store.drag.members.is_empty());
        assert_eq!(
            store
                .states
                .iter()
                .map(|state| state.position)
                .collect::<Vec<_>>(),
            positions
        );
        assert!(!app.world().resource::<PieceInteraction>().is_dragging());
        assert!(app
            .world()
            .resource::<crate::selection::PuzzleSelection>()
            .latest
            .is_none());
    }
    request(&mut pair.client, false);
    send(&mut pair.client, PieceCommand::Grab(PieceId(0)));
    send(&mut pair.host, PieceCommand::Grab(PieceId(0)));
    pair.converge();
    assert_paused(&pair.host, true);
    assert_paused(&pair.client, true);
    assert!(pair
        .host
        .world()
        .resource::<PieceDataStore>()
        .held_by
        .is_empty());

    let mut late = app();
    join_with_transport(
        late.world_mut(),
        Fake {
            id: 2,
            bus: pair.bus.clone(),
        },
        JoinOptions {
            display_name: None,
            address: "127.0.0.1:10000".parse().unwrap(),
            password: password(),
            cached_image: None,
        },
    )
    .unwrap();
    for _ in 0..2000 {
        pair.frame();
        late.update();
        assert_ne!(
            late.world().resource::<NetworkStatus>().phase,
            RuntimePhase::Failed
        );
        if late.world().resource::<NetworkStatus>().phase == RuntimePhase::Ready {
            break;
        }
        std::thread::yield_now();
    }
    assert_eq!(
        late.world().resource::<NetworkStatus>().phase,
        RuntimePhase::Ready
    );
    assert_paused(&late, true);
    request(&mut pair.host, false);
    pair.converge();
    late.update();
    for app in [&pair.host, &pair.client, &late] {
        assert_paused(app, false);
    }
    begin_gesture(&mut pair.client);
    pair.converge();
    assert!(!pair
        .host
        .world()
        .resource::<PieceDataStore>()
        .held_by
        .is_empty());
    assert_eq!(
        pair.client.world().resource::<NetworkStatus>().phase,
        RuntimePhase::Ready
    );
}

#[test]
fn grab_in_flight_at_pause_is_rejected_without_poisoning_resume_sequence() {
    let mut pair = Pair::new();
    pair.ready();
    request(&mut pair.host, true);
    pair.host.update(); // Pause is queued for client; client has not seen it.
    begin_gesture(&mut pair.client);
    let commands: Vec<_> = {
        let messages = pair.client.world().resource::<Messages<ClientCommand>>();
        messages.get_cursor().read(messages).cloned().collect()
    };
    // Send the sampled intent without polling the queued pause first.
    pair.client.world_mut().run_schedule(PostUpdate);
    assert!(!commands.is_empty());
    pair.converge();
    assert_paused(&pair.client, true);
    assert_eq!(
        pair.client.world().resource::<NetworkStatus>().phase,
        RuntimePhase::Ready
    );
    assert!(pair
        .host
        .world()
        .resource::<PieceDataStore>()
        .held_by
        .is_empty());
    request(&mut pair.host, false);
    pair.converge();
    begin_gesture(&mut pair.client);
    pair.converge();
    assert!(!pair
        .host
        .world()
        .resource::<PieceDataStore>()
        .held_by
        .is_empty());
    assert_eq!(
        pair.client.world().resource::<NetworkStatus>().phase,
        RuntimePhase::Ready
    );
}

#[test]
fn pause_during_baseline_transfer_replays_catch_up_and_clears_drag_bases() {
    let mut pair = Pair::new();
    send(&mut pair.host, PieceCommand::Grab(PieceId(0)));
    pair.host.update();
    for _ in 0..2000 {
        pair.frame();
        if pair.client.world().resource::<NetworkStatus>().phase
            == RuntimePhase::Syncing(SyncPhase::BaselineTransfer)
        {
            break;
        }
        std::thread::yield_now();
    }
    assert_eq!(
        pair.client.world().resource::<NetworkStatus>().phase,
        RuntimePhase::Syncing(SyncPhase::BaselineTransfer)
    );
    request(&mut pair.host, true);
    pair.host.update();
    pair.ready();
    assert_paused(&pair.client, true);
    assert!(pair
        .client
        .world()
        .resource::<PieceDataStore>()
        .held_by
        .is_empty());
    assert_eq!(cursor(&pair.host), cursor(&pair.client));
    request(&mut pair.host, false);
    pair.converge();
    begin_gesture(&mut pair.client);
    pair.converge();
    assert!(!pair
        .host
        .world()
        .resource::<PieceDataStore>()
        .held_by
        .is_empty());
}

#[test]
fn pause_discards_in_flight_rotation_prediction_and_unsent_suffix() {
    let mut pair = Pair::new();
    pair.ready();
    pair.client
        .world_mut()
        .resource_mut::<PieceDataStore>()
        .selected_pieces = members();
    request(&mut pair.host, true);
    pair.host.update();
    let command = pair
        .client
        .world()
        .resource::<PieceInteraction>()
        .rotation_command(pair.client.world().resource::<PieceDataStore>(), 1)
        .unwrap();
    send(&mut pair.client, command.clone());
    send(&mut pair.client, command);
    pair.client.world_mut().run_schedule(PostUpdate);
    assert!(!pair
        .client
        .world()
        .resource::<PieceDataStore>()
        .local_rotation
        .poses
        .is_empty());
    pair.converge();
    assert_paused(&pair.client, true);
    let store = pair.client.world().resource::<PieceDataStore>();
    assert!(store.local_rotation.poses.is_empty());
    assert_eq!(
        store.states,
        pair.host.world().resource::<PieceDataStore>().states
    );
    assert_eq!(
        pair.client.world().resource::<NetworkStatus>().phase,
        RuntimePhase::Ready
    );
    request(&mut pair.host, false);
    pair.converge();
    let command = pair
        .client
        .world()
        .resource::<PieceInteraction>()
        .rotation_command(pair.client.world().resource::<PieceDataStore>(), 1)
        .unwrap();
    send(&mut pair.client, command);
    pair.converge();
    assert_eq!(
        jigsall_core::decode_rotation(
            pair.host.world().resource::<PieceDataStore>().states[0].flags
        ),
        1
    );
    assert_eq!(cursor(&pair.host), cursor(&pair.client));
}
