use super::*;

fn test_app(path: Option<PathBuf>) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .insert_resource(DisplaySettingsState::load(path))
        .add_plugins(DisplaySettingsPlugin);
    app.world_mut().spawn((Window::default(), PrimaryWindow));
    app.world_mut().spawn((
        Monitor {
            name: Some("Test display".into()),
            physical_width: 1920,
            physical_height: 1080,
            physical_position: IVec2::ZERO,
            refresh_rate_millihertz: Some(144000),
            scale_factor: 1.5,
            video_modes: vec![
                VideoMode {
                    physical_size: UVec2::new(1280, 720),
                    bit_depth: 32,
                    refresh_rate_millihertz: 60000,
                },
                VideoMode {
                    physical_size: UVec2::new(1920, 1080),
                    bit_depth: 32,
                    refresh_rate_millihertz: 60000,
                },
                VideoMode {
                    physical_size: UVec2::new(1920, 1080),
                    bit_depth: 32,
                    refresh_rate_millihertz: 144000,
                },
            ],
        },
        PrimaryMonitor,
    ));
    app.update();
    app
}

fn action(app: &mut App, action: DisplaySettingsAction) {
    app.world_mut().write_message(action);
    app.update();
}

#[test]
fn unchanged_settings_do_not_write_or_show_a_completion_notice() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let mut app = test_app(Some(path.clone()));
    action(
        &mut app,
        DisplaySettingsAction::Apply(DisplaySettings::default()),
    );
    assert!(!path.exists());
    assert!(app
        .world()
        .resource::<DisplaySettingsState>()
        .notice
        .is_none());
    let borderless = DisplaySettings {
        mode: ScreenMode::Borderless,
        ..default()
    };
    action(&mut app, DisplaySettingsAction::Apply(borderless.clone()));
    action(&mut app, DisplaySettingsAction::Keep);
    action(&mut app, DisplaySettingsAction::Dismiss);
    action(
        &mut app,
        DisplaySettingsAction::Apply(DisplaySettings {
            resolution: UVec2::new(1920, 1080),
            ..borderless.clone()
        }),
    );
    let state = app.world().resource::<DisplaySettingsState>();
    assert_eq!(state.current, borderless);
    assert!(state.notice.is_none());
    assert_eq!(DisplaySettingsState::load(Some(path)).current, borderless);
}

#[test]
fn dismiss_clears_notices_and_reverts_only_unconfirmed_changes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let mut app = test_app(Some(path.clone()));
    let saved = DisplaySettings {
        max_fps: None,
        ..default()
    };
    action(&mut app, DisplaySettingsAction::Apply(saved.clone()));
    assert_eq!(
        app.world().resource::<DisplaySettingsState>().notice,
        Some(DisplaySettingsNotice::Saved)
    );
    action(&mut app, DisplaySettingsAction::Dismiss);
    let state = app.world().resource::<DisplaySettingsState>();
    assert!(state.notice.is_none());
    assert_eq!(state.current, saved);
    action(
        &mut app,
        DisplaySettingsAction::Apply(DisplaySettings {
            resolution: UVec2::new(800, 600),
            ..saved.clone()
        }),
    );
    assert!(app
        .world()
        .resource::<DisplaySettingsState>()
        .confirmation_seconds()
        .is_some());
    action(&mut app, DisplaySettingsAction::Dismiss);
    let state = app.world().resource::<DisplaySettingsState>();
    assert!(state.notice.is_none());
    assert!(state.confirmation_seconds().is_none());
    assert_eq!(state.current, saved);
    assert_eq!(DisplaySettingsState::load(Some(path)).current, saved);
}

#[test]
fn a_failed_fps_save_can_be_retried_without_changing_settings() {
    let dir = tempfile::tempdir().unwrap();
    let blocked = dir.path().join("not-a-directory");
    std::fs::write(&blocked, b"preserve me").unwrap();
    let mut app = test_app(Some(blocked.join("settings.json")));
    let settings = DisplaySettings {
        max_fps: None,
        ..default()
    };
    action(&mut app, DisplaySettingsAction::Apply(settings.clone()));
    action(&mut app, DisplaySettingsAction::Dismiss);
    let state = app.world().resource::<DisplaySettingsState>();
    assert!(matches!(
        state.error,
        Some(DisplaySettingsError::SaveFailed(_))
    ));
    assert!(state.can_apply(&settings, app.world().resource::<DisplayCapabilities>()));
    std::fs::remove_file(&blocked).unwrap();
    action(&mut app, DisplaySettingsAction::Apply(settings.clone()));
    let state = app.world().resource::<DisplaySettingsState>();
    assert!(state.error.is_none());
    assert!(!state.can_apply(&settings, app.world().resource::<DisplayCapabilities>()));
    assert_eq!(
        DisplaySettingsState::load(Some(blocked.join("settings.json"))).current,
        settings
    );
}

#[test]
fn modes_use_supported_fullscreen_video_and_restore_physical_window_size() {
    let mut app = test_app(None);
    let settings = DisplaySettings {
        resolution: UVec2::new(1920, 1080),
        mode: ScreenMode::Fullscreen,
        max_fps: None,
    };
    action(&mut app, DisplaySettingsAction::Apply(settings));
    let window = app
        .world_mut()
        .query_filtered::<&Window, With<PrimaryWindow>>()
        .single(app.world())
        .unwrap();
    let WindowMode::Fullscreen(_, VideoModeSelection::Specific(video)) = window.mode else {
        panic!("Expected exclusive fullscreen");
    };
    assert_eq!(video.physical_size, UVec2::new(1920, 1080));
    assert_eq!(video.refresh_rate_millihertz, 144000);
    assert_eq!(window.present_mode, PresentMode::AutoNoVsync);
    action(&mut app, DisplaySettingsAction::Keep);
    let borderless = DisplaySettings {
        mode: ScreenMode::Borderless,
        ..app
            .world()
            .resource::<DisplaySettingsState>()
            .current
            .clone()
    };
    action(&mut app, DisplaySettingsAction::Apply(borderless));
    assert!(matches!(
        app.world_mut()
            .query::<&Window>()
            .single(app.world())
            .unwrap()
            .mode,
        WindowMode::BorderlessFullscreen(_)
    ));
    action(&mut app, DisplaySettingsAction::Keep);
    let windowed = DisplaySettings {
        resolution: UVec2::new(800, 600),
        ..default()
    };
    action(&mut app, DisplaySettingsAction::Apply(windowed));
    let window = app
        .world_mut()
        .query::<&Window>()
        .single(app.world())
        .unwrap();
    assert_eq!(window.mode, WindowMode::Windowed);
    assert_eq!(window.resolution.physical_size(), UVec2::new(800, 600));
}

#[test]
fn unsupported_fullscreen_is_rejected_and_startup_falls_back() {
    let mut app = test_app(None);
    let unsupported = DisplaySettings {
        mode: ScreenMode::Fullscreen,
        resolution: UVec2::new(1234, 567),
        ..default()
    };
    action(&mut app, DisplaySettingsAction::Apply(unsupported.clone()));
    assert_eq!(
        app.world().resource::<DisplaySettingsState>().current,
        DisplaySettings::default()
    );
    assert!(app
        .world()
        .resource::<DisplaySettingsState>()
        .error
        .is_some());
    app.world_mut()
        .resource_mut::<DisplaySettingsState>()
        .current = unsupported;
    app.update();
    assert_eq!(
        app.world().resource::<DisplaySettingsState>().current.mode,
        ScreenMode::Windowed
    );
}

#[test]
fn preview_is_not_persisted_until_confirmed_and_timeout_restores_previous() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let mut app = test_app(Some(path.clone()));
    let settings = DisplaySettings {
        resolution: UVec2::new(800, 600),
        max_fps: Some(144),
        ..default()
    };
    action(&mut app, DisplaySettingsAction::Apply(settings.clone()));
    assert!(!path.exists());
    app.world_mut()
        .resource_mut::<DisplaySettingsState>()
        .preview
        .as_mut()
        .unwrap()
        .deadline = Instant::now() - Duration::from_secs(1);
    app.update();
    assert_eq!(
        app.world().resource::<DisplaySettingsState>().current,
        DisplaySettings::default()
    );
    action(&mut app, DisplaySettingsAction::Apply(settings.clone()));
    action(&mut app, DisplaySettingsAction::Keep);
    assert_eq!(
        DisplaySettingsState::load(Some(path.clone())).current,
        settings
    );
    action(
        &mut app,
        DisplaySettingsAction::Apply(DisplaySettings::default()),
    );
    action(&mut app, DisplaySettingsAction::Revert);
    assert_eq!(
        app.world().resource::<DisplaySettingsState>().current,
        settings
    );
    assert_eq!(DisplaySettingsState::load(Some(path)).current, settings);
}

#[test]
fn fps_only_changes_save_immediately_and_invalid_config_uses_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let mut app = test_app(Some(path.clone()));
    let unlimited = DisplaySettings {
        max_fps: None,
        ..default()
    };
    action(&mut app, DisplaySettingsAction::Apply(unlimited.clone()));
    assert!(app
        .world()
        .resource::<DisplaySettingsState>()
        .confirmation_seconds()
        .is_none());
    assert_eq!(
        DisplaySettingsState::load(Some(path.clone())).current,
        unlimited
    );
    std::fs::write(&path, b"{broken").unwrap();
    let loaded = DisplaySettingsState::load(Some(path.clone()));
    assert_eq!(loaded.current, DisplaySettings::default());
    assert!(loaded.error.is_some());
    let invalid = DisplaySettings {
        max_fps: Some(0),
        ..default()
    };
    std::fs::write(&path, serde_json::to_vec(&invalid).unwrap()).unwrap();
    assert_eq!(
        DisplaySettingsState::load(Some(path)).current,
        DisplaySettings::default()
    );
}

#[test]
fn failed_save_preserves_previous_config_and_allows_revert() {
    let dir = tempfile::tempdir().unwrap();
    let blocked = dir.path().join("not-a-directory");
    std::fs::write(&blocked, b"preserve me").unwrap();
    let mut app = test_app(Some(blocked.join("settings.json")));
    action(
        &mut app,
        DisplaySettingsAction::Apply(DisplaySettings {
            resolution: UVec2::new(800, 600),
            ..default()
        }),
    );
    action(&mut app, DisplaySettingsAction::Keep);
    assert!(matches!(
        app.world()
            .resource::<DisplaySettingsState>()
            .error
            .as_ref()
            .unwrap(),
        DisplaySettingsError::SaveFailed(_)
    ));
    assert!(app
        .world()
        .resource::<DisplaySettingsState>()
        .confirmation_seconds()
        .is_some());
    action(&mut app, DisplaySettingsAction::Revert);
    assert_eq!(std::fs::read(blocked).unwrap(), b"preserve me");
    assert_eq!(
        app.world().resource::<DisplaySettingsState>().current,
        DisplaySettings::default()
    );
}

#[test]
fn frame_pacing_counts_work_and_never_catches_up_with_extra_frames() {
    let start = Instant::now();
    let interval = Duration::from_millis(10);
    let mut pacer = FramePacer::default();
    assert!(pacer.remaining(start, Some(interval)).is_zero());
    pacer.last_start = Some(start);
    assert_eq!(
        pacer.remaining(start + Duration::from_millis(3), Some(interval)),
        Duration::from_millis(7)
    );
    assert!(pacer
        .remaining(start + Duration::from_millis(50), Some(interval))
        .is_zero());
    assert!(pacer.remaining(start, None).is_zero());
    assert!(pacer.last_start.is_none());
    assert!(pacer
        .remaining(start, Some(Duration::from_millis(20)))
        .is_zero());
}

#[test]
fn fps_changes_preserve_a_manually_resized_window() {
    let mut app = test_app(None);
    app.world_mut()
        .query::<&mut Window>()
        .single_mut(app.world_mut())
        .unwrap()
        .resolution
        .set_physical_resolution(900, 600);
    action(
        &mut app,
        DisplaySettingsAction::Apply(DisplaySettings {
            max_fps: Some(144),
            ..default()
        }),
    );
    assert_eq!(
        app.world_mut()
            .query::<&Window>()
            .single(app.world())
            .unwrap()
            .physical_size(),
        UVec2::new(900, 600)
    );
}

#[test]
fn resolution_choices_are_unique_and_fullscreen_is_monitor_specific() {
    let mut app = test_app(None);
    let caps = app.world().resource::<DisplayCapabilities>();
    assert_eq!(
        caps.resolutions(ScreenMode::Fullscreen, UVec2::new(1234, 567)),
        vec![UVec2::new(1280, 720), UVec2::new(1920, 1080)]
    );
    assert!(!caps
        .resolutions(ScreenMode::Windowed, UVec2::new(1280, 720))
        .contains(&UVec2::new(3840, 2160)));
    // Moving to a monitor with different modes changes the available options.
    let other = app
        .world_mut()
        .spawn(Monitor {
            name: None,
            physical_width: 2560,
            physical_height: 1440,
            physical_position: IVec2::new(1920, 0),
            refresh_rate_millihertz: None,
            scale_factor: 1.0,
            video_modes: vec![],
        })
        .id();
    let window = app
        .world_mut()
        .query_filtered::<Entity, With<PrimaryWindow>>()
        .single(app.world())
        .unwrap();
    app.world_mut().entity_mut(window).insert(OnMonitor(other));
    app.update();
    assert_eq!(
        app.world().resource::<DisplayCapabilities>().monitor,
        Some(other)
    );
    assert!(app
        .world()
        .resource::<DisplayCapabilities>()
        .resolutions(ScreenMode::Fullscreen, UVec2::new(1280, 720))
        .is_empty());
}
