use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use puzzella_core::PuzzleDefinition;
use puzzella_game::persistence::runtime::PersistenceService;
use puzzella_game::{
    persistence::{runtime::*, SaveTitle, MAX_SAVE_TITLE_CHARS},
    resources::*,
};

#[derive(Resource, Default)]
pub struct SaveDialogs {
    pub load_open: bool,
    pub title: String,
}
impl SaveDialogs {
    pub fn open_title(&mut self, state: &mut PersistenceState) {
        self.title = state
            .current_save
            .as_ref()
            .map(|m| m.title.as_str().to_owned())
            .unwrap_or_else(|| "My Puzzle".into());
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
    mut state: ResMut<PersistenceState>,
    service: Res<PersistenceService>,
    definition: Option<Res<PuzzleDefinition>>,
    original: Option<Res<OriginalPuzzleImage>>,
    app_state: Res<State<AppState>>,
) {
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    if dialogs.load_open && *app_state.get() == AppState::Menu {
        egui::Modal::new("load_game".into()).show(ctx, |ui| {
            ui.set_width(520.0_f32.min((ctx.content_rect().width() - 48.0).max(160.0)));
            ui.heading("Load Game");
            let mut action = None;
            egui::ScrollArea::vertical()
                .max_height((ctx.content_rect().height() - 180.0).max(100.0))
                .show(ui, |ui| {
                    if state.entries.is_empty() && !state.busy {
                        ui.label("No saved games yet.");
                    }
                    for entry in &state.entries {
                        ui.group(|ui| {
                            match &entry.summary {
                                Ok(summary) => {
                                    ui.label(
                                        egui::RichText::new(summary.metadata.title.as_str())
                                            .strong(),
                                    );
                                    let time = i64::try_from(summary.metadata.updated_at)
                                        .ok()
                                        .and_then(|s| chrono::DateTime::from_timestamp(s, 0))
                                        .map(|t| {
                                            t.with_timezone(&chrono::Local)
                                                .format("%Y-%m-%d %H:%M:%S")
                                                .to_string()
                                        })
                                        .unwrap_or_else(|| "Unknown time".into());
                                    ui.label(format!(
                                        "{}  /  {:.1}%  /  {} pieces",
                                        time,
                                        summary.placed_count as f64 / summary.piece_count as f64
                                            * 100.0,
                                        summary.piece_count
                                    ));
                                    if ui
                                        .add_enabled(!state.busy, egui::Button::new("Load"))
                                        .clicked()
                                    {
                                        action = Some((entry.id, false));
                                    }
                                }
                                Err(error) => {
                                    ui.colored_label(
                                        egui::Color32::LIGHT_RED,
                                        format!("Unavailable save: {error}"),
                                    );
                                }
                            }
                            if ui
                                .add_enabled(!state.busy, egui::Button::new("Delete save"))
                                .clicked()
                            {
                                action = Some((entry.id, true));
                            }
                        });
                    }
                });
            if let Some((id, delete)) = action {
                if delete {
                    service.delete(&mut state, id);
                } else {
                    service.load(&mut state, id);
                }
            }
            status(ui, &state);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!state.busy, egui::Button::new("Refresh"))
                    .clicked()
                {
                    service.list(&mut state);
                }
                if ui
                    .add_enabled(!state.busy, egui::Button::new("Back"))
                    .clicked()
                {
                    dialogs.load_open = false;
                }
            });
        });
    }
    if state.title_dialog_open
        && matches!(app_state.get(), AppState::InGame | AppState::GameComplete)
    {
        egui::Modal::new("save_game".into()).show(ctx, |ui| {
            ui.set_width(360.0_f32.min((ctx.content_rect().width() - 48.0).max(160.0)));
            ui.heading("Save Game");
            ui.add_enabled(
                !state.busy,
                egui::TextEdit::singleline(&mut dialogs.title).desired_width(f32::INFINITY),
            );
            ui.label(format!(
                "{} / {}",
                dialogs.title.trim().chars().count(),
                MAX_SAVE_TITLE_CHARS
            ));
            let title = SaveTitle::new(&dialogs.title);
            if let Err(error) = &title {
                ui.colored_label(egui::Color32::LIGHT_RED, error.to_string());
            }
            status(ui, &state);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        !state.busy && title.is_ok() && original.is_some() && definition.is_some(),
                        egui::Button::new("Save"),
                    )
                    .clicked()
                {
                    if let Ok(title) = title {
                        service.request_save(&mut state, title);
                    }
                }
                if ui
                    .add_enabled(!state.busy, egui::Button::new("Back"))
                    .clicked()
                {
                    state.title_dialog_open = false;
                }
            });
            if original.is_none() {
                ui.colored_label(
                    egui::Color32::LIGHT_RED,
                    "Original image bytes are unavailable.",
                );
            }
        });
    }
}
pub fn status(ui: &mut egui::Ui, state: &PersistenceState) {
    if state.busy {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Working…");
        });
    }
    if let Some(error) = &state.error {
        ui.colored_label(egui::Color32::LIGHT_RED, error);
    }
    if let Some(message) = &state.message {
        ui.colored_label(egui::Color32::LIGHT_GREEN, message);
    }
}
