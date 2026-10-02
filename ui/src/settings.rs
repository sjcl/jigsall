use crate::theme;
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
}

pub fn reset_dialog(mut dialog: ResMut<SettingsDialog>) {
    *dialog = default();
}

pub fn draw_settings_ui(
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
    if let Some(action) = paint_settings(ctx, &mut dialog, &state, &capabilities) {
        actions.write(action);
    }
}

fn mode_label(mode: ScreenMode) -> &'static str {
    match mode {
        ScreenMode::Windowed => "Windowed",
        ScreenMode::Borderless => "Borderless",
        ScreenMode::Fullscreen => "Fullscreen",
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
            theme::heading(ui, "Settings");
            ui.separator();
            if let Some(seconds) = seconds {
                ui.colored_label(
                    theme::ACCENT,
                    format!("Keep these display settings? Reverting in {seconds}s."),
                );
                ctx.request_repaint();
            }
            egui::ScrollArea::vertical()
                .max_height(
                    (screen.height() - if seconds.is_some() { 280.0 } else { 240.0 }).max(80.0),
                )
                .show(ui, |ui| {
                    ui.add_enabled_ui(seconds.is_none(), |ui| {
                        theme::card().show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label("Display mode");
                            let previous_mode = dialog.draft.mode;
                            egui::ComboBox::from_id_salt("display_mode")
                                .width(ui.available_width())
                                .selected_text(mode_label(dialog.draft.mode))
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
                                                    mode_label(mode),
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
                            ui.label("Resolution");
                            if dialog.draft.mode == ScreenMode::Borderless {
                                ui.add_enabled(
                                    false,
                                    egui::Button::new(format!(
                                        "{}  (Desktop)",
                                        capabilities
                                            .desktop_resolution
                                            .map(resolution_label)
                                            .unwrap_or_else(|| "Unavailable".into())
                                    ))
                                    .min_size(egui::vec2(ui.available_width(), 34.0)),
                                );
                                theme::hint(ui, "Borderless uses your desktop resolution.");
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
                                        "Fullscreen uses a resolution supported by your display."
                                    } else {
                                        "Window size in pixels."
                                    },
                                );
                            }
                        });
                        ui.add_space(2.0);
                        theme::card().show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label("Maximum FPS");
                            let mut unlimited = dialog.draft.max_fps.is_none();
                            ui.checkbox(&mut unlimited, "Unlimited");
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
                                        .selected_text("Presets")
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
                        ui.colored_label(theme::DANGER, error);
                    }
                    if let Some(notice) = &state.notice {
                        ui.colored_label(theme::ACCENT, notice);
                    }
                    ui.add_space(2.0);
                });
            ui.separator();
            ui.horizontal(|ui| {
                let width = (ui.available_width() - 10.0) * 0.5;
                if seconds.is_some() {
                    if theme::button(ui, "Revert", width, false).clicked() {
                        action = Some(DisplaySettingsAction::Revert);
                    }
                    if theme::button(ui, "Keep Changes", width, true).clicked() {
                        action = Some(DisplaySettingsAction::Keep);
                    }
                } else {
                    if theme::button(ui, "Back to Title", width, false).clicked() {
                        dialog.open = false;
                    }
                    let valid = dialog.draft.validate().is_ok()
                        && (dialog.draft.mode != ScreenMode::Fullscreen
                            || capabilities
                                .fullscreen_mode(dialog.draft.resolution)
                                .is_some());
                    ui.add_enabled_ui(valid, |ui| {
                        if theme::button(ui, "Apply", width, true).clicked() {
                            action = Some(DisplaySettingsAction::Apply(dialog.draft.clone()));
                        }
                    });
                }
            });
        });
    if response.should_close() {
        dialog.open = false;
        if seconds.is_some() {
            action = Some(DisplaySettingsAction::Revert);
        }
    }
    action
}

#[cfg(test)]
mod tests;
