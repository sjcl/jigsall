use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use crate::resources::*;

/// インゲームUI（プレイ中のUI）
pub fn draw_game_ui(
    mut contexts: EguiContexts,
    game_state: Res<GameData>,
) {
    let _span = info_span!("draw_game_ui").entered();
    if game_state.current_screen != GameScreen::InGame {
        return;
    }
    
    let Ok(ctx) = contexts.ctx_mut() else { return; };
    egui::TopBottomPanel::top("game_info").show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.label(format!("Progress: {:.1}%", game_state.puzzle_progress * 100.0));
            ui.separator();
            ui.label(format!("Players: {}", game_state.players.len()));
            
            if game_state.is_host {
                ui.separator();
                ui.label("(Host)");
            }
            
            ui.separator();
            ui.label("Click and drag puzzle pieces to move them");
            ui.separator();
            ui.label("Hold Tab to view players");
        });
    });
}

/// プレイヤー一覧オーバーレイ（Tabキーで表示）
pub fn draw_players_overlay(
    mut contexts: EguiContexts,
    game_state: Res<GameData>,
) {
    let _span = info_span!("draw_players_overlay").entered();
    if game_state.current_screen != GameScreen::InGame {
        return;
    }
    
    let Ok(ctx) = contexts.ctx_mut() else { return; };
    
    // 半透明の背景を表示
    egui::Area::new(egui::Id::new("players_overlay_background"))
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
                        egui::Color32::from_black_alpha(100), // 少し薄めの半透明
                    );
                },
            );
        });
    
    // プレイヤー一覧を画面上部中央に表示（上部UIの下に配置）
    egui::Window::new("Players")
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 120.0)) // 上部UIを避けて配置
        .collapsible(false)
        .resizable(false)
        .title_bar(true)
        .show(ctx, |ui| {
            ui.set_min_width(400.0);
            
            // ウィンドウの高さを画面の半分に制限
            let screen_height = ctx.screen_rect().height();
            let max_height = screen_height * 0.5;
            
            egui::ScrollArea::vertical()
                .max_height(max_height)
                .show(ui, |ui| {
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 10.0;
                        
                        if game_state.players.is_empty() {
                            ui.centered_and_justified(|ui| {
                                ui.label("No players connected");
                            });
                        } else {
                            // プレイヤー一覧
                            for player in &game_state.players {
                                ui.group(|ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(&player.name);
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            ui.label(format!("Score: {}", player.score));
                                        });
                                    });
                                });
                            }
                        }
                    });
                });
        });
}

/// ゲーム完了UI
pub fn draw_completion_ui(
    mut contexts: EguiContexts,
    mut game_state: ResMut<GameData>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    if game_state.current_screen != GameScreen::GameComplete {
        return;
    }
    
    let Ok(ctx) = contexts.ctx_mut() else { return; };
    egui::CentralPanel::default().show(ctx, |ui| {
        ui.heading("🎉 Puzzle Complete!");
        
        ui.separator();
        
        ui.label("Congratulations! You have completed the puzzle.");
        
        ui.separator();
        
        if ui.button("New Game").clicked() {
            game_state.current_screen = GameScreen::Menu;
            game_state.puzzle_completed = false;
            game_state.puzzle_progress = 0.0;
            game_state.needs_reset = true; // パズルリセットフラグを設定
            next_state.set(AppState::Menu);
        }
        
        if ui.button("Exit").clicked() {
            std::process::exit(0);
        }
    });
}