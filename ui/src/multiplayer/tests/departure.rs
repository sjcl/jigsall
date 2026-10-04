use super::*;
use crate::persistence::{DepartureAction, SaveDialogs};
use puzzella_core::{session::ImageHash, PuzzleDefinition, GENERATOR_VERSION};
use puzzella_game::persistence::{SaveError, StorageError};

fn paused_game(role: Option<RuntimeRole>, completed: bool) -> (App, egui::Context) {
    let (mut app, ctx) = scheduled_screens();
    let world = app.world_mut();
    world.resource_mut::<NetworkStatus>().role = role;
    world.resource_mut::<NetworkStatus>().phase = RuntimePhase::Ready;
    world.insert_resource(State::new(GameSubState::Paused));
    if completed {
        world.remove_resource::<State<GameSubState>>();
        world.insert_resource(State::new(AppState::GameComplete));
        world.insert_resource(State::new(GameCompleteSubState::Summary));
    }
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
    (app, ctx)
}

fn menu_label(action: DepartureAction) -> &'static str {
    match action {
        DepartureAction::Title => "Return to Title",
        DepartureAction::Exit => "Exit Game",
    }
}

fn save_label(action: DepartureAction) -> &'static str {
    match action {
        DepartureAction::Title => "Save and Return to Title",
        DepartureAction::Exit => "Save and Exit",
    }
}

fn discard_label(action: DepartureAction) -> &'static str {
    match action {
        DepartureAction::Title => "Return to Title Without Saving",
        DepartureAction::Exit => "Exit Without Saving",
    }
}

fn assert_staying(app: &App, role: Option<RuntimeRole>) {
    assert!(matches!(
        app.world().resource::<NextState<AppState>>(),
        NextState::Unchanged
    ));
    assert!(app.world().resource::<Messages<AppExit>>().is_empty());
    assert_eq!(app.world().resource::<NetworkStatus>().role, role);
    assert!(!app.world().resource::<LocalGameplayBlocked>().0);
}

fn assert_departed(app: &App, action: DepartureAction) {
    match action {
        DepartureAction::Title => {
            assert!(matches!(
                app.world().resource::<NextState<AppState>>(),
                NextState::Pending(AppState::Menu)
            ));
            assert!(app.world().resource::<Messages<AppExit>>().is_empty());
            assert_eq!(app.world().resource::<NetworkStatus>().role, None);
            assert!(app.world().resource::<LocalGameplayBlocked>().0);
        }
        DepartureAction::Exit => {
            assert_eq!(app.world().resource::<Messages<AppExit>>().len(), 1);
            assert!(matches!(
                app.world().resource::<NextState<AppState>>(),
                NextState::Unchanged
            ));
        }
    }
    assert!(!app.world().resource::<SaveDialogs>().departure_pending());
    assert!(!app.world().resource::<PersistenceState>().title_dialog_open);
}

#[test]
fn departure_without_saving_requires_confirmation_for_single_player_and_host() {
    for role in [None, Some(RuntimeRole::Host)] {
        for action in [DepartureAction::Title, DepartureAction::Exit] {
            let (mut app, ctx) = paused_game(role, false);
            click_label(&mut app, &ctx, menu_label(action));
            assert_staying(&app, role);
            assert!(app.world().resource::<PersistenceState>().title_dialog_open);
            assert!(!app.world().resource::<PersistenceState>().busy);
            let mut output = render_schedule(&mut app, &ctx, vec![]);
            output.textures_delta.clear();
            assert!(labels(&output).contains(&save_label(action)));
            assert!(labels(&output).contains(&discard_label(action)));
            output.drop_without_applying_deltas();

            app.world_mut().resource_mut::<SaveDialogs>().title = "Keep my title".into();
            click_label(&mut app, &ctx, discard_label(action));
            assert_staying(&app, role);
            // A new modal first performs an invisible sizing pass.
            for _ in 0..3 {
                render_schedule(&mut app, &ctx, vec![]).drop_without_applying_deltas();
            }
            let mut output = render_schedule(&mut app, &ctx, vec![]);
            output.textures_delta.clear();
            assert!(labels(&output).contains(&"Leave Without Saving?"));
            assert!(!labels(&output).contains(&save_label(action)));
            output.drop_without_applying_deltas();
            click_label(&mut app, &ctx, "Cancel");
            assert_staying(&app, role);
            assert_eq!(app.world().resource::<SaveDialogs>().title, "Keep my title");
            let mut output = render_schedule(&mut app, &ctx, vec![]);
            output.textures_delta.clear();
            assert!(labels(&output).contains(&save_label(action)));
            output.drop_without_applying_deltas();

            click_label(&mut app, &ctx, discard_label(action));
            click_label(&mut app, &ctx, discard_label(action));
            assert_departed(&app, action);
            render_schedule(&mut app, &ctx, vec![]).drop_without_applying_deltas();
            assert_departed(&app, action);
        }
    }
}

fn escape(app: &mut App, ctx: &egui::Context) {
    // Allow the newly opened modal to become egui's top modal before input.
    for _ in 0..3 {
        render_schedule(app, ctx, vec![]).drop_without_applying_deltas();
    }
    for pressed in [true, false] {
        render_schedule(
            app,
            ctx,
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: default(),
            }],
        )
        .drop_without_applying_deltas();
    }
}

#[test]
fn departure_cancel_and_escape_keep_the_game_and_clear_the_pending_action() {
    for action in [DepartureAction::Title, DepartureAction::Exit] {
        for use_escape in [false, true] {
            let (mut app, ctx) = paused_game(None, false);
            click_label(&mut app, &ctx, menu_label(action));
            if use_escape {
                click_label(&mut app, &ctx, discard_label(action));
                escape(&mut app, &ctx);
                assert!(app.world().resource::<PersistenceState>().title_dialog_open);
                assert_staying(&app, None);
                escape(&mut app, &ctx);
            } else {
                click_label(&mut app, &ctx, "Cancel");
            }
            assert_staying(&app, None);
            assert!(!app.world().resource::<PersistenceState>().title_dialog_open);
            assert!(!app.world().resource::<SaveDialogs>().departure_pending());

            // Ordinary saving after cancellation must not execute a stale departure.
            click_label(&mut app, &ctx, "Save Game");
            assert!(!app.world().resource::<SaveDialogs>().departure_pending());
            let mut state = app.world_mut().resource_mut::<PersistenceState>();
            state.title_dialog_open = false;
            state.message = Some(PersistenceNotice::Saved);
            render_schedule(&mut app, &ctx, vec![]).drop_without_applying_deltas();
            assert_staying(&app, None);
        }
    }
}

#[test]
fn departure_waits_for_manual_save_success_and_allows_retry_after_failure() {
    for role in [None, Some(RuntimeRole::Host)] {
        for action in [DepartureAction::Title, DepartureAction::Exit] {
            let (mut app, ctx) = paused_game(role, false);
            click_label(&mut app, &ctx, menu_label(action));
            click_label(&mut app, &ctx, save_label(action));
            assert!(app.world().resource::<PersistenceState>().busy);
            click_label(&mut app, &ctx, discard_label(action));
            click_label(&mut app, &ctx, "Cancel");
            escape(&mut app, &ctx);
            assert_staying(&app, role);
            assert!(app.world().resource::<PersistenceState>().title_dialog_open);
            let mut output = render_schedule(&mut app, &ctx, vec![]);
            output.textures_delta.clear();
            assert!(!labels(&output).contains(&"Leave Without Saving?"));
            output.drop_without_applying_deltas();

            // Feed the public outcomes produced by the independently tested worker.
            let mut state = app.world_mut().resource_mut::<PersistenceState>();
            state.busy = false;
            state.error = Some(PersistenceError::Save(SaveError::Storage(
                StorageError::LockTimeout,
            )));
            let mut output = render_schedule(&mut app, &ctx, vec![]);
            output.textures_delta.clear();
            assert!(labels(&output).contains(&"Timed out waiting for access to saved data. Close other Puzzella instances and try again."));
            output.drop_without_applying_deltas();
            assert_staying(&app, role);

            click_label(&mut app, &ctx, save_label(action));
            assert!(app.world().resource::<PersistenceState>().busy);
            assert!(app.world().resource::<PersistenceState>().error.is_none());
            let mut state = app.world_mut().resource_mut::<PersistenceState>();
            state.busy = false;
            state.title_dialog_open = false;
            state.message = Some(PersistenceNotice::Saved);
            render_schedule(&mut app, &ctx, vec![]).drop_without_applying_deltas();
            assert_departed(&app, action);
        }
    }
}

#[test]
fn departure_ignores_unrelated_save_notices_and_missing_original_allows_discard() {
    let (mut app, ctx) = paused_game(None, false);
    click_label(&mut app, &ctx, "Exit Game");
    app.world_mut().resource_mut::<PersistenceState>().message = Some(PersistenceNotice::Saved);
    render_schedule(&mut app, &ctx, vec![]).drop_without_applying_deltas();
    assert_staying(&app, None);
    app.world_mut().remove_resource::<OriginalPuzzleImage>();
    app.world_mut().resource_mut::<SaveDialogs>().title.clear();
    click_label(&mut app, &ctx, "Save and Exit");
    assert!(!app.world().resource::<PersistenceState>().busy);
    assert_staying(&app, None);
    click_label(&mut app, &ctx, "Exit Without Saving");
    click_label(&mut app, &ctx, "Exit Without Saving");
    assert_departed(&app, DepartureAction::Exit);
}

#[test]
fn completed_puzzle_departure_opens_the_save_ui_and_clients_leave_directly() {
    for role in [None, Some(RuntimeRole::Host)] {
        let (mut app, ctx) = paused_game(role, true);
        click_label(&mut app, &ctx, "Return to Title");
        assert_staying(&app, role);
        assert!(app.world().resource::<PersistenceState>().title_dialog_open);
        let mut output = render_schedule(&mut app, &ctx, vec![]);
        output.textures_delta.clear();
        assert!(!labels(&output).contains(&"Puzzle Complete"));
        assert!(labels(&output).contains(&"Save and Return to Title"));
        output.drop_without_applying_deltas();
        click_label(&mut app, &ctx, "Return to Title Without Saving");
        click_label(&mut app, &ctx, "Return to Title Without Saving");
        assert_departed(&app, DepartureAction::Title);
    }
    for completed in [false, true] {
        for action in [DepartureAction::Title, DepartureAction::Exit] {
            if completed && matches!(action, DepartureAction::Exit) {
                continue;
            }
            let (mut app, ctx) = paused_game(Some(RuntimeRole::Client), completed);
            click_label(&mut app, &ctx, menu_label(action));
            assert_departed(&app, action);
        }
    }
}

#[test]
fn departure_buttons_and_confirmation_fit_small_english_and_japanese_viewports() {
    for locale in [Locale::EN_US, Locale::JA] {
        for action in [DepartureAction::Title, DepartureAction::Exit] {
            let (mut app, ctx) = paused_game(None, false);
            app.world_mut()
                .resource_mut::<Localization>()
                .set_preference(LanguagePreference::Locale(locale));
            let i18n = app.world().resource::<Localization>();
            let menu = i18n.text(match action {
                DepartureAction::Title => "common-return-title",
                DepartureAction::Exit => "pause-exit",
            });
            let save = i18n.text(match action {
                DepartureAction::Title => "save-and-title",
                DepartureAction::Exit => "save-and-exit",
            });
            let discard = i18n.text(match action {
                DepartureAction::Title => "save-discard-title",
                DepartureAction::Exit => "save-discard-exit",
            });
            let cancel = i18n.text("common-cancel");
            let heading = i18n.text("save-discard-confirm-title");
            click_label(&mut app, &ctx, &menu);
            for confirming in [false, true] {
                for size in [egui::vec2(320.0, 360.0), egui::vec2(640.0, 360.0)] {
                    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
                    let render = |app: &mut App| {
                        ctx.run_ui(
                            egui::RawInput {
                                screen_rect: Some(screen),
                                ..default()
                            },
                            |_| {
                                app.world_mut()
                                    .run_schedule(bevy_egui::EguiPrimaryContextPass)
                            },
                        )
                    };
                    for outcome in 0..if confirming { 1 } else { 3 } {
                        let mut state = app.world_mut().resource_mut::<PersistenceState>();
                        state.busy = outcome == 1;
                        state.error = (outcome == 2).then_some(PersistenceError::Save(
                            SaveError::Storage(StorageError::LockTimeout),
                        ));
                        for _ in 0..3 {
                            render(&mut app).drop_without_applying_deltas();
                        }
                        let mut output = render(&mut app);
                        output.textures_delta.clear();
                        for label in if confirming {
                            [&heading, &discard, &cancel]
                        } else {
                            [&save, &discard, &cancel]
                        } {
                            let rect = output.shapes.iter().find_map(|shape| match &shape.shape {
                                egui::Shape::Text(text) if &text.galley.job.text == label => {
                                    let rect = text.galley.rect.translate(text.pos.to_vec2());
                                    assert!(
                                        screen.expand(1.0).contains_rect(rect),
                                        "{label}: {rect:?} outside {screen:?}"
                                    );
                                    assert!(
                                        shape.clip_rect.expand(1.0).contains_rect(rect),
                                        "{label}: clipped {rect:?}"
                                    );
                                    Some(rect)
                                }
                                _ => None,
                            });
                            assert!(rect.is_some(), "missing {label}: {:?}", labels(&output));
                        }
                        output.drop_without_applying_deltas();
                    }
                }
                let mut state = app.world_mut().resource_mut::<PersistenceState>();
                state.busy = false;
                state.error = None;
                if !confirming {
                    click_label(&mut app, &ctx, &discard);
                }
            }
        }
    }
}
