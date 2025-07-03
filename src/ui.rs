use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPlugin};
use crate::resources::*;
use crate::networking::{start_server, start_client};
use std::path::PathBuf;

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_fonts)
            .add_systems(Update, (
                draw_menu_ui,
                draw_game_ui,
                draw_host_setup_ui,
                draw_join_game_ui,
                draw_completion_ui,
            ));
    }
}

fn setup_fonts(mut contexts: EguiContexts) {
    let ctx = contexts.ctx_mut();
    
    // デフォルトの日本語フォント設定
    let mut fonts = egui::FontDefinitions::default();
    
    // Windowsの標準日本語フォントを追加
    #[cfg(target_os = "windows")]
    {
        // Windows標準の日本語フォント
        if let Ok(font_data) = std::fs::read("C:/Windows/Fonts/msgothic.ttc") {
            fonts.font_data.insert(
                "msgothic".to_owned(),
                egui::FontData::from_owned(font_data),
            );
            
            fonts.families.entry(egui::FontFamily::Proportional).or_default()
                .insert(0, "msgothic".to_owned());
        } else if let Ok(font_data) = std::fs::read("C:/Windows/Fonts/meiryo.ttc") {
            fonts.font_data.insert(
                "meiryo".to_owned(),
                egui::FontData::from_owned(font_data),
            );
            
            fonts.families.entry(egui::FontFamily::Proportional).or_default()
                .insert(0, "meiryo".to_owned());
        }
    }
    
    // フォールバック: 英語UIに変更
    ctx.set_fonts(fonts);
}

fn draw_menu_ui(
    mut contexts: EguiContexts,
    mut game_state: ResMut<GameState>,
) {
    if game_state.current_screen != GameScreen::Menu {
        return;
    }
    
    egui::CentralPanel::default().show(contexts.ctx_mut(), |ui| {
        ui.heading("Puzzella - Multiplayer Jigsaw Puzzle");
        
        ui.separator();
        
        if ui.button("Host Game").clicked() {
            game_state.current_screen = GameScreen::HostSetup;
            game_state.is_host = true;
        }
        
        if ui.button("Join Game").clicked() {
            game_state.current_screen = GameScreen::JoinGame;
            game_state.is_host = false;
        }
        
        ui.separator();
        
        if ui.button("Exit").clicked() {
            std::process::exit(0);
        }
    });
}

fn draw_host_setup_ui(
    mut contexts: EguiContexts,
    mut game_state: ResMut<GameState>,
    mut puzzle_config: ResMut<PuzzleConfig>,
    mut network_info: ResMut<NetworkInfo>,
    mut commands: Commands,
) {
    if game_state.current_screen != GameScreen::HostSetup {
        return;
    }
    
    egui::CentralPanel::default().show(contexts.ctx_mut(), |ui| {
        ui.heading("Host Game Setup");
        
        ui.separator();
        
        ui.horizontal(|ui| {
            ui.label("Grid Size:");
            ui.add(egui::Slider::new(&mut puzzle_config.grid_size.0, 2..=20).text("Width"));
            ui.add(egui::Slider::new(&mut puzzle_config.grid_size.1, 2..=20).text("Height"));
        });
        
        ui.horizontal(|ui| {
            ui.label("Port:");
            let mut port_string = network_info.port.to_string();
            if ui.text_edit_singleline(&mut port_string).changed() {
                if let Ok(port) = port_string.parse::<u16>() {
                    network_info.port = port;
                }
            }
        });
        
        ui.separator();
        
        ui.horizontal(|ui| {
            ui.label("Selected Image:");
            if puzzle_config.image_path.is_empty() {
                ui.colored_label(egui::Color32::RED, "No image selected");
            } else {
                ui.label(&puzzle_config.image_path);
            }
        });
        
        if ui.button("Select Puzzle Image").clicked() {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("Image files", &["png", "jpg", "jpeg", "bmp", "gif"])
                .pick_file()
            {
                puzzle_config.image_path = path.to_string_lossy().to_string();
                println!("Selected image: {}", puzzle_config.image_path);
            }
        }
        
        ui.separator();
        
        ui.add_enabled_ui(!puzzle_config.image_path.is_empty(), |ui| {
            if ui.button("Start Game").clicked() {
                if let Err(e) = start_server(&mut commands, &network_info) {
                    eprintln!("Server start error: {}", e);
                } else {
                    game_state.current_screen = GameScreen::InGame;
                }
            }
        });
        
        if ui.button("Back").clicked() {
            game_state.current_screen = GameScreen::Menu;
        }
    });
}

fn draw_join_game_ui(
    mut contexts: EguiContexts,
    mut game_state: ResMut<GameState>,
    mut network_info: ResMut<NetworkInfo>,
    mut commands: Commands,
) {
    if game_state.current_screen != GameScreen::JoinGame {
        return;
    }
    
    egui::CentralPanel::default().show(contexts.ctx_mut(), |ui| {
        ui.heading("Join Game");
        
        ui.separator();
        
        ui.horizontal(|ui| {
            ui.label("Server Address:");
            ui.text_edit_singleline(&mut network_info.server_address);
        });
        
        ui.horizontal(|ui| {
            ui.label("Port:");
            let mut port_string = network_info.port.to_string();
            if ui.text_edit_singleline(&mut port_string).changed() {
                if let Ok(port) = port_string.parse::<u16>() {
                    network_info.port = port;
                }
            }
        });
        
        ui.separator();
        
        if ui.button("Connect").clicked() {
            if let Err(e) = start_client(&mut commands, &network_info) {
                eprintln!("Client connection error: {}", e);
            } else {
                game_state.current_screen = GameScreen::InGame;
            }
        }
        
        if ui.button("Back").clicked() {
            game_state.current_screen = GameScreen::Menu;
        }
    });
}

fn draw_game_ui(
    mut contexts: EguiContexts,
    game_state: Res<GameState>,
) {
    if game_state.current_screen != GameScreen::InGame {
        return;
    }
    
    egui::TopBottomPanel::top("game_info").show(contexts.ctx_mut(), |ui| {
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
    
    egui::SidePanel::right("players").show(contexts.ctx_mut(), |ui| {
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

fn draw_completion_ui(
    mut contexts: EguiContexts,
    mut game_state: ResMut<GameState>,
) {
    if game_state.current_screen != GameScreen::GameComplete {
        return;
    }
    
    egui::CentralPanel::default().show(contexts.ctx_mut(), |ui| {
        ui.heading("🎉 Puzzle Complete!");
        
        ui.separator();
        
        ui.label("Congratulations! You have completed the puzzle.");
        
        ui.separator();
        
        if ui.button("New Game").clicked() {
            game_state.current_screen = GameScreen::Menu;
            game_state.puzzle_completed = false;
            game_state.puzzle_progress = 0.0;
        }
        
        if ui.button("Exit").clicked() {
            std::process::exit(0);
        }
    });
}