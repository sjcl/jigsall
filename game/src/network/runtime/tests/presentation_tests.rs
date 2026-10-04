use super::*;
use crate::resources::remote_drag::RemoteDragPresentation;

fn offset(app: &App, id: u32) -> Vec2 {
    app.world()
        .resource::<RemoteDragPresentation>()
        .offset(PieceId(id))
}
fn drag_delta(app: &mut App, delta: Vec2) {
    app.world_mut().resource_mut::<PieceDataStore>().drag.delta = delta;
}
fn fixed_frame_time(app: &mut App) {
    app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        Duration::from_secs_f64(1.0 / 60.0),
    ));
}

#[test]
fn remote_presentation_host_client_join_rotation_release_and_teardown() {
    let mut pair = Pair::new();
    fixed_frame_time(&mut pair.host);
    fixed_frame_time(&mut pair.client);
    pair.ready();
    begin_gesture(&mut pair.client);
    pair.converge();
    let canonical = pair
        .host
        .world()
        .resource::<PieceDataStore>()
        .states
        .clone();
    let delta = Vec2::new(17.0, -23.0);
    drag_delta(&mut pair.client, delta);
    pair.converge();
    assert_eq!(
        pair.host.world().resource::<PieceDataStore>().states,
        canonical
    );
    assert_eq!(offset(&pair.host, 0), delta);
    assert_eq!(offset(&pair.host, 1), delta);
    assert_eq!(offset(&pair.client, 0), Vec2::ZERO); // local path only

    // Already-active drag must be correct at Ready without a new Transient.
    let mut other = second_client(&mut pair);
    fixed_frame_time(&mut other);
    assert_eq!(offset(&other, 0), delta);
    assert_eq!(offset(&other, 1), delta);
    send(&mut other, PieceCommand::Grab(PieceId(2)));
    send(&mut pair.host, PieceCommand::Grab(PieceId(3)));
    for _ in 0..20 {
        pair.frame();
        other.update();
    }
    let canonical = pair
        .host
        .world()
        .resource::<PieceDataStore>()
        .states
        .clone();
    send(
        &mut other,
        PieceCommand::Move {
            id: PieceId(2),
            position: canonical[2].position + Vec2::new(-9.0, 12.0),
        },
    );
    send(
        &mut pair.host,
        PieceCommand::Move {
            id: PieceId(3),
            position: canonical[3].position + Vec2::new(22.0, 31.0),
        },
    );
    for _ in 0..20 {
        pair.frame();
        other.update();
    }
    assert_eq!(
        pair.host.world().resource::<PieceDataStore>().states,
        canonical
    );
    assert_eq!(offset(&pair.host, 0), delta);
    assert_eq!(offset(&pair.host, 2), Vec2::new(-9.0, 12.0));
    assert_eq!(offset(&pair.host, 3), Vec2::ZERO);
    assert_eq!(offset(&pair.client, 2), Vec2::new(-9.0, 12.0));
    assert_eq!(offset(&pair.client, 3), Vec2::new(22.0, 31.0));
    assert_eq!(offset(&other, 2), Vec2::ZERO);
    assert_eq!(offset(&other, 3), Vec2::new(22.0, 31.0));

    send(
        &mut pair.client,
        PieceCommand::RotateDrag {
            members: members(),
            delta,
            quarter_turns: 1,
        },
    );
    for _ in 0..20 {
        pair.frame();
        other.update();
    }
    assert_eq!(offset(&pair.host, 0), Vec2::ZERO);
    assert_eq!(offset(&other, 0), Vec2::ZERO);
    let rebased = Vec2::new(3.0, 5.0);
    drag_delta(&mut pair.client, rebased);
    for _ in 0..20 {
        pair.frame();
        other.update();
    }
    assert_eq!(offset(&pair.host, 0), rebased);
    assert_eq!(offset(&other, 0), rebased);
    send(
        &mut pair.client,
        PieceCommand::ReleaseGroup {
            members: members(),
            delta: rebased,
        },
    );
    for _ in 0..20 {
        pair.frame();
        other.update();
    }
    assert_eq!(offset(&pair.host, 0), Vec2::ZERO);
    assert_eq!(offset(&other, 0), Vec2::ZERO);
    assert_eq!(
        pair.host.world().resource::<PieceDataStore>().states,
        other.world().resource::<PieceDataStore>().states
    );

    // Stopping the second client publishes cancellation to host and first client.
    stop_session(other.world_mut());
    pair.converge();
    assert_eq!(offset(&pair.host, 2), Vec2::ZERO);
    assert_eq!(offset(&pair.client, 2), Vec2::ZERO);
    assert_eq!(offset(&other, 0), Vec2::ZERO);
    assert_eq!(offset(&other, 3), Vec2::ZERO);
    stop_session(pair.host.world_mut());
    assert_eq!(offset(&pair.host, 0), Vec2::ZERO);
    assert_eq!(offset(&pair.host, 2), Vec2::ZERO);
}

#[test]
fn remote_presentation_partial_acceptance_uses_only_authority_members() {
    let mut pair = Pair::new();
    fixed_frame_time(&mut pair.host);
    fixed_frame_time(&mut pair.client);
    pair.ready();
    begin_gesture(&mut pair.client);
    send(&mut pair.host, PieceCommand::Grab(PieceId(1)));
    pair.host.update();
    pair.converge();
    let delta = Vec2::new(33.0, 14.0);
    drag_delta(&mut pair.client, delta);
    pair.converge();
    assert_eq!(offset(&pair.host, 0), delta);
    assert_eq!(offset(&pair.host, 1), Vec2::ZERO);
    let other = second_client(&mut pair);
    assert_eq!(offset(&other, 0), delta);
    assert_eq!(offset(&other, 1), Vec2::ZERO);
}

#[test]
fn remote_smoothing_runtime_targets_advance_before_upload_and_reliable_boundaries_snap() {
    let mut pair = Pair::new();
    fixed_frame_time(&mut pair.host);
    fixed_frame_time(&mut pair.client);
    pair.ready();
    let mut other = second_client(&mut pair);
    fixed_frame_time(&mut other);
    begin_gesture(&mut pair.client);
    pair.converge();
    other.update();
    let canonical = pair
        .host
        .world()
        .resource::<PieceDataStore>()
        .states
        .clone();
    let authority_cursor = cursor(&pair.host);
    let player = pair.client.world().resource::<LocalPlayerId>().0;
    pair.host
        .world_mut()
        .resource_mut::<Time<Virtual>>()
        .pause();
    other.world_mut().resource_mut::<Time<Virtual>>().pause();
    let a = Vec2::new(30.0, -20.0);
    drag_delta(&mut pair.client, a);
    pair.client.update(); // Emit accepted target; remote display hasn't moved yet.
    assert_eq!(offset(&pair.host, 0), Vec2::ZERO);
    pair.host.update();
    other.update();
    for app in [&pair.host, &other] {
        let presentation = app.world().resource::<RemoteDragPresentation>();
        assert_eq!(presentation.target(PieceId(0)), a);
        let displayed = presentation.offset(PieceId(0));
        assert!(displayed.x > 0.0 && displayed.x < a.x);
        assert!(displayed.y < 0.0 && displayed.y > a.y);
        let upload = app.world().resource::<remote_drag::RemoteDragUpload>();
        assert_eq!(Vec2::from_array(upload.deltas[0]), displayed);
        assert_eq!(app.world().resource::<PieceDataStore>().states, canonical);
    }
    // Replica retains the accepted scalar, never the smoothed display.
    let network = other.world().get_non_send::<NetworkSession>().unwrap();
    assert_eq!(
        network
            .replica()
            .unwrap()
            .remote_drag(
                network.authority().unwrap(),
                other.world().resource::<PieceDataStore>(),
                player
            )
            .unwrap()
            .delta,
        a
    );
    let before = offset(&pair.host, 0);
    let b = Vec2::new(-20.0, 40.0);
    drag_delta(&mut pair.client, b);
    pair.client.update();
    pair.host.update();
    other.update();
    let displayed = offset(&pair.host, 0);
    assert!(displayed.x < before.x && displayed.x > b.x);
    assert!(displayed.y > before.y && displayed.y < b.y);
    assert_eq!(
        pair.host.world().resource::<PieceDataStore>().states,
        canonical
    );
    assert_eq!(cursor(&pair.host), authority_cursor);
    // Rotation while display still lags must instantly discard the old basis.
    send(
        &mut pair.client,
        PieceCommand::RotateDrag {
            members: members(),
            delta: b,
            quarter_turns: 1,
        },
    );
    pair.client.update();
    pair.host.update();
    other.update();
    for app in [&pair.host, &other] {
        let presentation = app.world().resource::<RemoteDragPresentation>();
        assert_eq!(presentation.offset(PieceId(0)), Vec2::ZERO);
        assert_eq!(presentation.target(PieceId(0)), Vec2::ZERO);
    }
    pair.client.update(); // Consume rotation ACK before moving in the new basis.
    drag_delta(&mut pair.client, a);
    pair.client.update();
    pair.host.update();
    other.update();
    assert_ne!(offset(&pair.host, 0), a);
    send(
        &mut pair.client,
        PieceCommand::ReleaseGroup {
            members: members(),
            delta: a,
        },
    );
    pair.client.update();
    pair.host.update();
    other.update();
    for app in [&pair.host, &other] {
        let presentation = app.world().resource::<RemoteDragPresentation>();
        assert_eq!(presentation.offset(PieceId(0)), Vec2::ZERO);
        assert_eq!(presentation.target(PieceId(0)), Vec2::ZERO);
    }
    pair.converge();
    // A new player's Grab reuses the freed slot at the new context value.
    send(&mut other, PieceCommand::Grab(PieceId(2)));
    other.update();
    pair.host.update();
    pair.client.update();
    assert_eq!(offset(&pair.host, 2), Vec2::ZERO);
    assert_eq!(
        pair.host
            .world()
            .resource::<RemoteDragPresentation>()
            .target(PieceId(2)),
        Vec2::ZERO
    );
    other.update();
    send(
        &mut other,
        PieceCommand::Move {
            id: PieceId(2),
            position: canonical[2].position + a,
        },
    );
    other.update();
    pair.host.update();
    pair.client.update();
    assert_ne!(offset(&pair.host, 2), Vec2::ZERO);
    assert_ne!(offset(&pair.host, 2), a);
    stop_session(other.world_mut()); // Cancel an actively smoothing slot.
    pair.frame();
    assert_eq!(offset(&pair.host, 2), Vec2::ZERO);
    assert_eq!(offset(&pair.client, 2), Vec2::ZERO);
}
