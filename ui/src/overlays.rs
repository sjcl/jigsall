use crate::localization::Localization;
use crate::theme;
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use jigsall_game::resources::*;

/// Local menu actions use the same visual language as the title and save dialog.
#[allow(clippy::too_many_arguments)]
pub fn draw_in_game_menu_ui(
    i18n: Res<Localization>,
    mut contexts: EguiContexts,
    mut next_sub_state: ResMut<NextState<GameSubState>>,
    completion_state: Option<Res<State<GameCompleteSubState>>>,
    mut next_completion_state: ResMut<NextState<GameCompleteSubState>>,
    mut dialogs: ResMut<crate::persistence::SaveDialogs>,
    mut persistence: ResMut<jigsall_game::persistence::runtime::PersistenceState>,
    network_status: Res<jigsall_game::network::runtime::NetworkStatus>,
) {
    if persistence.title_dialog_open || dialogs.departure_pending() {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    theme::prepare(ctx);
    let completed = completion_state.is_some();
    let multiplayer = network_status.role.is_some();
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
                        } else if multiplayer {
                            i18n.text("pause-multiplayer-menu")
                        } else {
                            i18n.text("pause-title")
                        },
                    );
                    if multiplayer && !completed {
                        theme::hint(ui, i18n.text("pause-multiplayer-hint"));
                        ui.add_space(8.0);
                    }
                    ui.add_enabled_ui(!persistence.busy, |ui| {
                        let width = ui.available_width();
                        if theme::button(
                            ui,
                            if completed || multiplayer {
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
                        if theme::button(
                            ui,
                            i18n.text(
                                crate::persistence::DepartureAction::Title
                                    .menu_key(network_status.role),
                            ),
                            width,
                            false,
                        )
                        .clicked()
                        {
                            dialogs.request_departure(
                                crate::persistence::DepartureAction::Title,
                                network_status.role,
                                &mut persistence,
                                &i18n,
                            );
                        }
                        if theme::danger_button(
                            ui,
                            i18n.text(
                                crate::persistence::DepartureAction::Exit
                                    .menu_key(network_status.role),
                            ),
                            width,
                        )
                        .clicked()
                        {
                            dialogs.request_departure(
                                crate::persistence::DepartureAction::Exit,
                                network_status.role,
                                &mut persistence,
                                &i18n,
                            );
                        }
                    });
                    crate::persistence::status(ui, &persistence, &i18n);
                    crate::multiplayer::paint_host_status(ui, &network_status, &i18n);
                    ui.add_space(8.0);
                    theme::hint(
                        ui,
                        if completed || multiplayer {
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
    multiplayer: Res<crate::multiplayer::MultiplayerUi>,
    status: Res<jigsall_game::network::runtime::NetworkStatus>,
) {
    if multiplayer.connection_screen(&status) {
        return;
    }
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

                let phase_text = match progress.generation_phase {
                    GenerationPhase::NotStarted => i18n.text("generation-starting"),
                    GenerationPhase::GeneratingState => i18n.text("generation-state"),
                    GenerationPhase::UploadingGpu => i18n.text("generation-uploading"),
                    GenerationPhase::Completed => i18n.text("generation-completed"),
                    GenerationPhase::Failed => i18n.text("generation-failed"),
                };

                if let Some(error) = &progress.error {
                    ui.colored_label(theme::DANGER, i18n.text("generation-failed"));
                    egui::CollapsingHeader::new(i18n.text("common-details"))
                        .id_salt("generation_error_details")
                        .show(ui, |ui| {
                            ui.label(i18n.generation_error(error));
                        });
                } else {
                    if progress.is_generating {
                        ui.spinner();
                    }
                    ui.label(phase_text);
                }
                if ui.button(i18n.text("common-return-title")).clicked() {
                    next_state.set(AppState::Menu);
                }
            });
        });
}
