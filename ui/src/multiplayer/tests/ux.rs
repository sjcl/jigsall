use super::*;

#[test]
fn join_target_is_empty_and_cannot_silently_connect_to_localhost() {
    let mut state = MultiplayerUi::default();
    assert!(state.join.address.is_empty());
    *state.join.password = "test password".into();
    assert!(!state.join.valid(false));
    state.submit_join(&PlayerSettingsState::load(None));
    assert_eq!(state.error, Some(UiError::Address));
    assert!(state.action.is_none());
    assert!(!state.submitted);
}

#[test]
fn configured_room_codes_are_preferred_without_overriding_an_explicit_choice() {
    let mut state = MultiplayerUi::default();
    state.configure_connection_methods(true);
    let preferred = if cfg!(feature = "rendezvous") {
        RuntimeConnectionMethod::Internet
    } else {
        RuntimeConnectionMethod::DirectIp
    };
    assert_eq!(state.host.method, preferred);
    assert_eq!(state.join.method, preferred);
    state.method_selected = true;
    state.set_connection_method(RuntimeConnectionMethod::DirectIp);
    state.configure_connection_methods(true);
    state.navigate(MenuScreen::Join);
    state.configure_connection_methods(true);
    assert_eq!(state.join.method, RuntimeConnectionMethod::DirectIp);
    state.configure_connection_methods(false);
    assert_eq!(state.host.method, RuntimeConnectionMethod::DirectIp);
    assert!(!state.internet_available);
}

#[test]
fn changing_method_or_opening_settings_wipes_both_secrets_with_a_visible_reason() {
    let i18n = crate::localization::tests::english();
    for settings in [true, false] {
        let mut state = MultiplayerUi::default();
        *state.host.password = "private host password".into();
        *state.join.password = "private join password".into();
        if settings {
            state.clear_passwords_for_settings();
        } else {
            state.set_connection_method(RuntimeConnectionMethod::Internet);
        }
        assert!(state.host.password.is_empty());
        assert!(state.join.password.is_empty());
        let ctx = egui::Context::default();
        let output = ctx.run_ui(default(), |ui| state.paint_password_notice(ui, &i18n));
        assert!(labels(&output).contains(&i18n.text("multiplayer-password-cleared").as_str()));
        assert!(!labels(&output).join(" ").contains("private"));
        output.drop_without_applying_deltas();
    }
}

#[test]
fn missing_fields_are_explicit_and_internet_never_requires_an_address() {
    let mut i18n = crate::localization::tests::english();
    for locale in [Locale::EN_US, Locale::JA] {
        i18n.set_preference(LanguagePreference::Locale(locale));
        for host in [true, false] {
            for method in [
                RuntimeConnectionMethod::DirectIp,
                RuntimeConnectionMethod::Internet,
            ] {
                let mut draft = ConnectionDraft::new("");
                draft.method = method;
                let ctx = egui::Context::default();
                let output = ctx.run_ui(default(), |ui| {
                    paint_required_fields(ui, &draft, host, &i18n);
                });
                let text = labels(&output);
                assert!(text.contains(&i18n.text("multiplayer-password-required").as_str()));
                assert_eq!(
                    text.contains(&i18n.text("multiplayer-address-required").as_str()),
                    method == RuntimeConnectionMethod::DirectIp
                );
                assert_eq!(
                    text.contains(&i18n.text("multiplayer-room-code-required").as_str()),
                    method == RuntimeConnectionMethod::Internet && !host
                );
                output.drop_without_applying_deltas();
            }
        }
    }
}

#[test]
fn room_errors_do_not_ask_for_an_ip_address_in_either_language() {
    let mut i18n = crate::localization::tests::english();
    for locale in [Locale::EN_US, Locale::JA] {
        i18n.set_preference(LanguagePreference::Locale(locale));
        for error in [UiError::ConnectionFailed, UiError::Timeout] {
            let room_key = error.key_for_method(RuntimeConnectionMethod::Internet);
            assert_ne!(room_key, error.key());
            let text = i18n.text(room_key);
            assert!(!text.to_lowercase().contains("address"));
            assert!(!text.contains("アドレス"));
            assert_eq!(
                error.key_for_method(RuntimeConnectionMethod::DirectIp),
                error.key()
            );
        }
    }
}

#[test]
fn host_status_never_claims_room_accepts_players_without_a_working_room_service() {
    let mut status = NetworkStatus {
        role: Some(RuntimeRole::Host),
        phase: RuntimePhase::Hosting,
        connection_method: Some(RuntimeConnectionMethod::Internet),
        room_code: Some("ABCDEFGHJK".into()),
        rendezvous_control: Some(RendezvousControlStatus::Available),
        ..default()
    };
    assert_eq!(host_status_key(&status), Some("multiplayer-hosting"));
    status.rendezvous_control = Some(RendezvousControlStatus::Unavailable);
    assert_eq!(
        host_status_key(&status),
        Some("multiplayer-host-not-accepting")
    );
    status.rendezvous_control = Some(RendezvousControlStatus::Available);
    status.room_code = None;
    assert_eq!(
        host_status_key(&status),
        Some("multiplayer-host-not-accepting")
    );
    status.connection_method = Some(RuntimeConnectionMethod::DirectIp);
    assert_eq!(host_status_key(&status), Some("multiplayer-hosting"));
    status.phase = RuntimePhase::Failed;
    assert_eq!(host_status_key(&status), Some("multiplayer-host-not-open"));
    status.role = Some(RuntimeRole::Client);
    assert_eq!(host_status_key(&status), None);
}

#[test]
fn connection_messages_distinguish_real_image_and_sync_phases_without_fake_percentages() {
    for (phase, key) in [
        (SyncPhase::ImageNegotiation, "multiplayer-checking-image"),
        (SyncPhase::AwaitingImageSlot, "multiplayer-image-queued"),
        (SyncPhase::ImageTransfer, "multiplayer-receiving-image"),
        (SyncPhase::AwaitingImageReady, "multiplayer-preparing-image"),
        (SyncPhase::AwaitingBaselineSlot, "multiplayer-sync-queued"),
        (SyncPhase::BaselineTransfer, "multiplayer-syncing"),
        (SyncPhase::CatchingUp, "multiplayer-catching-up"),
        (SyncPhase::Finalizing, "multiplayer-finalizing"),
    ] {
        assert_eq!(
            connection_text(&NetworkStatus {
                phase: RuntimePhase::Syncing(phase),
                ..default()
            }),
            key
        );
    }
}

#[test]
fn invitation_is_dismissible_keeps_playing_and_does_not_reappear_each_frame() {
    let (mut app, ctx) = scheduled_screens();
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .reset_all();
    app.world_mut().resource_mut::<MultiplayerUi>().owns_session = true;
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
    let text = labels(&output);
    assert!(text.contains(&"Invite Players"));
    assert!(text.contains(&"ABCDEFGHJK"));
    assert!(text.iter().any(|text| text.contains("room password")));
    assert!(text
        .iter()
        .any(|text| text.contains("others can join later")));
    assert!(!app.world().resource::<LocalGameplayBlocked>().0);
    assert!(matches!(
        app.world().resource::<NextState<GameSubState>>(),
        NextState::Unchanged
    ));
    output.drop_without_applying_deltas();
    click_label(&mut app, &ctx, "Close");
    assert!(!app.world().resource::<MultiplayerUi>().invite_open);
    for _ in 0..3 {
        render_schedule(&mut app, &ctx, vec![]).drop_without_applying_deltas();
        assert!(!app.world().resource::<MultiplayerUi>().invite_open);
        assert!(!app.world().resource::<LocalGameplayBlocked>().0);
    }
    app.world_mut().resource_mut::<MultiplayerUi>().cancel();
    process_actions(app.world_mut());
    assert!(!app.world().resource::<MultiplayerUi>().invite_shown);
}

#[test]
fn room_code_copy_reports_success_without_taking_keyboard_focus() {
    let status = NetworkStatus {
        room_code: Some("ABCDEFGHJK".into()),
        ..default()
    };
    let i18n = crate::localization::tests::english();
    let ctx = egui::Context::default();
    let render = |events| {
        ctx.run_ui(
            egui::RawInput {
                events,
                ..default()
            },
            |ui| {
                paint_room_code(ui, &status, &i18n);
            },
        )
    };
    render(vec![]).drop_without_applying_deltas();
    let output = render(vec![]);
    let point = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.job.text == "Copy" => {
                Some(text.pos + text.galley.size() * 0.5)
            }
            _ => None,
        })
        .unwrap();
    output.drop_without_applying_deltas();
    let mut copied = false;
    let mut confirmed = false;
    for pressed in [true, false] {
        let output = render(vec![
            egui::Event::PointerMoved(point),
            egui::Event::PointerButton {
                pos: point,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: default(),
            },
        ]);
        copied |= output.platform_output.commands.iter().any(|command| matches!(command, egui::OutputCommand::CopyText(text) if text == "ABCDEFGHJK"));
        confirmed |= labels(&output).contains(&"Copied");
        output.drop_without_applying_deltas();
    }
    assert!(copied && confirmed);
    assert!(ctx.memory(|memory| memory.focused().is_none()));
}

#[test]
fn explicitly_selecting_the_direct_ip_fallback_survives_later_configuration() {
    let (mut app, ctx) = scheduled_screens();
    app.world_mut().insert_resource(State::new(AppState::Menu));
    app.world_mut()
        .resource_mut::<MultiplayerUi>()
        .navigate(MenuScreen::Join);
    click_label(&mut app, &ctx, "Direct IP / LAN");
    let mut state = app.world_mut().resource_mut::<MultiplayerUi>();
    assert!(state.method_selected);
    *state.join.password = "test password".into();
    state.configure_connection_methods(true);
    assert_eq!(state.join.method, RuntimeConnectionMethod::DirectIp);
    assert_eq!(state.join.password.as_str(), "test password");
}
