use super::*;

fn elapsed(app: &App) -> Duration {
    app.world().resource::<PieceDataStore>().elapsed_play_time
}

#[test]
fn ticking_host_clock_does_not_restart_join_finalization_or_change_gameplay_cursors() {
    let mut pair = Pair::new();
    pair.host
        .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            Duration::from_secs(1),
        ));
    pair.host
        .world_mut()
        .insert_resource(State::new(AppState::InGame));
    pair.host
        .world_mut()
        .insert_resource(State::new(GameSubState::Playing));
    pair.ready();
    pair.frame();
    assert!(elapsed(&pair.host) > Duration::ZERO);
    assert_eq!(
        elapsed(&pair.client).as_secs(),
        elapsed(&pair.host).as_secs()
    );
    assert_eq!(cursor(&pair.host), AuthorityCursor::new(0, 0));
    assert_eq!(cursor(&pair.client), AuthorityCursor::new(0, 0));
}

#[test]
fn shared_clock_ignores_local_menus_freezes_at_host_pause_and_syncs_late_join() {
    let mut pair = Pair::new();
    pair.ready();
    pair.host
        .world_mut()
        .insert_resource(State::new(AppState::InGame));
    pair.host
        .world_mut()
        .insert_resource(State::new(GameSubState::Playing));
    pair.host
        .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            Duration::from_secs(1),
        ));
    pair.frame();
    pair.frame();
    pair.frame();
    assert!(elapsed(&pair.host) >= Duration::from_secs(2));
    assert_eq!(
        elapsed(&pair.host).as_secs(),
        elapsed(&pair.client).as_secs()
    );
    let before = elapsed(&pair.host);
    pair.host
        .world_mut()
        .insert_resource(State::new(GameSubState::Paused));
    pair.client
        .world_mut()
        .insert_resource(State::new(GameSubState::Paused));
    pair.frame();
    assert!(
        elapsed(&pair.host) > before,
        "local menus do not stop the shared game"
    );
    pair.host
        .world_mut()
        .resource_mut::<GameplayPause>()
        .request(true);
    pair.frame();
    let frozen = elapsed(&pair.host);
    let cursor_before = cursor(&pair.host);
    for _ in 0..5 {
        pair.frame();
    }
    assert_eq!(elapsed(&pair.host), frozen);
    assert_eq!(elapsed(&pair.client), frozen);
    assert_eq!(
        cursor(&pair.host),
        cursor_before,
        "clock does not consume gameplay cursors"
    );
    // The client's local time and local pause requests cannot alter authority time.
    pair.client
        .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            Duration::from_secs(3600),
        ));
    pair.client
        .world_mut()
        .resource_mut::<GameplayPause>()
        .request(false);
    pair.frame();
    assert_eq!(elapsed(&pair.client), frozen);
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
    assert_eq!(elapsed(&late), frozen);
    pair.host
        .world_mut()
        .resource_mut::<GameplayPause>()
        .request(false);
    pair.host
        .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            Duration::from_secs(3600),
        ));
    pair.frame();
    assert_eq!(
        elapsed(&pair.host),
        frozen,
        "resume must not charge the paused interval"
    );
    pair.host
        .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            Duration::from_secs(1),
        ));
    pair.frame();
    late.update();
    assert!(elapsed(&pair.host) > frozen);
    assert_eq!(
        elapsed(&pair.host).as_secs(),
        elapsed(&pair.client).as_secs()
    );
    pair.host
        .world_mut()
        .resource_mut::<PieceDataStore>()
        .placed_count = 4;
    let completed = elapsed(&pair.host);
    pair.frame();
    pair.frame();
    assert_eq!(elapsed(&pair.host), completed);
    assert_eq!(elapsed(&pair.client), completed);
}
