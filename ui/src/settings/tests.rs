use super::*;

#[test]
fn piece_quality_widgets_select_all_three_values_and_enable_apply() {
    let ctx = egui::Context::default();
    let mut dialog = SettingsDialog::default();
    dialog.open(&DisplaySettingsState::load(None));
    dialog.tab = SettingsTab::Graphics;
    assert_eq!(dialog.draft.piece_visual_quality, PieceVisualQuality::High);
    for (label, quality) in [
        ("Low", PieceVisualQuality::Low),
        ("Medium", PieceVisualQuality::Medium),
    ] {
        assert!(click(&ctx, &mut dialog, label).is_none());
        assert_eq!(dialog.draft.piece_visual_quality, quality);
        let Some(DisplaySettingsAction::Apply(settings)) = click(&ctx, &mut dialog, "Apply") else {
            panic!("Quality-only change must enable Apply");
        };
        assert_eq!(settings.piece_visual_quality, quality);
        assert_eq!(settings.resolution, DisplaySettings::default().resolution);
        assert_eq!(settings.mode, ScreenMode::Windowed);
    }
    assert!(click(&ctx, &mut dialog, "High").is_none());
    assert_eq!(dialog.draft.piece_visual_quality, PieceVisualQuality::High);
    assert!(click(&ctx, &mut dialog, "Apply").is_none());
    let mut state = DisplaySettingsState::load(None);
    state.current.piece_visual_quality = PieceVisualQuality::Low;
    dialog.open(&state);
    dialog.tab = SettingsTab::Graphics;
    assert!(click_with_state(&ctx, &mut dialog, &state, &capabilities(), "High").is_none());
    let Some(DisplaySettingsAction::Apply(settings)) =
        click_with_state(&ctx, &mut dialog, &state, &capabilities(), "Apply")
    else {
        panic!("High must be selectable from a saved Low quality");
    };
    assert_eq!(settings.piece_visual_quality, PieceVisualQuality::High);
    let mut i18n = english();
    for locale in [Locale::EN_US, Locale::JA] {
        i18n.set_preference(LanguagePreference::Locale(locale));
        for key in [
            "settings-piece-visual-quality",
            "settings-quality-low",
            "settings-quality-medium",
            "settings-quality-high",
        ] {
            assert_ne!(i18n.text(key), key);
        }
    }
}

#[test]
fn background_widgets_select_dark_and_light() {
    let ctx = egui::Context::default();
    let mut dialog = SettingsDialog::default();
    dialog.open(&DisplaySettingsState::load(None));
    dialog.tab = SettingsTab::Graphics;
    assert_eq!(dialog.draft.game_background, GameBackground::Dark);
    assert!(click(&ctx, &mut dialog, "Light").is_none());
    let Some(DisplaySettingsAction::Apply(settings)) = click(&ctx, &mut dialog, "Apply") else {
        panic!("Background change must enable Apply");
    };
    assert_eq!(settings.game_background, GameBackground::Light);
    assert!(click(&ctx, &mut dialog, "Dark").is_none());
    assert_eq!(dialog.draft.game_background, GameBackground::Dark);
    assert!(click(&ctx, &mut dialog, "Apply").is_none());
}

#[test]
fn player_name_draft_does_not_save_until_commit_and_errors_are_visible() {
    let mut state = PlayerSettingsState::load(None);
    let mut draft = Some("Alice".to_owned());
    let ctx = egui::Context::default();
    let render = |draft: &mut Option<String>, state: &mut PlayerSettingsState, events| {
        ctx.run_ui(
            egui::RawInput {
                events,
                ..default()
            },
            |ui| {
                paint_player_settings(
                    ui,
                    draft,
                    state,
                    &english(),
                    &english().text("settings-player-name"),
                );
            },
        )
    };
    for _ in 0..3 {
        render(&mut draft, &mut state, vec![]).drop_without_applying_deltas();
    }
    assert!(state.current.display_name.is_none());
    assert!(!state.is_save_pending());
    let output = render(&mut draft, &mut state, vec![]);
    let pos = text_position(&output, "Save name");
    output.drop_without_applying_deltas();
    for pressed in [true, false] {
        render(
            &mut draft,
            &mut state,
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
    assert_eq!(
        state.current.display_name.as_ref().unwrap().as_ref(),
        "Alice"
    );
    state.poll_save();
    draft = Some("x".repeat(33));
    assert!(!state.commit(draft.as_ref().unwrap()));
    let output = render(&mut draft, &mut state, vec![]);
    assert!(output.shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.job.text == english().text("settings-player-name-chars"))));
    assert_eq!(
        state.current.display_name.as_ref().unwrap().as_ref(),
        "Alice"
    );
    assert!(!state.is_save_pending());
    output.drop_without_applying_deltas();
}

#[test]
fn image_widgets_persist_auto_manual_percentage_and_show_next_load_help() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let mut settings = ImageSettingsState::load(Some(path.clone()));
    let limits = PuzzleImageLimits {
        device_max_dimension: 16384,
        gpu_memory_bytes: Some(8 * 1024 * 1024 * 1024),
    };
    let ctx = egui::Context::default();
    let render = |settings: &mut ImageSettingsState, events| {
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(640.0, 900.0),
                )),
                events,
                ..default()
            },
            |ui| {
                paint_image_settings(ui, settings, &limits, &english());
            },
        )
    };
    for _ in 0..3 {
        render(&mut settings, vec![]).drop_without_applying_deltas();
    }
    let click = |settings: &mut ImageSettingsState, label: &str| {
        let output = render(settings, vec![]);
        let pos = text_position(&output, label);
        output.drop_without_applying_deltas();
        for pressed in [true, false] {
            render(
                settings,
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
    };
    let output = render(&mut settings, vec![]);
    assert!(!has_text(&output, &english().text("settings-texture-auto")));
    assert!(!settings.is_save_pending());
    output.drop_without_applying_deltas();
    click(&mut settings, &english().text("settings-texture-budget"));
    // Finish the expanding animation before interacting with the controls.
    for _ in 0..30 {
        render(&mut settings, vec![]).drop_without_applying_deltas();
    }
    click(&mut settings, &english().text("settings-texture-auto"));
    assert_eq!(
        settings.current.texture_budget,
        TextureBudget::Manual { mib: 1638 }
    );
    // DragValue renders its numeric value and unit as separate text shapes.
    click(&mut settings, "1638");
    render(
        &mut settings,
        vec![
            egui::Event::Text("4096".into()),
            egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: default(),
            },
        ],
    )
    .drop_without_applying_deltas();
    assert_eq!(
        settings.current.texture_budget,
        TextureBudget::Manual { mib: 4096 }
    );
    click(&mut settings, &english().text("settings-texture-auto"));
    click(&mut settings, "20");
    render(
        &mut settings,
        vec![
            egui::Event::Text("10".into()),
            egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: default(),
            },
        ],
    )
    .drop_without_applying_deltas();
    assert_eq!(
        settings.current.texture_budget,
        TextureBudget::Auto { percent: 10 }
    );
    crate::preferences::wait_for_save(|| {
        settings.poll_save();
        settings.is_save_pending()
    });
    assert_eq!(
        ImageSettingsState::load(Some(path)).current,
        settings.current
    );
    let output = render(&mut settings, vec![]);
    assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.job.text == english().text("settings-texture-budget-next-load"))));
    output.drop_without_applying_deltas();
}

#[test]
fn autosave_widgets_persist_disable_enable_interval_and_limit_edits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let mut settings = AutosaveSettingsState::load(Some(path.clone()));
    let ctx = egui::Context::default();
    let render = |settings: &mut AutosaveSettingsState, events| {
        ctx.run_ui(
            egui::RawInput {
                events,
                ..default()
            },
            |ui| {
                paint_autosave_settings(ui, settings, &english());
            },
        )
    };
    for _ in 0..3 {
        render(&mut settings, vec![]).drop_without_applying_deltas();
    }
    let click = |settings: &mut AutosaveSettingsState, label: &str| {
        let output = render(settings, vec![]);
        let pos = text_position(&output, label);
        output.drop_without_applying_deltas();
        for pressed in [true, false] {
            render(
                settings,
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
    };
    click(&mut settings, &english().text("settings-autosave-enabled"));
    assert_eq!(settings.current.interval_minutes, None);
    crate::preferences::wait_for_save(|| {
        settings.poll_save();
        settings.is_save_pending()
    });
    assert_eq!(
        AutosaveSettingsState::load(Some(path.clone())).current,
        settings.current
    );
    click(&mut settings, &english().text("settings-autosave-enabled"));
    assert_eq!(
        settings.current.interval_minutes,
        std::num::NonZeroU32::new(5)
    );
    let output = render(&mut settings, vec![]);
    let labels: Vec<_> = output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => Some(text.galley.job.text.clone()),
            _ => None,
        })
        .collect();
    let number = labels
        .iter()
        .find(|text| text.starts_with('5'))
        .unwrap_or_else(|| panic!("Missing interval: {labels:?}"))
        .clone();
    output.drop_without_applying_deltas();
    click(&mut settings, &number);
    render(
        &mut settings,
        vec![
            egui::Event::Text("12".into()),
            egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: default(),
            },
        ],
    )
    .drop_without_applying_deltas();
    assert_eq!(
        settings.current.interval_minutes,
        std::num::NonZeroU32::new(12)
    );
    click(&mut settings, "1");
    render(
        &mut settings,
        vec![
            egui::Event::Text("3".into()),
            egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: default(),
            },
        ],
    )
    .drop_without_applying_deltas();
    assert_eq!(
        settings.current.max_saves_per_game,
        std::num::NonZeroU32::new(3).unwrap()
    );
    crate::preferences::wait_for_save(|| {
        settings.poll_save();
        settings.is_save_pending()
    });
    assert_eq!(
        AutosaveSettingsState::load(Some(path)).current,
        settings.current
    );
}
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
    frame_with_state(
        ctx,
        dialog,
        &DisplaySettingsState::load(None),
        &capabilities(),
        size,
        events,
    )
}

fn frame_with_state(
    ctx: &egui::Context,
    dialog: &mut SettingsDialog,
    state: &DisplaySettingsState,
    capabilities: &DisplayCapabilities,
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
                state,
                capabilities,
                &mut english(),
                &mut UiPreferences::load(None),
                &mut PlayerSettingsState::load(None),
                &mut AutosaveSettingsState::load(None),
                &mut ImageSettingsState::load(None),
                &PuzzleImageLimits {
                    device_max_dimension: 8192,
                    gpu_memory_bytes: Some(8 * 1024 * 1024 * 1024),
                },
                &mut KeyBindingsState::load(None),
                &CaptureInput {
                    keys: &ButtonInput::default(),
                    events: &[],
                    focused: true,
                },
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
        .unwrap_or_else(|| {
            let labels: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) => Some(text.galley.job.text.as_str()),
                    _ => None,
                })
                .collect();
            panic!("Missing label: {label}; rendered labels: {labels:?}")
        })
}

fn has_text(output: &egui::FullOutput, label: &str) -> bool {
    output.shapes.iter().any(
        |shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.job.text == label),
    )
}

fn click(
    ctx: &egui::Context,
    dialog: &mut SettingsDialog,
    label: &str,
) -> Option<DisplaySettingsAction> {
    click_with_state(
        ctx,
        dialog,
        &DisplaySettingsState::load(None),
        &capabilities(),
        label,
    )
}

fn click_with_state(
    ctx: &egui::Context,
    dialog: &mut SettingsDialog,
    state: &DisplaySettingsState,
    capabilities: &DisplayCapabilities,
    label: &str,
) -> Option<DisplaySettingsAction> {
    let size = egui::vec2(1280.0, 720.0);
    for _ in 0..3 {
        frame_with_state(ctx, dialog, state, capabilities, size, vec![])
            .0
            .drop_without_applying_deltas();
    }
    let (output, _) = frame_with_state(ctx, dialog, state, capabilities, size, vec![]);
    let position = text_position(&output, label);
    output.drop_without_applying_deltas();
    for pressed in [true, false] {
        let (output, action) = frame_with_state(
            ctx,
            dialog,
            state,
            capabilities,
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
    click(&ctx, &mut dialog, "Graphics");
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
            assert!(viewport.contains(text_position(&output, &english().text("common-close"))));
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
    click(&ctx, &mut dialog, "General");
    let (output, _) = frame(&ctx, &mut dialog, egui::vec2(1280.0, 720.0), vec![]);
    assert!(!has_text(&output, "Apply"));
    assert!(has_text(&output, "Language"));
    assert!(!has_text(&output, "Display mode"));
    output.drop_without_applying_deltas();
    click(&ctx, &mut dialog, "Graphics");
    assert!(dialog.draft.max_fps.is_none());
    let Some(DisplaySettingsAction::Apply(settings)) = click(&ctx, &mut dialog, "Apply") else {
        panic!("Apply must emit a settings request");
    };
    assert!(settings.max_fps.is_none());
    assert_eq!(settings.mode, ScreenMode::Windowed);
    click(&ctx, &mut dialog, &english().text("common-close"));
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
    assert!(matches!(action, Some(DisplaySettingsAction::Dismiss)));
    assert!(!dialog.open);
    assert_eq!(dialog.draft, DisplaySettings::default());
    dialog.open(&DisplaySettingsState::load(None));
    assert_eq!(dialog.draft.max_fps, Some(60));
}

#[test]
fn apply_requires_a_change_and_disables_again_after_applying() {
    let ctx = egui::Context::default();
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .insert_resource(DisplaySettingsState::load(None))
        .add_plugins(DisplaySettingsPlugin);
    app.update();
    let state = app.world().resource::<DisplaySettingsState>();
    let caps = capabilities();
    let mut dialog = SettingsDialog::default();
    dialog.open(state);
    click_with_state(&ctx, &mut dialog, state, &caps, "Graphics");
    assert!(click_with_state(&ctx, &mut dialog, state, &caps, "Apply").is_none());
    click_with_state(&ctx, &mut dialog, state, &caps, "Unlimited");
    click_with_state(&ctx, &mut dialog, state, &caps, "Unlimited");
    assert!(click_with_state(&ctx, &mut dialog, state, &caps, "Apply").is_none());
    click_with_state(&ctx, &mut dialog, state, &caps, "Unlimited");
    let Some(action @ DisplaySettingsAction::Apply(_)) =
        click_with_state(&ctx, &mut dialog, state, &caps, "Apply")
    else {
        panic!("A changed FPS limit must enable Apply");
    };
    app.world_mut().write_message(action);
    app.update();
    let state = app.world().resource::<DisplaySettingsState>();
    assert_eq!(state.notice, Some(DisplaySettingsNotice::Saved));
    assert!(click_with_state(&ctx, &mut dialog, state, &caps, "Apply").is_none());
    let Some(action @ DisplaySettingsAction::Dismiss) = click_with_state(
        &ctx,
        &mut dialog,
        state,
        &caps,
        &english().text("common-close"),
    ) else {
        panic!("Closing the dialog must dismiss its notice");
    };
    assert!(!dialog.open);
    assert_eq!(dialog.draft, DisplaySettings::default());
    app.world_mut().write_message(action);
    app.update();
    let state = app.world().resource::<DisplaySettingsState>();
    assert!(state.notice.is_none());
    assert_eq!(state.current.max_fps, None);
    dialog.open(state);
    assert_eq!(dialog.draft.max_fps, None);
    click_with_state(&ctx, &mut dialog, state, &caps, "Graphics");
    assert!(click_with_state(&ctx, &mut dialog, state, &caps, "Apply").is_none());
}

#[test]
fn apply_is_disabled_for_unsupported_or_ineffective_display_changes() {
    for (mode, resolution, caps) in [
        (
            ScreenMode::Fullscreen,
            UVec2::new(1280, 720),
            capabilities(),
        ),
        (
            ScreenMode::Borderless,
            UVec2::new(1280, 720),
            DisplayCapabilities::default(),
        ),
        (ScreenMode::Windowed, UVec2::new(100, 100), capabilities()),
    ] {
        let ctx = egui::Context::default();
        let state = DisplaySettingsState::load(None);
        let mut dialog = SettingsDialog::default();
        dialog.open(&state);
        dialog.tab = SettingsTab::Graphics;
        dialog.draft.mode = mode;
        dialog.draft.resolution = resolution;
        assert!(click_with_state(&ctx, &mut dialog, &state, &caps, "Apply").is_none());
    }
    let ctx = egui::Context::default();
    let mut state = DisplaySettingsState::load(None);
    state.current.mode = ScreenMode::Borderless;
    let mut dialog = SettingsDialog::default();
    dialog.open(&state);
    dialog.tab = SettingsTab::Graphics;
    dialog.draft.resolution = UVec2::new(1920, 1080);
    assert!(click_with_state(&ctx, &mut dialog, &state, &capabilities(), "Apply").is_none());
}

#[test]
fn backdrop_dismisses_and_discards_the_draft() {
    let ctx = egui::Context::default();
    let mut dialog = SettingsDialog::default();
    dialog.open(&DisplaySettingsState::load(None));
    dialog.draft.max_fps = None;
    let size = egui::vec2(1280.0, 720.0);
    for _ in 0..3 {
        frame(&ctx, &mut dialog, size, vec![])
            .0
            .drop_without_applying_deltas();
    }
    let position = egui::pos2(10.0, 10.0);
    let mut dismissed = false;
    for pressed in [true, false] {
        let (output, action) = frame(
            &ctx,
            &mut dialog,
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
        dismissed |= matches!(action, Some(DisplaySettingsAction::Dismiss));
    }
    assert!(dismissed);
    assert!(!dialog.open);
    assert_eq!(dialog.draft, DisplaySettings::default());
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
                &mut PlayerSettingsState::load(None),
                &mut AutosaveSettingsState::load(None),
                &mut ImageSettingsState::load(None),
                &PuzzleImageLimits {
                    device_max_dimension: 8192,
                    gpu_memory_bytes: Some(8 * 1024 * 1024 * 1024),
                },
                &mut KeyBindingsState::load(None),
                &CaptureInput {
                    keys: &ButtonInput::default(),
                    events: &[],
                    focused: true,
                },
            );
        },
    )
}

#[test]
fn language_widgets_persist_selection_and_update_next_frame_without_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
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
    crate::preferences::wait_for_save(|| {
        preferences.poll_save();
        preferences.is_save_pending()
    });
    assert_eq!(
        UiPreferences::load(Some(path)).language,
        preferences.language
    );
    let output = localized_frame(&ctx, &mut dialog, &mut preferences, &mut i18n, size, vec![]);
    text_position(&output, "設定");
    text_position(&output, "グラフィック");
    assert!(!has_text(&output, "適用"));
    assert!(!DisplaySettingsState::load(None).can_apply(&dialog.draft, &capabilities()));
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
    for tab in [SettingsTab::General, SettingsTab::Graphics] {
        dialog.tab = tab;
        for size in [
            egui::vec2(1280.0, 720.0),
            egui::vec2(640.0, 360.0),
            egui::vec2(320.0, 360.0),
        ] {
            for _ in 0..3 {
                localized_frame(&ctx, &mut dialog, &mut preferences, &mut i18n, size, vec![])
                    .drop_without_applying_deltas();
            }
            let output =
                localized_frame(&ctx, &mut dialog, &mut preferences, &mut i18n, size, vec![]);
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
            for label in ["設定", "グラフィック", "閉じる"] {
                assert!(viewport.contains(text_position(&output, label)));
            }
            assert_eq!(has_text(&output, "適用"), tab == SettingsTab::Graphics);
            if tab == SettingsTab::Graphics {
                assert!(viewport.contains(text_position(&output, "適用")));
            }
            output.drop_without_applying_deltas();
        }
    }
}

fn key_frame(
    ctx: &egui::Context,
    dialog: &mut SettingsDialog,
    key_state: &mut KeyBindingsState,
    size: egui::Vec2,
    events: Vec<egui::Event>,
    keyboard: &[KeyboardInput],
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
                &mut english(),
                &mut UiPreferences::load(None),
                &mut PlayerSettingsState::load(None),
                &mut AutosaveSettingsState::load(None),
                &mut ImageSettingsState::load(None),
                &PuzzleImageLimits {
                    device_max_dimension: 8192,
                    gpu_memory_bytes: Some(8 * 1024 * 1024 * 1024),
                },
                key_state,
                &CaptureInput {
                    keys: &ButtonInput::default(),
                    events: keyboard,
                    focused: true,
                },
            );
        },
    )
}

fn key_click(
    ctx: &egui::Context,
    dialog: &mut SettingsDialog,
    state: &mut KeyBindingsState,
    label: &str,
) {
    let size = egui::vec2(1280.0, 1100.0);
    for _ in 0..3 {
        key_frame(ctx, dialog, state, size, vec![], &[]).drop_without_applying_deltas();
    }
    let output = key_frame(ctx, dialog, state, size, vec![], &[]);
    let pos = text_position(&output, label);
    output.drop_without_applying_deltas();
    for pressed in [true, false] {
        key_frame(
            ctx,
            dialog,
            state,
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
            &[],
        )
        .drop_without_applying_deltas();
    }
}

fn keyboard_event(key_code: KeyCode, state: bevy::input::ButtonState) -> KeyboardInput {
    KeyboardInput {
        key_code,
        state,
        logical_key: bevy::input::keyboard::Key::Unidentified(
            bevy::input::keyboard::NativeKey::Unidentified,
        ),
        text: None,
        repeat: false,
        window: Entity::PLACEHOLDER,
    }
}

#[test]
fn real_key_widgets_capture_both_slots_save_reset_and_discard_edits() {
    use bevy::input::ButtonState::{Pressed, Released};
    use jigsall_game::keybindings::{KeyAction, KeyBindings};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let mut state = KeyBindingsState::load(Some(path.clone()));
    let ctx = egui::Context::default();
    let mut dialog = SettingsDialog::default();
    dialog.open(&DisplaySettingsState::load(None));
    let size = egui::vec2(1280.0, 1100.0);
    key_click(
        &ctx,
        &mut dialog,
        &mut state,
        &english().text("settings-keys"),
    );
    key_click(&ctx, &mut dialog, &mut state, "Q");
    assert!(dialog.keys.is_capturing());
    key_frame(
        &ctx,
        &mut dialog,
        &mut state,
        size,
        vec![],
        &[
            keyboard_event(KeyCode::ShiftRight, Pressed),
            keyboard_event(KeyCode::KeyR, Pressed),
            keyboard_event(KeyCode::KeyR, Released),
            keyboard_event(KeyCode::ShiftRight, Released),
        ],
    )
    .drop_without_applying_deltas();
    assert!(!dialog.keys.is_capturing());
    assert_eq!(state.current, KeyBindings::default(), "Edits require Apply");
    key_click(&ctx, &mut dialog, &mut state, "Unassigned");
    key_frame(
        &ctx,
        &mut dialog,
        &mut state,
        size,
        vec![],
        &[
            keyboard_event(KeyCode::KeyT, Pressed),
            keyboard_event(KeyCode::KeyT, Released),
        ],
    )
    .drop_without_applying_deltas();
    key_click(&ctx, &mut dialog, &mut state, "Apply");
    crate::preferences::wait_for_save(|| {
        state.poll_save();
        state.is_save_pending()
    });
    assert!(state.error.is_none());
    assert_eq!(
        state.current.binding(KeyAction::RotateLeft).label(),
        "Shift + R / T"
    );
    assert_eq!(
        KeyBindingsState::load(Some(path.clone())).current,
        state.current
    );
    assert!(!dialog.keys.changed(&state));
    key_click(&ctx, &mut dialog, &mut state, &english().text("keys-reset"));
    assert!(dialog.keys.changed(&state));
    key_click(
        &ctx,
        &mut dialog,
        &mut state,
        &english().text("common-close"),
    );
    assert!(!dialog.open);
    assert_eq!(
        state.current.binding(KeyAction::RotateLeft).label(),
        "Shift + R / T"
    );
    dialog.open(&DisplaySettingsState::load(None));
    key_click(
        &ctx,
        &mut dialog,
        &mut state,
        &english().text("settings-keys"),
    );
    key_click(&ctx, &mut dialog, &mut state, &english().text("keys-reset"));
    key_click(&ctx, &mut dialog, &mut state, "Apply");
    crate::preferences::wait_for_save(|| {
        state.poll_save();
        state.is_save_pending()
    });
    assert_eq!(
        KeyBindingsState::load(Some(path)).current,
        KeyBindings::default()
    );
}

#[test]
fn key_capture_consumes_escape_tab_and_enter_without_activating_settings_widgets() {
    use bevy::input::ButtonState::{Pressed, Released};
    use jigsall_game::keybindings::KeyBindings;
    let mut state = KeyBindingsState::load(None);
    let ctx = egui::Context::default();
    let mut dialog = SettingsDialog::default();
    dialog.open(&DisplaySettingsState::load(None));
    let size = egui::vec2(1280.0, 1100.0);
    key_click(
        &ctx,
        &mut dialog,
        &mut state,
        &english().text("settings-keys"),
    );
    for (key, egui_key) in [
        (KeyCode::Escape, egui::Key::Escape),
        (KeyCode::Tab, egui::Key::Tab),
        (KeyCode::Enter, egui::Key::Enter),
    ] {
        key_click(&ctx, &mut dialog, &mut state, "Q");
        assert!(dialog.keys.is_capturing());
        key_frame(
            &ctx,
            &mut dialog,
            &mut state,
            size,
            vec![egui::Event::Key {
                key: egui_key,
                physical_key: Some(egui_key),
                pressed: true,
                repeat: false,
                modifiers: default(),
            }],
            &[keyboard_event(key, Pressed), keyboard_event(key, Released)],
        )
        .drop_without_applying_deltas();
        assert!(dialog.open);
        assert!(!dialog.keys.is_capturing());
        assert_eq!(state.current, KeyBindings::default());
        key_click(&ctx, &mut dialog, &mut state, &english().text("keys-reset"));
    }
}

#[test]
fn key_configuration_fits_small_windows_in_both_languages() {
    for preference in [Locale::EN_US, Locale::JA] {
        let ctx = egui::Context::default();
        let mut dialog = SettingsDialog::default();
        dialog.open(&DisplaySettingsState::load(None));
        dialog.tab = SettingsTab::Keys;
        let mut i18n = english();
        let mut preferences = UiPreferences::load(None);
        preferences.set_language(LanguagePreference::Locale(preference), &mut i18n);
        for size in [
            egui::vec2(1280.0, 720.0),
            egui::vec2(640.0, 360.0),
            egui::vec2(320.0, 360.0),
        ] {
            for _ in 0..4 {
                localized_frame(&ctx, &mut dialog, &mut preferences, &mut i18n, size, vec![])
                    .drop_without_applying_deltas();
            }
            let output =
                localized_frame(&ctx, &mut dialog, &mut preferences, &mut i18n, size, vec![]);
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
                "Key panel exceeds viewport: {panel:?}"
            );
            assert!(viewport.contains(text_position(&output, &i18n.text("settings-apply"))));
            assert!(viewport.contains(text_position(&output, &i18n.text("common-close"))));
            output.drop_without_applying_deltas();
        }
    }
}

#[test]
fn settings_geometry_is_stable_from_the_first_visible_frame() {
    for locale in [Locale::EN_US, Locale::JA] {
        for size in [
            egui::vec2(1280.0, 720.0),
            egui::vec2(640.0, 360.0),
            egui::vec2(320.0, 360.0),
        ] {
            let ctx = egui::Context::default();
            let state = DisplaySettingsState::load(None);
            let mut dialog = SettingsDialog::default();
            let mut preferences = UiPreferences::load(None);
            let mut i18n = english();
            preferences.set_language(LanguagePreference::Locale(locale), &mut i18n);
            // The menu prepares the theme before the settings are opened in the game.
            ctx.run_ui(egui::RawInput::default(), |ui| theme::prepare(ui.ctx()))
                .drop_without_applying_deltas();
            dialog.open(&state);
            for transition in [
                "open",
                "graphics",
                "keys",
                "general",
                "keys again",
                "graphics again",
                "reopen",
            ] {
                let mut events = match transition {
                    "keys" | "keys again" | "general" | "graphics" | "graphics again" => {
                        let label = i18n.text(match transition {
                            "general" => "settings-general",
                            "graphics" | "graphics again" => "settings-graphics",
                            _ => "settings-keys",
                        });
                        let output = localized_frame(
                            &ctx,
                            &mut dialog,
                            &mut preferences,
                            &mut i18n,
                            size,
                            vec![],
                        );
                        let pos = text_position(&output, &label);
                        output.drop_without_applying_deltas();
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
                                    pressed: true,
                                    modifiers: default(),
                                },
                            ],
                        )
                        .drop_without_applying_deltas();
                        vec![egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed: false,
                            modifiers: default(),
                        }]
                    }
                    "reopen" => {
                        dialog.close();
                        dialog.open(&state);
                        vec![]
                    }
                    _ => vec![],
                };
                let mut panels = vec![];
                for _ in 0..6 {
                    let mut output = localized_frame(
                        &ctx,
                        &mut dialog,
                        &mut preferences,
                        &mut i18n,
                        size,
                        std::mem::take(&mut events),
                    );
                    output.textures_delta.clear();
                    let panel = output
                        .shapes
                        .iter()
                        .find_map(|shape| match &shape.shape {
                            egui::Shape::Rect(rect) if rect.corner_radius.nw == 16 => {
                                Some(rect.rect)
                            }
                            _ => None,
                        })
                        .expect("settings must be visible without a blank frame");
                    panels.push(panel);
                    let tab = match transition {
                        "keys" | "keys again" => SettingsTab::Keys,
                        "graphics" | "graphics again" => SettingsTab::Graphics,
                        _ => SettingsTab::General,
                    };
                    assert!(dialog.tab == tab);
                    assert_eq!(
                        has_text(&output, &i18n.text("settings-apply")),
                        tab != SettingsTab::General
                    );
                    assert_eq!(
                        has_text(&output, &i18n.text("settings-language")),
                        tab == SettingsTab::General
                    );
                    assert_eq!(
                        has_text(&output, &i18n.text("settings-display-mode")),
                        tab == SettingsTab::Graphics
                    );
                    if size.y >= 720.0 {
                        assert_eq!(
                            has_text(&output, &i18n.text("settings-autosave-enabled")),
                            tab == SettingsTab::General
                        );
                        assert_eq!(
                            has_text(&output, &i18n.text("settings-max-fps")),
                            tab == SettingsTab::Graphics
                        );
                        assert_eq!(
                            has_text(&output, &i18n.text("settings-piece-visual-quality")),
                            tab == SettingsTab::Graphics,
                            "{locale:?} {size:?} {transition}: graphics quality must be visible at 720px height"
                        );
                    }
                    output.drop_without_applying_deltas();
                }
                let settled = *panels.last().expect("settings must become visible");
                for panel in panels {
                    assert!(
                        (panel.min - settled.min).length() <= 1.0
                            && (panel.max - settled.max).length() <= 1.0,
                        "{locale:?} {size:?} {transition}: visible panel {panel:?} changed to {settled:?}"
                    );
                }
            }
        }
    }
}

/// Render the real game menus and save screenshots without touching user settings.
#[test]
#[ignore = "requires a native window and GPU"]
fn native_image_budget_ui_probe() {
    use bevy::{
        render::view::screenshot::{save_to_disk, Screenshot},
        winit::{WinitPlugin, WinitSettings},
    };
    use std::time::{Duration, Instant};
    let started = std::time::SystemTime::now();
    #[derive(Resource)]
    struct Probe {
        start: Instant,
        phase: usize,
    }
    fn paint(
        mut contexts: EguiContexts,
        mut settings: ResMut<ImageSettingsState>,
        limits: Res<PuzzleImageLimits>,
        i18n: Res<Localization>,
    ) {
        let Ok(ctx) = contexts.ctx_mut() else { return };
        theme::prepare(ctx);
        egui::Modal::new("image_budget_probe".into())
            .frame(theme::frame())
            .show(ctx, |ui| {
                ui.set_width(520.0);
                theme::card().show(ui, |ui| {
                    paint_image_budget(ui, &mut settings, &limits, &i18n)
                });
            });
    }
    fn advance(
        mut commands: Commands,
        mut probe: ResMut<Probe>,
        mut settings: ResMut<ImageSettingsState>,
        limits: Res<PuzzleImageLimits>,
        mut i18n: ResMut<Localization>,
        mut exit: MessageWriter<AppExit>,
    ) {
        if probe.start.elapsed() < Duration::from_secs(2 * (probe.phase as u64 + 1)) {
            return;
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target");
        std::fs::create_dir_all(&root).unwrap();
        match probe.phase {
            0 => {
                commands
                    .spawn(Screenshot::primary_window())
                    .observe(save_to_disk(root.join("image-budget-auto-en.png")));
            }
            1 => {
                settings.current.texture_budget = TextureBudget::Manual {
                    mib: limits.max_budget_mib(),
                };
                i18n.set_preference(LanguagePreference::Locale(Locale::JA));
            }
            2 => {
                commands
                    .spawn(Screenshot::primary_window())
                    .observe(save_to_disk(root.join("image-budget-manual-ja.png")));
            }
            _ => {
                exit.write(AppExit::Success);
            }
        }
        probe.phase += 1;
    }
    let mut preferences = UiPreferences::load(None);
    preferences.language = LanguagePreference::Locale(Locale::EN_US);
    let (service, _storage) =
        jigsall_game::persistence::runtime::PersistenceService::with_storage_requests();
    App::new()
        .add_plugins(
            DefaultPlugins
                .set(WinitPlugin {
                    run_on_any_thread: true,
                })
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "Image budget UI probe".into(),
                        resolution: (800, 900).into(),
                        visible: false,
                        ..default()
                    }),
                    ..default()
                }),
        )
        .insert_resource(DisplaySettingsState::load(None))
        .insert_resource(preferences)
        .insert_resource(ImageSettingsState::load(None))
        .insert_resource(AutosaveSettingsState::load(None))
        .insert_resource(service)
        .insert_resource(KeyBindingsState::load(None))
        .insert_resource(english())
        .insert_resource(WinitSettings::continuous())
        .add_plugins((
            crate::GameUiPlugin,
            jigsall_game::asset_reader::DirectFileAssetPlugin,
            jigsall_game::GamePlugin,
        ))
        .insert_resource(Probe {
            start: Instant::now(),
            phase: 0,
        })
        .add_systems(
            bevy_egui::EguiPrimaryContextPass,
            paint.after(draw_settings_ui),
        )
        .add_systems(Update, advance)
        .run();
    for name in ["image-budget-auto-en.png", "image-budget-manual-ja.png"] {
        let metadata = std::fs::metadata(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../target")
                .join(name),
        )
        .unwrap();
        assert!(metadata.len() > 0);
        assert!(metadata.modified().unwrap() >= started);
    }
}

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
        mut next_app: ResMut<NextState<jigsall_game::resources::AppState>>,
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
                dialog.tab = SettingsTab::Graphics;
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
                dialog.tab = SettingsTab::Graphics;
                dialog.draft.mode = ScreenMode::Borderless;
                dialog.draft.max_fps = None;
                commands
                    .spawn(Screenshot::primary_window())
                    .observe(save_to_disk(screenshot_path("settings-ui-borderless.png")));
            }
            6 => {
                preferences.set_language(LanguagePreference::Locale(Locale::JA), &mut i18n);
                actions.write(dialog.close());
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
                dialog.tab = SettingsTab::Keys;
                commands
                    .spawn(Screenshot::primary_window())
                    .observe(save_to_disk(screenshot_path("settings-ui-keys-ja.png")));
            }
            11 => {
                dialog.tab = SettingsTab::Graphics;
                actions.write(DisplaySettingsAction::Apply(DisplaySettings {
                    resolution: UVec2::new(640, 360),
                    ..default()
                }));
            }
            12 => {
                commands
                    .spawn(Screenshot::primary_window())
                    .observe(save_to_disk(screenshot_path(
                        "settings-ui-confirm-ja-small.png",
                    )));
            }
            13 => {
                actions.write(DisplaySettingsAction::Revert);
            }
            14 => {
                commands
                    .spawn(Screenshot::primary_window())
                    .observe(save_to_disk(screenshot_path("settings-ui-graphics-ja.png")));
            }
            15 => {
                next_app.set(jigsall_game::resources::AppState::GameSetup);
            }
            16 => {
                commands
                    .spawn(Screenshot::primary_window())
                    .observe(save_to_disk(screenshot_path("setup-ui-ja.png")));
            }
            17 => {
                actions.write(DisplaySettingsAction::Apply(DisplaySettings {
                    resolution: UVec2::new(640, 360),
                    ..default()
                }));
                i18n.set_preference(LanguagePreference::Locale(Locale::EN_US));
            }
            18 => {
                commands
                    .spawn(Screenshot::primary_window())
                    .observe(save_to_disk(screenshot_path("setup-ui-en-small.png")));
            }
            19 => {
                actions.write(DisplaySettingsAction::Revert);
                i18n.set_preference(LanguagePreference::Locale(Locale::JA));
                commands.queue(|world: &mut World| {
                    use jigsall_game::{asset_reader::*, resources::*};
                    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("../assets/menu-icon.png");
                    let key = world
                        .resource::<ExternalFileRegistry>()
                        .register_file(&path);
                    world.resource_mut::<PuzzleConfig>().image_path = key.clone();
                    start_thread_image_load(
                        key,
                        path,
                        world.resource::<ImageLoadSender>().tx_results.clone(),
                        world
                            .resource::<PuzzleImageLimits>()
                            .decode_limits(&world.resource::<ImageSettingsState>().current),
                    );
                });
            }
            20 => {
                commands
                    .spawn(Screenshot::primary_window())
                    .observe(save_to_disk(screenshot_path("setup-ui-image-ja.png")));
            }
            21 => {
                commands.queue(|world: &mut World| {
                    use jigsall_game::resources::{PieceMode, PuzzleConfig};
                    let mut config = world.resource_mut::<PuzzleConfig>();
                    config.piece_mode = PieceMode::ManualGrid;
                    config.grid_size = (8, 4);
                    config.rotation_enabled = true;
                });
            }
            22 => {
                commands
                    .spawn(Screenshot::primary_window())
                    .observe(save_to_disk(screenshot_path(
                        "setup-ui-rotation-warning-ja.png",
                    )));
            }
            23 => next_app.set(jigsall_game::resources::AppState::InGame),
            24 => {
                commands
                    .spawn(Screenshot::primary_window())
                    .observe(save_to_disk(screenshot_path("playing-ui-ja.png")));
            }
            25 => {
                commands.queue(|world: &mut World| {
                    use jigsall_game::resources::GameSubState;
                    assert_eq!(
                        world.resource::<State<GameSubState>>().get(),
                        &GameSubState::Playing
                    );
                    world
                        .resource_mut::<NextState<GameSubState>>()
                        .set(GameSubState::Paused);
                });
            }
            26 => {
                commands
                    .spawn(Screenshot::primary_window())
                    .observe(save_to_disk(screenshot_path("pause-ui-ja.png")));
            }
            27 => {
                commands.queue(|world: &mut World| {
                    world
                        .resource_mut::<jigsall_game::resources::GameData>()
                        .puzzle_completed = true;
                    world
                        .resource_mut::<NextState<jigsall_game::resources::AppState>>()
                        .set(jigsall_game::resources::AppState::GameComplete);
                });
            }
            28 => {
                commands
                    .spawn(Screenshot::primary_window())
                    .observe(save_to_disk(screenshot_path("complete-ui-ja.png")));
            }
            29 => {
                exit.write(AppExit::Success);
            }
            _ => {}
        }
        probe.phase += 1;
    }
    let started = std::time::SystemTime::now();
    let (service, _storage) =
        jigsall_game::persistence::runtime::PersistenceService::with_storage_requests();
    App::new()
        .add_plugins(DefaultPlugins.set(WinitPlugin {
            run_on_any_thread: true,
        }))
        .insert_resource(DisplaySettingsState::load(None))
        .insert_resource(UiPreferences::load(None))
        .insert_resource(KeyBindingsState::load(None))
        .insert_resource(PlayerSettingsState::load(None))
        .insert_resource(ImageSettingsState::load(None))
        .insert_resource(AutosaveSettingsState::load(None))
        .insert_resource(service)
        .insert_resource(crate::localization::tests::english())
        .insert_resource(WinitSettings::continuous())
        .add_plugins((
            crate::GameUiPlugin,
            jigsall_game::asset_reader::DirectFileAssetPlugin,
            jigsall_game::GamePlugin,
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
        "settings-ui-keys-ja.png",
        "settings-ui-confirm-ja-small.png",
        "settings-ui-graphics-ja.png",
        "setup-ui-ja.png",
        "setup-ui-en-small.png",
        "setup-ui-image-ja.png",
        "setup-ui-rotation-warning-ja.png",
        "playing-ui-ja.png",
        "pause-ui-ja.png",
        "complete-ui-ja.png",
    ] {
        let metadata = std::fs::metadata(screenshot_path(name)).expect("native screenshot saved");
        assert!(metadata.len() > 0);
        assert!(
            metadata.modified().unwrap() >= started,
            "screenshot was not refreshed: {name}"
        );
    }
}
