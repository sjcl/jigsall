use crate::key_config::{CaptureInput, KeyConfigEditor};
use crate::theme;
use crate::{
    localization::{LanguagePreference, Locale, Localization},
    preferences::{PreferenceError, UiPreferences},
};
use bevy::input::keyboard::KeyboardInput;
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use puzzella_game::keybindings::KeyBindingsState;
use puzzella_game::persistence::autosave::{AutosaveSettingsError, AutosaveSettingsState};
use puzzella_game::settings::*;

#[derive(Resource, Default)]
pub struct SettingsDialog {
    pub open: bool,
    draft: DisplaySettings,
    last_applied: DisplaySettings,
    limited_fps: u32,
    confirming: bool,
    key_tab: bool,
    keys: KeyConfigEditor,
}

impl SettingsDialog {
    pub fn open(&mut self, state: &DisplaySettingsState) {
        self.key_tab = false;
        self.keys = default();
        self.open = true;
        self.sync(state);
    }

    fn sync(&mut self, state: &DisplaySettingsState) {
        self.draft = state.current.clone();
        self.last_applied = state.current.clone();
        self.limited_fps = self.draft.max_fps.unwrap_or(60);
        self.confirming = state.confirmation_seconds().is_some();
    }

    fn close(&mut self) -> DisplaySettingsAction {
        *self = default();
        DisplaySettingsAction::Dismiss
    }
}

pub fn reset_dialog(
    mut dialog: ResMut<SettingsDialog>,
    mut actions: MessageWriter<DisplaySettingsAction>,
) {
    if dialog.open {
        actions.write(dialog.close());
    } else {
        *dialog = default();
    }
}

#[allow(clippy::too_many_arguments)]
pub fn draw_settings_ui(
    mut i18n: ResMut<Localization>,
    mut preferences: ResMut<UiPreferences>,
    mut contexts: EguiContexts,
    mut dialog: ResMut<SettingsDialog>,
    state: Res<DisplaySettingsState>,
    capabilities: Res<DisplayCapabilities>,
    mut actions: MessageWriter<DisplaySettingsAction>,
    mut key_state: ResMut<KeyBindingsState>,
    mut autosave: ResMut<AutosaveSettingsState>,
    keys: Res<ButtonInput<KeyCode>>,
    mut events: MessageReader<KeyboardInput>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
) {
    let events: Vec<_> = events.read().cloned().collect();
    if !dialog.open {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    if let Some(action) = paint_settings(
        ctx,
        &mut dialog,
        &state,
        &capabilities,
        &mut i18n,
        &mut preferences,
        &mut autosave,
        &mut key_state,
        &CaptureInput {
            keys: &keys,
            events: &events,
            focused: windows.single().is_ok_and(|window| window.focused),
        },
    ) {
        actions.write(action);
    }
}

fn mode_label(mode: ScreenMode, i18n: &Localization) -> String {
    match mode {
        ScreenMode::Windowed => i18n.text("settings-windowed"),
        ScreenMode::Borderless => i18n.text("settings-borderless"),
        ScreenMode::Fullscreen => i18n.text("settings-fullscreen"),
    }
}

fn resolution_label(size: UVec2) -> String {
    format!("{} x {}", size.x, size.y)
}

#[allow(clippy::too_many_arguments)]
fn paint_settings(
    ctx: &egui::Context,
    dialog: &mut SettingsDialog,
    state: &DisplaySettingsState,
    capabilities: &DisplayCapabilities,
    i18n: &mut Localization,
    preferences: &mut UiPreferences,
    autosave: &mut AutosaveSettingsState,
    key_state: &mut KeyBindingsState,
    key_input: &CaptureInput<'_>,
) -> Option<DisplaySettingsAction> {
    theme::prepare(ctx);
    let screen = ctx.content_rect();
    let seconds = state.confirmation_seconds();
    if dialog.last_applied != state.current || (dialog.confirming && seconds.is_none()) {
        dialog.sync(state);
    }
    dialog.confirming = seconds.is_some();
    let was_capturing = dialog.keys.is_capturing();
    let capture_cancelled = dialog.keys.capture_input(key_input);
    if was_capturing {
        ctx.input_mut(|input| {
            // Tab, Space and Enter must be bindable without navigating or activating widgets.
            input
                .events
                .retain(|event| !matches!(event, egui::Event::Key { .. } | egui::Event::Text(_)));
        });
    }
    let mut action = None;
    let modal_id = egui::Id::new("display_settings");
    let previous_size = ctx.memory(|memory| memory.area_rect(modal_id).map(|rect| rect.size()));
    let response = egui::Modal::new(modal_id)
        .backdrop_color(egui::Color32::from_black_alpha(185))
        .frame(theme::frame())
        .show(ctx, |ui| {
            // A cached area height must not restrict this frame's content measurement.
            ui.set_max_height(screen.height());
            theme::heading(ui, i18n.text("settings-title"));
            ui.add_enabled_ui(seconds.is_none(), |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .selectable_label(!dialog.key_tab, i18n.text("settings-general"))
                        .clicked()
                    {
                        dialog.key_tab = false;
                        dialog.keys.cancel_capture();
                    }
                    if ui
                        .selectable_label(dialog.key_tab, i18n.text("settings-keys"))
                        .clicked()
                    {
                        dialog.key_tab = true;
                    }
                });
            });
            let preferred_width = if dialog.key_tab {
                KeyConfigEditor::preferred_width(ctx, i18n)
            } else {
                520.0
            };
            ui.set_width((screen.width() - 96.0).clamp(160.0, preferred_width));
            ui.separator();
            if let Some(seconds) = seconds {
                ui.colored_label(
                    theme::ACCENT,
                    i18n.format("settings-display-confirm", &[("seconds", seconds.into())]),
                );
                ctx.request_repaint();
            }
            egui::ScrollArea::vertical()
                .max_height(
                    (screen.height() - if seconds.is_some() { 380.0 } else { 320.0 }).max(32.0),
                )
                .show(ui, |ui| {
                    if dialog.key_tab {
                        ui.add_enabled_ui(seconds.is_none(), |ui| {
                            dialog.keys.paint(ui, key_state, i18n, key_input);
                        });
                        return;
                    }
                    theme::card().show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(i18n.text("settings-language"));
                        let mut language = preferences.language;
                        egui::ComboBox::from_id_salt("language_preference")
                            .width(ui.available_width())
                            .selected_text(match language {
                                LanguagePreference::Auto => i18n.text("settings-language-auto"),
                                LanguagePreference::Locale(locale) => i18n.native_name(locale),
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut language,
                                    LanguagePreference::Auto,
                                    i18n.text("settings-language-auto"),
                                );
                                for locale in Locale::available() {
                                    ui.selectable_value(
                                        &mut language,
                                        LanguagePreference::Locale(locale),
                                        i18n.native_name(locale),
                                    );
                                }
                            });
                        if language != preferences.language {
                            preferences.set_language(language, i18n);
                            ctx.request_repaint();
                        }
                        if let Some(error) = &preferences.error {
                            let (key, reason) = match error {
                                PreferenceError::Read(reason) => {
                                    ("settings-language-read-failed", reason)
                                }
                                PreferenceError::Save(reason) => {
                                    ("settings-language-save-failed", reason)
                                }
                            };
                            ui.colored_label(
                                theme::DANGER,
                                i18n.format(key, &[("reason", reason.as_str().into())]),
                            );
                        }
                    });
                    ui.add_enabled_ui(seconds.is_none(), |ui| {
                        theme::card().show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(i18n.text("settings-display-mode"));
                            let previous_mode = dialog.draft.mode;
                            egui::ComboBox::from_id_salt("display_mode")
                                .width(ui.available_width())
                                .selected_text(mode_label(dialog.draft.mode, i18n))
                                .show_ui(ui, |ui| {
                                    for mode in [
                                        ScreenMode::Windowed,
                                        ScreenMode::Borderless,
                                        ScreenMode::Fullscreen,
                                    ] {
                                        ui.add_enabled_ui(
                                            mode == ScreenMode::Windowed
                                                || capabilities.monitor.is_some(),
                                            |ui| {
                                                ui.selectable_value(
                                                    &mut dialog.draft.mode,
                                                    mode,
                                                    mode_label(mode, i18n),
                                                );
                                            },
                                        );
                                    }
                                });
                            if dialog.draft.mode != previous_mode
                                && dialog.draft.mode == ScreenMode::Fullscreen
                                && capabilities
                                    .fullscreen_mode(dialog.draft.resolution)
                                    .is_none()
                            {
                                if let Some(size) = capabilities
                                    .desktop_resolution
                                    .filter(|size| capabilities.fullscreen_mode(*size).is_some())
                                    .or_else(|| {
                                        capabilities
                                            .video_modes
                                            .first()
                                            .map(|mode| mode.physical_size)
                                    })
                                {
                                    dialog.draft.resolution = size;
                                }
                            }
                            ui.add_space(4.0);
                            ui.label(i18n.text("settings-resolution"));
                            if dialog.draft.mode == ScreenMode::Borderless {
                                ui.add_enabled(
                                    false,
                                    egui::Button::new(
                                        i18n.format(
                                            "settings-desktop-resolution",
                                            &[(
                                                "resolution",
                                                capabilities
                                                    .desktop_resolution
                                                    .map(resolution_label)
                                                    .unwrap_or_else(|| {
                                                        i18n.text("settings-unavailable")
                                                    })
                                                    .as_str()
                                                    .into(),
                                            )],
                                        ),
                                    )
                                    .min_size(egui::vec2(ui.available_width(), 34.0)),
                                );
                                theme::hint(ui, i18n.text("settings-borderless-hint"));
                            } else {
                                egui::ComboBox::from_id_salt("display_resolution")
                                    .width(ui.available_width())
                                    .selected_text(resolution_label(dialog.draft.resolution))
                                    .show_ui(ui, |ui| {
                                        for size in capabilities
                                            .resolutions(dialog.draft.mode, dialog.draft.resolution)
                                        {
                                            ui.selectable_value(
                                                &mut dialog.draft.resolution,
                                                size,
                                                resolution_label(size),
                                            );
                                        }
                                    });
                                if dialog.draft.mode == ScreenMode::Fullscreen {
                                    theme::hint(ui, i18n.text("settings-fullscreen-hint"));
                                }
                            }
                        });
                        ui.add_space(2.0);
                        theme::card().show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(i18n.text("settings-max-fps"));
                            let mut unlimited = dialog.draft.max_fps.is_none();
                            ui.checkbox(&mut unlimited, i18n.text("settings-unlimited"));
                            ui.add_enabled_ui(!unlimited, |ui| {
                                ui.horizontal_wrapped(|ui| {
                                    ui.add(
                                        egui::DragValue::new(&mut dialog.limited_fps)
                                            .range(10..=1000)
                                            .speed(1.0)
                                            .suffix(" FPS"),
                                    );
                                    egui::ComboBox::from_id_salt("fps_presets")
                                        .width(100.0)
                                        .selected_text(i18n.text("settings-presets"))
                                        .show_ui(ui, |ui| {
                                            for fps in
                                                [30, 60, 90, 120, 144, 165, 180, 240, 360, 480]
                                            {
                                                ui.selectable_value(
                                                    &mut dialog.limited_fps,
                                                    fps,
                                                    format!("{fps} FPS"),
                                                );
                                            }
                                        });
                                });
                            });
                            dialog.draft.max_fps = if unlimited {
                                None
                            } else {
                                Some(dialog.limited_fps)
                            };
                        });
                        theme::card().show(ui, |ui| {
                            paint_autosave_settings(ui, autosave, i18n);
                        });
                    });
                    if let Some(error) = &state.error {
                        ui.colored_label(theme::DANGER, i18n.display_error(error));
                    }
                    if state.notice == Some(DisplaySettingsNotice::Restored) {
                        ui.colored_label(theme::ACCENT, i18n.text("settings-restored"));
                    }
                    ui.add_space(2.0);
                });
            ui.separator();
            ui.horizontal(|ui| {
                let width = (ui.available_width() - 10.0) * 0.5;
                if seconds.is_some() {
                    if theme::button(ui, i18n.text("settings-revert"), width, false).clicked() {
                        action = Some(DisplaySettingsAction::Revert);
                    }
                    if theme::button(ui, i18n.text("settings-keep"), width, true).clicked() {
                        action = Some(DisplaySettingsAction::Keep);
                    }
                } else {
                    if theme::button(ui, i18n.text("common-back-title"), width, false).clicked() {
                        action = Some(dialog.close());
                    }
                    ui.add_enabled_ui(
                        dialog.open
                            && dialog.keys.can_apply()
                            && (state.can_apply(&dialog.draft, capabilities)
                                || dialog.keys.changed(key_state)),
                        |ui| {
                            if theme::button(ui, i18n.text("settings-apply"), width, true).clicked()
                            {
                                if dialog.keys.changed(key_state) {
                                    dialog.keys.apply(key_state);
                                }
                                if state.can_apply(&dialog.draft, capabilities) {
                                    action =
                                        Some(DisplaySettingsAction::Apply(dialog.draft.clone()));
                                }
                            }
                        },
                    );
                }
            });
        });
    if previous_size.is_none_or(|size| (size - response.response.rect.size()).length() > 1.0) {
        // Re-center with the measured size before submitting a visible frame.
        ctx.request_discard("settings dialog size changed");
    }
    if response.should_close() && !capture_cancelled {
        action = Some(dialog.close());
    }
    action
}

fn paint_autosave_settings(
    ui: &mut egui::Ui,
    state: &mut AutosaveSettingsState,
    i18n: &Localization,
) {
    use std::num::NonZeroU32;
    ui.set_width(ui.available_width());
    let mut enabled = state.current.interval_minutes.is_some();
    let mut minutes = state.current.interval_minutes.map_or(5, NonZeroU32::get);
    let enable_changed = ui
        .checkbox(&mut enabled, i18n.text("settings-autosave-enabled"))
        .changed();
    ui.label(i18n.text("settings-autosave-interval"));
    let interval_changed = ui
        .add_enabled(
            enabled,
            egui::DragValue::new(&mut minutes)
                .range(1..=60)
                .suffix(format!(" {}", i18n.text("settings-autosave-minutes"))),
        )
        .changed();
    if enable_changed || interval_changed {
        state.set_interval(if enabled {
            NonZeroU32::new(minutes)
        } else {
            None
        });
    }
    theme::hint(ui, i18n.text("settings-autosave-hint"));
    if let Some(error) = &state.error {
        let (key, reason) = match error {
            AutosaveSettingsError::Read(reason) => ("settings-autosave-read-failed", reason),
            AutosaveSettingsError::Save(reason) => ("settings-autosave-save-failed", reason),
        };
        ui.colored_label(
            theme::DANGER,
            i18n.format(key, &[("reason", reason.as_str().into())]),
        );
    }
}

#[cfg(test)]
mod tests;
