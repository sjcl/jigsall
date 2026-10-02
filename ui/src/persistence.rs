use crate::theme;
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiTextureHandle};
use puzzella_core::PuzzleDefinition;
use puzzella_game::{
    persistence::{runtime::*, SaveId, SaveTitle, MAX_SAVE_TITLE_CHARS},
    resources::*,
};
pub(crate) mod thumbnails;
use thumbnails::SaveThumbnails;

#[derive(Resource, Default)]
pub struct SaveDialogs {
    pub load_open: bool,
    pub title: String,
    pending_delete: Option<SaveId>,
    loading_save: Option<SaveId>,
    focus_title: bool,
}
impl SaveDialogs {
    pub fn open_title(&mut self, state: &mut PersistenceState) {
        self.title = state
            .current_save
            .as_ref()
            .map(|m| m.title.as_str().to_owned())
            .unwrap_or_else(|| "My Puzzle".into());
        self.focus_title = true;
        state.title_dialog_open = true;
        state.error = None;
        state.message = None;
    }
}
pub fn reset_dialogs(mut dialogs: ResMut<SaveDialogs>) {
    *dialogs = default();
}

#[allow(clippy::too_many_arguments)]
pub fn draw_save_dialogs(
    mut contexts: EguiContexts,
    mut dialogs: ResMut<SaveDialogs>,
    mut thumbnails: ResMut<SaveThumbnails>,
    mut state: ResMut<PersistenceState>,
    service: Res<PersistenceService>,
    definition: Option<Res<PuzzleDefinition>>,
    original: Option<Res<OriginalPuzzleImage>>,
    image: Option<Res<PuzzleImage>>,
    store: Res<PieceDataStore>,
    app_state: Res<State<AppState>>,
) {
    let texture = if state.title_dialog_open {
        image
            .as_ref()
            .map(|image| contexts.add_image(EguiTextureHandle::Weak(image.handle.id())))
    } else {
        None
    };
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    theme::prepare(ctx);
    let screen = ctx.content_rect();
    let load_open = dialogs.load_open && *app_state.get() == AppState::Menu;
    if !state.busy {
        dialogs.loading_save = None;
    }
    thumbnails.begin_frame(ctx, &service, state.generation, load_open);
    if load_open {
        paint_load_dialog(ctx, &mut dialogs, &mut state, &service, &mut thumbnails);
    }
    if state.title_dialog_open
        && matches!(app_state.get(), AppState::InGame | AppState::GameComplete)
    {
        let response = egui::Modal::new("save_game".into())
            .backdrop_color(egui::Color32::from_black_alpha(185))
            .frame(theme::frame())
            .show(ctx, |ui| {
                ui.set_width((screen.width() - 96.0).clamp(160.0, 460.0));
                ui.set_max_height((screen.height() - 96.0).max(120.0));
                egui::ScrollArea::vertical()
                    .max_height(
                        (screen.height()
                            - 180.0
                            - if state.busy || state.error.is_some() || state.message.is_some() {
                                32.0
                            } else {
                                0.0
                            })
                        .max(80.0),
                    )
                    .show(ui, |ui| {
                        theme::heading(ui, "Save Game");
                        theme::card().show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal_wrapped(|ui| {
                                if let (Some(image), Some(texture)) = (image.as_ref(), texture) {
                                    let scale = (72.0 / image.size.x).min(72.0 / image.size.y);
                                    ui.add(
                                        egui::Image::new((
                                            texture,
                                            egui::vec2(image.size.x * scale, image.size.y * scale),
                                        ))
                                        .corner_radius(6),
                                    );
                                }
                                ui.vertical(|ui| {
                                    ui.label(
                                        egui::RichText::new(format!("{} pieces", store.len()))
                                            .strong(),
                                    );
                                    let progress = store.placed_count as f64
                                        / store.len().max(1) as f64
                                        * 100.0;
                                    theme::hint(ui, format!("{progress:.1}% complete"));
                                    theme::hint(
                                        ui,
                                        if state.current_save.is_some() {
                                            "Updates your current save"
                                        } else {
                                            "Creates a new save"
                                        },
                                    );
                                });
                            });
                        });
                        ui.add_space(8.0);
                        ui.label("Puzzle title");
                        let field = ui.add_enabled(
                            !state.busy,
                            egui::TextEdit::singleline(&mut dialogs.title)
                                .desired_width(f32::INFINITY)
                                .hint_text("Give your puzzle a name"),
                        );
                        if dialogs.focus_title && !state.busy {
                            field.request_focus();
                            dialogs.focus_title = false;
                        }
                        theme::hint(
                            ui,
                            format!(
                                "{} / {} characters",
                                dialogs.title.trim().chars().count(),
                                MAX_SAVE_TITLE_CHARS
                            ),
                        );
                        let title = SaveTitle::new(&dialogs.title);
                        if let Err(error) = &title {
                            ui.colored_label(theme::DANGER, error.to_string());
                        }
                        theme::hint(ui, "Your original image is included in the save.");
                        if original.is_none() {
                            ui.colored_label(
                                theme::DANGER,
                                "Original image bytes are unavailable.",
                            );
                        }
                        ui.add_space(8.0);
                    });
                let title = SaveTitle::new(&dialogs.title);
                status(ui, &state);
                ui.separator();
                ui.horizontal(|ui| {
                    let width = (ui.available_width() - 10.0) * 0.5;
                    ui.add_enabled_ui(!state.busy, |ui| {
                        if theme::button(ui, "Cancel", width, false).clicked() {
                            state.title_dialog_open = false;
                        }
                    });
                    ui.add_enabled_ui(
                        !state.busy && title.is_ok() && original.is_some() && definition.is_some(),
                        |ui| {
                            if theme::button(ui, "Save Game", width, true).clicked() {
                                if let Ok(title) = title {
                                    service.request_save(&mut state, title);
                                }
                            }
                        },
                    );
                });
            });
        if !state.busy && response.should_close() {
            state.title_dialog_open = false;
        }
    }
}

fn paint_load_dialog(
    ctx: &egui::Context,
    dialogs: &mut SaveDialogs,
    state: &mut PersistenceState,
    service: &PersistenceService,
    thumbnails: &mut SaveThumbnails,
) {
    let screen = ctx.content_rect();
    let mut visible = Vec::new();
    let response = egui::Modal::new("load_game".into())
        .backdrop_color(egui::Color32::from_black_alpha(185))
        .frame(theme::frame())
        .show(ctx, |ui| {
            ui.set_width((screen.width() - 96.0).clamp(160.0, 640.0));
            ui.set_max_height((screen.height() - 96.0).max(120.0));
            theme::heading(ui, "Load Game");
            ui.separator();
            let mut action = None;
            egui::ScrollArea::vertical()
                .max_height((screen.height() - 280.0).max(80.0))
                .show(ui, |ui| {
                    if state.entries.is_empty() && !state.busy {
                        ui.vertical_centered(|ui| {
                            ui.add_space(28.0);
                            let (rect, _) = ui
                                .allocate_exact_size(egui::Vec2::splat(48.0), egui::Sense::hover());
                            theme::piece_outline(
                                ui.painter(),
                                rect,
                                egui::Stroke::new(1.5, theme::BORDER),
                            );
                            ui.add_space(10.0);
                            ui.label(
                                egui::RichText::new("A fresh collection")
                                    .size(18.0)
                                    .strong(),
                            );
                            theme::hint(ui, "Save a puzzle during play and it will appear here.");
                            ui.add_space(28.0);
                        });
                    }
                    for entry in &state.entries {
                        let delete_was_open = dialogs.pending_delete == Some(entry.id);
                        theme::card().show(ui, |ui| {
                            ui.set_width((ui.available_width()).max(0.0));
                            match &entry.summary {
                                Ok(summary) => {
                                    let wide = ui.available_width() >= 360.0;
                                    let mut details = |ui: &mut egui::Ui| {
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(
                                                    summary.metadata.title.as_str(),
                                                )
                                                .size(17.0)
                                                .strong(),
                                            )
                                            .truncate(),
                                        )
                                        .on_hover_text(summary.metadata.title.as_str());
                                        let time = i64::try_from(summary.metadata.updated_at)
                                            .ok()
                                            .and_then(|s| chrono::DateTime::from_timestamp(s, 0))
                                            .map(|t| {
                                                t.with_timezone(&chrono::Local)
                                                    .format("%Y-%m-%d  %H:%M")
                                                    .to_string()
                                            })
                                            .unwrap_or_else(|| "Unknown time".into());
                                        theme::hint(
                                            ui,
                                            format!("{} pieces  /  {time}", summary.piece_count),
                                        );
                                        let progress = summary.placed_count as f32
                                            / summary.piece_count.max(1) as f32;
                                        ui.add(
                                            egui::ProgressBar::new(progress.clamp(0.0, 1.0))
                                                .fill(theme::ACCENT)
                                                .corner_radius(4)
                                                .desired_width(ui.available_width())
                                                .text(format!("{:.1}% complete", progress * 100.0)),
                                        );
                                        if paint_load_actions(ui, dialogs, state, entry.id, true) {
                                            action = Some((entry.id, false));
                                        }
                                    };
                                    if wide {
                                        ui.horizontal_top(|ui| {
                                            thumbnails.paint(ui, summary.image_hash, &mut visible);
                                            ui.vertical(|ui| {
                                                ui.set_width(ui.available_width());
                                                details(ui);
                                            });
                                        });
                                    } else {
                                        thumbnails.paint(ui, summary.image_hash, &mut visible);
                                        details(ui);
                                    }
                                }
                                Err(error) => {
                                    ui.label(
                                        egui::RichText::new("Unavailable save")
                                            .strong()
                                            .color(theme::DANGER),
                                    );
                                    theme::hint(ui, error.to_string());
                                    paint_load_actions(ui, dialogs, state, entry.id, false);
                                }
                            }
                            if dialogs.pending_delete == Some(entry.id) {
                                let confirmation = ui.vertical(|ui| {
                                    ui.label(
                                        egui::RichText::new("Permanently delete this save?")
                                            .color(theme::DANGER),
                                    );
                                    ui.horizontal_wrapped(|ui| {
                                        if ui
                                            .add_enabled(
                                                !state.busy,
                                                egui::Button::new(
                                                    egui::RichText::new("Delete")
                                                        .color(theme::DANGER),
                                                ),
                                            )
                                            .clicked()
                                        {
                                            action = Some((entry.id, true));
                                            dialogs.pending_delete = None;
                                        }
                                        if ui
                                            .add_enabled(!state.busy, egui::Button::new("Cancel"))
                                            .clicked()
                                        {
                                            dialogs.pending_delete = None;
                                        }
                                    });
                                });
                                if !delete_was_open {
                                    confirmation.response.scroll_to_me(None);
                                }
                            }
                        });
                        ui.add_space(2.0);
                    }
                });
            if let Some((id, delete)) = action {
                if delete {
                    service.delete(state, id);
                } else {
                    dialogs.loading_save = Some(id);
                    service.load(state, id);
                }
            }
            status_with_label(
                ui,
                state,
                if dialogs.loading_save.is_some() {
                    "Loading puzzle..."
                } else {
                    "Loading saves..."
                },
            );
            ui.separator();
            ui.add_enabled_ui(!state.busy, |ui| {
                ui.horizontal(|ui| {
                    let width = ((ui.available_width() - 10.0) * 0.5).min(200.0);
                    if theme::button(ui, "Back to Title", width, false).clicked() {
                        dialogs.load_open = false;
                        dialogs.pending_delete = None;
                    }
                    if theme::button(ui, "Refresh", width, false).clicked() {
                        thumbnails.invalidate();
                        service.list(state);
                    }
                });
            });
        });
    if !state.busy && response.should_close() {
        dialogs.load_open = false;
        dialogs.pending_delete = None;
    }
    if dialogs.load_open {
        thumbnails.request_visible(service, &visible, state.busy);
    } else {
        thumbnails.invalidate();
    }
}

fn paint_load_actions(
    ui: &mut egui::Ui,
    dialogs: &mut SaveDialogs,
    state: &PersistenceState,
    id: SaveId,
    can_resume: bool,
) -> bool {
    let mut resume = false;
    ui.add_enabled_ui(!state.busy, |ui| {
        ui.horizontal(|ui| {
            let available = ui.available_width();
            let show_delete = dialogs.pending_delete != Some(id);
            let delete_width = if show_delete {
                90.0_f32.min(available * 0.45)
            } else {
                0.0
            };
            let gap = if show_delete {
                ui.spacing().item_spacing.x
            } else {
                0.0
            };
            if available < 270.0 {
                ui.spacing_mut().button_padding.x = 6.0;
            }
            if can_resume {
                resume = theme::button(
                    ui,
                    if dialogs.loading_save == Some(id) {
                        "Loading puzzle..."
                    } else {
                        "Resume Puzzle"
                    },
                    180.0_f32.min((available - delete_width - gap).max(0.0)),
                    true,
                )
                .clicked();
            }
            if show_delete {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let height = if ui.ctx().content_rect().height() < 600.0 {
                        38.0
                    } else {
                        44.0
                    };
                    if ui
                        .add_sized(
                            [delete_width, height],
                            egui::Button::new(
                                egui::RichText::new("Delete save")
                                    .size(12.0)
                                    .color(theme::MUTED),
                            )
                            .frame(false)
                            .truncate(),
                        )
                        .on_hover_text("Delete save")
                        .clicked()
                    {
                        dialogs.pending_delete = Some(id);
                    }
                });
            }
        });
    });
    resume
}

pub fn status(ui: &mut egui::Ui, state: &PersistenceState) {
    status_with_label(ui, state, "Working...");
}

fn status_with_label(ui: &mut egui::Ui, state: &PersistenceState, label: &str) {
    if state.busy {
        ui.horizontal(|ui| {
            ui.spinner();
            theme::hint(ui, label);
        });
    }
    if let Some(error) = &state.error {
        ui.colored_label(theme::DANGER, error);
    }
    if let Some(message) = &state.message {
        ui.colored_label(theme::ACCENT, message);
    }
}
