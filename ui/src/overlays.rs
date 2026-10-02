use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use puzzella_game::resources::*;

/// インゲームメニューUI（ESCキーで表示）
pub fn draw_in_game_menu_ui(
    mut contexts: EguiContexts,
    mut next_state: ResMut<NextState<AppState>>,
    mut next_sub_state: ResMut<NextState<GameSubState>>,
    completion_state: Option<Res<State<GameCompleteSubState>>>,
    mut next_completion_state: ResMut<NextState<GameCompleteSubState>>,
) {
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };

    let viewing_completed_puzzle = completion_state.is_some();

    // 半透明の背景を表示してゲーム画面を暗くする
    egui::Area::new(egui::Id::new("in_game_menu_background"))
        .fixed_pos(egui::pos2(0.0, 0.0))
        .show(ctx, |ui| {
            let screen_rect = ctx.content_rect();
            ui.allocate_ui_with_layout(
                screen_rect.size(),
                egui::Layout::centered_and_justified(egui::Direction::TopDown),
                |ui| {
                    // 背景全体を半透明の黒で覆う
                    ui.painter().rect_filled(
                        screen_rect,
                        egui::CornerRadius::ZERO,
                        egui::Color32::from_black_alpha(128), // 半透明の黒
                    );
                },
            );
        });

    // メニューを画面中央に表示
    egui::Window::new("Game Menu")
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .collapsible(false)
        .resizable(false)
        .title_bar(false)
        .show(ctx, |ui| {
            ui.set_min_size(egui::vec2(300.0, 200.0));

            ui.vertical_centered(|ui| {
                ui.spacing_mut().item_spacing.y = 20.0;

                ui.heading(if viewing_completed_puzzle {
                    "Puzzle Menu"
                } else {
                    "Game Menu"
                });

                ui.separator();

                // Resume Game ボタン
                if ui
                    .add_sized(
                        [200.0, 40.0],
                        egui::Button::new(if viewing_completed_puzzle {
                            "Back to Puzzle"
                        } else {
                            "Resume Game"
                        }),
                    )
                    .clicked()
                {
                    if viewing_completed_puzzle {
                        next_completion_state.set(GameCompleteSubState::Viewing);
                    } else {
                        next_sub_state.set(GameSubState::Playing);
                    }
                }

                // Return to Title ボタン
                if ui
                    .add_sized([200.0, 40.0], egui::Button::new("Return to Title"))
                    .clicked()
                {
                    next_state.set(AppState::Menu);
                    println!("🎮 Returning to title screen");
                }

                ui.separator();

                // Exit Game ボタン
                if ui
                    .add_sized([200.0, 40.0], egui::Button::new("Exit Game"))
                    .clicked()
                {
                    println!("🎮 Exiting game");
                    std::process::exit(0);
                }

                ui.separator();

                ui.label(if viewing_completed_puzzle {
                    "Press ESC to return to your puzzle"
                } else {
                    "Press ESC to resume"
                });
            });
        });
}

/// パズル生成進捗UI
pub fn draw_generation_progress_ui(
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
                    GenerationPhase::NotStarted => "Starting...",
                    GenerationPhase::GeneratingState => "Generating state",
                    GenerationPhase::UploadingGpu => "Uploading GPU state",
                    GenerationPhase::Completed => "Completed",
                    GenerationPhase::Failed => "Failed",
                };

                // プログレスバー（フェーズに応じて適切な数値を表示）
                let (current_count, label) = match progress.generation_phase {
                    GenerationPhase::GeneratingState => (0, "pieces"),
                    GenerationPhase::UploadingGpu => (progress.pieces_created, "pieces"),
                    _ => (0, "items"),
                };

                let progress_bar = egui::ProgressBar::new(progress_ratio)
                    .text(format!(
                        "{} / {} {}",
                        current_count.min(progress.total_pieces),
                        progress.total_pieces,
                        label
                    ))
                    .desired_width(400.0);

                ui.add(progress_bar);
                ui.label(format!("Phase: {}", phase_text));
                if let Some(error) = &progress.error {
                    ui.colored_label(egui::Color32::RED, error);
                }
                if ui.button("Return to Title").clicked() {
                    next_state.set(AppState::Menu);
                }
            });
        });
}
