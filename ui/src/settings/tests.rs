use super::*;
fn english() -> Localization {
    let mut i18n = Localization::default();
    i18n.set_preference(LanguagePreference::Locale(Locale::EN_US));
    i18n
}

fn capabilities() -> DisplayCapabilities {
    DisplayCapabilities {
        monitor: Some(Entity::from_raw_u32(1).unwrap()),
        desktop_resolution: Some(UVec2::new(1920, 1080)),
        video_modes: vec![bevy::window::VideoMode {
            physical_size: UVec2::new(1920, 1080),
            bit_depth: 32,
            refresh_rate_millihertz: 144000,
        }],
    }
}

fn frame(
    ctx: &egui::Context,
    dialog: &mut SettingsDialog,
    size: egui::Vec2,
    events: Vec<egui::Event>,
) -> (egui::FullOutput, Option<DisplaySettingsAction>) {
    let mut action = None;
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            events,
            ..default()
        },
        |ui| {
            action = paint_settings(
                ui.ctx(),
                dialog,
                &DisplaySettingsState::load(None),
                &capabilities(),
                &mut english(),
                &mut UiPreferences::load(None),
            );
        },
    );
    output.textures_delta.clear();
    (output, action)
}

fn text_position(output: &egui::FullOutput, label: &str) -> egui::Pos2 {
    output
        .shapes
        .iter()
        .find_map(|shape| {
            if let egui::Shape::Text(text) = &shape.shape {
                (text.galley.job.text == label).then_some(text.pos + text.galley.size() * 0.5)
            } else {
                None
            }
        })
        .unwrap_or_else(|| panic!("Missing label: {label}"))
}

fn click(
    ctx: &egui::Context,
    dialog: &mut SettingsDialog,
    label: &str,
) -> Option<DisplaySettingsAction> {
    let size = egui::vec2(1280.0, 720.0);
    let (output, _) = frame(ctx, dialog, size, vec![]);
    let position = text_position(&output, label);
    output.drop_without_applying_deltas();
    for pressed in [true, false] {
        let (output, action) = frame(
            ctx,
            dialog,
            size,
            vec![
                egui::Event::PointerMoved(position),
                egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: default(),
                },
            ],
        );
        output.drop_without_applying_deltas();
        if action.is_some() {
            return action;
        }
    }
    None
}

#[test]
fn settings_fit_small_window_and_apply_unlimited_through_real_widgets() {
    let ctx = egui::Context::default();
    let mut dialog = SettingsDialog::default();
    dialog.open(&DisplaySettingsState::load(None));
    for size in [egui::vec2(1280.0, 720.0), egui::vec2(640.0, 360.0)] {
        for _ in 0..3 {
            frame(&ctx, &mut dialog, size, vec![])
                .0
                .drop_without_applying_deltas();
        }
        {
            let (output, _) = frame(&ctx, &mut dialog, size, vec![]);
            let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
            let panel = output
                .shapes
                .iter()
                .find_map(|shape| {
                    if let egui::Shape::Rect(rect) = &shape.shape {
                        (rect.corner_radius.nw == 16).then_some(rect.rect)
                    } else {
                        None
                    }
                })
                .unwrap();
            assert!(
                viewport.contains_rect(panel),
                "Settings panel exceeds viewport: {panel:?}"
            );
            assert!(viewport.contains(text_position(&output, "Apply")));
            assert!(viewport.contains(text_position(&output, "Back to Title")));
            output.drop_without_applying_deltas();
        }
    }
    for _ in 0..3 {
        frame(&ctx, &mut dialog, egui::vec2(1280.0, 720.0), vec![])
            .0
            .drop_without_applying_deltas();
    }
    click(&ctx, &mut dialog, "Unlimited");
    assert!(dialog.draft.max_fps.is_none());
    let Some(DisplaySettingsAction::Apply(settings)) = click(&ctx, &mut dialog, "Apply") else {
        panic!("Apply must emit a settings request");
    };
    assert!(settings.max_fps.is_none());
    assert_eq!(settings.mode, ScreenMode::Windowed);
    click(&ctx, &mut dialog, "Back to Title");
    assert!(!dialog.open);
}

#[test]
fn escape_discards_unapplied_edits() {
    let ctx = egui::Context::default();
    let mut dialog = SettingsDialog::default();
    dialog.open(&DisplaySettingsState::load(None));
    frame(&ctx, &mut dialog, egui::vec2(1280.0, 720.0), vec![])
        .0
        .drop_without_applying_deltas();
    dialog.draft.max_fps = None;
    let (output, action) = frame(
        &ctx,
        &mut dialog,
        egui::vec2(1280.0, 720.0),
        vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: default(),
        }],
    );
    output.drop_without_applying_deltas();
    assert!(action.is_none());
    assert!(!dialog.open);
    dialog.open(&DisplaySettingsState::load(None));
    assert_eq!(dialog.draft.max_fps, Some(60));
}

fn localized_frame(
    ctx: &egui::Context,
    dialog: &mut SettingsDialog,
    preferences: &mut UiPreferences,
    i18n: &mut Localization,
    size: egui::Vec2,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            events,
            ..default()
        },
        |ui| {
            let _ = paint_settings(
                ui.ctx(),
                dialog,
                &DisplaySettingsState::load(None),
                &capabilities(),
                i18n,
                preferences,
            );
        },
    )
}

#[test]
fn language_widgets_persist_selection_and_update_next_frame_without_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ui-settings.json");
    let mut preferences = UiPreferences::load(Some(path.clone()));
    let mut i18n = crate::localization::tests::english();
    let ctx = egui::Context::default();
    let mut dialog = SettingsDialog::default();
    dialog.open(&DisplaySettingsState::load(None));
    let size = egui::vec2(1280.0, 720.0);
    for label in ["Automatic", "日本語"] {
        for _ in 0..3 {
            localized_frame(&ctx, &mut dialog, &mut preferences, &mut i18n, size, vec![])
                .drop_without_applying_deltas();
        }
        let output = localized_frame(&ctx, &mut dialog, &mut preferences, &mut i18n, size, vec![]);
        let pos = text_position(&output, label);
        output.drop_without_applying_deltas();
        for pressed in [true, false] {
            localized_frame(
                &ctx,
                &mut dialog,
                &mut preferences,
                &mut i18n,
                size,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: default(),
                    },
                ],
            )
            .drop_without_applying_deltas();
        }
    }
    assert_eq!(preferences.language, LanguagePreference::Locale(Locale::JA));
    assert_eq!(
        UiPreferences::load(Some(path)).language,
        preferences.language
    );
    let output = localized_frame(&ctx, &mut dialog, &mut preferences, &mut i18n, size, vec![]);
    text_position(&output, "設定");
    text_position(&output, "適用");
    assert!(dialog.open);
    output.drop_without_applying_deltas();
}

#[test]
fn japanese_settings_fit_small_windows_with_actions_visible() {
    let ctx = egui::Context::default();
    let mut dialog = SettingsDialog::default();
    dialog.open(&DisplaySettingsState::load(None));
    let mut preferences = UiPreferences::load(None);
    let mut i18n = english();
    preferences.set_language(LanguagePreference::Locale(Locale::JA), &mut i18n);
    for size in [
        egui::vec2(1280.0, 720.0),
        egui::vec2(640.0, 360.0),
        egui::vec2(320.0, 360.0),
    ] {
        for _ in 0..3 {
            localized_frame(&ctx, &mut dialog, &mut preferences, &mut i18n, size, vec![])
                .drop_without_applying_deltas();
        }
        let output = localized_frame(&ctx, &mut dialog, &mut preferences, &mut i18n, size, vec![]);
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
        let panel = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Rect(rect) if rect.corner_radius.nw == 16 => Some(rect.rect),
                _ => None,
            })
            .unwrap();
        assert!(
            viewport.contains_rect(panel),
            "Japanese panel outside viewport {panel:?}"
        );
        for label in ["設定", "適用", "タイトルへ戻る"] {
            assert!(viewport.contains(text_position(&output, label)));
        }
        output.drop_without_applying_deltas();
    }
}

/// Render the real game menus and save screenshots without touching user settings.
#[test]
#[ignore = "requires a native window and GPU"]
fn native_settings_ui_probe() {
    use bevy::{
        render::view::screenshot::{save_to_disk, Screenshot},
        winit::{WinitPlugin, WinitSettings},
    };
    use std::time::{Duration, Instant};
    fn screenshot_path(name: &str) -> std::path::PathBuf {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }
    #[derive(Resource)]
    struct Probe {
        start: Instant,
        phase: usize,
    }
    #[allow(clippy::too_many_arguments)]
    fn advance(
        mut probe: ResMut<Probe>,
        mut dialog: ResMut<SettingsDialog>,
        state: Res<DisplaySettingsState>,
        mut actions: MessageWriter<DisplaySettingsAction>,
        mut commands: Commands,
        mut exit: MessageWriter<AppExit>,
        mut preferences: ResMut<UiPreferences>,
        mut i18n: ResMut<Localization>,
        mut next_app: ResMut<NextState<puzzella_game::resources::AppState>>,
    ) {
        if probe.start.elapsed() < Duration::from_secs(probe.phase as u64 * 2) {
            return;
        }
        match probe.phase {
            0 => dialog.open(&state),
            1 => {
                commands
                    .spawn(Screenshot::primary_window())
                    .observe(save_to_disk(screenshot_path("settings-ui-default.png")));
            }
            2 => {
                actions.write(DisplaySettingsAction::Apply(DisplaySettings {
                    resolution: UVec2::new(640, 360),
                    ..default()
                }));
            }
            3 => {
                commands
                    .spawn(Screenshot::primary_window())
                    .observe(save_to_disk(screenshot_path(
                        "settings-ui-confirm-small.png",
                    )));
            }
            4 => {
                actions.write(DisplaySettingsAction::Revert);
            }
            5 => {
                dialog.draft.mode = ScreenMode::Borderless;
                dialog.draft.max_fps = None;
                commands
                    .spawn(Screenshot::primary_window())
                    .observe(save_to_disk(screenshot_path("settings-ui-borderless.png")));
            }
            6 => {
                preferences.set_language(LanguagePreference::Locale(Locale::JA), &mut i18n);
                dialog.open = false;
            }
            7 => {
                commands
                    .spawn(Screenshot::primary_window())
                    .observe(save_to_disk(screenshot_path("title-ui-ja.png")));
            }
            8 => dialog.open(&state),
            9 => {
                commands
                    .spawn(Screenshot::primary_window())
                    .observe(save_to_disk(screenshot_path("settings-ui-ja.png")));
            }
            10 => {
                actions.write(DisplaySettingsAction::Apply(DisplaySettings {
                    resolution: UVec2::new(640, 360),
                    ..default()
                }));
            }
            11 => {
                commands
                    .spawn(Screenshot::primary_window())
                    .observe(save_to_disk(screenshot_path(
                        "settings-ui-confirm-ja-small.png",
                    )));
            }
            12 => {
                actions.write(DisplaySettingsAction::Revert);
                next_app.set(puzzella_game::resources::AppState::GameSetup);
            }
            13 => {
                commands
                    .spawn(Screenshot::primary_window())
                    .observe(save_to_disk(screenshot_path("setup-ui-ja.png")));
            }
            14 => {
                exit.write(AppExit::Success);
            }
            _ => {}
        }
        probe.phase += 1;
    }
    let started = std::time::SystemTime::now();
    App::new()
        .add_plugins(DefaultPlugins.set(WinitPlugin {
            run_on_any_thread: true,
        }))
        .insert_resource(DisplaySettingsState::load(None))
        .insert_resource(UiPreferences::load(None))
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
    for name in [
        "settings-ui-default.png",
        "settings-ui-confirm-small.png",
        "settings-ui-borderless.png",
        "title-ui-ja.png",
        "settings-ui-ja.png",
        "settings-ui-confirm-ja-small.png",
        "setup-ui-ja.png",
    ] {
        let metadata = std::fs::metadata(screenshot_path(name)).expect("native screenshot saved");
        assert!(metadata.len() > 0);
        assert!(
            metadata.modified().unwrap() >= started,
            "screenshot was not refreshed: {name}"
        );
    }
}
