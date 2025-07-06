use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use crate::resources::*;
// use crate::networking::{start_server, start_client}; // ネットワーキング無効化
use std::path::PathBuf;
use puzzle_paths::generate_columns_rows_numbers;

fn setup_fonts(mut contexts: EguiContexts) {
    let Ok(ctx) = contexts.ctx_mut() else { return; };
    
    // デフォルトの日本語フォント設定
    let mut fonts = egui::FontDefinitions::default();
    
    // Windowsの標準日本語フォントを追加
    #[cfg(target_os = "windows")]
    {
        // Windows標準の日本語フォント
        if let Ok(font_data) = std::fs::read("C:/Windows/Fonts/msgothic.ttc") {
            fonts.font_data.insert(
                "msgothic".to_owned(),
                egui::FontData::from_owned(font_data).into(),
            );
            
            fonts.families.entry(egui::FontFamily::Proportional).or_default()
                .insert(0, "msgothic".to_owned());
        } else if let Ok(font_data) = std::fs::read("C:/Windows/Fonts/meiryo.ttc") {
            fonts.font_data.insert(
                "meiryo".to_owned(),
                egui::FontData::from_owned(font_data).into(),
            );
            
            fonts.families.entry(egui::FontFamily::Proportional).or_default()
                .insert(0, "meiryo".to_owned());
        }
    }
    
    // フォールバック: 英語UIに変更
    ctx.set_fonts(fonts);
}

pub fn draw_menu_ui(
    mut contexts: EguiContexts,
    mut game_state: ResMut<GameState>,
) {
    if game_state.current_screen != GameScreen::Menu {
        return;
    }
    
    if let Ok(ctx) = contexts.ctx_mut() {
        egui::CentralPanel::default().show(ctx, |ui| {
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
}

pub fn draw_host_setup_ui(
    mut contexts: EguiContexts,
    mut game_state: ResMut<GameState>,
    mut puzzle_config: ResMut<PuzzleConfig>,
    mut network_info: ResMut<NetworkInfo>,
    mut commands: Commands,
    puzzle_image: Option<Res<PuzzleImage>>,
) {
    if game_state.current_screen != GameScreen::HostSetup {
        return;
    }
    
    if let Ok(ctx) = contexts.ctx_mut() {
        egui::CentralPanel::default().show(ctx, |ui| {
        ui.heading("Host Game Setup");
        
        ui.separator();
        
        // ピース数設定モードの切り替え
        ui.horizontal(|ui| {
            ui.label("Piece Count Mode:");
            ui.radio_value(&mut puzzle_config.use_target_mode, true, "Target Piece Count");
            ui.radio_value(&mut puzzle_config.use_target_mode, false, "Manual Grid Size");
        });
        
        ui.separator();
        
        if puzzle_config.use_target_mode {
            // ターゲットピース数モード
            ui.horizontal(|ui| {
                ui.label("Target Piece Count:");
                ui.add(egui::Slider::new(&mut puzzle_config.target_piece_count, 4..=1000).text("pieces"));
            });
            
            // 最適なグリッドサイズを計算して表示
            let (image_width, image_height) = if let Some(puzzle_image) = puzzle_image.as_ref() {
                // 実際の画像サイズを使用
                (puzzle_image.size.x, puzzle_image.size.y)
            } else {
                // 画像が読み込まれていない場合は標準的な16:9の比率を仮定
                (1920.0, 1080.0)
            };
            
            let (optimal_cols, optimal_rows) = generate_columns_rows_numbers(
                image_width, 
                image_height, 
                puzzle_config.target_piece_count
            );
            
            puzzle_config.grid_size = (optimal_cols, optimal_rows);
            
            ui.horizontal(|ui| {
                ui.label("Image Size:");
                if let Some(_) = puzzle_image.as_ref() {
                    ui.colored_label(egui::Color32::BLUE, format!("{:.0}x{:.0}", image_width, image_height));
                } else {
                    ui.colored_label(egui::Color32::GRAY, format!("{:.0}x{:.0} (assumed)", image_width, image_height));
                }
            });
            
            ui.horizontal(|ui| {
                ui.label("Calculated Grid:");
                ui.colored_label(egui::Color32::GREEN, format!("{}x{} = {} pieces", 
                    optimal_cols, optimal_rows, optimal_cols * optimal_rows));
            });
            
            // 実際のピース数が目標と異なる場合は警告
            let actual_pieces = optimal_cols * optimal_rows;
            if actual_pieces != puzzle_config.target_piece_count {
                ui.horizontal(|ui| {
                    ui.colored_label(egui::Color32::YELLOW, format!(
                        "Note: Actual pieces ({}) differs from target ({})", 
                        actual_pieces, puzzle_config.target_piece_count
                    ));
                });
            }
        } else {
            // 手動グリッドサイズモード
            ui.horizontal(|ui| {
                ui.label("Grid Size:");
                ui.add(egui::Slider::new(&mut puzzle_config.grid_size.0, 2..=100).text("Width"));
                ui.add(egui::Slider::new(&mut puzzle_config.grid_size.1, 2..=100).text("Height"));
            });
            
            let total_pieces = puzzle_config.grid_size.0 * puzzle_config.grid_size.1;
            ui.horizontal(|ui| {
                ui.label("Total pieces:");
                ui.colored_label(egui::Color32::BLUE, format!("{}", total_pieces));
            });
        }
        
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
            // WSL環境でのGTK問題回避のため、テスト用画像パスを使用
            #[cfg(target_os = "windows")]
            {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Image files", &["png", "jpg", "jpeg", "bmp", "gif"])
                    .pick_file()
                {
                    puzzle_config.image_path = path.to_string_lossy().to_string();
                    println!("Selected image: {}", puzzle_config.image_path);
                    println!("File exists: {}", std::path::Path::new(&puzzle_config.image_path).exists());
                    println!("File metadata: {:?}", std::fs::metadata(&puzzle_config.image_path));
                }
            }
            #[cfg(not(target_os = "windows"))]
            {
                // テスト用の固定パス（実際のプロジェクトでは適切な画像ファイルを指定）
                puzzle_config.image_path = "test_image.png".to_string();
                println!("Using test image: {}", puzzle_config.image_path);
            }
        }
        
        ui.separator();
        
        ui.add_enabled_ui(!puzzle_config.image_path.is_empty(), |ui| {
            if ui.button("Start Game").clicked() {
                // ローカルゲーム開始（ネットワーキング無効のため）
                game_state.current_screen = GameScreen::InGame;
                game_state.needs_reset = true; // 新しいゲーム開始時にリセット
            }
        });
        
        if ui.button("Back").clicked() {
            game_state.current_screen = GameScreen::Menu;
        }
    });
    }
}

pub fn draw_join_game_ui(
    mut contexts: EguiContexts,
    mut game_state: ResMut<GameState>,
    mut network_info: ResMut<NetworkInfo>,
    mut commands: Commands,
) {
    if game_state.current_screen != GameScreen::JoinGame {
        return;
    }
    
    if let Ok(ctx) = contexts.ctx_mut() {
        egui::CentralPanel::default().show(ctx, |ui| {
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
            // ネットワーキング無効のため、一時的に無効化
            // game_state.current_screen = GameScreen::InGame;
        }
        
        if ui.button("Back").clicked() {
            game_state.current_screen = GameScreen::Menu;
        }
    });
    }
}

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