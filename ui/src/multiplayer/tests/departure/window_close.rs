use super::*;
use bevy::{
    state::{app::StatesPlugin, state::StateTransition},
    window::{ClosingWindow, PrimaryWindow, WindowCloseRequested},
};

fn window_game(role: Option<RuntimeRole>, completed: bool) -> (App, egui::Context, Entity) {
    let (mut app, ctx) = paused_game(role, completed);
    // The screen fixture inserts read-only State resources. Initialize real
    // transition messages and schedules for the native-close lifecycle tests.
    app.world_mut().remove_resource::<State<AppState>>();
    app.world_mut().remove_resource::<State<GameSubState>>();
    app.world_mut()
        .remove_resource::<State<GameCompleteSubState>>();
    app.world_mut()
        .init_resource::<bevy::app::MainScheduleOrder>();
    app.add_plugins((
        StatesPlugin,
        WindowPlugin {
            primary_window: None,
            close_when_requested: false,
            ..default()
        },
    ))
    .init_state::<AppState>()
    .add_sub_state::<GameSubState>()
    .add_sub_state::<GameCompleteSubState>()
    .add_systems(PreUpdate, crate::window_close::handle_close_requests);
    let primary = app
        .world_mut()
        .spawn((Window::default(), PrimaryWindow))
        .id();
    app.world_mut()
        .resource_mut::<NextState<AppState>>()
        .set(if completed {
            AppState::GameComplete
        } else {
            AppState::InGame
        });
    app.world_mut().run_schedule(StateTransition);
    if completed {
        app.world_mut()
            .resource_mut::<NextState<GameCompleteSubState>>()
            .set(GameCompleteSubState::Viewing);
    } else {
        app.world_mut()
            .resource_mut::<NextState<GameSubState>>()
            .set(GameSubState::Playing);
    }
    app.world_mut().run_schedule(StateTransition);
    (app, ctx, primary)
}

fn close(app: &mut App, primary: Entity) {
    app.world_mut()
        .write_message(WindowCloseRequested { window: primary });
    app.world_mut().run_schedule(PreUpdate);
    app.world_mut().run_schedule(StateTransition);
    app.world_mut().run_schedule(Last);
}

fn assert_window_open(app: &App, primary: Entity, role: Option<RuntimeRole>) {
    assert!(app.world().get::<Window>(primary).is_some());
    assert!(app.world().get::<ClosingWindow>(primary).is_none());
    assert_staying(app, role);
}

fn click_save_button(app: &mut App, ctx: &egui::Context, label: &str) {
    // The ordinary save modal has the same label in its heading and button.
    for _ in 0..3 {
        render_schedule(app, ctx, vec![]).drop_without_applying_deltas();
    }
    let output = render_schedule(app, ctx, vec![]);
    let point = output
        .shapes
        .iter()
        .rev()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.job.text == label => {
                Some(text.pos + text.galley.size() * 0.5)
            }
            _ => None,
        });
    output.drop_without_applying_deltas();
    let point = point.unwrap();
    for pressed in [true, false] {
        render_schedule(
            app,
            ctx,
            vec![
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: default(),
                },
            ],
        )
        .drop_without_applying_deltas();
    }
}

#[test]
fn window_close_pauses_before_showing_save_and_keeps_the_window_until_confirmed() {
    for role in [None, Some(RuntimeRole::Host)] {
        for completed in [false, true] {
            let (mut app, ctx, primary) = window_game(role, completed);
            close(&mut app, primary);
            assert_window_open(&app, primary, role);
            assert!(app.world().resource::<PersistenceState>().title_dialog_open);
            if completed {
                assert_eq!(
                    *app.world().resource::<State<GameCompleteSubState>>().get(),
                    GameCompleteSubState::Paused
                );
            } else {
                assert_eq!(
                    *app.world().resource::<State<GameSubState>>().get(),
                    GameSubState::Paused
                );
            }
            // Repeated OS requests cannot close the window or erase the title draft.
            app.world_mut().resource_mut::<SaveDialogs>().title = "Window puzzle".into();
            close(&mut app, primary);
            assert_eq!(app.world().resource::<SaveDialogs>().title, "Window puzzle");
            for _ in 0..3 {
                render_schedule(&mut app, &ctx, vec![]).drop_without_applying_deltas();
            }
            let output = render_schedule(&mut app, &ctx, vec![]);
            assert_host_warning(&app, &output, role);
            output.drop_without_applying_deltas();
            click_label(&mut app, &ctx, "Exit Without Saving");
            close(&mut app, primary);
            assert_window_open(&app, primary, role);
            for _ in 0..3 {
                render_schedule(&mut app, &ctx, vec![]).drop_without_applying_deltas();
            }
            let mut output = render_schedule(&mut app, &ctx, vec![]);
            output.textures_delta.clear();
            assert!(labels(&output).contains(&"Leave Without Saving?"));
            assert_host_warning(&app, &output, role);
            output.drop_without_applying_deltas();
            click_label(&mut app, &ctx, "Cancel");
            click_label(&mut app, &ctx, "Cancel");
            assert_window_open(&app, primary, role);
            assert!(!app.world().resource::<PersistenceState>().title_dialog_open);

            close(&mut app, primary);
            click_label(&mut app, &ctx, "Exit Without Saving");
            click_label(&mut app, &ctx, "Exit Without Saving");
            assert_departed(&app, DepartureAction::Exit);
        }
    }
}

#[test]
fn window_close_keeps_pending_manual_save_and_exits_only_after_success() {
    for (original_action, save_button) in [
        ("Save Game", "Save Game"),
        ("Close Room and Return to Title", "Save and Return to Title"),
        ("Close Room and Exit", "Save and Exit"),
    ] {
        let (mut app, ctx, primary) = window_game(Some(RuntimeRole::Host), false);
        app.world_mut()
            .resource_mut::<NextState<GameSubState>>()
            .set(GameSubState::Paused);
        app.world_mut().run_schedule(StateTransition);
        click_label(&mut app, &ctx, original_action);
        app.world_mut().resource_mut::<SaveDialogs>().title = "Pending title".into();
        click_save_button(&mut app, &ctx, save_button);
        assert!(app.world().resource::<PersistenceState>().busy);
        close(&mut app, primary);
        close(&mut app, primary);
        assert_window_open(&app, primary, Some(RuntimeRole::Host));
        assert_eq!(app.world().resource::<SaveDialogs>().title, "Pending title");
        assert!(app.world().resource::<PersistenceState>().busy);

        let mut state = app.world_mut().resource_mut::<PersistenceState>();
        state.busy = false;
        state.error = Some(PersistenceError::WorkerStopped);
        render_schedule(&mut app, &ctx, vec![]).drop_without_applying_deltas();
        assert_window_open(&app, primary, Some(RuntimeRole::Host));
        click_label(&mut app, &ctx, "Save and Exit");
        let mut state = app.world_mut().resource_mut::<PersistenceState>();
        state.busy = false;
        state.title_dialog_open = false;
        state.message = Some(PersistenceNotice::Saved);
        render_schedule(&mut app, &ctx, vec![]).drop_without_applying_deltas();
        assert_departed(&app, DepartureAction::Exit);
    }
}

#[test]
fn window_close_during_autosave_waits_then_still_requires_a_choice() {
    let (mut app, ctx, primary) = window_game(None, false);
    let mut state = app.world_mut().resource_mut::<PersistenceState>();
    state.busy = true;
    state.autosaving = true;
    close(&mut app, primary);
    let mut state = app.world_mut().resource_mut::<PersistenceState>();
    state.busy = false;
    state.autosaving = false;
    render_schedule(&mut app, &ctx, vec![]).drop_without_applying_deltas();
    assert_window_open(&app, primary, None);
    assert!(app.world().resource::<PersistenceState>().title_dialog_open);
    click_label(&mut app, &ctx, "Cancel");
    assert_window_open(&app, primary, None);
}

#[test]
fn window_close_exits_directly_outside_play_and_for_clients_and_ignores_stale_requests() {
    for scenario in [
        "title",
        "setup",
        "initializing",
        "client",
        "missing definition",
    ] {
        let role = (scenario == "client").then_some(RuntimeRole::Client);
        let (mut app, _, primary) = window_game(role, false);
        match scenario {
            "title" => app
                .world_mut()
                .resource_mut::<NextState<AppState>>()
                .set(AppState::Menu),
            "setup" => app
                .world_mut()
                .resource_mut::<NextState<AppState>>()
                .set(AppState::GameSetup),
            "initializing" => app
                .world_mut()
                .resource_mut::<NextState<GameSubState>>()
                .set(GameSubState::Initializing),
            "missing definition" => {
                app.world_mut().remove_resource::<PuzzleDefinition>();
            }
            _ => {}
        }
        app.world_mut().run_schedule(StateTransition);
        close(&mut app, primary);
        assert_eq!(app.world().resource::<Messages<AppExit>>().len(), 1);
        assert!(!app.world().resource::<PersistenceState>().title_dialog_open);
        assert!(!app.world().resource::<SaveDialogs>().departure_pending());
    }
    let (mut app, _, primary) = window_game(None, false);
    let stale = app.world_mut().spawn(Window::default()).id();
    app.world_mut().despawn(stale);
    close(&mut app, stale);
    assert_window_open(&app, primary, None);
    assert!(!app.world().resource::<PersistenceState>().title_dialog_open);
}

#[test]
fn closing_a_secondary_window_leaves_the_puzzle_and_primary_window_running() {
    let (mut app, _, primary) = window_game(None, false);
    let secondary = app.world_mut().spawn(Window::default()).id();
    close(&mut app, secondary);
    assert!(app.world().get::<ClosingWindow>(secondary).is_some());
    assert_window_open(&app, primary, None);
    assert!(!app.world().resource::<PersistenceState>().title_dialog_open);
    app.world_mut().run_schedule(PreUpdate);
    app.world_mut().run_schedule(Last);
    assert!(app.world().get::<Window>(secondary).is_none());
    assert_window_open(&app, primary, None);
}

#[test]
fn window_close_save_dialog_is_accessible_while_retrying_a_prepared_host() {
    let (mut app, ctx, primary) = window_game(None, false);
    app.world_mut().resource_mut::<MultiplayerUi>().connecting = true;
    app.world_mut().resource_mut::<NetworkStatus>().phase = RuntimePhase::Failed;
    close(&mut app, primary);
    click_label(&mut app, &ctx, "Exit Without Saving");
    assert!(app.world().get::<Window>(primary).is_some());
    assert!(app.world().get::<ClosingWindow>(primary).is_none());
    assert!(app.world().resource::<Messages<AppExit>>().is_empty());
    assert!(app.world().resource::<LocalGameplayBlocked>().0);
    click_label(&mut app, &ctx, "Exit Without Saving");
    assert_departed(&app, DepartureAction::Exit);
}

#[test]
fn window_close_during_disconnected_save_waits_for_success_before_exit() {
    for game_state in [GameSubState::Playing, GameSubState::Initializing] {
        let (mut app, ctx, primary) = window_game(Some(RuntimeRole::Client), false);
        app.world_mut()
            .resource_mut::<NextState<GameSubState>>()
            .set(game_state);
        app.world_mut().run_schedule(StateTransition);
        {
            let mut status = app.world_mut().resource_mut::<NetworkStatus>();
            status.local_player = Some(jigsall_core::PlayerId(1));
            status.phase = RuntimePhase::Disconnected;
            status.failure = Some(NetworkFailureKind::ConnectionLost);
        }
        click_label(&mut app, &ctx, "Save Last State");
        click_save_button(&mut app, &ctx, "Save Game");
        assert!(app.world().resource::<PersistenceState>().busy);
        close(&mut app, primary);
        close(&mut app, primary);
        render_schedule(&mut app, &ctx, vec![]).drop_without_applying_deltas();
        assert!(app.world().resource::<Messages<AppExit>>().is_empty());
        assert!(app.world().get::<Window>(primary).is_some());
        assert!(app.world().resource::<PersistenceState>().title_dialog_open);
        let mut state = app.world_mut().resource_mut::<PersistenceState>();
        state.busy = false;
        state.title_dialog_open = false;
        state.message = Some(PersistenceNotice::Saved);
        render_schedule(&mut app, &ctx, vec![]).drop_without_applying_deltas();
        assert_departed(&app, DepartureAction::Exit);
    }
}
