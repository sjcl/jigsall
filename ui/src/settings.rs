use crate::key_config::{CaptureInput, KeyConfigEditor};
use crate::theme;
use crate::{
    localization::{LanguagePreference, Locale, Localization},
    preferences::{PreferenceError, UiPreferences},
};
use bevy::input::keyboard::KeyboardInput;
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use puzzella_game::image_settings::{ImageSettingsError, ImageSettingsState, TextureBudget};
use puzzella_game::keybindings::KeyBindingsState;
use puzzella_game::persistence::autosave::{AutosaveSettingsError, AutosaveSettingsState};
use puzzella_game::player_settings::{PlayerSettingsError, PlayerSettingsState};
use puzzella_game::resources::PuzzleImageLimits;
use puzzella_game::settings::*;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum SettingsTab {
    #[default]
    General,
    Graphics,
    Keys,
}

#[derive(Resource, Default)]
pub struct SettingsDialog {
    pub open: bool,
    draft: DisplaySettings,
    last_applied: DisplaySettings,
    limited_fps: u32,
    confirming: bool,
    tab: SettingsTab,
    keys: KeyConfigEditor,
    profile_draft: Option<String>,
}

impl SettingsDialog {
    pub fn open(&mut self, state: &DisplaySettingsState) {
        self.profile_draft = None;
        self.tab = SettingsTab::General;
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
    mut profile: ResMut<PlayerSettingsState>,
    mut contexts: EguiContexts,
    mut dialog: ResMut<SettingsDialog>,
    state: Res<DisplaySettingsState>,
    capabilities: Res<DisplayCapabilities>,
    mut actions: MessageWriter<DisplaySettingsAction>,
    mut key_state: ResMut<KeyBindingsState>,
    mut autosave: ResMut<AutosaveSettingsState>,
    mut image_settings: ResMut<ImageSettingsState>,
    image_limits: Res<PuzzleImageLimits>,
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
        &mut profile,
        &mut autosave,
        &mut image_settings,
        &image_limits,
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

fn paint_player_settings(
    ui: &mut egui::Ui,
    draft: &mut Option<String>,
    state: &mut PlayerSettingsState,
    i18n: &Localization,
) {
    let input = draft.get_or_insert_with(|| {
        state
            .current
            .display_name
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default()
    });
    ui.set_width(ui.available_width());
    ui.label(i18n.text("settings-player-name"));
    let response = ui.add(egui::TextEdit::singleline(input).desired_width(f32::INFINITY));
    ui.label(egui::RichText::new(i18n.text("settings-player-name-help")).small());
    let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    if (ui.button(i18n.text("settings-player-name-save")).clicked() || enter) && state.commit(input)
    {
        *input = state
            .current
            .display_name
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default();
    }
    if let Some(error) = &state.error {
        let text = match error {
            PlayerSettingsError::Invalid(error) => i18n.text(match error {
                puzzella_core::DisplayNameError::Empty => "settings-player-name-empty",
                puzzella_core::DisplayNameError::TooManyChars => "settings-player-name-chars",
                puzzella_core::DisplayNameError::TooManyBytes => "settings-player-name-bytes",
                puzzella_core::DisplayNameError::ForbiddenCharacter => {
                    "settings-player-name-control"
                }
            }),
            PlayerSettingsError::Read(reason) => i18n.format(
                "settings-read-failed",
                &[("reason", reason.as_str().into())],
            ),
            PlayerSettingsError::Save(reason) => i18n.format(
                "settings-save-failed",
                &[("reason", reason.as_str().into())],
            ),
        };
        ui.colored_label(theme::DANGER, text);
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
    profile: &mut PlayerSettingsState,
    autosave: &mut AutosaveSettingsState,
    image_settings: &mut ImageSettingsState,
    image_limits: &PuzzleImageLimits,
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
            let preferred_width = if dialog.tab == SettingsTab::Keys {
                KeyConfigEditor::preferred_width(ctx, i18n)
            } else {
                520.0
            };
            // Constrain tab wrapping without reserving the previous tab's minimum width.
            ui.set_max_width((screen.width() - 96.0).clamp(160.0, preferred_width));
            theme::heading(ui, i18n.text("settings-title"));
            ui.add_enabled_ui(seconds.is_none(), |ui| {
                ui.horizontal_wrapped(|ui| {
                    for (tab, label) in [
                        (SettingsTab::General, "settings-general"),
                        (SettingsTab::Graphics, "settings-graphics"),
                        (SettingsTab::Keys, "settings-keys"),
                    ] {
                        if ui
                            .selectable_label(dialog.tab == tab, i18n.text(label))
                            .clicked()
                        {
                            dialog.tab = tab;
                            if tab != SettingsTab::Keys {
                                dialog.keys.cancel_capture();
                            }
                        }
                    }
                });
            });
            let preferred_width = if dialog.tab == SettingsTab::Keys {
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
                    if dialog.tab == SettingsTab::Keys {
                        ui.add_enabled_ui(seconds.is_none(), |ui| {
                            dialog.keys.paint(ui, key_state, i18n, key_input);
                        });
                        return;
                    }
                    if dialog.tab == SettingsTab::General {
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
                        theme::card().show(ui, |ui| {
                            paint_player_settings(ui, &mut dialog.profile_draft, profile, i18n);
                        });
                        ui.add_enabled_ui(seconds.is_none(), |ui| {
                            theme::card().show(ui, |ui| {
                                paint_autosave_settings(ui, autosave, i18n);
                            });
                        });
                        return;
                    }
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
                            paint_image_settings(ui, image_settings, image_limits, i18n);
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
                let show_apply = dialog.tab != SettingsTab::General;
                let width = if seconds.is_some() || show_apply {
                    (ui.available_width() - 10.0) * 0.5
                } else {
                    ui.available_width()
                };
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
                    if show_apply {
                        let can_apply = match dialog.tab {
                            SettingsTab::General => false,
                            SettingsTab::Graphics => state.can_apply(&dialog.draft, capabilities),
                            SettingsTab::Keys => {
                                dialog.keys.can_apply() && dialog.keys.changed(key_state)
                            }
                        };
                        ui.add_enabled_ui(dialog.open && can_apply, |ui| {
                            if theme::button(ui, i18n.text("settings-apply"), width, true).clicked()
                            {
                                match dialog.tab {
                                    SettingsTab::General => {}
                                    SettingsTab::Graphics => {
                                        action = Some(DisplaySettingsAction::Apply(
                                            dialog.draft.clone(),
                                        ));
                                    }
                                    SettingsTab::Keys => dialog.keys.apply(key_state),
                                }
                            }
                        });
                    }
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

fn paint_image_settings(
    ui: &mut egui::Ui,
    settings: &mut ImageSettingsState,
    limits: &PuzzleImageLimits,
    i18n: &Localization,
) {
    ui.set_width(ui.available_width());
    ui.label(i18n.text("settings-texture-budget"));
    if let Some(bytes) = limits.gpu_memory_bytes {
        theme::hint(
            ui,
            i18n.format(
                "settings-texture-gpu-memory",
                &[("mib", (bytes / (1024 * 1024)).to_string().as_str().into())],
            ),
        );
    } else {
        theme::hint(ui, i18n.text("settings-texture-gpu-unknown"));
    }
    let mut budget = settings.current.texture_budget;
    let mut automatic = matches!(budget, TextureBudget::Auto { .. });
    if ui
        .checkbox(&mut automatic, i18n.text("settings-texture-auto"))
        .changed()
    {
        budget = if automatic {
            TextureBudget::Auto { percent: 20 }
        } else {
            TextureBudget::Manual {
                mib: settings.current.budget_mib(limits),
            }
        };
    }
    match &mut budget {
        TextureBudget::Auto { percent } => {
            ui.label(i18n.text("settings-texture-percent"));
            ui.add_enabled(
                limits.gpu_memory_bytes.is_some(),
                egui::DragValue::new(percent).range(1..=100).suffix(" %"),
            );
        }
        TextureBudget::Manual { mib } => {
            *mib = (*mib).clamp(1, limits.max_budget_mib());
            ui.add(
                egui::DragValue::new(mib)
                    .range(1..=limits.max_budget_mib())
                    .speed(16.0)
                    .suffix(" MiB"),
            );
            theme::hint(
                ui,
                i18n.format(
                    "settings-texture-manual-range",
                    &[("mib", limits.max_budget_mib().to_string().as_str().into())],
                ),
            );
        }
    }
    if budget != settings.current.texture_budget {
        settings.set_budget(budget);
    }
    theme::hint(
        ui,
        i18n.format(
            "settings-texture-resolved-budget",
            &[(
                "mib",
                settings
                    .current
                    .budget_mib(limits)
                    .to_string()
                    .as_str()
                    .into(),
            )],
        ),
    );
    let max_edge = limits
        .decode_limits(&settings.current)
        .max_texture_dimension;
    theme::hint(
        ui,
        i18n.format("settings-texture-limit", &[("edge", max_edge.into())]),
    );
    theme::hint(ui, i18n.text("settings-texture-budget-hint"));
    theme::hint(ui, i18n.text("settings-texture-budget-scope"));
    theme::hint(ui, i18n.text("settings-texture-budget-next-load"));
    if let Some(error) = &settings.error {
        let (key, reason) = match error {
            ImageSettingsError::Read(reason) => ("settings-texture-read-failed", reason),
            ImageSettingsError::Save(reason) => ("settings-texture-save-failed", reason),
        };
        ui.colored_label(
            theme::DANGER,
            i18n.format(key, &[("reason", reason.as_str().into())]),
        );
    }
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
    ui.label(i18n.text("settings-autosave-limit"));
    let mut limit = state.current.max_saves_per_game.get();
    if ui
        .add_enabled(
            enabled,
            egui::DragValue::new(&mut limit).range(1..=u32::MAX),
        )
        .changed()
    {
        state.set_max_saves_per_game(NonZeroU32::new(limit).expect("positive save limit"));
    }
    theme::hint(ui, i18n.text("settings-autosave-limit-hint"));
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
