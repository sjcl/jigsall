//! Opt-in visual check using the application's real egui/GPU schedules.
use super::*;
use bevy::{
    render::view::screenshot::{save_to_disk, Screenshot},
    winit::{WinitPlugin, WinitSettings},
};
use jigsall_game::{
    network::runtime::{NetworkStatus, RuntimePhase, RuntimeRole},
    persistence::runtime::PersistenceService,
    resources::{AppState, GameSubState},
};

#[test]
#[ignore = "requires a native window and GPU"]
fn native_notification_ui_probe() {
    fn path(name: &str) -> std::path::PathBuf {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/ui-probes");
        std::fs::create_dir_all(&root).unwrap();
        root.join(format!("notifications-{name}.png"))
    }
    #[derive(Resource)]
    struct Probe {
        next: Instant,
        phase: usize,
    }
    fn advance(world: &mut World) {
        let (phase, next) = {
            let probe = world.resource::<Probe>();
            (probe.phase, probe.next)
        };
        if Instant::now() < next {
            return;
        }
        match phase {
            0 => {
                // This fixture checks presentation with a ready HUD, without
                // creating sockets or inventing network connection handles.
                world.insert_resource(State::new(AppState::InGame));
                world.insert_resource(State::new(GameSubState::Playing));
                world.insert_resource(NetworkStatus {
                    role: Some(RuntimeRole::Host),
                    phase: RuntimePhase::Hosting,
                    ..default()
                });
                world.insert_resource(jigsall_game::players::PlayerRoster::host_only(
                    PlayerId(0),
                    Some(PlayerDisplayName::from_user_input("Host").unwrap()),
                ));
                world
                    .resource_mut::<Localization>()
                    .set_preference(LanguagePreference::Locale(Locale::JA));
                world.resource_mut::<Messages<GameEvent>>().write_batch([
                    GameEvent::PlayerJoining {
                        connection: ConnectionId(1),
                    },
                    GameEvent::PlayerJoining {
                        connection: ConnectionId(2),
                    },
                    GameEvent::PlayerJoined {
                        player: player(1, Some("Alice")),
                        connection: Some(ConnectionId(1)),
                    },
                    GameEvent::AutosaveStarted,
                    GameEvent::AutosaveRotationFailed {
                        error: PersistenceError::AutosaveRotation(
                            jigsall_game::persistence::SaveError::Storage(
                                jigsall_game::persistence::StorageError::Io("fixture".into()),
                            ),
                        ),
                    },
                    GameEvent::PlayerJoining {
                        connection: ConnectionId(3),
                    },
                    GameEvent::PlayerJoinCancelled {
                        connection: ConnectionId(3),
                        reason: DisconnectReason::SyncPhaseTimeout,
                    },
                ]);
            }
            1 => {
                world
                    .spawn(Screenshot::primary_window())
                    .observe(save_to_disk(path("ja-toasts")));
            }
            2 => {
                world.resource_mut::<Notifications>().opened = true;
            }
            3 => {
                world
                    .spawn(Screenshot::primary_window())
                    .observe(save_to_disk(path("ja-history")));
            }
            4 => {
                world
                    .resource_mut::<Localization>()
                    .set_preference(LanguagePreference::Locale(Locale::EN_US));
            }
            5 => {
                world
                    .spawn(Screenshot::primary_window())
                    .observe(save_to_disk(path("en-history")));
            }
            6 => {
                world.resource_mut::<Notifications>().opened = false;
                world.resource_mut::<Messages<GameEvent>>().write_batch([
                    GameEvent::AutosaveStarted,
                    GameEvent::AutosaveFailed {
                        error: PersistenceError::WorkerStopped,
                    },
                    GameEvent::PlayerJoined {
                        player: player(2, Some("Bob")),
                        connection: Some(ConnectionId(2)),
                    },
                ]);
            }
            7 => {
                world
                    .spawn(Screenshot::primary_window())
                    .observe(save_to_disk(path("en-toasts")));
            }
            8 => {
                assert!(world.resource::<Notifications>().visible().is_empty());
                assert!(world.resource::<Notifications>().autosave_issue.is_some());
                world
                    .spawn(Screenshot::primary_window())
                    .observe(save_to_disk(path("en-persistent")));
            }
            _ => {
                world
                    .resource_mut::<Messages<AppExit>>()
                    .write(AppExit::Success);
            }
        }
        let mut probe = world.resource_mut::<Probe>();
        probe.phase += 1;
        probe.next = Instant::now()
            + if phase == 7 {
                // Verify the HUD warning after real monotonic toast expiry.
                TOAST_DURATION + Duration::from_millis(800)
            } else {
                Duration::from_millis(800)
            };
    }
    let started = std::time::SystemTime::now();
    let mut preferences = crate::preferences::UiPreferences::load(None);
    preferences.language = LanguagePreference::Locale(Locale::JA);
    let (service, _storage) = PersistenceService::with_storage_requests();
    App::new()
        .add_plugins(
            DefaultPlugins
                .set(WinitPlugin {
                    run_on_any_thread: true,
                })
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        resolution: (1280, 720).into(),
                        visible: false,
                        title: "Notification UI probe".into(),
                        ..default()
                    }),
                    ..default()
                }),
        )
        .insert_resource(jigsall_game::settings::DisplaySettingsState::load(None))
        .insert_resource(preferences)
        .insert_resource(jigsall_game::image_settings::ImageSettingsState::load(None))
        .insert_resource(jigsall_game::persistence::autosave::AutosaveSettingsState::load(None))
        .insert_resource(jigsall_game::player_settings::PlayerSettingsState::load(
            None,
        ))
        .insert_resource(jigsall_game::keybindings::KeyBindingsState::load(None))
        .insert_resource(service)
        .insert_resource(crate::localization::tests::english())
        .insert_resource(WinitSettings::continuous())
        .add_plugins((
            crate::GameUiPlugin,
            jigsall_game::asset_reader::DirectFileAssetPlugin,
            jigsall_game::GamePlugin,
        ))
        .insert_resource(Probe {
            next: Instant::now() + Duration::from_millis(1200),
            phase: 0,
        })
        .add_systems(Update, advance)
        .run();
    for name in [
        "ja-toasts",
        "ja-history",
        "en-history",
        "en-toasts",
        "en-persistent",
    ] {
        let metadata = std::fs::metadata(path(name)).unwrap();
        assert!(metadata.len() > 0);
        assert!(metadata.modified().unwrap() >= started);
    }
}
