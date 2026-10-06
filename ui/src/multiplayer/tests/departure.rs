use super::*;
use crate::persistence::{DepartureAction, SaveDialogs};
use jigsall_core::{session::ImageHash, PuzzleDefinition, GENERATOR_VERSION};
use jigsall_game::persistence::{SaveError, StorageError};
mod window_close;

fn record_saved_game(app: &mut App, autosave: bool) {
    use jigsall_game::persistence::{SaveId, SaveMetadata, SaveTitle};
    let mut state = app.world_mut().resource_mut::<PersistenceState>();
    let metadata = SaveMetadata {
        id: SaveId(1),
        game_id: state.game_id,
        title: SaveTitle::new("Earlier checkpoint").unwrap(),
        revision: 1,
        created_at: 1,
        updated_at: 1,
        is_autosave: autosave,
    };
    if autosave {
        state.current_autosave = Some(metadata);
    } else {
        state.current_save = Some(metadata);
    }
}

#[test]
fn saved_puzzle_departure_skips_confirmation_only_after_completion() {
    for role in [None, Some(RuntimeRole::Host)] {
        for autosave in [false, true] {
            for completed in [false, true] {
                for action in [DepartureAction::Title, DepartureAction::Exit] {
                    let (mut app, ctx) = paused_game(role, completed);
                    record_saved_game(&mut app, autosave);
                    if completed {
                        app.world_mut()
                            .insert_resource(State::new(GameCompleteSubState::Paused));
                    }
                    click_label(&mut app, &ctx, menu_label(action, role));
                    if completed {
                        assert_departed(&app, action);
                    } else {
                        assert_staying(&app, role);
                        assert!(app.world().resource::<PersistenceState>().title_dialog_open);
                    }
                }
            }
            let (mut app, ctx) = paused_game(role, true);
            record_saved_game(&mut app, autosave);
            click_label(&mut app, &ctx, menu_label(DepartureAction::Title, role));
            assert_departed(&app, DepartureAction::Title);
        }
    }
}

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

fn menu_label(action: DepartureAction, role: Option<RuntimeRole>) -> &'static str {
    match (action, role) {
        (DepartureAction::Title, Some(RuntimeRole::Host)) => "Close Room and Return to Title",
        (DepartureAction::Exit, Some(RuntimeRole::Host)) => "Close Room and Exit",
        (DepartureAction::Title, Some(RuntimeRole::Client)) => "Leave Room and Return to Title",
        (DepartureAction::Exit, Some(RuntimeRole::Client)) => "Leave Room and Exit",
        (DepartureAction::Title, None) => "Return to Title",
        (DepartureAction::Exit, None) => "Exit Game",
    }
}

fn assert_host_warning(app: &App, output: &egui::FullOutput, role: Option<RuntimeRole>) {
    let warning = app
        .world()
        .resource::<Localization>()
        .text("save-host-departure-warning");
    assert_eq!(
        labels(output).contains(&warning.as_str()),
        role == Some(RuntimeRole::Host)
    );
}

#[test]
fn multiplayer_menu_explains_local_pause_and_distinguishes_host_and_client_departures() {
    for locale in [Locale::EN_US, Locale::JA] {
        for role in [None, Some(RuntimeRole::Host), Some(RuntimeRole::Client)] {
            for completed in [false, true] {
                let (mut app, ctx) = paused_game(role, completed);
                if completed {
                    app.world_mut()
                        .insert_resource(State::new(GameCompleteSubState::Paused));
                }
                app.world_mut()
                    .resource_mut::<Localization>()
                    .set_preference(LanguagePreference::Locale(locale));
                for _ in 0..3 {
                    render_schedule(&mut app, &ctx, vec![]).drop_without_applying_deltas();
                }
                let output = render_schedule(&mut app, &ctx, vec![]);
                let i18n = app.world().resource::<Localization>();
                let text = labels(&output);
                for key in [
                    if completed {
                        "pause-puzzle-menu"
                    } else if role.is_some() {
                        "pause-multiplayer-menu"
                    } else {
                        "pause-title"
                    },
                    if completed || role.is_some() {
                        "pause-back-puzzle"
                    } else {
                        "pause-resume"
                    },
                    DepartureAction::Title.menu_key(role),
                    DepartureAction::Exit.menu_key(role),
                    "common-save-game",
                    "menu-settings",
                ] {
                    assert!(text.contains(&i18n.text(key).as_str()), "missing {key}");
                }
                assert_eq!(
                    text.contains(&i18n.text("pause-multiplayer-hint").as_str()),
                    role.is_some() && !completed
                );
                if role.is_some() || completed {
                    assert!(!text.contains(&i18n.text("pause-title").as_str()));
                    assert!(!text.contains(&i18n.text("pause-resume").as_str()));
                }
                output.drop_without_applying_deltas();
            }
        }
    }
}

#[test]
fn pause_settings_close_and_escape_return_to_the_same_menu_without_resuming() {
    for role in [None, Some(RuntimeRole::Host), Some(RuntimeRole::Client)] {
        for completed in [false, true] {
            for escape in [false, true] {
                let (mut app, ctx) = paused_game(role, completed);
                if completed {
                    app.world_mut()
                        .insert_resource(State::new(GameCompleteSubState::Paused));
                }
                let epoch = app.world().resource::<PieceDataStore>().epoch;
                let pieces = app.world().resource::<PieceDataStore>().states.to_vec();
                let i18n = app.world().resource::<Localization>();
                let heading = i18n.text(if completed {
                    "pause-puzzle-menu"
                } else if role.is_some() {
                    "pause-multiplayer-menu"
                } else {
                    "pause-title"
                });
                let settings = i18n.text("settings-title");
                let close = i18n.text("common-close");
                click_label(&mut app, &ctx, "Settings");
                let output = render_schedule(&mut app, &ctx, vec![]);
                assert!(labels(&output).contains(&settings.as_str()));
                assert!(!labels(&output).contains(&heading.as_str()));
                assert!(
                    app.world()
                        .resource::<crate::settings::SettingsDialog>()
                        .open
                );
                assert!(!app
                    .world_mut()
                    .run_system_once(jigsall_game::resources::local_gameplay_enabled)
                    .unwrap());
                output.drop_without_applying_deltas();

                if escape {
                    render_schedule(
                        &mut app,
                        &ctx,
                        vec![egui::Event::Key {
                            key: egui::Key::Escape,
                            physical_key: None,
                            pressed: true,
                            repeat: false,
                            modifiers: default(),
                        }],
                    )
                    .drop_without_applying_deltas();
                } else {
                    click_label(&mut app, &ctx, &close);
                }
                assert!(
                    !app.world()
                        .resource::<crate::settings::SettingsDialog>()
                        .open
                );
                for _ in 0..2 {
                    render_schedule(&mut app, &ctx, vec![]).drop_without_applying_deltas();
                }
                let output = render_schedule(&mut app, &ctx, vec![]);
                assert!(labels(&output).contains(&heading.as_str()));
                assert!(!labels(&output).contains(&close.as_str()));
                output.drop_without_applying_deltas();
                assert!(app
                    .world_mut()
                    .run_system_once(jigsall_game::resources::local_gameplay_enabled)
                    .unwrap());
                assert!(matches!(
                    app.world().resource::<NextState<GameSubState>>(),
                    NextState::Unchanged
                ));
                assert!(matches!(
                    app.world().resource::<NextState<GameCompleteSubState>>(),
                    NextState::Unchanged
                ));
                assert!(matches!(
                    app.world().resource::<NextState<AppState>>(),
                    NextState::Unchanged
                ));
                assert_eq!(app.world().resource::<NetworkStatus>().role, role);
                let store = app.world().resource::<PieceDataStore>();
                assert_eq!(store.epoch, epoch);
                assert_eq!(&*store.states, pieces.as_slice());
            }
        }
    }
}

#[test]
fn host_departure_warning_is_localized_and_does_not_appear_for_ordinary_saving() {
    for locale in [Locale::EN_US, Locale::JA] {
        for action in [DepartureAction::Title, DepartureAction::Exit] {
            let role = Some(RuntimeRole::Host);
            let (mut app, ctx) = paused_game(role, false);
            app.world_mut()
                .resource_mut::<Localization>()
                .set_preference(LanguagePreference::Locale(locale));
            let i18n = app.world().resource::<Localization>();
            let save = i18n.text("common-save-game");
            let cancel = i18n.text("common-cancel");
            let menu = i18n.text(action.menu_key(role));
            let discard = i18n.text(match action {
                DepartureAction::Title => "save-discard-title",
                DepartureAction::Exit => "save-discard-exit",
            });
            click_label(&mut app, &ctx, &save);
            for _ in 0..3 {
                render_schedule(&mut app, &ctx, vec![]).drop_without_applying_deltas();
            }
            let output = render_schedule(&mut app, &ctx, vec![]);
            assert_host_warning(&app, &output, None);
            output.drop_without_applying_deltas();
            click_label(&mut app, &ctx, &cancel);

            click_label(&mut app, &ctx, &menu);
            for confirming in [false, true] {
                if confirming {
                    click_label(&mut app, &ctx, &discard);
                }
                for _ in 0..3 {
                    render_schedule(&mut app, &ctx, vec![]).drop_without_applying_deltas();
                }
                let output = render_schedule(&mut app, &ctx, vec![]);
                assert_host_warning(&app, &output, role);
                output.drop_without_applying_deltas();
                assert_staying(&app, role);
            }
        }
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
            click_label(&mut app, &ctx, menu_label(action, role));
            assert_staying(&app, role);
            assert!(app.world().resource::<PersistenceState>().title_dialog_open);
            assert!(!app.world().resource::<PersistenceState>().busy);
            let mut output = render_schedule(&mut app, &ctx, vec![]);
            output.textures_delta.clear();
            assert!(labels(&output).contains(&save_label(action)));
            assert!(labels(&output).contains(&discard_label(action)));
            assert_host_warning(&app, &output, role);
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
            assert_host_warning(&app, &output, role);
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
    for role in [None, Some(RuntimeRole::Host)] {
        for action in [DepartureAction::Title, DepartureAction::Exit] {
            for use_escape in [false, true] {
                let (mut app, ctx) = paused_game(role, false);
                click_label(&mut app, &ctx, menu_label(action, role));
                if use_escape {
                    click_label(&mut app, &ctx, discard_label(action));
                    escape(&mut app, &ctx);
                    assert!(app.world().resource::<PersistenceState>().title_dialog_open);
                    assert_staying(&app, role);
                    escape(&mut app, &ctx);
                } else {
                    click_label(&mut app, &ctx, "Cancel");
                }
                assert_staying(&app, role);
                assert!(!app.world().resource::<PersistenceState>().title_dialog_open);
                assert!(!app.world().resource::<SaveDialogs>().departure_pending());

                // Ordinary saving after cancellation must not execute a stale departure.
                click_label(&mut app, &ctx, "Save Game");
                assert!(!app.world().resource::<SaveDialogs>().departure_pending());
                let mut state = app.world_mut().resource_mut::<PersistenceState>();
                state.title_dialog_open = false;
                state.message = Some(PersistenceNotice::Saved);
                render_schedule(&mut app, &ctx, vec![]).drop_without_applying_deltas();
                assert_staying(&app, role);
            }
        }
    }
}

#[test]
fn departure_waits_for_manual_save_success_and_allows_retry_after_failure() {
    for role in [None, Some(RuntimeRole::Host)] {
        for action in [DepartureAction::Title, DepartureAction::Exit] {
            let (mut app, ctx) = paused_game(role, false);
            click_label(&mut app, &ctx, menu_label(action, role));
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
            assert!(labels(&output).contains(&"Timed out waiting for access to saved data. Close other Jigsall instances and try again."));
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
        click_label(&mut app, &ctx, menu_label(DepartureAction::Title, role));
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
            click_label(
                &mut app,
                &ctx,
                menu_label(action, Some(RuntimeRole::Client)),
            );
            assert_departed(&app, action);
        }
    }
}

#[test]
fn departure_buttons_and_confirmation_fit_small_english_and_japanese_viewports() {
    for (locale, role) in [
        (Locale::EN_US, None),
        (Locale::JA, None),
        (Locale::EN_US, Some(RuntimeRole::Host)),
        (Locale::JA, Some(RuntimeRole::Host)),
    ] {
        for action in [DepartureAction::Title, DepartureAction::Exit] {
            let (mut app, ctx) = paused_game(role, false);
            app.world_mut()
                .resource_mut::<Localization>()
                .set_preference(LanguagePreference::Locale(locale));
            let i18n = app.world().resource::<Localization>();
            let menu = i18n.text(action.menu_key(role));
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
