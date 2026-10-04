use super::*;
use crate::localization::{LanguagePreference, Locale};
use bevy::ecs::system::RunSystemOnce;
mod native;

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
    world.insert_resource(PersistenceService::with_storage_requests().0);
    world.insert_resource(puzzella_game::settings::DisplaySettingsState::load(None));
    world.insert_resource(PlayerSettingsState::load(None));
    world.init_resource::<Messages<AppExit>>();
    world.init_resource::<PuzzleConfig>();
    world.init_resource::<crate::game_setup::image_picker::ImagePicker>();
    world.init_resource::<puzzella_game::asset_reader::ExternalFileRegistry>();
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
        match screen {
            MenuScreen::Title => {
                assert!(labels.contains(&"Single Player"));
                assert!(labels.contains(&"Multiplayer"));
                assert!(!labels.contains(&"New Game"));
            }
            MenuScreen::SinglePlayer | MenuScreen::Host => {
                assert!(labels.contains(&"New Game"));
                assert!(labels.contains(&"Load Game"));
            }
            MenuScreen::Multiplayer => {
                assert!(labels.contains(&"Host"));
                assert!(labels.contains(&"Join"));
                assert!(!labels.contains(&"New Game"));
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
        assert!(!labels.contains(&"New Game"));
        assert!(!labels.contains(&"Select Image"));
        assert!(!labels
            .iter()
            .any(|text| text.contains("Finalizing") || text.contains("ImageTransfer")));
        output.drop_without_applying_deltas();
    }
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
        assert!(labels.contains(&"New multiplayer game"));
        assert!(labels.contains(&"Puzzle settings"));
        assert!(labels.contains(&"Multiplayer settings"));
        assert_eq!(labels.contains(&"Listen IP address and port"), network_tab);
        assert_eq!(labels.contains(&"Select Image"), !network_tab);
        assert_eq!(
            world.resource::<MultiplayerUi>().host.password.as_str(),
            "test password"
        );
        output.drop_without_applying_deltas();
    }
}

#[test]
fn addresses_accept_ipv4_and_bracketed_ipv6_but_reject_dns_and_unspecified_join() {
    for address in ["192.168.1.10:27015", "[2001:db8::1]:27015", " [::1]:27015 "] {
        assert!(parse_address(address, false).is_ok(), "{address}");
    }
    for address in [
        "example.com:27015",
        "2001:db8::1:27015",
        "127.0.0.1",
        "127.0.0.1:0",
        "0.0.0.0:27015",
        "[::]:27015",
    ] {
        assert_eq!(
            parse_address(address, false),
            Err(UiError::Address),
            "{address}"
        );
    }
    assert!(parse_address("0.0.0.0:27015", true).is_ok());
    assert!(parse_address("[::]:27015", true).is_ok());
}

#[test]
fn join_moves_password_once_and_uses_the_committed_profile() {
    let mut profile = PlayerSettingsState::load(None);
    profile.commit("Alice");
    let mut state = MultiplayerUi::default();
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
fn password_cannot_be_restored_from_egui_undo_history_after_the_form_closes() {
    let ctx = egui::Context::default();
    let profile = PlayerSettingsState::load(None);
    let i18n = crate::localization::tests::english();
    let mut draft = ConnectionDraft::new("127.0.0.1:27015");
    let id = egui::Id::new("multiplayer-join-password");
    let mut paint = |time, events| {
        ctx.run_ui(
            egui::RawInput {
                time: Some(time),
                events,
                ..default()
            },
            |ui| paint_connection_fields(ui, &mut draft, false, &profile, &i18n),
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
        |ui| paint_connection_fields(ui, &mut draft, false, &profile, &i18n),
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
fn network_diagnostics_map_to_readable_error_categories() {
    for (error, category) in [
        ("Rejected(AuthenticationFailed)", UiError::WrongPassword),
        ("BackendConnectionTimeout", UiError::Timeout),
        ("JoinCapacity", UiError::ServerFull),
        ("CapacityWaitTimeout", UiError::ServerFull),
        ("BackendFailure", UiError::ConnectionFailed),
        ("Wire(UnsupportedVersion(99))", UiError::ProtocolMismatch),
        ("ImageHashMismatch", UiError::ImageUnavailable),
    ] {
        assert_eq!(UiError::diagnostic(Some(error)), category);
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
    world.init_resource::<puzzella_game::render::RenderReady>();
    start_prepared_host(&mut world);
    assert!(world.resource::<MultiplayerUi>().pending_host.is_none());
    assert_eq!(
        world.resource::<MultiplayerUi>().error,
        Some(UiError::ImageUnavailable)
    );
}

#[test]
fn forms_show_confirmed_name_address_contract_and_secret_field_in_both_languages() {
    let mut i18n = crate::localization::tests::english();
    let mut profile = PlayerSettingsState::load(None);
    profile.commit("Alice");
    for locale in [Locale::EN_US, Locale::JA] {
        i18n.set_preference(LanguagePreference::Locale(locale));
        for host in [true, false] {
            let mut draft =
                ConnectionDraft::new(if host { "0.0.0.0:27015" } else { "[::1]:27015" });
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
                    paint_connection_fields(ui, &mut draft, host, &profile, &i18n);
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
            assert!(labels.contains(
                &i18n
                    .format("multiplayer-player-name", &[("name", "Alice".into())])
                    .as_str()
            ));
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
            address: Some("0.0.0.0:27015".parse().unwrap()),
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
    assert!(labels
        .iter()
        .any(|text| text.contains("Listening on: 0.0.0.0:27015")));
    assert!(!labels
        .iter()
        .any(|text| text.contains("Address to share with players: 0.0.0.0")));
    output.drop_without_applying_deltas();
}

#[cfg(feature = "gns")]
#[test]
fn prepared_host_starts_the_native_listener_once_with_the_committed_name() {
    use puzzella_core::PuzzleDefinition;
    let (mut world, _, _) = screen_world();
    let mut encoded = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(32, 32, image::Rgb([1, 2, 3])))
        .write_to(&mut encoded, image::ImageFormat::Png)
        .unwrap();
    let encoded = encoded.into_inner();
    world.insert_resource(OriginalPuzzleImage {
        hash: puzzella_game::persistence::image_hash(&encoded),
        encoded: Some(encoded.into()),
        image_lease: None,
    });
    let definition = PuzzleDefinition {
        generator_version: puzzella_core::GENERATOR_VERSION,
        seed: 12,
        grid_size: UVec2::splat(2),
        image_size: UVec2::splat(32),
        snap_distance: 1.0,
    };
    let mut store = PieceDataStore::default();
    store.initialize(
        (0..4)
            .map(|id| definition.correct_position(puzzella_core::PieceId(id)) + Vec2::splat(100.0))
            .collect(),
    );
    world.insert_resource(store);
    world.insert_resource(definition);
    world.insert_resource(State::new(AppState::InGame));
    world.init_resource::<puzzella_game::render::RenderReady>();
    world.init_resource::<LocalPlayerId>();
    world.insert_resource(PieceGenerationProgress {
        generation_phase: GenerationPhase::UploadingGpu,
        is_generating: true,
        ..default()
    });
    world.resource_mut::<PlayerSettingsState>().commit("Alice");
    world.resource_mut::<MultiplayerUi>().pending_host = Some(PendingHost {
        address: "127.0.0.1:0".parse().unwrap(),
        password: SessionPassword::new("test password".into()).unwrap(),
    });
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
    assert!(world.contains_non_send::<puzzella_game::network::runtime::NetworkSession>());
    let player = world.resource::<LocalPlayerId>().0;
    assert_eq!(
        world
            .resource::<puzzella_game::players::PlayerRoster>()
            .get(player)
            .unwrap()
            .display_name,
        world.resource::<PlayerSettingsState>().current.display_name
    );
    start_prepared_host(&mut world);
    assert_eq!(world.resource::<NetworkStatus>().address, Some(address));
    world.resource_mut::<MultiplayerUi>().cancel();
    process_actions(&mut world);
    assert!(!world.contains_non_send::<puzzella_game::network::runtime::NetworkSession>());
}
