use super::*;
use std::time::Duration;

#[test]
fn elapsed_clock_counts_fractional_frames_and_stops_for_pause_completion_and_initialization() {
    let mut app = App::new();
    app.insert_resource(Time::<Real>::default())
        .insert_resource(State::new(GameSubState::Playing))
        .init_resource::<GameplayPause>()
        .init_resource::<LocalGameplayBlocked>()
        .init_resource::<PieceDataStore>()
        .add_systems(Update, tick_offline_game_clock);
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .initialize(vec![Vec2::ZERO]);
    app.update(); // Begin the first active interval with zero delta.
    let advance = |app: &mut App, duration| {
        app.world_mut()
            .resource_mut::<Time<Real>>()
            .advance_by(duration);
        app.update();
    };
    for _ in 0..125 {
        advance(&mut app, Duration::from_millis(16));
    }
    assert_eq!(
        app.world().resource::<PieceDataStore>().elapsed_play_time,
        Duration::from_secs(2)
    );
    app.world_mut().resource_mut::<GameplayPause>().paused = true;
    advance(&mut app, Duration::from_secs(3600));
    assert_eq!(
        app.world().resource::<PieceDataStore>().elapsed_play_time,
        Duration::from_secs(2)
    );
    app.world_mut().resource_mut::<GameplayPause>().paused = false;
    advance(&mut app, Duration::from_secs(3600));
    assert_eq!(
        app.world().resource::<PieceDataStore>().elapsed_play_time,
        Duration::from_secs(2)
    );
    for state in [GameSubState::Paused, GameSubState::Initializing] {
        app.insert_resource(State::new(state));
        advance(&mut app, Duration::from_secs(60));
        assert_eq!(
            app.world().resource::<PieceDataStore>().elapsed_play_time,
            Duration::from_secs(2)
        );
    }
    app.insert_resource(State::new(GameSubState::Playing));
    advance(&mut app, Duration::from_secs(3600));
    assert_eq!(
        app.world().resource::<PieceDataStore>().elapsed_play_time,
        Duration::from_secs(2)
    );
    advance(&mut app, Duration::from_secs(3));
    assert_eq!(
        app.world().resource::<PieceDataStore>().elapsed_play_time,
        Duration::from_secs(5)
    );
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .placed_count = 1;
    advance(&mut app, Duration::from_secs(60));
    assert_eq!(
        app.world().resource::<PieceDataStore>().elapsed_play_time,
        Duration::from_secs(5)
    );
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .initialize(vec![Vec2::ZERO]);
    assert_eq!(
        app.world().resource::<PieceDataStore>().elapsed_play_time,
        Duration::ZERO
    );
}
