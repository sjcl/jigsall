use crate::localization::Localization;
use crate::theme;
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiTextureHandle};
use jigsall_core::PuzzleDefinition;
use jigsall_game::{
    persistence::{runtime::*, SaveId, SaveTitle, MAX_SAVE_TITLE_CHARS},
    resources::*,
};
mod departure;
pub(crate) mod thumbnails;
use departure::DepartureFlow;
pub(crate) use departure::{process_departure, DepartureAction};
use thumbnails::SaveThumbnails;

#[derive(Resource, Default)]
pub struct SaveDialogs {
    pub load_open: bool,
    pub host_load: bool,
    pub title: String,
    pending_delete: Option<SaveId>,
    reveal_delete: Option<SaveId>,
    loading_save: Option<SaveId>,
    focus_title: bool,
    departure: Option<DepartureFlow>,
}
impl SaveDialogs {
    pub fn open_title(&mut self, state: &mut PersistenceState, i18n: &Localization) {
        self.title = state
            .current_save
            .as_ref()
            .map(|m| m.title.as_str().to_owned())
            .unwrap_or_else(|| i18n.text("save-default-title"));
        self.focus_title = true;
        self.departure = None;
        state.title_dialog_open = true;
        state.error = None;
        state.message = None;
    }

    fn cancel_title(&mut self, state: &mut PersistenceState) {
        state.title_dialog_open = false;
        self.departure = None;
    }
}
pub fn reset_dialogs(mut dialogs: ResMut<SaveDialogs>) {
    *dialogs = default();
}

#[allow(clippy::too_many_arguments)]
pub fn draw_save_dialogs(
    i18n: Res<Localization>,
    mut contexts: EguiContexts,
    mut dialogs: ResMut<SaveDialogs>,
    mut thumbnails: ResMut<SaveThumbnails>,
    mut state: ResMut<PersistenceState>,
    service: Res<PersistenceService>,
    image_limits: Res<PuzzleImageLimits>,
    image_settings: Res<jigsall_game::image_settings::ImageSettingsState>,
    definition: Option<Res<PuzzleDefinition>>,
    original: Option<Res<OriginalPuzzleImage>>,
    image: Option<Res<PuzzleImage>>,
    store: Res<PieceDataStore>,
    app_state: Res<State<AppState>>,
    network_status: Res<jigsall_game::network::runtime::NetworkStatus>,
    mut multiplayer: ResMut<crate::multiplayer::MultiplayerUi>,
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
        paint_load_dialog(
            ctx,
            &mut dialogs,
            &mut state,
            &service,
            image_limits.decode_limits(&image_settings.current),
            &mut thumbnails,
            &i18n,
            &mut multiplayer,
        );
    }
    if state.title_dialog_open
        && matches!(app_state.get(), AppState::InGame | AppState::GameComplete)
    {
        if let Some(DepartureFlow::Confirm(action)) = dialogs.departure {
            departure::paint_confirmation(
                ctx,
                &mut dialogs,
                &mut state,
                action,
                network_status.role,
                &i18n,
            );
            return;
        }
        let departure = dialogs.departure.map(DepartureFlow::action);
        let response = egui::Modal::new("save_game".into())
            .backdrop_color(egui::Color32::from_black_alpha(185))
            .frame(theme::frame())
            .show(ctx, |ui| {
                ui.set_width((screen.width() - 96.0).clamp(160.0, 460.0));
                ui.set_max_height((screen.height() - 96.0).max(120.0));
                egui::ScrollArea::vertical()
                    .max_height(
                        (screen.height()
                            - if departure.is_some() { 280.0 } else { 180.0 }
                            - if departure.is_none()
                                && (state.busy || state.error.is_some() || state.message.is_some())
                            {
                                32.0
                            } else {
                                0.0
                            })
                        .max(80.0),
                    )
                    .show(ui, |ui| {
                        theme::heading(ui, i18n.text("common-save-game"));
                        if let Some(action) = departure {
                            theme::hint(ui, i18n.text(action.prompt_key()));
                            departure::paint_host_warning(ui, network_status.role, &i18n);
                            status(ui, &state, &i18n);
                        }
                        theme::card().show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal_wrapped(|ui| {
                                if let (Some(image), Some(texture)) = (image.as_ref(), texture) {
                                    let size = image.logical_size.as_vec2();
                                    let scale = (72.0 / size.x).min(72.0 / size.y);
                                    ui.add(
                                        egui::Image::new((
                                            texture,
                                            egui::vec2(size.x * scale, size.y * scale),
                                        ))
                                        .corner_radius(6),
                                    );
                                }
                                ui.vertical(|ui| {
                                    ui.label(
                                        egui::RichText::new(i18n.format(
                                            "setup-piece-count",
                                            &[("count", store.len().into())],
                                        ))
                                        .strong(),
                                    );
                                    let progress = store.placed_count as f64
                                        / store.len().max(1) as f64
                                        * 100.0;
                                    theme::hint(
                                        ui,
                                        i18n.format(
                                            "save-progress",
                                            &[(
                                                "percent",
                                                format!("{progress:.1}").as_str().into(),
                                            )],
                                        ),
                                    );
                                    if state.current_save.is_some() {
                                        theme::hint(ui, i18n.text("save-update-hint"));
                                    }
                                });
                            });
                        });
                        ui.add_space(8.0);
                        ui.label(i18n.text("save-puzzle-title"));
                        let field = ui.add_enabled(
                            !state.busy,
                            egui::TextEdit::singleline(&mut dialogs.title)
                                .desired_width(f32::INFINITY)
                                .hint_text(i18n.text("save-title-placeholder")),
                        );
                        if dialogs.focus_title && !state.busy {
                            field.request_focus();
                            dialogs.focus_title = false;
                        }
                        theme::hint(
                            ui,
                            i18n.format(
                                "save-title-length",
                                &[
                                    ("count", dialogs.title.trim().chars().count().into()),
                                    ("max", MAX_SAVE_TITLE_CHARS.into()),
                                ],
                            ),
                        );
                        let title = SaveTitle::new(&dialogs.title);
                        if let Err(error) = &title {
                            ui.colored_label(theme::DANGER, i18n.save_error(error));
                        }
                        theme::hint(ui, i18n.text("save-original-hint"));
                        if original.is_none() {
                            ui.colored_label(theme::DANGER, i18n.text("save-original-unavailable"));
                        }
                        ui.add_space(8.0);
                    });
                let title = SaveTitle::new(&dialogs.title);
                if departure.is_none() {
                    status(ui, &state, &i18n);
                }
                ui.separator();
                if let Some(action) = departure {
                    let width = ui.available_width();
                    ui.add_enabled_ui(
                        !state.busy && title.is_ok() && original.is_some() && definition.is_some(),
                        |ui| {
                            if theme::button(ui, i18n.text(action.save_key()), width, true)
                                .clicked()
                            {
                                if let Ok(title) = title {
                                    service.request_save(&mut state, title);
                                    dialogs.departure = Some(DepartureFlow::Saving(action));
                                }
                            }
                        },
                    );
                    ui.add_enabled_ui(!state.busy, |ui| {
                        if theme::danger_button(ui, i18n.text(action.discard_key()), width)
                            .clicked()
                        {
                            dialogs.departure = Some(DepartureFlow::Confirm(action));
                        }
                        if theme::button(ui, i18n.text("common-cancel"), width, false).clicked() {
                            dialogs.cancel_title(&mut state);
                        }
                    });
                    return;
                }
                ui.horizontal(|ui| {
                    let width = (ui.available_width() - 10.0) * 0.5;
                    ui.add_enabled_ui(!state.busy, |ui| {
                        if theme::button(ui, i18n.text("common-cancel"), width, false).clicked() {
                            dialogs.cancel_title(&mut state);
                        }
                    });
                    ui.add_enabled_ui(
                        !state.busy && title.is_ok() && original.is_some() && definition.is_some(),
                        |ui| {
                            if theme::button(ui, i18n.text("common-save-game"), width, true)
                                .clicked()
                            {
                                if let Ok(title) = title {
                                    service.request_save(&mut state, title);
                                }
                            }
                        },
                    );
                });
            });
        if !state.busy && response.should_close() {
            dialogs.cancel_title(&mut state);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_load_dialog(
    ctx: &egui::Context,
    dialogs: &mut SaveDialogs,
    state: &mut PersistenceState,
    service: &PersistenceService,
    image_limits: ImageDecodeLimits,
    thumbnails: &mut SaveThumbnails,
    i18n: &Localization,
    multiplayer: &mut crate::multiplayer::MultiplayerUi,
) {
    let screen = ctx.content_rect();
    let mut visible = Vec::new();
    let response = egui::Modal::new("load_game".into())
        .backdrop_color(egui::Color32::from_black_alpha(185))
        .frame(theme::frame())
        .show(ctx, |ui| {
            ui.set_width((screen.width() - 96.0).clamp(160.0, 640.0));
            ui.set_max_height((screen.height() - 96.0).max(120.0));
            theme::heading(ui, i18n.text("menu-load-game"));
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
                                egui::RichText::new(i18n.text("save-empty-title"))
                                    .size(18.0)
                                    .strong(),
                            );
                            theme::hint(ui, i18n.text("save-empty-hint"));
                            ui.add_space(28.0);
                        });
                    }
                    for entry in &state.entries {
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
                                        let mut time = i64::try_from(summary.metadata.updated_at)
                                            .ok()
                                            .and_then(|s| chrono::DateTime::from_timestamp(s, 0))
                                            .map(|t| {
                                                t.with_timezone(&chrono::Local)
                                                    .format("%Y-%m-%d  %H:%M")
                                                    .to_string()
                                            })
                                            .unwrap_or_else(|| i18n.text("save-unknown-time"));
                                        if summary.metadata.is_autosave {
                                            time.push_str("  ·  ");
                                            time.push_str(&i18n.text("save-autosave"));
                                        }
                                        theme::hint(
                                            ui,
                                            i18n.format(
                                                "save-summary",
                                                &[
                                                    ("count", summary.piece_count.into()),
                                                    ("time", time.as_str().into()),
                                                ],
                                            ),
                                        );
                                        let progress = summary.placed_count as f32
                                            / summary.piece_count.max(1) as f32;
                                        ui.add(
                                            egui::ProgressBar::new(progress.clamp(0.0, 1.0))
                                                .fill(theme::ACCENT)
                                                .corner_radius(4)
                                                .desired_width(ui.available_width())
                                                .text(
                                                    i18n.format(
                                                        "save-progress",
                                                        &[(
                                                            "percent",
                                                            format!("{:.1}", progress * 100.0)
                                                                .as_str()
                                                                .into(),
                                                        )],
                                                    ),
                                                ),
                                        );
                                        if let Some(delete) = paint_load_actions(
                                            ui, dialogs, state, entry.id, true, i18n,
                                        ) {
                                            action = Some((entry.id, delete));
                                        }
                                    };
                                    if wide {
                                        ui.horizontal_top(|ui| {
                                            thumbnails.paint(
                                                ui,
                                                summary.image_hash,
                                                &mut visible,
                                                i18n,
                                            );
                                            ui.vertical(|ui| {
                                                ui.set_width(ui.available_width());
                                                details(ui);
                                            });
                                        });
                                    } else {
                                        thumbnails.paint(
                                            ui,
                                            summary.image_hash,
                                            &mut visible,
                                            i18n,
                                        );
                                        details(ui);
                                    }
                                }
                                Err(error) => {
                                    ui.label(
                                        egui::RichText::new(i18n.text("save-unavailable"))
                                            .strong()
                                            .color(theme::DANGER),
                                    );
                                    theme::hint(ui, i18n.save_error(error));
                                    if let Some(delete) = paint_load_actions(
                                        ui, dialogs, state, entry.id, false, i18n,
                                    ) {
                                        action = Some((entry.id, delete));
                                    }
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
                    if dialogs.host_load {
                        let title = state
                            .entries
                            .iter()
                            .find(|entry| entry.id == id)
                            .and_then(|entry| entry.summary.as_ref().ok())
                            .map(|summary| summary.metadata.title.as_str().to_owned())
                            .unwrap_or_default();
                        multiplayer.navigate(crate::multiplayer::MenuScreen::HostLoadSettings);
                        multiplayer.selected_save = Some((id, title));
                        dialogs.load_open = false;
                        dialogs.loading_save = None;
                    } else {
                        service.load(state, id, image_limits);
                    }
                }
            }
            status_with_label(
                ui,
                state,
                i18n,
                if dialogs.loading_save.is_some() {
                    i18n.text("save-loading-puzzle")
                } else {
                    i18n.text("save-loading-saves")
                },
            );
            ui.separator();
            ui.add_enabled_ui(!state.busy, |ui| {
                ui.horizontal(|ui| {
                    let width = ((ui.available_width() - 10.0) * 0.5).min(200.0);
                    if theme::button(ui, i18n.text("common-back-title"), width, false).clicked() {
                        dialogs.load_open = false;
                        dialogs.pending_delete = None;
                        dialogs.reveal_delete = None;
                    }
                    if theme::button(ui, i18n.text("save-refresh"), width, false).clicked() {
                        thumbnails.invalidate();
                        service.list(state);
                    }
                });
            });
        });
    if !state.busy && response.should_close() {
        dialogs.load_open = false;
        dialogs.pending_delete = None;
        dialogs.reveal_delete = None;
    }
    if dialogs.load_open {
        thumbnails.request_visible(service, &visible, state.busy);
    }
}

fn paint_load_actions(
    ui: &mut egui::Ui,
    dialogs: &mut SaveDialogs,
    state: &PersistenceState,
    id: SaveId,
    can_resume: bool,
    i18n: &Localization,
) -> Option<bool> {
    let mut action = None;
    let confirming = dialogs.pending_delete == Some(id);
    let delete_label = i18n.text("save-delete-action");
    ui.add_enabled_ui(!state.busy, |ui| {
        let draw_row = |ui: &mut egui::Ui| {
            let available = ui.available_width();
            let delete_width = if confirming {
                200.0_f32.min(available * 0.55)
            } else {
                let text_width = ui
                    .painter()
                    .layout_no_wrap(
                        delete_label.clone(),
                        egui::FontId::proportional(12.0),
                        theme::MUTED,
                    )
                    .size()
                    .x;
                (text_width + 24.0).max(112.0).min(available * 0.6)
            };
            let gap = ui.spacing().item_spacing.x;
            if available < 270.0 {
                ui.spacing_mut().button_padding.x = 6.0;
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
            }
            if can_resume
                && theme::button(
                    ui,
                    if dialogs.loading_save == Some(id) {
                        i18n.text("save-loading-puzzle")
                    } else {
                        i18n.text("save-resume")
                    },
                    180.0_f32.min((available - delete_width - gap).max(0.0)),
                    true,
                )
                .clicked()
            {
                action = Some(false);
            }
            let align = if confirming {
                egui::Align::Min
            } else {
                egui::Align::Center
            };
            ui.with_layout(egui::Layout::right_to_left(align), |ui| {
                if confirming {
                    let confirmation = ui.allocate_ui_with_layout(
                        egui::vec2(delete_width, 0.0),
                        egui::Layout::top_down(egui::Align::RIGHT),
                        |ui| {
                            ui.set_width(delete_width);
                            let compact = delete_width < 180.0;
                            let font_size = if compact { 12.0 } else { 14.0 };
                            if compact {
                                ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                                ui.spacing_mut().button_padding.x = 6.0;
                            }
                            ui.label(
                                egui::RichText::new(i18n.text("save-delete-confirm"))
                                    .size(font_size)
                                    .color(theme::DANGER),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .button(
                                            egui::RichText::new(i18n.text("common-cancel"))
                                                .size(font_size),
                                        )
                                        .clicked()
                                    {
                                        dialogs.pending_delete = None;
                                        dialogs.reveal_delete = None;
                                    }
                                    if ui
                                        .button(
                                            egui::RichText::new(i18n.text("save-delete"))
                                                .size(font_size)
                                                .color(theme::DANGER),
                                        )
                                        .clicked()
                                    {
                                        action = Some(true);
                                        dialogs.pending_delete = None;
                                        dialogs.reveal_delete = None;
                                    }
                                },
                            );
                        },
                    );
                    if dialogs.reveal_delete == Some(id) {
                        confirmation.response.scroll_to_me(None);
                        dialogs.reveal_delete = None;
                    }
                } else {
                    let height = if ui.ctx().content_rect().height() < 600.0 {
                        38.0
                    } else {
                        44.0
                    };
                    if ui
                        .add_sized(
                            [delete_width, height],
                            egui::Button::new(
                                egui::RichText::new(delete_label.as_str())
                                    .size(12.0)
                                    .color(theme::MUTED),
                            )
                            .frame(false)
                            .wrap(),
                        )
                        .on_hover_text(delete_label.as_str())
                        .clicked()
                    {
                        dialogs.pending_delete = Some(id);
                        dialogs.reveal_delete = Some(id);
                    }
                }
            });
        };
        if confirming {
            ui.horizontal_top(draw_row);
        } else {
            ui.horizontal(draw_row);
        }
    });
    action
}

pub fn status(ui: &mut egui::Ui, state: &PersistenceState, i18n: &Localization) {
    status_with_label(ui, state, i18n, i18n.text("save-working"));
}

fn status_with_label(
    ui: &mut egui::Ui,
    state: &PersistenceState,
    i18n: &Localization,
    label: String,
) {
    if state.busy {
        ui.horizontal(|ui| {
            ui.spinner();
            theme::hint(ui, label);
        });
    }
    if let Some(error) = &state.error {
        ui.colored_label(theme::DANGER, i18n.persistence_error(error));
    }
    if let Some(message) = &state.message {
        ui.colored_label(theme::ACCENT, i18n.persistence_notice(message));
    }
}
