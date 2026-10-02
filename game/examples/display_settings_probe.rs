//! Native backend verification, without menu UI or changes to user preferences.
//! cargo run --locked -p puzzella-game --example display_settings_probe
use bevy::{
    ecs::system::NonSendMarker,
    prelude::*,
    window::PrimaryWindow,
    winit::{WinitSettings, WINIT_WINDOWS},
};
use puzzella_game::settings::*;
use std::time::{Duration, Instant};

#[derive(Resource)]
struct Probe {
    phase: usize,
    since: Instant,
    samples: Vec<Instant>,
}

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Puzzella display settings probe".into(),
                ..default()
            }),
            ..default()
        }))
        .insert_resource(DisplaySettingsState::load(None))
        .insert_resource(WinitSettings::continuous())
        .add_plugins(DisplaySettingsPlugin)
        .insert_resource(Probe {
            phase: 0,
            since: Instant::now(),
            samples: vec![],
        })
        .add_systems(Startup, |mut commands: Commands| {
            commands.spawn(Camera2d);
        })
        .add_systems(Update, probe)
        .run();
}

fn probe(
    mut probe: ResMut<Probe>,
    windows: Query<(Entity, &Window), With<PrimaryWindow>>,
    capabilities: Res<DisplayCapabilities>,
    state: Res<DisplaySettingsState>,
    mut actions: MessageWriter<DisplaySettingsAction>,
    mut exit: MessageWriter<AppExit>,
    _main_thread: NonSendMarker,
) {
    let now = Instant::now();
    if capabilities.monitor.is_none() || windows.is_empty() {
        return;
    }
    if probe.since.elapsed() < Duration::from_secs(2) {
        return;
    }
    let (entity, _) = windows.single().unwrap();
    if (3..=5).contains(&probe.phase) && probe.since.elapsed() < Duration::from_secs(4) {
        probe.samples.push(now);
        return;
    }
    WINIT_WINDOWS.with_borrow(|native| {
        let window = native.get_window(entity).unwrap();
        match probe.phase {
            1 | 3 | 7 => {
                assert!(window.fullscreen().is_none());
                assert_eq!(
                    (window.inner_size().width, window.inner_size().height),
                    (800, 600)
                );
            }
            2 => {
                assert!(window.fullscreen().is_some());
                assert_eq!(
                    (window.inner_size().width, window.inner_size().height),
                    (
                        capabilities.desktop_resolution.unwrap().x,
                        capabilities.desktop_resolution.unwrap().y
                    )
                );
            }
            6 => {
                assert!(window.fullscreen().is_some());
                assert_eq!(
                    (window.inner_size().width, window.inner_size().height),
                    (state.current.resolution.x, state.current.resolution.y)
                );
            }
            _ => {}
        }
        println!(
            "phase {}: native size {:?}, fullscreen {:?}",
            probe.phase,
            window.inner_size(),
            window.fullscreen()
        );
    });
    if (3..=5).contains(&probe.phase) {
        let duration = probe
            .samples
            .last()
            .unwrap()
            .duration_since(probe.samples[0]);
        let fps = (probe.samples.len() - 1) as f64 / duration.as_secs_f64();
        println!("measured FPS: {fps:.2}, cap: {:?}", state.current.max_fps);
        if let Some(cap) = state.current.max_fps {
            assert!(fps <= f64::from(cap) * 1.05);
        }
    }
    actions.write(DisplaySettingsAction::Keep);
    let settings = match probe.phase {
        0 => DisplaySettings {
            resolution: UVec2::new(800, 600),
            ..default()
        },
        1 => DisplaySettings {
            mode: ScreenMode::Borderless,
            ..state.current.clone()
        },
        2 => DisplaySettings {
            mode: ScreenMode::Windowed,
            max_fps: Some(30),
            ..state.current.clone()
        },
        3 => DisplaySettings {
            max_fps: Some(120),
            ..state.current.clone()
        },
        4 => DisplaySettings {
            max_fps: None,
            ..state.current.clone()
        },
        5 => {
            let size = capabilities
                .fullscreen_mode(UVec2::new(1280, 720))
                .map(|mode| mode.physical_size)
                .unwrap_or(capabilities.video_modes[0].physical_size);
            DisplaySettings {
                mode: ScreenMode::Fullscreen,
                resolution: size,
                max_fps: Some(60),
            }
        }
        6 => DisplaySettings {
            mode: ScreenMode::Windowed,
            resolution: UVec2::new(800, 600),
            ..default()
        },
        _ => {
            println!("Native display settings probe passed.");
            exit.write(AppExit::Success);
            return;
        }
    };
    actions.write(DisplaySettingsAction::Apply(settings));
    probe.phase += 1;
    probe.since = now;
    probe.samples.clear();
}
