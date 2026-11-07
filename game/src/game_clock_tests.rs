use super::*;
use std::time::Duration;

#[test]
fn offline_clock_stops_for_menus_completion_initialization_and_modal_operations() {
    let mut app = App::new();
    app.insert_resource(Time::<Real>::default())
        .insert_resource(State::new(GameSubState::Playing))
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
    advance(&mut app, Duration::from_secs(2));
    for state in [GameSubState::Paused, GameSubState::Initializing] {
        app.insert_resource(State::new(state));
        advance(&mut app, Duration::from_secs(60));
        assert_eq!(
            app.world().resource::<PieceDataStore>().elapsed_play_time,
            Duration::from_secs(2)
        );
    }
    app.world_mut().resource_mut::<LocalGameplayBlocked>().0 = true;
    app.insert_resource(State::new(GameSubState::Playing));
    advance(&mut app, Duration::from_secs(60));
    assert_eq!(
        app.world().resource::<PieceDataStore>().elapsed_play_time,
        Duration::from_secs(2)
    );
    app.world_mut().resource_mut::<LocalGameplayBlocked>().0 = false;
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
