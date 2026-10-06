use super::*;
use jigsall_game::network::{
    gns::{rendezvous::EndpointUrl, IceConfig},
    runtime::{RendezvousRuntimeConfig, RoomCode},
};
fn configure(world: &mut World) {
    world.insert_resource(RendezvousRuntimeConfig {
        endpoint: EndpointUrl::loopback_for_test("ws://127.0.0.1:9/v1/ws").unwrap(),
        ice: IceConfig::default(),
    });
}
fn internet(state: &mut MultiplayerUi) {
    state.host.method = RuntimeConnectionMethod::Internet;
    state.join.method = RuntimeConnectionMethod::Internet;
    state.internet_available = true;
}

#[test]
fn internet_host_failure_returns_to_settings_and_retries_generated_or_loaded_puzzle() {
    for app_state in [AppState::InGame, AppState::GameComplete] {
        let (mut app, ctx) = scheduled_screens();
        configure(app.world_mut());
        app.world_mut().insert_resource(State::new(app_state));
        app.world_mut()
            .insert_resource(State::new(GameCompleteSubState::Summary));
        app.world_mut()
            .resource_mut::<PieceDataStore>()
            .initialize(vec![Vec2::new(25.0, 30.0)]);
        let epoch = app.world().resource::<PieceDataStore>().epoch;
        let states = app.world().resource::<PieceDataStore>().states.clone();
        let generation = app.world().resource::<PersistenceState>().generation;
        let mut state = app.world_mut().resource_mut::<MultiplayerUi>();
        internet(&mut state);
        state.screen = MenuScreen::Host;
        if app_state == AppState::GameComplete {
            state.selected_save = Some((SaveId(123), "Completed save".into()));
        }
        state.submitted = true;
        state.connecting = true;
        state.owns_session = true;
        app.world_mut().insert_resource(NetworkStatus {
            role: Some(RuntimeRole::Host),
            connection_method: Some(RuntimeConnectionMethod::Internet),
            phase: RuntimePhase::Failed,
            failure: Some(NetworkFailureKind::Connection),
            error: Some("injected WSS failure".into()),
            host_start_failed: true,
            ..default()
        });
        render_schedule(&mut app, &ctx, vec![]).drop_without_applying_deltas();
        let output = render_schedule(&mut app, &ctx, vec![]);
        let text = labels(&output).join(" ");
        assert!(text.contains("Room Settings"), "{text}");
        assert!(text.contains("Your puzzle is ready"), "{text}");
        assert!(!text.contains("0.0.0.0:27015"));
        output.drop_without_applying_deltas();
        let state = app.world().resource::<MultiplayerUi>();
        assert!(state.editing_host_retry && state.prepared_host);
        assert_eq!(state.error, Some(UiError::ConnectionFailed));
        assert!(state.host.password.is_empty());
        assert!(app.world().resource::<LocalGameplayBlocked>().0);
        click_label(&mut app, &ctx, "Open Room & Play");
        assert!(app
            .world()
            .resource::<MultiplayerUi>()
            .pending_host
            .is_none());
        *app.world_mut()
            .resource_mut::<MultiplayerUi>()
            .host
            .password = "retry password".into();
        click_label(&mut app, &ctx, "Open Room & Play");
        let state = app.world().resource::<MultiplayerUi>();
        assert!(state.pending_host.is_some() && state.submitted);
        assert!(!state.editing_host_retry);
        assert!(state.host.password.is_empty());
        assert!(matches!(
            app.world().resource::<NextState<AppState>>(),
            NextState::Unchanged
        ));
        assert_eq!(*app.world().resource::<State<AppState>>().get(), app_state);
        assert_eq!(app.world().resource::<PieceDataStore>().epoch, epoch);
        assert_eq!(
            app.world().resource::<PieceDataStore>().states.as_ptr(),
            states.as_ptr()
        );
        assert_eq!(
            app.world().resource::<PersistenceState>().generation,
            generation
        );
        assert!(!app.world().resource::<PersistenceState>().busy);
        // A second asynchronous failure must reopen the settings too.
        app.world_mut().resource_mut::<MultiplayerUi>().pending_host = None;
        render_schedule(&mut app, &ctx, vec![]).drop_without_applying_deltas();
        assert!(app.world().resource::<MultiplayerUi>().editing_host_retry);
        click_label(&mut app, &ctx, "Cancel");
        assert!(!app.world().resource::<MultiplayerUi>().prepared_host);
        assert!(!app.world().resource::<NetworkStatus>().host_start_failed);
        assert!(matches!(
            app.world().resource::<NextState<AppState>>(),
            NextState::Pending(AppState::Menu)
        ));
    }
}
#[test]
fn internet_failure_status_does_not_replace_a_direct_listen_retry() {
    let (mut app, ctx) = scheduled_screens();
    app.world_mut().insert_resource(NetworkStatus {
        role: Some(RuntimeRole::Host),
        connection_method: Some(RuntimeConnectionMethod::Internet),
        phase: RuntimePhase::Failed,
        failure: Some(NetworkFailureKind::Timeout),
        host_start_failed: true,
        ..default()
    });
    let mut state = app.world_mut().resource_mut::<MultiplayerUi>();
    state.prepared_host = true;
    state.connecting = true;
    state.owns_session = true;
    state.submitted = true;
    state.error = Some(UiError::ConnectionFailed);
    state.retry_host = Some(HostStartRequest::new(HostOptions {
        display_name: None,
        address: "0.0.0.0:27015".parse().unwrap(),
        session: jigsall_core::session::SessionDefinition {
            id: jigsall_core::session::SessionId(42),
            image_hash: jigsall_core::session::ImageHash([0; 32]),
        },
        host: jigsall_core::PlayerId(0),
        password: SessionPassword::new("test password".into()).unwrap(),
    }));
    render_schedule(&mut app, &ctx, vec![]).drop_without_applying_deltas();
    let state = app.world().resource::<MultiplayerUi>();
    assert_eq!(state.error, Some(UiError::ConnectionFailed));
    assert!(!state.editing_host_retry);
    click_label(&mut app, &ctx, "Back");
    click_label(&mut app, &ctx, "Open Room & Play");
    let state = app.world().resource::<MultiplayerUi>();
    assert!(state.retrying && state.retry_host.is_some());
    assert!(state.pending_host.is_none());
}

#[test]
fn internet_scheduled_menu_switches_methods_and_falls_back_without_configuration() {
    let (mut app, ctx) = scheduled_screens();
    app.world_mut().insert_resource(State::new(AppState::Menu));
    app.world_mut()
        .resource_mut::<MultiplayerUi>()
        .navigate(MenuScreen::Join);
    render_schedule(&mut app, &ctx, vec![]).drop_without_applying_deltas();
    assert_eq!(
        app.world().resource::<MultiplayerUi>().host.method,
        RuntimeConnectionMethod::DirectIp
    );
    configure(app.world_mut());
    render_schedule(&mut app, &ctx, vec![]).drop_without_applying_deltas();
    assert_eq!(
        app.world().resource::<MultiplayerUi>().host.method,
        RuntimeConnectionMethod::Internet
    );
    click_label(&mut app, &ctx, "Direct IP / LAN");
    assert_eq!(
        app.world().resource::<MultiplayerUi>().join.method,
        RuntimeConnectionMethod::DirectIp
    );
    app.world_mut().remove_resource::<RendezvousRuntimeConfig>();
    internet(&mut app.world_mut().resource_mut::<MultiplayerUi>());
    render_schedule(&mut app, &ctx, vec![]).drop_without_applying_deltas();
    assert_eq!(
        app.world().resource::<MultiplayerUi>().join.method,
        RuntimeConnectionMethod::DirectIp
    );
}

#[test]
fn internet_forms_show_room_fields_and_hide_direct_address_and_network_details() {
    let ctx = egui::Context::default();
    let mut profile = PlayerSettingsState::load(None);
    let i18n = crate::localization::tests::english();
    for host in [true, false] {
        for method in [
            RuntimeConnectionMethod::Internet,
            RuntimeConnectionMethod::DirectIp,
        ] {
            let mut draft = ConnectionDraft::new("127.0.0.1:27015");
            draft.method = method;
            draft.room_code = "abcdefghjk".into();
            *draft.password = "private password".into();
            let output = ctx.run_ui(egui::RawInput::default(), |ui| {
                paint_connection_fields(ui, &mut draft, host, &mut profile, &i18n)
            });
            let text = labels(&output).join(" ");
            assert!(!text.contains("private password"));
            if method == RuntimeConnectionMethod::Internet {
                assert!(!text.contains("127.0.0.1:27015"));
                assert!(!text.contains("Network details"));
                if host {
                    assert!(text.contains("room code will be created"));
                } else {
                    assert!(text.contains("Room Code"));
                    assert_eq!(draft.room_code, "ABCDEFGHJK");
                }
            } else {
                assert!(text.contains("127.0.0.1:27015"));
            }
            output.drop_without_applying_deltas();
        }
    }
}
#[test]
fn internet_room_code_uses_protocol_parser_and_submits_owned_password() {
    let profile = PlayerSettingsState::load(None);
    let mut state = MultiplayerUi::default();
    internet(&mut state);
    for code in [
        "",
        "ABCDEFGHJ",
        "ABCDEFGHIJ",
        "ABCDEFGHLK",
        "ABCDEFGHOK",
        "ABCDEFGHUK",
        "ＡBCDEFGHJK",
    ] {
        state.join.room_code = code.into();
        *state.join.password = "test password".into();
        assert_eq!(valid_room_code(code), code.parse::<RoomCode>().is_ok());
        assert!(!state.join.valid(false));
        state.submit_join(&profile);
        assert_eq!(state.error, Some(UiError::RoomCode));
        assert!(state.action.is_none());
    }
    state.join.room_code = "abcdefghjk".into();
    *state.join.password = "test password".into();
    assert!(state.join.valid(false));
    state.submit_join(&profile);
    assert!(state.join.password.is_empty());
    let Some(Action::JoinInternet(options)) = state.action.take() else {
        panic!()
    };
    assert_eq!(options.room_code.to_string(), "ABCDEFGHJK");
    assert_eq!(state.join.room_code, "ABCDEFGHJK");
    state.submit_join(&profile);
    assert!(state.action.is_none());
}
#[test]
fn internet_scheduled_join_invalid_code_keeps_submit_disabled() {
    let (mut app, ctx) = scheduled_screens();
    configure(app.world_mut());
    app.world_mut().insert_resource(State::new(AppState::Menu));
    let mut state = app.world_mut().resource_mut::<MultiplayerUi>();
    state.navigate(MenuScreen::Join);
    internet(&mut state);
    state.join.room_code = "ABCDEFGHIK".into();
    *state.join.password = "test password".into();
    click_label(&mut app, &ctx, "Join Game");
    assert!(!app.world().resource::<MultiplayerUi>().submitted);
    assert!(app.world().resource::<MultiplayerUi>().action.is_none());
}
#[test]
fn internet_wrong_password_keeps_code_clears_secret_and_returns_to_join() {
    let (mut app, ctx) = scheduled_screens();
    configure(app.world_mut());
    let mut state = app.world_mut().resource_mut::<MultiplayerUi>();
    internet(&mut state);
    state.screen = MenuScreen::Join;
    state.connecting = true;
    state.join.room_code = "ABCDEFGHJK".into();
    *state.join.password = "draft password".into();
    app.world_mut().insert_resource(NetworkStatus {
        role: Some(RuntimeRole::Client),
        connection_method: Some(RuntimeConnectionMethod::Internet),
        phase: RuntimePhase::Failed,
        failure: Some(NetworkFailureKind::Authentication),
        error: Some("misleading diagnostic".into()),
        ..default()
    });
    let output = render_schedule(&mut app, &ctx, vec![]);
    assert!(!labels(&output).join(" ").contains("draft password"));
    output.drop_without_applying_deltas();
    click_label(&mut app, &ctx, "Back");
    assert_eq!(
        app.world().resource::<MultiplayerUi>().join.room_code,
        "ABCDEFGHJK"
    );
    assert!(app
        .world()
        .resource::<MultiplayerUi>()
        .join
        .password
        .is_empty());
    assert_eq!(
        app.world().resource::<MultiplayerUi>().join.method,
        RuntimeConnectionMethod::Internet
    );
    assert!(app.world().resource::<MultiplayerUi>().screen == MenuScreen::Join);
}
#[test]
fn internet_host_hides_code_allows_copy_and_omits_code_controls_from_pause_menu() {
    let (mut app, ctx) = scheduled_screens();
    app.world_mut().insert_resource(NetworkStatus {
        role: Some(RuntimeRole::Host),
        phase: RuntimePhase::Hosting,
        connection_method: Some(RuntimeConnectionMethod::Internet),
        room_code: Some("ABCDEFGHJK".into()),
        rendezvous_control: Some(RendezvousControlStatus::Available),
        ..default()
    });
    render_schedule(&mut app, &ctx, vec![]).drop_without_applying_deltas();
    let output = render_schedule(&mut app, &ctx, vec![]);
    assert!(!labels(&output).contains(&"ABCDEFGHJK"));
    assert!(labels(&output).contains(&"Show"));
    let point = output
        .shapes
        .iter()
        .find_map(|s| match &s.shape {
            egui::Shape::Text(t) if t.galley.job.text == "Copy" => {
                Some(t.pos + t.galley.size() * 0.5)
            }
            _ => None,
        })
        .unwrap();
    output.drop_without_applying_deltas();
    let mut copied = false;
    for pressed in [true, false] {
        let output = render_schedule(
            &mut app,
            &ctx,
            vec![
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: default(),
                },
            ],
        );
        copied |= output
            .platform_output
            .commands
            .iter()
            .any(|c| matches!(c,egui::OutputCommand::CopyText(text) if text=="ABCDEFGHJK"));
        output.drop_without_applying_deltas();
    }
    assert!(copied);
    click_label(&mut app, &ctx, "Show");
    let output = render_schedule(&mut app, &ctx, vec![]);
    assert!(labels(&output).contains(&"ABCDEFGHJK"));
    assert!(labels(&output).contains(&"Hide"));
    assert!(ctx.memory(|memory| memory.focused().is_none()));
    output.drop_without_applying_deltas();
    app.world_mut()
        .insert_resource(State::new(GameSubState::Paused));
    render_schedule(&mut app, &ctx, vec![]).drop_without_applying_deltas();
    let output = render_schedule(&mut app, &ctx, vec![]);
    let text = labels(&output);
    assert!(text.contains(&"Multiplayer Menu"));
    // The only code and controls belong to the HUD, even after revealing it.
    for label in ["Room Code", "ABCDEFGHJK", "Hide", "Copy"] {
        assert_eq!(text.iter().filter(|&&text| text == label).count(), 1);
    }
    output.drop_without_applying_deltas();
    let mut status = app.world_mut().resource_mut::<NetworkStatus>();
    status.room_code = None;
    status.rendezvous_control = Some(RendezvousControlStatus::Unavailable);
    let output = render_schedule(&mut app, &ctx, vec![]);
    assert!(labels(&output)
        .iter()
        .any(|text| text.contains("New players cannot join")));
    assert!(!labels(&output).contains(&"ABCDEFGHJK"));
    output.drop_without_applying_deltas();
}
#[test]
fn internet_host_action_keeps_password_out_of_control_options_and_typed_error_messages() {
    let mut state = MultiplayerUi::default();
    internet(&mut state);
    *state.host.password = "test password".into();
    state.submit_host();
    assert!(state.host.password.is_empty());
    assert!(matches!(
        state.action,
        Some(Action::PrepareHost(PendingHost::Internet(_)))
    ));
    state.cancel();
    assert!(state.pending_host.is_none());
    assert_eq!(
        UiError::failure(NetworkFailureKind::RoomNotFound),
        UiError::RoomNotFound
    );
    assert_eq!(
        UiError::start(RuntimeStartError::InternetUnavailable),
        UiError::InternetUnavailable
    );
}

#[test]
fn internet_scheduled_host_setup_has_no_bind_fields_and_direct_keeps_them() {
    let (mut app, ctx) = scheduled_screens();
    configure(app.world_mut());
    app.world_mut()
        .insert_resource(State::new(AppState::GameSetup));
    let mut state = app.world_mut().resource_mut::<MultiplayerUi>();
    internet(&mut state);
    state.host_setup = true;
    state.host_settings_tab = true;
    render_schedule(&mut app, &ctx, vec![]).drop_without_applying_deltas();
    let output = render_schedule(&mut app, &ctx, vec![]);
    let text = labels(&output).join(" ");
    assert!(text.contains("room code will be created"));
    assert!(!text.contains("0.0.0.0:27015"));
    output.drop_without_applying_deltas();
    click_label(&mut app, &ctx, "Direct IP / LAN");
    let output = render_schedule(&mut app, &ctx, vec![]);
    assert!(labels(&output).contains(&"0.0.0.0:27015"));
    output.drop_without_applying_deltas();
}
#[test]
fn internet_valid_code_and_password_submit_through_scheduled_widgets_once() {
    // Stop at the public request boundary: no native backend/server is required.
    let (mut world, _, ctx) = screen_world();
    configure(&mut world);
    world.insert_resource(State::new(AppState::Menu));
    world.init_resource::<bevy::ecs::schedule::Schedules>();
    let mut state = world.resource_mut::<MultiplayerUi>();
    state.navigate(MenuScreen::Join);
    internet(&mut state);
    state.join.room_code = "abcdefghjk".into();
    *state.join.password = "test password".into();
    let mut app = App::new();
    *app.world_mut() = world;
    app.add_systems(bevy_egui::EguiPrimaryContextPass, crate::menu::draw_menu_ui);
    click_label(&mut app, &ctx, "Join Game");
    let mut state = app.world_mut().resource_mut::<MultiplayerUi>();
    assert!(state.submitted);
    assert!(state.join.password.is_empty());
    let Some(Action::JoinInternet(options)) = state.action.take() else {
        panic!()
    };
    assert_eq!(options.room_code.to_string(), "ABCDEFGHJK");
    state.submit_join(&PlayerSettingsState::load(None));
    assert!(state.action.is_none());
}
