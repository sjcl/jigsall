//! Opt-in visual probe of the real plugin schedules, without a visible window.
use super::*;
use bevy::{
    render::view::screenshot::{save_to_disk, Screenshot},
    winit::{WinitPlugin, WinitSettings},
};
use std::time::{Duration, Instant, SystemTime};

#[test]
#[ignore = "requires a native window and GPU"]
fn native_multiplayer_ui_probe() {
    fn path(name: &str) -> std::path::PathBuf {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/ui-probes");
        std::fs::create_dir_all(&root).unwrap();
        root.join(format!("multiplayer-{name}.png"))
    }
    #[derive(Resource)]
    struct Probe {
        start: Instant,
        phase: usize,
    }
    #[allow(clippy::too_many_arguments)]
    fn advance(
        mut probe: ResMut<Probe>,
        mut menu: ResMut<MultiplayerUi>,
        mut status: ResMut<NetworkStatus>,
        mut next: ResMut<NextState<AppState>>,
        mut persistence: ResMut<PersistenceState>,
        mut commands: Commands,
        mut exit: MessageWriter<AppExit>,
    ) {
        if probe.start.elapsed() < Duration::from_millis(1200 * (probe.phase as u64 + 1)) {
            return;
        }
        match probe.phase {
            0 => {
                commands
                    .spawn(Screenshot::primary_window())
                    .observe(save_to_disk(path("title")));
            }
            1 => menu.navigate(MenuScreen::Multiplayer),
            2 => {
                commands
                    .spawn(Screenshot::primary_window())
                    .observe(save_to_disk(path("menu")));
            }
            3 => {
                menu.navigate(MenuScreen::Join);
                *menu.join.password = "test password".into();
            }
            4 => {
                commands
                    .spawn(Screenshot::primary_window())
                    .observe(save_to_disk(path("join")));
            }
            5 => {
                menu.navigate(MenuScreen::Host);
                menu.host_setup = true;
                *menu.host.password = "test password".into();
                persistence.retain_image_for_host = true;
                next.set(AppState::GameSetup);
            }
            6 => {
                commands
                    .spawn(Screenshot::primary_window())
                    .observe(save_to_disk(path("new-host")));
            }
            7 => next.set(AppState::Menu),
            8 => {
                menu.navigate(MenuScreen::HostLoadSettings);
                menu.selected_save = Some((SaveId(1), "保存したパズル".into()));
                *menu.host.password = "test password".into();
            }
            9 => {
                commands
                    .spawn(Screenshot::primary_window())
                    .observe(save_to_disk(path("load-host")));
            }
            10 => {
                *status = NetworkStatus {
                    role: Some(RuntimeRole::Client),
                    phase: RuntimePhase::Syncing(SyncPhase::Finalizing),
                    ..default()
                };
                next.set(AppState::GameSetup);
            }
            11 => {
                commands
                    .spawn(Screenshot::primary_window())
                    .observe(save_to_disk(path("sync")));
            }
            _ => {
                exit.write(AppExit::Success);
            }
        }
        probe.phase += 1;
    }
    let started = SystemTime::now();
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
                        title: "Multiplayer UI probe".into(),
                        resolution: (1280, 900).into(),
                        visible: false,
                        ..default()
                    }),
                    ..default()
                }),
        )
        .insert_resource(puzzella_game::settings::DisplaySettingsState::load(None))
        .insert_resource(preferences)
        .insert_resource(puzzella_game::image_settings::ImageSettingsState::load(
            None,
        ))
        .insert_resource(puzzella_game::persistence::autosave::AutosaveSettingsState::load(None))
        .insert_resource(PlayerSettingsState::load(None))
        .insert_resource(puzzella_game::keybindings::KeyBindingsState::load(None))
        .insert_resource(service)
        .insert_resource(crate::localization::tests::english())
        .insert_resource(WinitSettings::continuous())
        .add_plugins((
            crate::GameUiPlugin,
            puzzella_game::asset_reader::DirectFileAssetPlugin,
            puzzella_game::GamePlugin,
        ))
        .insert_resource(Probe {
            start: Instant::now(),
            phase: 0,
        })
        .add_systems(Update, advance)
        .run();
    for name in ["title", "menu", "join", "new-host", "load-host", "sync"] {
        let metadata = std::fs::metadata(path(name)).unwrap();
        assert!(metadata.len() > 0);
        assert!(metadata.modified().unwrap() >= started);
    }
}
