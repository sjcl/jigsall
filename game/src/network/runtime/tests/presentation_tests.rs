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

#[test]
fn remote_presentation_host_client_join_rotation_release_and_teardown() {
    let mut pair = Pair::new();
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
