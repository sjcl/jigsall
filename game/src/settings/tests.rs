use super::*;

#[test]
fn piece_quality_defaults_and_json_round_trips_preserve_display_fields() {
    let legacy =
        r#"{"resolution":[800,600],"mode":"Windowed","max_fps":144,"game_background":"Dark"}"#;
    let mut settings: DisplaySettings = serde_json::from_str(legacy).unwrap();
    assert_eq!(settings.piece_visual_quality, PieceVisualQuality::High);
    assert_eq!(settings.resolution, UVec2::new(800, 600));
    assert_eq!(settings.max_fps, Some(144));
    assert_eq!(settings.game_background, GameBackground::Dark);
    for (quality, name) in [
        (PieceVisualQuality::Low, "Low"),
        (PieceVisualQuality::Medium, "Medium"),
        (PieceVisualQuality::High, "High"),
    ] {
        settings.piece_visual_quality = quality;
        let value = serde_json::to_value(&settings).unwrap();
        assert_eq!(value["piece_visual_quality"], name);
        assert_eq!(
            serde_json::from_value::<DisplaySettings>(value).unwrap(),
            settings
        );
        assert_eq!(
            serde_json::to_string(&quality).unwrap(),
            format!("\"{name}\"")
        );
        assert_eq!(
            serde_json::from_str::<PieceVisualQuality>(&format!("\"{name}\"")).unwrap(),
            quality
        );
    }
}

#[test]
fn piece_quality_only_apply_updates_renderer_and_saves_without_preview() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    std::fs::write(
        &path,
        r#"{"display":{"resolution":[800,600],"mode":"Windowed","max_fps":144,"game_background":"Dark"},"future":{"value":42}}"#,
    ).unwrap();
    let mut app = test_app(Some(path.clone()));
    let initial = app
        .world()
        .resource::<DisplaySettingsState>()
        .current
        .clone();
    assert_eq!(initial.piece_visual_quality, PieceVisualQuality::High);
    assert_eq!(
        *app.world().resource::<PieceVisualQuality>(),
        PieceVisualQuality::High
    );
    for quality in [
        PieceVisualQuality::Low,
        PieceVisualQuality::Medium,
        PieceVisualQuality::High,
    ] {
        let settings = DisplaySettings {
            piece_visual_quality: quality,
            ..initial.clone()
        };
        assert!(app
            .world()
            .resource::<DisplaySettingsState>()
            .can_apply(&settings, app.world().resource::<DisplayCapabilities>()));
        action(&mut app, DisplaySettingsAction::Apply(settings.clone()));
        let state = app.world().resource::<DisplaySettingsState>();
        assert_eq!(state.current, settings);
        assert_eq!(state.confirmation_seconds(), None);
        assert_eq!(state.notice, Some(DisplaySettingsNotice::Saved));
        assert!(!state.can_apply(&settings, app.world().resource::<DisplayCapabilities>()));
        assert_eq!(*app.world().resource::<PieceVisualQuality>(), quality);
        let loaded = DisplaySettingsState::load(Some(path.clone()));
        assert!(loaded.error.is_none());
        assert_eq!(loaded.current, settings);
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["future"]["value"], 42);
        let window = app
            .world_mut()
            .query_filtered::<&Window, With<PrimaryWindow>>()
            .single(app.world())
            .unwrap();
        assert_eq!(window.mode, WindowMode::Windowed);
        assert_eq!(window.resolution.physical_size(), initial.resolution);
        let changed = app
            .world()
            .resource_ref::<PieceVisualQuality>()
            .last_changed();
        app.update();
        assert_eq!(
            app.world()
                .resource_ref::<PieceVisualQuality>()
                .last_changed(),
            changed
        );
    }
    let mut restarted = test_app(Some(path));
    restarted.update();
    assert_eq!(
        *restarted.world().resource::<PieceVisualQuality>(),
        PieceVisualQuality::High
    );
}

#[test]
fn piece_quality_display_preview_reverts_renderer_and_only_keep_persists() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let mut app = test_app(Some(path.clone()));
    let previous = DisplaySettings {
        piece_visual_quality: PieceVisualQuality::Medium,
        ..default()
    };
    action(&mut app, DisplaySettingsAction::Apply(previous.clone()));
    let draft = DisplaySettings {
        resolution: UVec2::new(800, 600),
        piece_visual_quality: PieceVisualQuality::Low,
        ..previous.clone()
    };
    for end in [
        DisplaySettingsAction::Revert,
        DisplaySettingsAction::Dismiss,
    ] {
        action(&mut app, DisplaySettingsAction::Apply(draft.clone()));
        assert!(app
            .world()
            .resource::<DisplaySettingsState>()
            .confirmation_seconds()
            .is_some());
        assert_eq!(
            *app.world().resource::<PieceVisualQuality>(),
            PieceVisualQuality::Low
        );
        assert_eq!(
            DisplaySettingsState::load(Some(path.clone())).current,
            previous
        );
        action(&mut app, end);
        assert_eq!(
            app.world().resource::<DisplaySettingsState>().current,
            previous
        );
        assert_eq!(
            *app.world().resource::<PieceVisualQuality>(),
            PieceVisualQuality::Medium
        );
    }
    action(&mut app, DisplaySettingsAction::Apply(draft.clone()));
    app.world_mut()
        .resource_mut::<DisplaySettingsState>()
        .preview
        .as_mut()
        .unwrap()
        .deadline = Instant::now() - Duration::from_secs(1);
    app.update();
    assert_eq!(
        app.world().resource::<DisplaySettingsState>().current,
        previous
    );
    assert_eq!(
        *app.world().resource::<PieceVisualQuality>(),
        PieceVisualQuality::Medium
    );
    action(&mut app, DisplaySettingsAction::Apply(draft.clone()));
    action(&mut app, DisplaySettingsAction::Keep);
    assert_eq!(
        DisplaySettingsState::load(Some(path.clone())).current,
        draft
    );
    let restarted = test_app(Some(path));
    assert_eq!(
        *restarted.world().resource::<PieceVisualQuality>(),
        PieceVisualQuality::Low
    );
}

#[test]
fn background_defaults_for_existing_settings_and_applies_to_main_camera() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    std::fs::write(
        &path,
        r#"{"display":{"resolution":[800,600],"mode":"Windowed","max_fps":144}}"#,
    )
    .unwrap();
    let mut app = test_app(Some(path.clone()));
    let main = app
        .world_mut()
        .spawn((Camera::default(), crate::components::MainCamera))
        .id();
    let other = app.world_mut().spawn(Camera::default()).id();
    app.update();
    let initial = app.world().resource::<DisplaySettingsState>();
    assert!(initial.error.is_none());
    assert_eq!(initial.current.resolution, UVec2::new(800, 600));
    assert_eq!(initial.current.max_fps, Some(144));
    assert_eq!(initial.current.game_background, GameBackground::Light);
    assert!(matches!(
        app.world().get::<Camera>(main).unwrap().clear_color,
        bevy::camera::ClearColorConfig::Custom(color) if color == GameBackground::Light.color()
    ));
    assert_eq!(GameBackground::Dark.color(), ClearColor::default().0);
    let mut settings = initial.current.clone();
    settings.game_background = GameBackground::Dark;
    action(&mut app, DisplaySettingsAction::Apply(settings.clone()));
    assert!(app
        .world()
        .resource::<DisplaySettingsState>()
        .confirmation_seconds()
        .is_none());
    assert!(matches!(
        app.world().get::<Camera>(main).unwrap().clear_color,
        bevy::camera::ClearColorConfig::Custom(color) if color == GameBackground::Dark.color()
    ));
    assert!(matches!(
        app.world().get::<Camera>(other).unwrap().clear_color,
        bevy::camera::ClearColorConfig::Default
    ));
    assert_eq!(
        DisplaySettingsState::load(Some(path.clone())).current,
        settings
    );
    settings.game_background = GameBackground::Light;
    action(&mut app, DisplaySettingsAction::Apply(settings.clone()));
    assert_eq!(DisplaySettingsState::load(Some(path)).current, settings);
    assert!(matches!(
        app.world().get::<Camera>(main).unwrap().clear_color,
        bevy::camera::ClearColorConfig::Custom(color) if color == GameBackground::Light.color()
    ));
}

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
    // Waiting is confined to the test fixture; runtime only polls once per frame.
    crate::settings_file::wait_for_save(|| {
        let mut state = app.world_mut().resource_mut::<DisplaySettingsState>();
        state.poll_save();
        state.is_save_pending()
    });
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
        ..default()
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
fn fps_only_changes_request_save_without_confirmation_and_invalid_config_uses_defaults() {
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
    std::fs::write(
        &path,
        serde_json::to_vec(&serde_json::json!({"display": invalid})).unwrap(),
    )
    .unwrap();
    assert_eq!(
        DisplaySettingsState::load(Some(path)).current,
        DisplaySettings::default()
    );
}

#[test]
fn saving_other_sections_during_display_preview_preserves_confirmed_settings() {
    use crate::{
        keybindings::{KeyAction, KeyBindingsState},
        persistence::autosave::AutosaveSettingsState,
    };
    use serde_json::json;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let mut app = test_app(Some(path.clone()));
    // All resources load before any save, as they do at application startup.
    let mut autosave = AutosaveSettingsState::load(Some(path.clone()));
    let mut keys = KeyBindingsState::load(Some(path.clone()));
    let mut file = SettingsFile::new(Some(path.clone()));
    let confirmed = DisplaySettings {
        max_fps: Some(144),
        ..default()
    };
    action(&mut app, DisplaySettingsAction::Apply(confirmed.clone()));
    let preview = DisplaySettings {
        resolution: UVec2::new(800, 600),
        ..confirmed.clone()
    };
    action(&mut app, DisplaySettingsAction::Apply(preview.clone()));
    file.save(SettingsSection::Preferences, &json!({"language": "ja"}))
        .unwrap();
    autosave.set_interval(None);
    let mut bindings = keys.current.clone();
    bindings.binding_mut(KeyAction::Performance).primary = None;
    keys.apply(bindings.clone());
    crate::settings_file::wait_for_save(|| {
        autosave.poll_save();
        keys.poll_save();
        assert!(file.poll_save().is_none_or(|result| result.is_ok()));
        autosave.is_save_pending() || keys.is_save_pending() || file.is_save_pending()
    });
    assert!(autosave.error.is_none());
    assert!(keys.error.is_none());
    assert_eq!(
        DisplaySettingsState::load(Some(path.clone())).current,
        confirmed
    );
    action(&mut app, DisplaySettingsAction::Revert);
    assert_eq!(
        app.world().resource::<DisplaySettingsState>().current,
        confirmed
    );

    action(&mut app, DisplaySettingsAction::Apply(preview.clone()));
    action(&mut app, DisplaySettingsAction::Keep);
    file.save(SettingsSection::Preferences, &json!({"language": "en-US"}))
        .unwrap();
    crate::settings_file::wait_for_save(|| {
        assert!(file.poll_save().is_none_or(|result| result.is_ok()));
        file.is_save_pending()
    });
    assert_eq!(
        DisplaySettingsState::load(Some(path.clone())).current,
        preview
    );
    assert_eq!(
        AutosaveSettingsState::load(Some(path.clone()))
            .current
            .interval_minutes,
        None
    );
    assert_eq!(KeyBindingsState::load(Some(path.clone())).current, bindings);
    let (preferences, error) = file.load::<serde_json::Value>(SettingsSection::Preferences);
    assert_eq!(preferences, json!({"language": "en-US"}));
    assert!(error.is_none());
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
fn confirmed_display_does_not_timeout_or_report_saved_while_pending() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let mut app = test_app(None);
    let (file, resume) = SettingsFile::paused(path.clone());
    app.world_mut().resource_mut::<DisplaySettingsState>().file = file;
    let confirmed = DisplaySettings {
        resolution: UVec2::new(800, 600),
        ..default()
    };
    action(&mut app, DisplaySettingsAction::Apply(confirmed.clone()));
    app.world_mut().write_message(DisplaySettingsAction::Keep);
    app.update();
    {
        let mut state = app.world_mut().resource_mut::<DisplaySettingsState>();
        assert!(state.is_save_pending());
        assert!(state.notice.is_none());
        assert!(state.confirmation_seconds().is_none());
        state.saving_preview.as_mut().unwrap().deadline = Instant::now() - Duration::from_secs(1);
    }
    app.update();
    assert_eq!(
        app.world().resource::<DisplaySettingsState>().current,
        confirmed
    );
    assert!(!path.exists());
    resume.send(()).unwrap();
    crate::settings_file::wait_for_save(|| {
        app.update();
        app.world()
            .resource::<DisplaySettingsState>()
            .is_save_pending()
    });
    assert_eq!(
        app.world().resource::<DisplaySettingsState>().notice,
        Some(DisplaySettingsNotice::Saved)
    );
    assert_eq!(DisplaySettingsState::load(Some(path)).current, confirmed);
}

#[test]
fn dismissing_a_pending_display_save_suppresses_late_notices_and_confirmation() {
    for fail in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let mut app = test_app(None);
        let (file, resume) = SettingsFile::paused(path.clone());
        app.world_mut().resource_mut::<DisplaySettingsState>().file = file;
        let confirmed = DisplaySettings {
            resolution: UVec2::new(800, 600),
            ..default()
        };
        action(&mut app, DisplaySettingsAction::Apply(confirmed.clone()));
        app.world_mut().write_message(DisplaySettingsAction::Keep);
        app.update();
        app.world_mut()
            .write_message(DisplaySettingsAction::Dismiss);
        app.update();
        assert_eq!(
            app.world().resource::<DisplaySettingsState>().current,
            confirmed
        );
        if fail {
            std::fs::write(&path, b"{broken").unwrap();
        }
        resume.send(()).unwrap();
        crate::settings_file::wait_for_save(|| {
            app.update();
            app.world()
                .resource::<DisplaySettingsState>()
                .is_save_pending()
        });
        let state = app.world().resource::<DisplaySettingsState>();
        assert!(state.notice.is_none());
        assert!(state.confirmation_seconds().is_none());
        assert_eq!(state.current, confirmed);
        assert_eq!(state.error.is_some(), fail);
    }
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
