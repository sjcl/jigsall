use crate::localization::Localization;
use crate::theme;
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use puzzella_game::resources::*;

/// Pause actions use the same visual language as the title and save dialog.
#[allow(clippy::too_many_arguments)]
pub fn draw_in_game_menu_ui(
    i18n: Res<Localization>,
    mut contexts: EguiContexts,
    mut next_state: ResMut<NextState<AppState>>,
    mut next_sub_state: ResMut<NextState<GameSubState>>,
    completion_state: Option<Res<State<GameCompleteSubState>>>,
    mut next_completion_state: ResMut<NextState<GameCompleteSubState>>,
    mut dialogs: ResMut<crate::persistence::SaveDialogs>,
    mut persistence: ResMut<puzzella_game::persistence::runtime::PersistenceState>,
    mut exit: MessageWriter<AppExit>,
) {
    if persistence.title_dialog_open {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    theme::prepare(ctx);
    let completed = completion_state.is_some();
    egui::Modal::new("pause_menu".into())
        .backdrop_color(egui::Color32::from_black_alpha(165))
        .frame(theme::frame())
        .show(ctx, |ui| {
            ui.set_width((ctx.content_rect().width() - 96.0).clamp(160.0, 320.0));
            ui.set_max_height((ctx.content_rect().height() - 96.0).max(120.0));
            egui::ScrollArea::vertical()
                .max_height((ctx.content_rect().height() - 112.0).max(100.0))
                .show(ui, |ui| {
                    theme::heading(
                        ui,
                        if completed {
                            i18n.text("pause-puzzle-menu")
                        } else {
                            i18n.text("pause-title")
                        },
                    );
                    ui.add_enabled_ui(!persistence.busy, |ui| {
                        let width = ui.available_width();
                        if theme::button(
                            ui,
                            if completed {
                                i18n.text("pause-back-puzzle")
                            } else {
                                i18n.text("pause-resume")
                            },
                            width,
                            true,
                        )
                        .clicked()
                        {
                            if completed {
                                next_completion_state.set(GameCompleteSubState::Viewing);
                            } else {
                                next_sub_state.set(GameSubState::Playing);
                            }
                        }
                        if theme::button(ui, i18n.text("common-save-game"), width, false).clicked()
                        {
                            dialogs.open_title(&mut persistence, &i18n);
                        }
                        if theme::button(ui, i18n.text("common-return-title"), width, false)
                            .clicked()
                        {
                            next_state.set(AppState::Menu);
                        }
                        if theme::danger_button(ui, i18n.text("pause-exit"), width).clicked() {
                            exit.write(AppExit::Success);
                        }
                    });
                    crate::persistence::status(ui, &persistence, &i18n);
                    ui.add_space(8.0);
                    theme::hint(
                        ui,
                        if completed {
                            i18n.text("pause-puzzle-hint")
                        } else {
                            i18n.text("pause-resume-hint")
                        },
                    );
                });
        });
}

/// パズル生成進捗UI
pub fn draw_generation_progress_ui(
    i18n: Res<Localization>,
    mut contexts: EguiContexts,
    progress: Res<PieceGenerationProgress>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    if !progress.is_generating && progress.error.is_none() {
        return;
    }

    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };

    // 画面下部にプログレスバーを表示
    let mut viewport_ui = egui::Ui::new(
        ctx.clone(),
        "generation_viewport".into(),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.viewport_rect()),
    );
    egui::Panel::bottom("generation_progress")
        .default_size(60.0)
        .show(&mut viewport_ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(10.0);

                // 進捗率を計算
                let progress_ratio = match progress.generation_phase {
                    GenerationPhase::NotStarted => 0.0,
                    GenerationPhase::GeneratingState => 0.0,
                    GenerationPhase::UploadingGpu => {
                        0.8 + (progress.pieces_created as f32 / progress.total_pieces.max(1) as f32
                            * 0.2)
                    }
                    GenerationPhase::Completed => 1.0,
                    GenerationPhase::Failed => 0.0,
                };

                // フェーズ名
                let phase_text = match progress.generation_phase {
                    GenerationPhase::NotStarted => i18n.text("generation-starting"),
                    GenerationPhase::GeneratingState => i18n.text("generation-state"),
                    GenerationPhase::UploadingGpu => i18n.text("generation-uploading"),
                    GenerationPhase::Completed => i18n.text("generation-completed"),
                    GenerationPhase::Failed => i18n.text("generation-failed"),
                };

                // プログレスバー（フェーズに応じて適切な数値を表示）
                let (current_count, label) = match progress.generation_phase {
                    GenerationPhase::GeneratingState => (0, "generation-pieces"),
                    GenerationPhase::UploadingGpu => (progress.pieces_created, "generation-pieces"),
                    _ => (0, "generation-items"),
                };

                let progress_bar = egui::ProgressBar::new(progress_ratio)
                    .text(i18n.format(
                        label,
                        &[
                            ("current", current_count.min(progress.total_pieces).into()),
                            ("total", progress.total_pieces.into()),
                        ],
                    ))
                    .desired_width(400.0);

                ui.add(progress_bar);
                ui.label(i18n.format("generation-phase", &[("phase", phase_text.as_str().into())]));
                if let Some(error) = &progress.error {
                    ui.colored_label(egui::Color32::RED, i18n.generation_error(error));
                }
                if ui.button(i18n.text("common-return-title")).clicked() {
                    next_state.set(AppState::Menu);
                }
            });
        });
}
