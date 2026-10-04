use crate::{localization::Localization, theme};
use bevy::{
    input::{keyboard::KeyboardInput, ButtonState},
    prelude::*,
};
use bevy_egui::egui;
use jigsall_game::keybindings::*;

const KEY_COLUMN_WIDTH: f32 = 200.0;
const COLUMN_SPACING: f32 = 12.0;

pub(crate) struct CaptureInput<'a> {
    pub keys: &'a ButtonInput<KeyCode>,
    pub events: &'a [KeyboardInput],
    pub focused: bool,
}

#[derive(Default)]
pub(crate) struct KeyConfigEditor {
    draft: Option<KeyBindings>,
    capture: Option<KeyCapture>,
    error: Option<&'static str>,
}

struct KeyCapture {
    action: KeyAction,
    secondary: bool,
    waiting_for_release: bool,
    held: Vec<KeyCode>,
    chord: Vec<KeyCode>,
    too_many: bool,
}

impl KeyConfigEditor {
    pub fn preferred_width(ctx: &egui::Context, i18n: &Localization) -> f32 {
        let font = egui::TextStyle::Body.resolve(&ctx.style_of(egui::Theme::Dark));
        let description_width = ctx.fonts_mut(|fonts| {
            KeyAction::ALL
                .into_iter()
                .map(KeyAction::label_key)
                .chain(["keys-description", "keys-escape"])
                .map(|key| {
                    fonts
                        .layout_no_wrap(i18n.text(key), font.clone(), theme::TEXT)
                        .size()
                        .x
                })
                .fold(0.0_f32, f32::max)
        });
        description_width.ceil() + 8.0 + KEY_COLUMN_WIDTH * 2.0 + COLUMN_SPACING * 2.0
    }

    pub fn is_capturing(&self) -> bool {
        self.capture.is_some()
    }

    pub fn cancel_capture(&mut self) {
        self.capture = None;
    }

    pub fn changed(&self, state: &KeyBindingsState) -> bool {
        !state.is_save_pending()
            && self
                .draft
                .as_ref()
                .is_some_and(|draft| draft != &state.current)
    }

    pub fn can_apply(&self) -> bool {
        self.capture.is_none()
            && self
                .draft
                .as_ref()
                .is_none_or(|draft| draft.validate().is_ok())
    }

    pub fn apply(&mut self, state: &mut KeyBindingsState) {
        if let Some(draft) = &self.draft {
            state.apply(draft.clone());
        }
    }

    /// Events preserve short taps and distinguish a simultaneous chord from sequential keys.
    /// Returns true when Esc cancels a capture, so the settings modal stays open.
    pub fn capture_input(&mut self, input: &CaptureInput<'_>) -> bool {
        let Some(capture) = &mut self.capture else {
            return false;
        };
        if !input.focused {
            self.capture = None;
            return false;
        }
        if input.keys.just_pressed(KeyCode::Escape)
            || input.events.iter().any(|event| {
                event.key_code == KeyCode::Escape && event.state == ButtonState::Pressed
            })
        {
            self.capture = None;
            self.error = None;
            return true;
        }
        if capture.waiting_for_release {
            capture.waiting_for_release = input.keys.get_pressed().next().is_some();
            return false;
        }
        for event in input.events {
            if event.repeat {
                continue;
            }
            match event.state {
                ButtonState::Pressed => {
                    if !capture.held.contains(&event.key_code) {
                        capture.held.push(event.key_code);
                    }
                    let mut held: Vec<_> =
                        capture.held.iter().copied().map(normalize_key).collect();
                    held.sort_unstable();
                    held.dedup();
                    if held.len() > 2 {
                        capture.too_many = true;
                    } else if held.len() > capture.chord.len() {
                        capture.chord = held;
                    }
                }
                ButtonState::Released => {
                    capture.held.retain(|key| *key != event.key_code);
                    if capture.held.is_empty() && !capture.chord.is_empty() {
                        let chord = if capture.too_many {
                            Err("keys-too-many")
                        } else {
                            KeyChord::new(capture.chord[0], capture.chord.get(1).copied())
                        };
                        let action = capture.action;
                        let secondary = capture.secondary;
                        self.capture = None;
                        match chord {
                            Ok(chord) => {
                                let binding = self.draft.as_mut().unwrap().binding_mut(action);
                                if secondary {
                                    binding.secondary = Some(chord);
                                } else {
                                    binding.primary = Some(chord);
                                }
                                self.error = None;
                            }
                            Err(key) => self.error = Some(key),
                        }
                        break;
                    }
                }
            }
        }
        false
    }

    pub fn paint(
        &mut self,
        ui: &mut egui::Ui,
        state: &KeyBindingsState,
        i18n: &Localization,
        input: &CaptureInput<'_>,
    ) {
        self.draft.get_or_insert_with(|| state.current.clone());
        theme::hint(ui, i18n.text("keys-hint"));
        theme::hint(ui, i18n.text("keys-apply-hint"));
        if ui.button(i18n.text("keys-reset")).clicked() {
            self.draft = Some(KeyBindings::default());
            self.capture = None;
            self.error = None;
        }
        if let Some(key) = self
            .error
            .or_else(|| self.draft.as_ref().unwrap().validate().err())
        {
            ui.colored_label(theme::DANGER, i18n.text(key));
        }
        if let Some(error) = &state.error {
            let text = match error {
                KeyBindingsError::Invalid(key) => i18n.text(key),
                KeyBindingsError::Read(reason) => {
                    i18n.format("keys-read-failed", &[("reason", reason.as_str().into())])
                }
                KeyBindingsError::Save(reason) => {
                    i18n.format("keys-save-failed", &[("reason", reason.as_str().into())])
                }
            };
            ui.colored_label(theme::DANGER, text);
        }
        if self.capture.is_some() {
            theme::hint(ui, i18n.text("keys-capture-hint"));
            if ui.button(i18n.text("common-cancel")).clicked() {
                self.cancel_capture();
            }
        }
        ui.add_space(4.0);
        let table_width = ui
            .available_width()
            .max(Self::preferred_width(ui.ctx(), i18n));
        let key_width = KEY_COLUMN_WIDTH;
        let description_width = table_width - key_width * 2.0 - COLUMN_SPACING * 2.0;
        // Keep the three columns on one row, including in narrow windows.
        egui::ScrollArea::horizontal()
            .id_salt("key_binding_columns")
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.set_min_width(table_width);
                egui::Grid::new("key_binding_table")
                    .num_columns(3)
                    .striped(true)
                    .spacing(egui::vec2(COLUMN_SPACING, 8.0))
                    .show(ui, |ui| {
                        for (key, width) in [
                            ("keys-description", description_width),
                            ("keys-primary", key_width),
                            ("keys-secondary", key_width),
                        ] {
                            table_label(ui, i18n.text(key), width, true);
                        }
                        ui.end_row();
                        for action in KeyAction::ALL {
                            table_label(
                                ui,
                                i18n.text(action.label_key()),
                                description_width,
                                false,
                            );
                            for secondary in [false, true] {
                                self.paint_binding_cell(
                                    ui, action, secondary, i18n, input, key_width,
                                );
                            }
                            ui.end_row();
                        }
                        table_label(ui, i18n.text("keys-escape"), description_width, false);
                        ui.add_enabled_ui(false, |ui| {
                            ui.add_sized([key_width, 36.0], egui::Button::new("Esc"));
                        });
                        ui.add_enabled_ui(false, |ui| {
                            ui.add_sized([key_width, 36.0], egui::Button::new("—"));
                        });
                        ui.end_row();
                    });
            });
    }

    fn paint_binding_cell(
        &mut self,
        ui: &mut egui::Ui,
        action: KeyAction,
        secondary: bool,
        i18n: &Localization,
        input: &CaptureInput<'_>,
        width: f32,
    ) {
        let selected = self
            .capture
            .as_ref()
            .is_some_and(|capture| capture.action == action && capture.secondary == secondary);
        let binding = self.draft.as_ref().unwrap().binding(action);
        let chord = if secondary {
            binding.secondary
        } else {
            binding.primary
        };
        let label = if selected {
            i18n.text("keys-listening")
        } else {
            chord
                .map(KeyChord::label)
                .unwrap_or_else(|| i18n.text("keys-unassigned"))
        };
        ui.allocate_ui_with_layout(
            egui::vec2(width, 36.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.push_id((action as usize, secondary), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    if ui
                        .add_sized(
                            [width - 78.0, 36.0],
                            egui::Button::new(&label).truncate().selected(selected),
                        )
                        .on_hover_text(&label)
                        .clicked()
                    {
                        self.capture = Some(KeyCapture {
                            action,
                            secondary,
                            waiting_for_release: input.keys.get_pressed().next().is_some(),
                            held: vec![],
                            chord: vec![],
                            too_many: false,
                        });
                        self.error = None;
                    }
                    let clear = ui
                        .add_enabled_ui(chord.is_some(), |ui| {
                            ui.add_sized([70.0, 36.0], egui::Button::new(i18n.text("keys-clear")))
                        })
                        .inner;
                    if clear.clicked() {
                        let binding = self.draft.as_mut().unwrap().binding_mut(action);
                        if secondary {
                            binding.secondary = None;
                        } else {
                            binding.primary = None;
                        }
                        self.capture = None;
                        self.error = None;
                    }
                });
            },
        );
    }
}

fn table_label(ui: &mut egui::Ui, text: String, width: f32, heading: bool) {
    ui.allocate_ui_with_layout(
        egui::vec2(width, if heading { 26.0 } else { 36.0 }),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            let label = egui::RichText::new(&text);
            let label = if heading { label.strong() } else { label };
            ui.add(egui::Label::new(label).truncate())
                .on_hover_text(text);
        },
    );
}

#[cfg(test)]
mod tests;
