use super::*;

#[test]
fn autosave_widgets_persist_disable_enable_and_interval_edits() {
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
    click(&mut settings, "Enable autosave");
    assert_eq!(settings.current.interval_minutes, None);
    assert_eq!(
        AutosaveSettingsState::load(Some(path.clone())).current,
        settings.current
    );
    click(&mut settings, "Enable autosave");
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
                &mut AutosaveSettingsState::load(None),
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
        .unwrap_or_else(|| panic!("Missing label: {label}"))
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
    let Some(action @ DisplaySettingsAction::Dismiss) =
        click_with_state(&ctx, &mut dialog, state, &caps, "Back to Title")
    else {
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
        dialog.draft.mode = mode;
        dialog.draft.resolution = resolution;
        assert!(click_with_state(&ctx, &mut dialog, &state, &caps, "Apply").is_none());
    }
    let ctx = egui::Context::default();
    let mut state = DisplaySettingsState::load(None);
    state.current.mode = ScreenMode::Borderless;
    let mut dialog = SettingsDialog::default();
    dialog.open(&state);
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
                &mut AutosaveSettingsState::load(None),
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
    assert_eq!(
        UiPreferences::load(Some(path)).language,
        preferences.language
    );
    let output = localized_frame(&ctx, &mut dialog, &mut preferences, &mut i18n, size, vec![]);
    text_position(&output, "設定");
    text_position(&output, "適用");
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
                &mut AutosaveSettingsState::load(None),
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
    use puzzella_game::keybindings::{KeyAction, KeyBindings};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let mut state = KeyBindingsState::load(Some(path.clone()));
    let ctx = egui::Context::default();
    let mut dialog = SettingsDialog::default();
    dialog.open(&DisplaySettingsState::load(None));
    let size = egui::vec2(1280.0, 1100.0);
    key_click(&ctx, &mut dialog, &mut state, "Key Configuration");
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
    key_click(&ctx, &mut dialog, &mut state, "Reset key bindings");
    assert!(dialog.keys.changed(&state));
    key_click(&ctx, &mut dialog, &mut state, "Back to Title");
    assert!(!dialog.open);
    assert_eq!(
        state.current.binding(KeyAction::RotateLeft).label(),
        "Shift + R / T"
    );
    dialog.open(&DisplaySettingsState::load(None));
    key_click(&ctx, &mut dialog, &mut state, "Key Configuration");
    key_click(&ctx, &mut dialog, &mut state, "Reset key bindings");
    key_click(&ctx, &mut dialog, &mut state, "Apply");
    assert_eq!(
        KeyBindingsState::load(Some(path)).current,
        KeyBindings::default()
    );
}

#[test]
fn key_capture_consumes_escape_tab_and_enter_without_activating_settings_widgets() {
    use bevy::input::ButtonState::{Pressed, Released};
    use puzzella_game::keybindings::KeyBindings;
    let mut state = KeyBindingsState::load(None);
    let ctx = egui::Context::default();
    let mut dialog = SettingsDialog::default();
    dialog.open(&DisplaySettingsState::load(None));
    let size = egui::vec2(1280.0, 1100.0);
    key_click(&ctx, &mut dialog, &mut state, "Key Configuration");
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
        key_click(&ctx, &mut dialog, &mut state, "Reset key bindings");
    }
}

#[test]
fn key_configuration_fits_small_windows_in_both_languages() {
    for preference in [Locale::EN_US, Locale::JA] {
        let ctx = egui::Context::default();
        let mut dialog = SettingsDialog::default();
        dialog.open(&DisplaySettingsState::load(None));
        dialog.key_tab = true;
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
            assert!(viewport.contains(text_position(&output, &i18n.text("common-back-title"))));
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
            for transition in ["open", "keys", "general", "keys again", "reopen"] {
                let mut events = match transition {
                    "keys" | "keys again" | "general" => {
                        let label = i18n.text(if transition == "general" {
                            "settings-general"
                        } else {
                            "settings-keys"
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
                    let output = localized_frame(
                        &ctx,
                        &mut dialog,
                        &mut preferences,
                        &mut i18n,
                        size,
                        std::mem::take(&mut events),
                    );
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
                    assert_eq!(dialog.key_tab, matches!(transition, "keys" | "keys again"));
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
                dialog.key_tab = true;
                commands
                    .spawn(Screenshot::primary_window())
                    .observe(save_to_disk(screenshot_path("settings-ui-keys-ja.png")));
            }
            11 => {
                dialog.key_tab = false;
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
                next_app.set(puzzella_game::resources::AppState::GameSetup);
            }
            14 => {
                commands
                    .spawn(Screenshot::primary_window())
                    .observe(save_to_disk(screenshot_path("setup-ui-ja.png")));
            }
            15 => {
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
        .insert_resource(KeyBindingsState::load(None))
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
        "settings-ui-keys-ja.png",
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
