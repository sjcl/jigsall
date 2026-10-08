use super::*;

fn pair() -> Pair {
    let mut pair = Pair::with_host_setup(8192, None, encoded(), |host| {
        let def = PuzzleDefinition {
            grid_size: UVec2::new(8, 5),
            ..definition()
        };
        let positions = (0..40)
            .map(|id| {
                let offset = if id == 1 {
                    110.0
                } else {
                    100.0 + id as f32 * 4.0
                };
                def.correct_position(PieceId(id)) + Vec2::new(offset, 100.0)
            })
            .collect();
        host.world_mut()
            .resource_mut::<PieceDataStore>()
            .initialize(positions);
        host.world_mut().insert_resource(def);
    });
    pair.ready();
    pair.bus.lock().unwrap().hold_authority_control = true;
    pair
}

fn selection() -> PieceBitSet {
    let mut mask = PieceBitSet::new(40);
    mask.extend((0..33).map(PieceId));
    mask
}

fn snap_first(pair: &mut Pair) {
    send(&mut pair.host, PieceCommand::Grab(PieceId(1)));
    pair.host.update();
    send(
        &mut pair.host,
        PieceCommand::ReleaseGroup {
            members: PieceBitSet::new(40),
            delta: Vec2::new(-10.0, 0.0),
        },
    );
    pair.host.update();
    assert!(pair
        .host
        .world()
        .resource::<PieceDataStore>()
        .connectivity
        .same_component(PieceId(0), PieceId(1)));
}

fn deliver_all(pair: &mut Pair) {
    let mut bus = pair.bus.lock().unwrap();
    assert_eq!(
        bus.delayed.len(),
        3,
        "competing Grab, Release and empty result"
    );
    for (id, event) in std::mem::take(&mut bus.delayed) {
        bus.inbox.entry(id).or_default().push_back(event);
    }
    bus.hold_authority_control = false;
}

fn assert_ready_equal(pair: &Pair) {
    assert_eq!(
        pair.client.world().resource::<NetworkStatus>().phase,
        RuntimePhase::Ready
    );
    assert_eq!(
        pair.host.world().resource::<NetworkStatus>().phase,
        RuntimePhase::Hosting
    );
    assert!(pair.bus.lock().unwrap().closed.is_empty());
    assert_eq!(cursor(&pair.host), cursor(&pair.client));
    let host = pair.host.world().resource::<PieceDataStore>();
    let client = pair.client.world().resource::<PieceDataStore>();
    assert_eq!(host.states, client.states);
    assert_eq!(host.held_by, client.held_by);
    assert_eq!(host.next_z_order, client.next_z_order);
    for id in 0..40 {
        assert_eq!(
            host.connectivity.minimum_member(PieceId(id)),
            client.connectivity.minimum_member(PieceId(id))
        );
    }
}

#[test]
fn dense_stale_grab_empty_ack_cleans_drag_release_highlights_and_prediction() {
    for released in [false, true] {
        let mut pair = pair();
        snap_first(&mut pair);
        begin_selected_gesture(&mut pair.client, selection());
        pair.client.update(); // Send the Dense intent before observing the snap.
        pointer(&mut pair.client, Vec2::new(3.0, 5.0), true, false);
        let rotation = pair
            .client
            .world()
            .resource::<PieceInteraction>()
            .rotation_command(pair.client.world().resource::<PieceDataStore>(), 1)
            .unwrap();
        send(&mut pair.client, rotation);
        pair.client.update(); // Queued RotateDrag predicts while Grab awaits ACK.
        assert!(!pair
            .client
            .world()
            .resource::<PieceDataStore>()
            .local_rotation
            .poses
            .is_empty());
        if released {
            pointer(&mut pair.client, Vec2::new(7.0, 9.0), false, false);
            pair.client.update();
        }
        pair.host.update();
        let canonical = pair
            .host
            .world()
            .resource::<PieceDataStore>()
            .states
            .clone();
        deliver_all(&mut pair);
        pair.converge();
        assert_ready_equal(&pair);
        let store = pair.client.world().resource::<PieceDataStore>();
        assert_eq!(store.states, canonical);
        assert!(store.drag.members.is_empty());
        assert_eq!(store.drag.delta, Vec2::ZERO);
        assert!(store.selected_pieces.is_empty());
        assert!(store.highlights_dirty);
        assert!(store.local_rotation.poses.is_empty());
        assert!(!pair
            .client
            .world()
            .resource::<PieceInteraction>()
            .is_dragging());
        assert_eq!(
            cursor(&pair.host).sequence.0,
            3,
            "rejected gesture sent no queued controls"
        );
        // A new real pointer gesture sends Control(1), then rotates/releases.
        begin_selected_gesture(&mut pair.client, selection());
        pair.converge();
        assert!(pair
            .client
            .world()
            .resource::<PieceInteraction>()
            .is_dragging());
        let rotation = pair
            .client
            .world()
            .resource::<PieceInteraction>()
            .rotation_command(pair.client.world().resource::<PieceDataStore>(), 1)
            .unwrap();
        send(&mut pair.client, rotation);
        pair.converge();
        pointer(&mut pair.client, Vec2::ZERO, false, false);
        pair.converge();
        assert_ready_equal(&pair);
        assert!(pair
            .host
            .world()
            .resource::<PieceDataStore>()
            .held_by
            .is_empty());
    }
}

#[test]
fn dense_stale_rotate_empty_commit_rolls_back_prediction_and_next_control_succeeds() {
    let mut pair = pair();
    snap_first(&mut pair);
    pair.client
        .world_mut()
        .resource_mut::<PieceDataStore>()
        .selected_pieces = selection();
    let rotation = pair
        .client
        .world()
        .resource::<PieceInteraction>()
        .rotation_command(pair.client.world().resource::<PieceDataStore>(), -1)
        .unwrap();
    send(&mut pair.client, rotation);
    pair.client.update();
    assert_eq!(
        pair.client
            .world()
            .resource::<PieceDataStore>()
            .local_rotation
            .poses
            .len(),
        33
    );
    pair.host.update();
    let canonical = pair
        .host
        .world()
        .resource::<PieceDataStore>()
        .states
        .clone();
    deliver_all(&mut pair);
    pair.converge();
    assert_ready_equal(&pair);
    let store = pair.client.world().resource::<PieceDataStore>();
    assert_eq!(store.states, canonical);
    assert!(store.local_rotation.poses.is_empty());
    // Refresh the selection after the competing ownership notifications.
    pair.client
        .world_mut()
        .resource_mut::<PieceDataStore>()
        .selected_pieces = selection();
    let rotation = pair
        .client
        .world()
        .resource::<PieceInteraction>()
        .rotation_command(pair.client.world().resource::<PieceDataStore>(), 1)
        .unwrap();
    send(&mut pair.client, rotation);
    pair.converge();
    assert_ready_equal(&pair);
    assert_eq!(
        jigsall_core::decode_rotation(
            pair.client.world().resource::<PieceDataStore>().states[0].flags
        ),
        1
    );
}
