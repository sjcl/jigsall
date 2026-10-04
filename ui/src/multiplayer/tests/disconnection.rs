use super::*;
use jigsall_core::{session::ImageHash, PlayerId, PuzzleDefinition, GENERATOR_VERSION};
use jigsall_game::persistence::{SaveError, StorageError};

fn disconnected_game(completed: bool) -> (App, egui::Context) {
    let (mut app, ctx) = scheduled_screens();
    let world = app.world_mut();
    world.resource_mut::<MultiplayerUi>().screen = MenuScreen::Join;
    world.insert_resource(NetworkStatus {
        role: Some(RuntimeRole::Client),
        local_player: Some(PlayerId(1)),
        host: Some(PlayerId(0)),
        phase: RuntimePhase::Disconnected,
        failure: Some(NetworkFailureKind::ConnectionLost),
        ..default()
    });
    world.insert_resource(PuzzleDefinition {
        generator_version: GENERATOR_VERSION,
        seed: 1,
        grid_size: UVec2::splat(2),
        image_size: UVec2::splat(32),
        snap_distance: 1.0,
        rotation_enabled: false,
    });
    world.insert_resource(OriginalPuzzleImage {
        hash: ImageHash([0; 32]),
        encoded: Some(vec![1, 2, 3].into()),
        image_lease: None,
    });
    world
        .resource_mut::<PieceDataStore>()
        .initialize(vec![Vec2::splat(100.0); 4]);
    if completed {
        world.insert_resource(State::new(AppState::GameComplete));
        world.insert_resource(State::new(GameCompleteSubState::Summary));
    }
    (app, ctx)
}

#[test]
fn disconnected_save_modal_is_accessible_in_both_languages_without_gameplay_ui() {
    for locale in [Locale::EN_US, Locale::JA] {
        for completed in [false, true] {
            let (mut app, ctx) = disconnected_game(completed);
            app.world_mut()
                .resource_mut::<Localization>()
                .set_preference(LanguagePreference::Locale(locale));
            let save = app
                .world()
                .resource::<Localization>()
                .text("multiplayer-save-disconnected");
            click_label(&mut app, &ctx, &save);
            assert!(app.world().resource::<PersistenceState>().title_dialog_open);
            assert!(app.world().resource::<LocalGameplayBlocked>().0);
            let output = render_schedule(&mut app, &ctx, vec![]);
            let text = labels(&output);
            let i18n = app.world().resource::<Localization>();
            assert!(text.contains(&i18n.text("save-puzzle-title").as_str()));
            for key in ["common-return-title", "pause-exit", "completion-title"] {
                assert!(!text.contains(&i18n.text(key).as_str()));
            }
            output.drop_without_applying_deltas();
            let cancel = app.world().resource::<Localization>().text("common-cancel");
            click_label(&mut app, &ctx, &cancel);
            assert!(!app.world().resource::<PersistenceState>().title_dialog_open);
            assert!(app
                .world()
                .resource::<NetworkStatus>()
                .has_disconnected_game());
            assert_eq!(app.world().resource::<PieceDataStore>().len(), 4);
        }
    }
}

#[test]
fn disconnected_save_waits_allows_retry_and_shows_success_before_back() {
    let (mut app, ctx) = disconnected_game(false);
    click_label(&mut app, &ctx, "Save Last State");
    click_label(&mut app, &ctx, "Save Game");
    assert!(app.world().resource::<PersistenceState>().busy);
    click_label(&mut app, &ctx, "Back");
    click_label(&mut app, &ctx, "Cancel");
    assert!(matches!(
        app.world().resource::<NextState<AppState>>(),
        NextState::Unchanged
    ));
    assert!(app.world().resource::<PersistenceState>().title_dialog_open);
    let mut state = app.world_mut().resource_mut::<PersistenceState>();
    state.busy = false;
    state.error = Some(PersistenceError::Save(SaveError::Storage(
        StorageError::LockTimeout,
    )));
    let output = render_schedule(&mut app, &ctx, vec![]);
    assert!(labels(&output)
        .iter()
        .any(|text| text.contains("Timed out waiting for access")));
    output.drop_without_applying_deltas();
    click_label(&mut app, &ctx, "Save Game");
    assert!(app.world().resource::<PersistenceState>().busy);
    assert!(app.world().resource::<PersistenceState>().error.is_none());
    let mut state = app.world_mut().resource_mut::<PersistenceState>();
    state.busy = false;
    state.title_dialog_open = false;
    state.message = Some(PersistenceNotice::Saved);
    let output = render_schedule(&mut app, &ctx, vec![]);
    let saved = app
        .world()
        .resource::<Localization>()
        .persistence_notice(&PersistenceNotice::Saved);
    assert!(labels(&output).contains(&saved.as_str()));
    output.drop_without_applying_deltas();
    click_label(&mut app, &ctx, "Back");
    assert_eq!(app.world().resource::<NetworkStatus>().role, None);
    assert!(matches!(
        app.world().resource::<NextState<AppState>>(),
        NextState::Pending(AppState::Menu)
    ));
    assert!(app.world().resource::<MultiplayerUi>().screen == MenuScreen::Join);
}

#[test]
fn initial_join_failure_has_no_save_action_and_missing_image_disables_recovery_save() {
    let (mut app, ctx) = disconnected_game(false);
    app.world_mut().resource_mut::<NetworkStatus>().local_player = None;
    render_schedule(&mut app, &ctx, vec![]).drop_without_applying_deltas();
    let output = render_schedule(&mut app, &ctx, vec![]);
    assert!(!labels(&output).contains(&"Save Last State"));
    output.drop_without_applying_deltas();
    app.world_mut().resource_mut::<NetworkStatus>().local_player = Some(PlayerId(1));
    app.world_mut().remove_resource::<OriginalPuzzleImage>();
    click_label(&mut app, &ctx, "Save Last State");
    assert!(!app.world().resource::<PersistenceState>().title_dialog_open);
    app.world_mut()
        .insert_resource(State::new(AppState::GameSetup));
    let output = render_schedule(&mut app, &ctx, vec![]);
    assert!(!labels(&output).contains(&"Save Last State"));
    output.drop_without_applying_deltas();
}
