use crate::theme;
use crate::{
    localization::{LanguagePreference, Locale, Localization},
    preferences::{PreferenceError, UiPreferences},
};
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use puzzella_game::settings::*;

#[derive(Resource, Default)]
pub struct SettingsDialog {
    pub open: bool,
    draft: DisplaySettings,
    last_applied: DisplaySettings,
    limited_fps: u32,
    confirming: bool,
}

impl SettingsDialog {
    pub fn open(&mut self, state: &DisplaySettingsState) {
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

pub fn draw_settings_ui(
    mut i18n: ResMut<Localization>,
    mut preferences: ResMut<UiPreferences>,
    mut contexts: EguiContexts,
    mut dialog: ResMut<SettingsDialog>,
    state: Res<DisplaySettingsState>,
    capabilities: Res<DisplayCapabilities>,
    mut actions: MessageWriter<DisplaySettingsAction>,
) {
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

fn paint_settings(
    ctx: &egui::Context,
    dialog: &mut SettingsDialog,
    state: &DisplaySettingsState,
    capabilities: &DisplayCapabilities,
    i18n: &mut Localization,
    preferences: &mut UiPreferences,
) -> Option<DisplaySettingsAction> {
    theme::prepare(ctx);
    let screen = ctx.content_rect();
    let seconds = state.confirmation_seconds();
    if dialog.last_applied != state.current || (dialog.confirming && seconds.is_none()) {
        dialog.sync(state);
    }
    dialog.confirming = seconds.is_some();
    let mut action = None;
    let response = egui::Modal::new("display_settings".into())
        .backdrop_color(egui::Color32::from_black_alpha(185))
        .frame(theme::frame())
        .show(ctx, |ui| {
            ui.set_width((screen.width() - 96.0).clamp(160.0, 520.0));
            theme::heading(ui, i18n.text("settings-title"));
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
                    (screen.height() - if seconds.is_some() { 320.0 } else { 260.0 }).max(80.0),
                )
                .show(ui, |ui| {
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
                        theme::hint(ui, i18n.text("settings-language-hint"));
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
                                theme::hint(
                                    ui,
                                    if dialog.draft.mode == ScreenMode::Fullscreen {
                                        i18n.text("settings-fullscreen-hint")
                                    } else {
                                        i18n.text("settings-window-size-hint")
                                    },
                                );
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
                    });
                    if let Some(error) = &state.error {
                        ui.colored_label(theme::DANGER, i18n.display_error(error));
                    }
                    if let Some(notice) = &state.notice {
                        ui.colored_label(theme::ACCENT, i18n.display_notice(notice));
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
                        dialog.open && state.can_apply(&dialog.draft, capabilities),
                        |ui| {
                            if theme::button(ui, i18n.text("settings-apply"), width, true).clicked()
                            {
                                action = Some(DisplaySettingsAction::Apply(dialog.draft.clone()));
                            }
                        },
                    );
                }
            });
        });
    if response.should_close() {
        action = Some(dialog.close());
    }
    action
}

#[cfg(test)]
mod tests;
