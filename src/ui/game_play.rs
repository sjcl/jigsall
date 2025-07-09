use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use crate::resources::*;

/// インゲームUI（プレイ中のUI）
pub fn draw_game_ui(
    mut contexts: EguiContexts,
    game_state: Res<GameState>,
) {
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
        });
    });
    
    egui::SidePanel::right("players").show(ctx, |ui| {
        ui.heading("Players");
        ui.separator();
        
        for player in &game_state.players {
            ui.horizontal(|ui| {
                ui.label(&player.name);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(format!("Score: {}", player.score));
                });
            });
        }
    });
}

/// ゲーム完了UI
pub fn draw_completion_ui(
    mut contexts: EguiContexts,
    mut game_state: ResMut<GameState>,
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
        }
        
        if ui.button("Exit").clicked() {
            std::process::exit(0);
        }
    });
}