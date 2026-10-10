use super::*;
use crate::localization::{LanguagePreference, Locale};
use bevy::ecs::system::RunSystemOnce;
mod departure;
mod disconnection;
#[cfg(feature = "rendezvous")]
mod internet;
mod native;
mod ux;

fn screen_world() -> (World, Entity, egui::Context) {
    let mut world = World::new();
    let entity = world
        .spawn((
            bevy_egui::EguiContext::default(),
            bevy_egui::PrimaryEguiContext,
        ))
        .id();
    let ctx = world
        .get_mut::<bevy_egui::EguiContext>(entity)
        .unwrap()
        .get_mut()
        .clone();
    world.init_resource::<bevy_egui::EguiUserTextures>();
    world.insert_resource(crate::localization::tests::english());
    world.init_resource::<MultiplayerUi>();
    world.init_resource::<NetworkStatus>();
    world.init_resource::<NextState<AppState>>();
    world.init_resource::<crate::persistence::SaveDialogs>();
    world.init_resource::<crate::settings::SettingsDialog>();
    world.init_resource::<PersistenceState>();
    world.init_resource::<PieceDataStore>();
    world.insert_resource(PersistenceService::with_storage_requests().0);
    world.insert_resource(jigsall_game::settings::DisplaySettingsState::load(None));
    world.insert_resource(PlayerSettingsState::load(None));
    world.init_resource::<Messages<AppExit>>();
    world.init_resource::<PuzzleConfig>();
    world.init_resource::<crate::game_setup::image_picker::ImagePicker>();
    world.init_resource::<jigsall_game::asset_reader::ExternalFileRegistry>();
    (world, entity, ctx)
}

#[test]
fn title_menu_has_mode_then_game_choices_and_multiplayer_has_host_and_join() {
    let (mut world, _, ctx) = screen_world();
    for screen in [
        MenuScreen::Title,
        MenuScreen::SinglePlayer,
        MenuScreen::Multiplayer,
        MenuScreen::Host,
    ] {
        world.resource_mut::<MultiplayerUi>().navigate(screen);
        let mut render = || {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1280.0, 800.0),
                    )),
                    ..default()
                },
                |_| {
                    world.run_system_once(crate::menu::draw_menu_ui).unwrap();
                },
            )
        };
        render().drop_without_applying_deltas();
        let output = render();
        let labels: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text.galley.job.text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            labels.contains(&"Settings"),
            matches!(screen, MenuScreen::Title | MenuScreen::SinglePlayer)
        );
        match screen {
            MenuScreen::Title => {
                assert!(labels.contains(&"Single Player"));
                assert!(labels.contains(&"Multiplayer"));
                assert!(!labels.contains(&"New Puzzle"));
            }
            MenuScreen::SinglePlayer | MenuScreen::Host => {
                assert!(labels.contains(&"New Puzzle"));
                assert!(labels.contains(&"Continue"));
            }
            MenuScreen::Multiplayer => {
                assert!(labels.contains(&"Host a Game"));
                assert!(labels.contains(&"Join a Game"));
                assert!(!labels.contains(&"New Puzzle"));
            }
            _ => unreachable!(),
        }
        output.drop_without_applying_deltas();
    }
}

#[test]
fn actual_join_setup_draws_only_connection_status_and_no_image_or_piece_controls() {
    let (mut world, _, ctx) = screen_world();
    for phase in [
        RuntimePhase::Connecting,
        RuntimePhase::Authenticating,
        RuntimePhase::Syncing(SyncPhase::ImageTransfer),
        RuntimePhase::Syncing(SyncPhase::Finalizing),
    ] {
        world.insert_resource(NetworkStatus {
            role: Some(RuntimeRole::Client),
            phase,
            ..default()
        });
        let mut render = || {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(640.0, 500.0),
                    )),
                    ..default()
                },
                |_| {
                    world
                        .run_system_once(crate::game_setup::draw_game_setup_ui)
                        .unwrap();
                    world.run_system_once(draw_connection_ui).unwrap();
                },
            )
        };
        render().drop_without_applying_deltas();
        let output = render();
        let labels: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text.galley.job.text.as_str()),
                _ => None,
            })
            .collect();
        assert!(labels.contains(&"Cancel"));
        assert!(!labels.contains(&"New Puzzle"));
        assert!(!labels.contains(&"Select Image"));
        assert!(!labels
            .iter()
            .any(|text| text.contains("Finalizing") || text.contains("ImageTransfer")));
        output.drop_without_applying_deltas();
    }
}

fn scheduled_screens() -> (App, egui::Context) {
    let (mut world, _, ctx) = screen_world();
    world.insert_resource(State::new(AppState::InGame));
    world.insert_resource(State::new(GameSubState::Playing));
    world.init_resource::<NextState<GameSubState>>();
    world.init_resource::<NextState<GameCompleteSubState>>();
    world.init_resource::<LocalGameplayBlocked>();
    world.init_resource::<GameData>();
    world.init_resource::<GameUiPointerCapture>();
    world.init_resource::<crate::game_play::PlayersOverlayState>();
    world.init_resource::<PlayerRoster>();
    world.init_resource::<jigsall_game::resources::LocalPlayerId>();
    world.init_resource::<jigsall_game::resources::remote_cursor::RemoteCursorPresentation>();
    world.init_resource::<PieceDataStore>();
    world.init_resource::<PieceGenerationProgress>();
    world.init_resource::<PerformanceMonitor>();
    world.insert_resource(PuzzleImageLimits {
        device_max_dimension: 8192,
        gpu_memory_bytes: None,
    });
    world.init_resource::<crate::persistence::thumbnails::SaveThumbnails>();
    world.insert_resource(jigsall_game::image_settings::ImageSettingsState::load(None));
    world.insert_resource(jigsall_game::keybindings::KeyBindingsState::load(None));
    world.insert_resource(crate::preferences::UiPreferences::load(None));
    world.init_resource::<jigsall_game::settings::DisplayCapabilities>();
    world.init_resource::<Messages<jigsall_game::settings::DisplaySettingsAction>>();
    world.insert_resource(jigsall_game::persistence::autosave::AutosaveSettingsState::load(None));
    world.init_resource::<Messages<bevy::input::keyboard::KeyboardInput>>();
    world.init_resource::<ButtonInput<KeyCode>>();
    world
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::Tab);
    world.resource_mut::<PerformanceMonitor>().debug_level = PerformanceDebugLevel::Verbose;
    let mut app = App::new();
    world.init_resource::<bevy::ecs::schedule::Schedules>();
    *app.world_mut() = world;
    crate::register_screens(&mut app);
    (app, ctx)
}

fn render_schedule(
    app: &mut App,
    ctx: &egui::Context,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 800.0),
            )),
            events,
            ..default()
        },
        |_| {
            app.world_mut()
                .run_schedule(bevy_egui::EguiPrimaryContextPass)
        },
    )
}

fn labels(output: &egui::FullOutput) -> Vec<&str> {
    output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => Some(text.galley.job.text.as_str()),
            _ => None,
        })
        .collect()
}

fn click_label(app: &mut App, ctx: &egui::Context, label: &str) {
    render_schedule(app, ctx, vec![]).drop_without_applying_deltas();
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
        })
        .unwrap_or_else(|| {
            panic!(
                "missing {label}: {:?}; text bounds {:?}",
                labels(&output),
                output
                    .shapes
                    .iter()
                    .filter_map(|shape| match &shape.shape {
                        egui::Shape::Text(text) =>
                            Some((&text.galley.job.text, text.pos, shape.clip_rect)),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            )
        });
    output.drop_without_applying_deltas();
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
fn holding_tab_shows_players_without_capturing_gameplay_keyboard_input() {
    let (mut app, ctx) = scheduled_screens();
    app.world_mut()
        .init_resource::<bevy_egui::input::EguiWantsInput>();
    // Egui measures new windows in an invisible sizing pass.
    render_schedule(&mut app, &ctx, vec![]).drop_without_applying_deltas();
    render_schedule(&mut app, &ctx, vec![]).drop_without_applying_deltas();
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .reset_all();
    render_schedule(&mut app, &ctx, vec![]).drop_without_applying_deltas();

    for _ in 0..2 {
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Tab);
        for events in [
            vec![egui::Event::Key {
                key: egui::Key::Tab,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: default(),
            }],
            vec![],
            vec![egui::Event::Key {
                key: egui::Key::Tab,
                physical_key: None,
                pressed: true,
                repeat: true,
                modifiers: default(),
            }],
            vec![],
        ] {
            let output = render_schedule(&mut app, &ctx, events);
            let text: Vec<_> = labels(&output).into_iter().map(str::to_owned).collect();
            let players_visible = labels(&output).contains(&"Players");
            output.drop_without_applying_deltas();
            app.world_mut()
                .run_system_once(bevy_egui::input::write_egui_wants_input_system)
                .unwrap();
            assert!(ctx.memory(|memory| memory.focused().is_none()));
            assert!(!app
                .world()
                .resource::<bevy_egui::input::EguiWantsInput>()
                .wants_any_keyboard_input());
            assert!(players_visible, "{text:?}");
        }
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::Tab);
        let output = render_schedule(
            &mut app,
            &ctx,
            vec![egui::Event::Key {
                key: egui::Key::Tab,
                physical_key: None,
                pressed: false,
                repeat: false,
                modifiers: default(),
            }],
        );
        let players_visible = labels(&output).contains(&"Players");
        output.drop_without_applying_deltas();
        assert!(!players_visible);
    }
}

#[test]
fn hud_buttons_accept_clicks_and_pause_menu_keeps_tab_navigation() {
    let (mut app, ctx) = scheduled_screens();
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .reset_all();
    click_label(&mut app, &ctx, "How to Play");
    assert!(egui::Popup::is_any_open(&ctx));
    click_label(&mut app, &ctx, "How to Play");
    assert!(!egui::Popup::is_any_open(&ctx));
    assert!(ctx.memory(|memory| memory.focused().is_none()));

    click_label(&mut app, &ctx, "Menu · Esc");
    assert!(matches!(
        app.world().resource::<NextState<GameSubState>>(),
        NextState::Pending(GameSubState::Paused)
    ));
    app.world_mut()
        .insert_resource(State::new(GameSubState::Paused));
    render_schedule(&mut app, &ctx, vec![]).drop_without_applying_deltas();
    render_schedule(
        &mut app,
        &ctx,
        vec![egui::Event::Key {
            key: egui::Key::Tab,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: default(),
        }],
    )
    .drop_without_applying_deltas();
    assert!(ctx.memory(|memory| memory.focused().is_some()));
    assert!(ctx.egui_wants_keyboard_input());
}

#[test]
fn join_failure_back_keeps_address_clears_password_and_returns_to_join_form() {
    let (mut app, ctx) = scheduled_screens();
    let mut ui = app.world_mut().resource_mut::<MultiplayerUi>();
    ui.screen = MenuScreen::Join;
    ui.connecting = true;
    ui.join.address = "[2001:db8::1]:30123".into();
    *ui.join.password = "draft secret".into();
    app.world_mut().insert_resource(NetworkStatus {
        role: Some(RuntimeRole::Client),
        phase: RuntimePhase::Failed,
        failure: Some(NetworkFailureKind::Authentication),
        // Deliberately misleading diagnostic: the UI must use only the enum.
        error: Some("Timeout".into()),
        ..default()
    });
    render_schedule(&mut app, &ctx, vec![]).drop_without_applying_deltas();
    let output = render_schedule(&mut app, &ctx, vec![]);
    assert!(labels(&output).contains(
        &app.world()
            .resource::<Localization>()
            .text("multiplayer-error-wrong-password")
            .as_str()
    ));
    output.drop_without_applying_deltas();
    click_label(&mut app, &ctx, "Back");
    assert!(
        app.world().resource::<LocalGameplayBlocked>().0,
        "Menu transition must finish before input returns"
    );
    assert_eq!(app.world().resource::<NetworkStatus>().role, None);
    app.world_mut().run_system_once(reset_on_menu).unwrap();
    let ui = app.world().resource::<MultiplayerUi>();
    assert_eq!(ui.join.address, "[2001:db8::1]:30123");
    assert!(ui.join.password.is_empty());
    assert!(ui.screen == MenuScreen::Join);
    app.world_mut().insert_resource(State::new(AppState::Menu));
    app.world_mut()
        .insert_resource(NextState::<AppState>::Unchanged);
    render_schedule(&mut app, &ctx, vec![]).drop_without_applying_deltas();
    let output = render_schedule(&mut app, &ctx, vec![]);
    assert!(labels(&output).contains(&"[2001:db8::1]:30123"));
    output.drop_without_applying_deltas();
}

#[test]
fn host_failure_back_edits_port_without_reentering_setup_or_retaining_a_password_draft() {
    use jigsall_core::session::{SessionDefinition, SessionId};
    let (mut app, ctx) = scheduled_screens();
    let mut ui = app.world_mut().resource_mut::<MultiplayerUi>();
    ui.connecting = true;
    ui.error = Some(UiError::ConnectionFailed);
    ui.retry_host = Some(HostStartRequest::new(HostOptions {
        address: "0.0.0.0:43576".parse().unwrap(),
        password: SessionPassword::new("test password".into()).unwrap(),
        display_name: None,
        host: jigsall_core::PlayerId(0),
        session: SessionDefinition {
            id: SessionId(42),
            image_hash: jigsall_core::session::ImageHash([0; 32]),
        },
    }));
    click_label(&mut app, &ctx, "Back");
    assert!(app.world().resource::<MultiplayerUi>().editing_host_retry);
    app.world_mut().resource_mut::<MultiplayerUi>().host.address = "example.com:30123".into();
    click_label(&mut app, &ctx, "Open Room & Play");
    assert!(
        !app.world().resource::<MultiplayerUi>().retrying,
        "invalid address stays disabled"
    );
    app.world_mut().resource_mut::<MultiplayerUi>().host.address = "[::]:30123".into();
    click_label(&mut app, &ctx, "Open Room & Play");
    let ui = app.world().resource::<MultiplayerUi>();
    assert!(ui.retrying && ui.submitted);
    assert!(ui.pending_host.is_none());
    assert!(ui.host.password.is_empty());
    assert!(ui.retry_host.is_some());
    assert_eq!(
        *app.world().resource::<State<AppState>>().get(),
        AppState::InGame
    );
    assert!(app.world().resource::<LocalGameplayBlocked>().0);
    click_label(&mut app, &ctx, "Cancel");
    assert!(app.world().resource::<MultiplayerUi>().retry_host.is_none());
    assert!(matches!(
        app.world().resource::<NextState<AppState>>(),
        NextState::Pending(AppState::Menu)
    ));
}

#[test]
fn registered_screens_exclude_hud_roster_performance_pause_completion_and_save_during_connection() {
    let (mut app, ctx) = scheduled_screens();
    app.world_mut()
        .resource_mut::<PersistenceState>()
        .title_dialog_open = true;
    app.world_mut()
        .resource_mut::<PieceGenerationProgress>()
        .is_generating = true;
    for (state, sub, completion) in [
        (AppState::InGame, GameSubState::Initializing, None),
        (AppState::InGame, GameSubState::Playing, None),
        (AppState::InGame, GameSubState::Paused, None),
        (
            AppState::GameComplete,
            GameSubState::Playing,
            Some(GameCompleteSubState::Summary),
        ),
        (
            AppState::GameComplete,
            GameSubState::Playing,
            Some(GameCompleteSubState::Viewing),
        ),
        (
            AppState::GameComplete,
            GameSubState::Playing,
            Some(GameCompleteSubState::Paused),
        ),
    ] {
        app.world_mut().insert_resource(State::new(state));
        app.world_mut().insert_resource(State::new(sub));
        if let Some(completion) = completion {
            app.world_mut().insert_resource(State::new(completion));
        } else {
            app.world_mut()
                .remove_resource::<State<GameCompleteSubState>>();
        }
        for error in [None, Some(UiError::ConnectionFailed)] {
            let mut ui = app.world_mut().resource_mut::<MultiplayerUi>();
            ui.connecting = true;
            ui.error = error;
            ui.pending_host = error.is_none().then(|| {
                PendingHost::Direct(PendingDirectHost {
                    address: "127.0.0.1:43576".parse().unwrap(),
                    password: SessionPassword::new("test password".into()).unwrap(),
                })
            });
            render_schedule(&mut app, &ctx, vec![]).drop_without_applying_deltas();
            let output = render_schedule(&mut app, &ctx, vec![]);
            let i18n = app.world().resource::<Localization>();
            let text = labels(&output);
            let expected = i18n.text(
                error
                    .map(UiError::key)
                    .unwrap_or("multiplayer-preparing-host"),
            );
            assert!(
                text.contains(&expected.as_str()),
                "{state:?}/{sub:?}: {text:?}"
            );
            for key in [
                "game-menu",
                "game-controls",
                "game-players",
                "pause-title",
                "completion-title",
                "completion-puzzle",
                "common-save-game",
                "generation-state",
            ] {
                assert!(!text.contains(&i18n.text(key).as_str()), "{key}: {text:?}");
            }
            let progress = i18n.format("game-progress", &[("percent", "0.0".into())]);
            assert!(!text.contains(&progress.as_str()));
            assert!(!text.iter().any(|text| text.contains("FPS")));
            assert!(app.world().resource::<LocalGameplayBlocked>().0);
            output.drop_without_applying_deltas();
        }
    }
    // A Ready client clears the modal before the shared condition is evaluated,
    // so HUD and inputs return together in the same egui pass.
    app.world_mut()
        .insert_resource(State::new(AppState::InGame));
    app.world_mut()
        .insert_resource(State::new(GameSubState::Playing));
    app.world_mut()
        .remove_resource::<State<GameCompleteSubState>>();
    app.world_mut()
        .resource_mut::<PersistenceState>()
        .title_dialog_open = false;
    app.world_mut()
        .resource_mut::<PieceGenerationProgress>()
        .is_generating = false;
    app.world_mut().insert_resource(NetworkStatus {
        role: Some(RuntimeRole::Client),
        phase: RuntimePhase::Ready,
        ..default()
    });
    render_schedule(&mut app, &ctx, vec![]).drop_without_applying_deltas();
    let output = render_schedule(&mut app, &ctx, vec![]);
    let text = labels(&output);
    let i18n = app.world().resource::<Localization>();
    assert!(
        text.contains(
            &i18n
                .format("game-progress", &[("percent", "0.0".into())])
                .as_str()
        ),
        "{text:?}"
    );
    assert!(!app.world().resource::<LocalGameplayBlocked>().0);
    output.drop_without_applying_deltas();
}

#[test]
fn new_host_has_separate_settings_tabs_and_keeps_the_connection_draft_when_switching() {
    let (mut world, _, ctx) = screen_world();
    {
        let mut state = world.resource_mut::<MultiplayerUi>();
        state.host_setup = true;
        *state.host.password = "test password".into();
    }
    for network_tab in [true, false] {
        world.resource_mut::<MultiplayerUi>().host_settings_tab = network_tab;
        let mut render = || {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1280.0, 720.0),
                    )),
                    ..default()
                },
                |_| {
                    world
                        .run_system_once(crate::game_setup::draw_game_setup_ui)
                        .unwrap();
                },
            )
        };
        render().drop_without_applying_deltas();
        let output = render();
        let labels: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text.galley.job.text.as_str()),
                _ => None,
            })
            .collect();
        assert!(labels.contains(&"Multiplayer Puzzle"));
        assert!(labels.contains(&"Puzzle"));
        assert!(labels.contains(&"Room Settings"));
        assert_eq!(labels.contains(&"Accept connections at"), network_tab);
        assert_eq!(labels.contains(&"Save name"), network_tab);
        assert!(!labels.contains(&"Change Name"));
        assert!(!labels.contains(&"Settings"));
        assert_eq!(labels.contains(&"Select Image"), !network_tab);
        assert_eq!(
            world.resource::<MultiplayerUi>().host.password.as_str(),
            "test password"
        );
        output.drop_without_applying_deltas();
    }
}

#[test]
fn multiplayer_forms_save_names_inline_without_losing_passwords_or_other_settings() {
    for (screen, new_host, enter) in [
        (MenuScreen::Join, false, false),
        (MenuScreen::HostLoadSettings, false, true),
        (MenuScreen::Host, true, false),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{"player":{"display_name":"Before"},"preferences":{"language":"ja"}}"#,
        )
        .unwrap();
        let (mut app, ctx) = scheduled_screens();
        app.world_mut()
            .insert_resource(PlayerSettingsState::load(Some(path.clone())));
        app.world_mut().insert_resource(State::new(if new_host {
            AppState::GameSetup
        } else {
            AppState::Menu
        }));
        {
            let mut state = app.world_mut().resource_mut::<MultiplayerUi>();
            state.navigate(screen);
            state.host_setup = new_host;
            state.host_settings_tab = new_host;
            *state.host.password = "host password".into();
            *state.join.password = "join password".into();
        }
        render_schedule(&mut app, &ctx, vec![]).drop_without_applying_deltas();
        let output = render_schedule(&mut app, &ctx, vec![]);
        assert!(labels(&output).contains(&"Save name"));
        assert!(!labels(&output).contains(&"Change Name"));
        assert!(!labels(&output).contains(&"Settings"));
        output.drop_without_applying_deltas();

        click_label(&mut app, &ctx, "Before");
        let select_all = egui::Event::Key {
            key: egui::Key::A,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers {
                ctrl: true,
                command: true,
                ..default()
            },
        };
        render_schedule(
            &mut app,
            &ctx,
            vec![select_all, egui::Event::Paste("　Alice 🧩　".into())],
        )
        .drop_without_applying_deltas();
        let profile = app.world().resource::<PlayerSettingsState>();
        assert_eq!(
            profile.current.display_name.as_ref().unwrap().as_ref(),
            "Before"
        );
        assert!(!profile.is_save_pending());
        if enter {
            render_schedule(
                &mut app,
                &ctx,
                vec![egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: default(),
                }],
            )
            .drop_without_applying_deltas();
        } else {
            click_label(&mut app, &ctx, "Save name");
        }
        let profile = app.world().resource::<PlayerSettingsState>();
        assert_eq!(
            profile.current.display_name.as_ref().unwrap().as_ref(),
            "Alice 🧩"
        );
        let state = app.world().resource::<MultiplayerUi>();
        assert_eq!(state.host.password.as_str(), "host password");
        assert_eq!(state.join.password.as_str(), "join password");
        assert!(
            !app.world()
                .resource::<crate::settings::SettingsDialog>()
                .open
        );

        let mut profile = app.world_mut().resource_mut::<PlayerSettingsState>();
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        while profile.is_save_pending() {
            assert!(Instant::now() < deadline, "player name save timed out");
            profile.poll_save();
            std::thread::yield_now();
        }
        assert!(profile.error.is_none());
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["player"]["display_name"], "Alice 🧩");
        assert_eq!(saved["preferences"]["language"], "ja");
    }
}

#[test]
fn compact_host_setup_starts_only_from_room_settings_in_both_languages() {
    let (mut world, _, ctx) = screen_world();
    world.resource_mut::<MultiplayerUi>().host_setup = true;
    for locale in [Locale::EN_US, Locale::JA] {
        world
            .resource_mut::<Localization>()
            .set_preference(LanguagePreference::Locale(locale));
        for network_tab in [false, true] {
            world.resource_mut::<MultiplayerUi>().host_settings_tab = network_tab;
            let mut render = || {
                ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(640.0, 480.0),
                        )),
                        ..default()
                    },
                    |_| {
                        world
                            .run_system_once(crate::game_setup::draw_game_setup_ui)
                            .unwrap();
                    },
                )
            };
            render().drop_without_applying_deltas();
            let output = render();
            let text = labels(&output);
            let i18n = world.resource::<Localization>();
            assert!(text.contains(&i18n.text("common-back-title").as_str()));
            assert_eq!(
                text.contains(&i18n.text("multiplayer-start-host").as_str()),
                network_tab
            );
            assert_eq!(
                text.contains(&i18n.text("multiplayer-next-settings").as_str()),
                !network_tab
            );
            output.drop_without_applying_deltas();
        }
    }
}

#[cfg(feature = "gns")]
#[test]
fn starting_a_new_host_opens_puzzle_settings_even_after_a_previous_network_tab() {
    let (mut app, ctx) = scheduled_screens();
    app.world_mut().insert_resource(State::new(AppState::Menu));
    {
        let mut state = app.world_mut().resource_mut::<MultiplayerUi>();
        state.navigate(MenuScreen::Host);
        state.host_settings_tab = true;
    }
    click_label(&mut app, &ctx, "New Puzzle");
    let state = app.world().resource::<MultiplayerUi>();
    assert!(state.host_setup);
    assert!(!state.host_settings_tab);
    assert!(matches!(
        app.world().resource::<NextState<AppState>>(),
        NextState::Pending(AppState::GameSetup)
    ));
    app.world_mut()
        .insert_resource(State::new(AppState::GameSetup));
    render_schedule(&mut app, &ctx, vec![]).drop_without_applying_deltas();
    let output = render_schedule(&mut app, &ctx, vec![]);
    assert!(labels(&output).contains(&"Select Image"));
    assert!(!labels(&output).contains(&"Accept connections at"));
    assert!(!labels(&output).contains(&"Open Room & Play"));
    output.drop_without_applying_deltas();
    click_label(&mut app, &ctx, "Go to Room Settings");
    let state = app.world().resource::<MultiplayerUi>();
    assert!(state.host_settings_tab);
    assert!(!state.submitted);
    assert!(state.action.is_none());
    let output = render_schedule(&mut app, &ctx, vec![]);
    assert!(labels(&output).contains(&"Open Room & Play"));
    assert!(!labels(&output).contains(&"Select Image"));
    output.drop_without_applying_deltas();
}

#[test]
fn addresses_accept_dns_for_join_and_keep_bind_addresses_literal() {
    for address in [
        "192.168.1.10:43576",
        "[2001:db8::1]:43576",
        " [::1]:43576 ",
        "example.com:43576",
        "localhost:43576",
    ] {
        assert!(valid_address(address, false), "{address}");
    }
    for address in [
        "example.com",
        "2001:db8::1:43576",
        "127.0.0.1",
        "127.0.0.1:0",
        "0.0.0.0:43576",
        "[::]:43576",
    ] {
        assert!(!valid_address(address, false), "{address}");
    }
    assert!(valid_address("0.0.0.0:43576", true));
    assert!(valid_address("[::]:43576", true));
    assert!(!valid_address("example.com:43576", true));
    let mut draft = ConnectionDraft::new("example.com:43576");
    *draft.password = "test password".into();
    assert!(draft.valid(false));
    assert!(!draft.valid(true));
}

#[test]
fn join_moves_password_once_and_uses_the_committed_profile() {
    let mut profile = PlayerSettingsState::load(None);
    profile.commit("Alice");
    let mut state = MultiplayerUi::default();
    state.join.address = "127.0.0.1:43576".into();
    *state.join.password = "test password".into();
    state.submit_join(&profile);
    assert!(state.submitted);
    assert!(state.join.password.is_empty());
    let Some(Action::Join(options)) = state.action.take() else {
        panic!("missing join");
    };
    assert_eq!(options.display_name, profile.current.display_name);
    state.submit_join(&profile);
    assert!(
        state.action.is_none(),
        "a repeated click must not issue another start"
    );
}

fn submit_hostname(world: &mut World) {
    let profile = PlayerSettingsState::load(None);
    let mut state = world.resource_mut::<MultiplayerUi>();
    state.join.address = "localhost:43576".into();
    *state.join.password = "test password".into();
    state.submit_join(&profile);
    assert!(state.join.password.is_empty());
    let Some(Action::Join(request)) = state.action.take() else {
        panic!("hostname submission did not issue a join request");
    };
    assert_eq!(request.address.socket_addr(), None);
    // Feed a controlled queued reply through the real poll API; UI lifecycle
    // tests must not depend on the scheduling of OS resolver workers.
    state.pending_join = Some(PendingJoin {
        resolution: "127.0.0.1:43576"
            .parse::<ServerAddress>()
            .unwrap()
            .resolve()
            .unwrap(),
        password: request.password,
        display_name: request.display_name,
    });
    assert!(world.resource::<MultiplayerUi>().pending_join.is_some());
    assert!(!world.contains_non_send::<jigsall_game::network::runtime::NetworkSession>());
}

#[test]
fn hostname_join_cancel_or_navigation_discards_the_pending_request() {
    for cleanup in ["cancel", "navigate", "menu"] {
        let (mut world, _, _) = screen_world();
        submit_hostname(&mut world);
        match cleanup {
            "cancel" => world.resource_mut::<MultiplayerUi>().cancel(),
            "navigate" => world
                .resource_mut::<MultiplayerUi>()
                .navigate(MenuScreen::Multiplayer),
            _ => world.run_system_once(reset_on_menu).unwrap(),
        }
        process_actions(&mut world);
        process_actions(&mut world);
        let state = world.resource::<MultiplayerUi>();
        assert!(state.pending_join.is_none());
        assert!(state.join.password.is_empty());
        assert!(!state.connecting);
        assert!(state.error.is_none());
        assert!(!world.contains_non_send::<jigsall_game::network::runtime::NetworkSession>());
    }
}

#[test]
fn hostname_join_timeout_drops_credentials_and_shows_the_localized_error() {
    let (mut world, _, _) = screen_world();
    submit_hostname(&mut world);
    finish_address_resolution(
        &mut world,
        Instant::now() + std::time::Duration::from_secs(11),
    );
    let state = world.resource::<MultiplayerUi>();
    assert!(state.pending_join.is_none());
    assert_eq!(state.error, Some(UiError::Resolution));
    assert!(state.connection_screen(world.resource::<NetworkStatus>()));
    assert!(!world.contains_non_send::<jigsall_game::network::runtime::NetworkSession>());
    process_actions(&mut world);
    assert_eq!(
        world.resource::<MultiplayerUi>().error,
        Some(UiError::Resolution)
    );
}

#[test]
fn hostname_resolution_screen_shows_waiting_then_a_localized_failure() {
    let (mut world, _, ctx) = screen_world();
    // A previous connection's failure must not cover the new lookup spinner.
    world.insert_resource(NetworkStatus {
        phase: RuntimePhase::Disconnected,
        error: Some("BackendFailure".into()),
        ..default()
    });
    submit_hostname(&mut world);
    for failed in [false, true] {
        if failed {
            finish_address_resolution(
                &mut world,
                Instant::now() + std::time::Duration::from_secs(11),
            );
        }
        let mut render = || {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(640.0, 500.0),
                    )),
                    ..default()
                },
                |_| {
                    world.run_system_once(draw_connection_ui).unwrap();
                },
            )
        };
        render().drop_without_applying_deltas();
        let output = render();
        let labels: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text.galley.job.text.as_str()),
                _ => None,
            })
            .collect();
        let i18n = world.resource::<Localization>();
        let expected = i18n.text(if failed {
            "multiplayer-error-resolution"
        } else {
            "multiplayer-resolving"
        });
        assert!(labels.contains(&expected.as_str()));
        assert!(labels.contains(&if failed { "Back" } else { "Cancel" }));
        output.drop_without_applying_deltas();
    }
}

#[cfg(feature = "gns")]
#[test]
fn gns_localhost_hostname_join_ui_hands_the_resolved_address_to_the_runtime_once() {
    use jigsall_game::network::{gns::GnsDirectIp, transport::DirectIpTransport};
    let mut host = GnsDirectIp::new().unwrap();
    let listener = host.listen("127.0.0.1:0".parse().unwrap()).unwrap();
    let address = host.listener_address(listener).unwrap();
    let (mut world, _, _) = screen_world();
    world.insert_resource(PuzzleImageLimits {
        device_max_dimension: 8192,
        gpu_memory_bytes: None,
    });
    world.insert_resource(jigsall_game::image_settings::ImageSettingsState::load(None));
    let mut profile = PlayerSettingsState::load(None);
    profile.commit("Alice");
    {
        let mut state = world.resource_mut::<MultiplayerUi>();
        state.join.address = format!("localhost:{}", address.port());
        *state.join.password = "test password".into();
        state.submit_join(&profile);
    }
    process_actions(&mut world);
    assert!(world.resource::<MultiplayerUi>().pending_join.is_some());
    while world.resource::<MultiplayerUi>().pending_join.is_some() {
        process_actions(&mut world);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(world.resource::<MultiplayerUi>().error.is_none());
    assert_eq!(world.resource::<NetworkStatus>().address, Some(address));
    assert_eq!(
        world.resource::<NetworkStatus>().phase,
        RuntimePhase::Connecting
    );
    assert!(world.contains_non_send::<jigsall_game::network::runtime::NetworkSession>());
    process_actions(&mut world);
    assert_eq!(world.resource::<NetworkStatus>().address, Some(address));
    world.resource_mut::<MultiplayerUi>().cancel();
    process_actions(&mut world);
    assert!(!world.contains_non_send::<jigsall_game::network::runtime::NetworkSession>());
}

#[test]
fn leaving_connection_forms_wipes_both_password_drafts() {
    let mut state = MultiplayerUi::default();
    *state.host.password = "host password".into();
    *state.join.password = "join password".into();
    state.navigate(MenuScreen::Multiplayer);
    assert!(state.host.password.is_empty());
    assert!(state.join.password.is_empty());
}

#[test]
fn password_policy_matches_form_validity_and_errors_in_both_languages() {
    let mut i18n = crate::localization::tests::english();
    for locale in [Locale::EN_US, Locale::JA] {
        i18n.set_preference(LanguagePreference::Locale(locale));
        for host in [true, false] {
            for (value, valid) in [
                ("あ".repeat(3), false),
                ("か\u{3099}".repeat(7), false),
                ("か\u{3099}".repeat(8), true),
                ("あ".repeat(44), true),
                ("🧩".repeat(128), true),
                ("か\u{3099}".repeat(128), true),
                ("か\u{3099}".repeat(129), false),
                ("x".repeat(129), false),
            ] {
                let mut draft = ConnectionDraft::new("127.0.0.1:43576");
                *draft.password = value;
                assert_eq!(draft.valid(host), valid);
                let ctx = egui::Context::default();
                let output = ctx.run_ui(default(), |ui| {
                    paint_invalid_fields(ui, &draft, host, &i18n);
                });
                assert_eq!(
                    labels(&output).contains(&i18n.text("multiplayer-error-password").as_str()),
                    !valid
                );
                output.drop_without_applying_deltas();
                assert_eq!(draft.take_password().is_ok(), valid);
                assert!(draft.password.is_empty());
            }
        }
    }
}

#[test]
fn password_field_keeps_full_unicode_input_and_validates_after_normalization() {
    let i18n = crate::localization::tests::english();
    for host in [true, false] {
        for (value, valid) in [
            ("あ".repeat(3), false),
            ("あ".repeat(44), true),
            ("あ".repeat(128), true),
            ("か\u{3099}".repeat(128), true),
            ("あ".repeat(129), false),
        ] {
            let ctx = egui::Context::default();
            let mut profile = PlayerSettingsState::load(None);
            let mut draft = ConnectionDraft::new("127.0.0.1:43576");
            ctx.run_ui(default(), |ui| {
                paint_connection_fields(ui, &mut draft, host, &mut profile, &i18n);
            })
            .drop_without_applying_deltas();
            ctx.memory_mut(|memory| {
                memory.request_focus(egui::Id::new(if host {
                    "multiplayer-host-password"
                } else {
                    "multiplayer-join-password"
                }));
            });
            let output = ctx.run_ui(
                egui::RawInput {
                    events: vec![egui::Event::Paste(value.clone())],
                    ..default()
                },
                |ui| paint_connection_fields(ui, &mut draft, host, &mut profile, &i18n),
            );
            assert_eq!(draft.password.as_str(), value);
            assert_eq!(draft.valid(host), valid);
            assert_eq!(
                labels(&output).contains(&i18n.text("multiplayer-error-password").as_str()),
                !valid
            );
            output.drop_without_applying_deltas();
        }
    }
}

#[test]
fn password_cannot_be_restored_from_egui_undo_history_after_the_form_closes() {
    let ctx = egui::Context::default();
    let mut profile = PlayerSettingsState::load(None);
    let i18n = crate::localization::tests::english();
    let mut draft = ConnectionDraft::new("127.0.0.1:43576");
    let id = egui::Id::new("multiplayer-join-password");
    let mut paint = |time, events| {
        ctx.run_ui(
            egui::RawInput {
                time: Some(time),
                events,
                ..default()
            },
            |ui| paint_connection_fields(ui, &mut draft, false, &mut profile, &i18n),
        )
        .drop_without_applying_deltas();
    };
    paint(0.0, vec![]);
    ctx.memory_mut(|memory| memory.request_focus(id));
    paint(1.0, vec![egui::Event::Paste("secret-password".into())]);
    paint(3.0, vec![]);
    assert_eq!(draft.password.as_str(), "secret-password");
    draft.clear_password();
    ctx.run_ui(
        egui::RawInput {
            time: Some(4.0),
            events: vec![egui::Event::Key {
                key: egui::Key::Z,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers {
                    ctrl: true,
                    command: true,
                    ..default()
                },
            }],
            ..default()
        },
        |ui| paint_connection_fields(ui, &mut draft, false, &mut profile, &i18n),
    )
    .drop_without_applying_deltas();
    assert!(draft.password.is_empty());
}

#[test]
fn joining_suppresses_setup_through_every_sync_phase() {
    let state = MultiplayerUi::default();
    for phase in [
        RuntimePhase::Connecting,
        RuntimePhase::Authenticating,
        RuntimePhase::Syncing(SyncPhase::ImageNegotiation),
        RuntimePhase::Syncing(SyncPhase::ImageTransfer),
        RuntimePhase::Syncing(SyncPhase::BaselineTransfer),
        RuntimePhase::Syncing(SyncPhase::CatchingUp),
        RuntimePhase::Syncing(SyncPhase::Finalizing),
        RuntimePhase::Failed,
        RuntimePhase::Disconnected,
    ] {
        assert!(state.connection_screen(&NetworkStatus {
            role: Some(RuntimeRole::Client),
            phase,
            ..default()
        }));
    }
    assert!(!state.connection_screen(&NetworkStatus {
        role: Some(RuntimeRole::Client),
        phase: RuntimePhase::Ready,
        ..default()
    }));
}

#[test]
fn typed_network_failures_map_to_readable_error_categories() {
    for (kind, category) in [
        (NetworkFailureKind::Authentication, UiError::WrongPassword),
        (NetworkFailureKind::Timeout, UiError::Timeout),
        (NetworkFailureKind::Capacity, UiError::ServerFull),
        (NetworkFailureKind::Connection, UiError::ConnectionFailed),
        (NetworkFailureKind::ConnectionLost, UiError::ConnectionLost),
        (NetworkFailureKind::Protocol, UiError::ProtocolMismatch),
        (NetworkFailureKind::Image, UiError::ImageUnavailable),
    ] {
        assert_eq!(UiError::failure(kind), category);
    }
}

#[test]
fn connection_failure_and_loss_show_distinct_messages_in_both_languages() {
    let (mut app, ctx) = scheduled_screens();
    for locale in [Locale::EN_US, Locale::JA] {
        app.world_mut()
            .resource_mut::<Localization>()
            .set_preference(LanguagePreference::Locale(locale));
        for phase in [RuntimePhase::Failed, RuntimePhase::Disconnected] {
            for (kind, key, other_key) in [
                (
                    NetworkFailureKind::Connection,
                    "multiplayer-error-connection",
                    "multiplayer-error-connection-lost",
                ),
                (
                    NetworkFailureKind::ConnectionLost,
                    "multiplayer-error-connection-lost",
                    "multiplayer-error-connection",
                ),
            ] {
                // Menu teardown has already cleared the role and player identities.
                app.world_mut().insert_resource(NetworkStatus {
                    phase,
                    failure: Some(kind),
                    error: Some("transport diagnostic".into()),
                    ..default()
                });
                app.world_mut().resource_mut::<MultiplayerUi>().owns_session = true;
                app.world_mut().run_system_once(reset_on_menu).unwrap();
                render_schedule(&mut app, &ctx, vec![]).drop_without_applying_deltas();
                let output = render_schedule(&mut app, &ctx, vec![]);
                let i18n = app.world().resource::<Localization>();
                let text = labels(&output);
                assert!(text.contains(&i18n.text(key).as_str()));
                assert!(!text.contains(&i18n.text(other_key).as_str()));
                assert!(!text
                    .iter()
                    .any(|text| text.contains("transport diagnostic")));
                output.drop_without_applying_deltas();
            }
        }
    }
}

#[test]
fn cancel_uses_runtime_teardown_and_drops_pending_host() {
    let mut world = World::new();
    world.init_resource::<MultiplayerUi>();
    world.init_resource::<NetworkStatus>();
    world.init_resource::<NextState<AppState>>();
    {
        let mut state = world.resource_mut::<MultiplayerUi>();
        state.join.address = "[2001:db8::1]:30123".into();
        *state.join.password = "another secret".into();
        *state.host.password = "test password".into();
        state.submit_host();
    }
    let Some(Action::PrepareHost(host)) = world.resource_mut::<MultiplayerUi>().action.take()
    else {
        panic!("missing host");
    };
    world.resource_mut::<MultiplayerUi>().pending_host = Some(host);
    world.resource_mut::<MultiplayerUi>().cancel();
    process_actions(&mut world);
    assert!(world.resource::<MultiplayerUi>().pending_host.is_none());
    assert!(world.resource::<MultiplayerUi>().host.password.is_empty());
    assert!(world.resource::<MultiplayerUi>().join.password.is_empty());
    assert_eq!(
        world.resource::<MultiplayerUi>().join.address,
        "[2001:db8::1]:30123"
    );
    assert_eq!(
        world.resource::<NetworkStatus>().phase,
        RuntimePhase::Disconnected
    );
    assert!(matches!(
        world.resource::<NextState<AppState>>(),
        NextState::Pending(AppState::Menu)
    ));
}

#[test]
fn host_waits_for_generation_and_gpu_barrier_then_blocks_missing_encoded_image() {
    let mut world = World::new();
    world.init_resource::<MultiplayerUi>();
    world.init_resource::<NextState<AppState>>();
    world.insert_resource(State::new(AppState::InGame));
    world.init_resource::<PieceGenerationProgress>();
    world.init_resource::<PieceDataStore>();
    {
        let mut state = world.resource_mut::<MultiplayerUi>();
        *state.host.password = "test password".into();
        state.submit_host();
    }
    let Some(Action::PrepareHost(host)) = world.resource_mut::<MultiplayerUi>().action.take()
    else {
        panic!("missing host");
    };
    world.resource_mut::<MultiplayerUi>().pending_host = Some(host);
    start_prepared_host(&mut world);
    assert!(world.resource::<MultiplayerUi>().pending_host.is_some());
    world
        .resource_mut::<PieceGenerationProgress>()
        .generation_phase = GenerationPhase::Completed;
    start_prepared_host(&mut world);
    assert!(
        world.resource::<MultiplayerUi>().pending_host.is_some(),
        "no renderer readiness yet"
    );
    world.init_resource::<jigsall_game::render::RenderReady>();
    start_prepared_host(&mut world);
    assert!(world.resource::<MultiplayerUi>().pending_host.is_none());
    assert_eq!(
        world.resource::<MultiplayerUi>().error,
        Some(UiError::ImageUnavailable)
    );
}

#[test]
fn forms_show_name_editor_address_contract_and_secret_field_in_both_languages() {
    let mut i18n = crate::localization::tests::english();
    let mut profile = PlayerSettingsState::load(None);
    profile.commit("Alice");
    for locale in [Locale::EN_US, Locale::JA] {
        i18n.set_preference(LanguagePreference::Locale(locale));
        for host in [true, false] {
            let mut draft =
                ConnectionDraft::new(if host { "0.0.0.0:43576" } else { "[::1]:43576" });
            *draft.password = "secret-password".into();
            let ctx = egui::Context::default();
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(400.0, 800.0),
                    )),
                    ..default()
                },
                |ui| {
                    theme::prepare(ui.ctx());
                    ui.set_width(320.0);
                    paint_connection_fields(ui, &mut draft, host, &mut profile, &i18n);
                },
            );
            let labels: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) => Some(text.galley.job.text.as_str()),
                    _ => None,
                })
                .collect();
            assert!(labels.contains(&"Alice"));
            assert!(labels.contains(
                &i18n
                    .text(if host {
                        "multiplayer-your-player-name"
                    } else {
                        "settings-player-name"
                    })
                    .as_str()
            ));
            assert!(labels.contains(&i18n.text("settings-player-name-save").as_str()));
            assert!(labels.contains(
                &i18n
                    .text(if host {
                        "multiplayer-bind-hint"
                    } else {
                        "multiplayer-address-hint"
                    })
                    .as_str()
            ));
            assert!(!labels.contains(&"secret-password"));
            output.drop_without_applying_deltas();
        }
    }
}

#[test]
fn wildcard_listen_address_is_never_presented_as_an_invitation() {
    let ctx = egui::Context::default();
    let i18n = crate::localization::tests::english();
    let output = ctx.run_ui(default(), |ui| {
        let status = NetworkStatus {
            role: Some(RuntimeRole::Host),
            address: Some("0.0.0.0:43576".parse().unwrap()),
            ..default()
        };
        paint_host_status(ui, &status, &i18n);
    });
    let labels: Vec<_> = output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => Some(text.galley.job.text.as_str()),
            _ => None,
        })
        .collect();
    assert!(labels.contains(
        &i18n
            .format("multiplayer-invite-port", &[("port", 43576u32.into())])
            .as_str()
    ));
    assert!(!labels.iter().any(|text| text.contains("0.0.0.0")));
    assert!(!labels
        .iter()
        .any(|text| text.contains("Address to share with players: 0.0.0.0")));
    output.drop_without_applying_deltas();
}

#[cfg(feature = "gns")]
#[test]
fn prepared_host_starts_the_native_listener_once_with_the_committed_name() {
    use jigsall_core::PuzzleDefinition;
    let (mut world, _, _) = screen_world();
    let mut encoded = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(32, 32, image::Rgb([1, 2, 3])))
        .write_to(&mut encoded, image::ImageFormat::Png)
        .unwrap();
    let encoded = encoded.into_inner();
    world.insert_resource(OriginalPuzzleImage {
        hash: jigsall_game::persistence::image_hash(&encoded),
        encoded: Some(encoded.into()),
        image_lease: None,
    });
    let definition = PuzzleDefinition {
        generator_version: jigsall_core::GENERATOR_VERSION,
        seed: 12,
        grid_size: UVec2::splat(2),
        image_size: UVec2::splat(32),
        snap_distance: 1.0,
        rotation_enabled: true,
    };
    let mut store = PieceDataStore::default();
    store.initialize(
        (0..4)
            .map(|id| definition.correct_position(jigsall_core::PieceId(id)) + Vec2::splat(100.0))
            .collect(),
    );
    world.insert_resource(store);
    world.insert_resource(definition);
    world.insert_resource(State::new(AppState::InGame));
    world.init_resource::<jigsall_game::render::RenderReady>();
    world.init_resource::<LocalPlayerId>();
    world.insert_resource(PieceGenerationProgress {
        generation_phase: GenerationPhase::UploadingGpu,
        is_generating: true,
        ..default()
    });
    world.resource_mut::<PlayerSettingsState>().commit("Alice");
    world.resource_mut::<MultiplayerUi>().pending_host =
        Some(PendingHost::Direct(PendingDirectHost {
            address: "127.0.0.1:0".parse().unwrap(),
            password: SessionPassword::new("test password".into()).unwrap(),
        }));
    start_prepared_host(&mut world);
    assert_eq!(
        world.resource::<NetworkStatus>().phase,
        RuntimePhase::Offline
    );
    let mut progress = world.resource_mut::<PieceGenerationProgress>();
    progress.generation_phase = GenerationPhase::Completed;
    progress.is_generating = false;
    start_prepared_host(&mut world);
    let status = world.resource::<NetworkStatus>();
    assert_eq!(status.phase, RuntimePhase::Hosting);
    let address = status.address.unwrap();
    assert_ne!(address.port(), 0);
    assert!(world.contains_non_send::<jigsall_game::network::runtime::NetworkSession>());
    let player = world.resource::<LocalPlayerId>().0;
    assert_eq!(
        world
            .resource::<jigsall_game::players::PlayerRoster>()
            .get(player)
            .unwrap()
            .display_name,
        world.resource::<PlayerSettingsState>().current.display_name
    );
    start_prepared_host(&mut world);
    assert_eq!(world.resource::<NetworkStatus>().address, Some(address));
    world.resource_mut::<MultiplayerUi>().cancel();
    process_actions(&mut world);
    assert!(!world.contains_non_send::<jigsall_game::network::runtime::NetworkSession>());
}
