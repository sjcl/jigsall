use crate::resources::*;
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

/// インゲームメニューUI（ESCキーで表示）
pub fn draw_in_game_menu_ui(
    mut contexts: EguiContexts,
    mut game_state: ResMut<GameData>,
    mut next_state: ResMut<NextState<AppState>>,
    mut next_sub_state: ResMut<NextState<GameSubState>>,
) {
    if game_state.current_screen != GameScreen::InGameMenu {
        return;
    }

    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };

    // 半透明の背景を表示してゲーム画面を暗くする
    egui::Area::new(egui::Id::new("in_game_menu_background"))
        .fixed_pos(egui::pos2(0.0, 0.0))
        .show(ctx, |ui| {
            let screen_rect = ctx.screen_rect();
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

                ui.heading("🎮 Game Menu");

                ui.separator();

                // Resume Game ボタン
                if ui
                    .add_sized([200.0, 40.0], egui::Button::new("Resume Game"))
                    .clicked()
                {
                    game_state.current_screen = GameScreen::InGame;
                    next_sub_state.set(GameSubState::Playing);
                    println!("🎮 Resuming game from menu");
                }

                // Return to Title ボタン
                if ui
                    .add_sized([200.0, 40.0], egui::Button::new("Return to Title"))
                    .clicked()
                {
                    game_state.current_screen = GameScreen::Menu;
                    game_state.puzzle_completed = false;
                    game_state.puzzle_progress = 0.0;
                    game_state.needs_reset = true; // パズルと画像設定を完全リセット
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

                ui.label("Press ESC to resume");
            });
        });
}

/// パズル生成進捗UI
pub fn draw_generation_progress_ui(
    mut contexts: EguiContexts,
    progress: Res<PieceGenerationProgress>,
    game_state: Res<GameData>,
) {
    // ゲーム画面で生成中の場合のみ表示
    if game_state.current_screen != GameScreen::InGame || !progress.is_generating {
        return;
    }

    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };

    // 画面下部にプログレスバーを表示
    egui::TopBottomPanel::bottom("generation_progress")
        .min_height(60.0)
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(10.0);

                // 進捗率を計算
                let progress_ratio = match progress.generation_phase {
                    GenerationPhase::NotStarted => 0.0,
                    GenerationPhase::PreparingShapes => {
                        progress.shapes_generated as f32 / progress.total_pieces.max(1) as f32 * 0.4
                    }
                    GenerationPhase::CreatingPieces => {
                        0.4 + (progress.pieces_created as f32 / progress.total_pieces.max(1) as f32
                            * 0.4)
                    }
                    GenerationPhase::SpawningEntities => {
                        0.8 + (progress.pieces_created as f32 / progress.total_pieces.max(1) as f32
                            * 0.2)
                    }
                    GenerationPhase::Completed => 1.0,
                };

                // フェーズ名
                let phase_text = match progress.generation_phase {
                    GenerationPhase::NotStarted => "Starting...",
                    GenerationPhase::PreparingShapes => "Generating shapes",
                    GenerationPhase::CreatingPieces => "Creating pieces",
                    GenerationPhase::SpawningEntities => "Spawning entities",
                    GenerationPhase::Completed => "Completed",
                };

                // プログレスバー（フェーズに応じて適切な数値を表示）
                let (current_count, label) = match progress.generation_phase {
                    GenerationPhase::PreparingShapes => (progress.shapes_generated, "shapes"),
                    GenerationPhase::CreatingPieces => (progress.pieces_created, "pieces"),
                    GenerationPhase::SpawningEntities => (progress.pieces_created, "spawned"),
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
            });
        });
}
