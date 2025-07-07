use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use crate::resources::*;
use crate::puzzle_utils::calculate_grid_from_config;
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
    asset_server: Res<AssetServer>,
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
            ui.label("Piece Mode:");
            ui.radio_value(&mut puzzle_config.piece_mode, PieceMode::TargetCount, "Target Piece Count");
            ui.radio_value(&mut puzzle_config.piece_mode, PieceMode::ManualGrid, "Manual Grid Size");
            ui.radio_value(&mut puzzle_config.piece_mode, PieceMode::SquarePieces, "Aspect Ratio Scale");
        });
        
        // 下位互換性のため、piece_modeの変更をuse_target_modeに反映
        puzzle_config.use_target_mode = puzzle_config.piece_mode == PieceMode::TargetCount;
        
        ui.separator();
        
        // 画像の読み込み状態をチェック
        let image_loaded = puzzle_image.as_ref()
            .map(|img| img.size.x > 10.0 && img.size.y > 10.0)
            .unwrap_or(false);
        
        if image_loaded {
            let puzzle_image_ref = puzzle_image.as_ref().unwrap();
            let image_width = puzzle_image_ref.size.x;
            let image_height = puzzle_image_ref.size.y;
            
            ui.horizontal(|ui| {
                ui.label("Image Size:");
                ui.colored_label(egui::Color32::BLUE, format!("{:.0}x{:.0}", image_width, image_height));
            });
            
            ui.separator();
            
            match puzzle_config.piece_mode {
                PieceMode::TargetCount => {
                    // ターゲットピース数モード
                    ui.horizontal(|ui| {
                        ui.label("Target Piece Count:");
                        ui.add(egui::Slider::new(&mut puzzle_config.target_piece_count, 4..=10000).text("pieces"));
                    });
                    
                    // 最適なグリッドサイズを計算して表示
                    if let Some((grid_width, grid_height, info)) = calculate_grid_from_config(&puzzle_config, puzzle_image.as_deref()) {
                        puzzle_config.grid_size = (grid_width, grid_height);
                        ui.horizontal(|ui| {
                            ui.label("Calculated Grid:");
                            ui.colored_label(egui::Color32::GREEN, info);
                        });
                    }
                }
                
                PieceMode::ManualGrid => {
                    // 手動グリッドサイズモード
                    ui.horizontal(|ui| {
                        ui.label("Grid Size:");
                        ui.add(egui::Slider::new(&mut puzzle_config.grid_size.0, 2..=1000).text("Width"));
                        ui.add(egui::Slider::new(&mut puzzle_config.grid_size.1, 2..=1000).text("Height"));
                    });
                    
                    let total_pieces = puzzle_config.grid_size.0 * puzzle_config.grid_size.1;
                    ui.horizontal(|ui| {
                        ui.label("Total pieces:");
                        ui.colored_label(egui::Color32::BLUE, format!("{}", total_pieces));
                    });
                }
                
                PieceMode::SquarePieces => {
                    // 縦横比保持スケールモード
                    ui.horizontal(|ui| {
                        ui.label("Grid Scale:");
                        ui.add(egui::Slider::new(&mut puzzle_config.target_piece_size, 1.0..=50.0).text("x"));
                    });
                    
                    ui.label("Scale 1x = 1 piece, 2x ≈ 4 pieces, 4x ≈ 16 pieces, etc.");
                    
                    // 最適なグリッドサイズを計算して表示
                    if let Some((grid_width, grid_height, info)) = calculate_grid_from_config(&puzzle_config, puzzle_image.as_deref()) {
                        puzzle_config.grid_size = (grid_width, grid_height);
                        ui.horizontal(|ui| {
                            ui.label("Calculated Grid:");
                            ui.colored_label(egui::Color32::GREEN, info);
                        });
                    }
                }
            }
        } else {
            // 画像が読み込まれていない場合の表示
            ui.horizontal(|ui| {
                ui.label("Image Status:");
                if puzzle_config.image_path.is_empty() {
                    ui.colored_label(egui::Color32::RED, "No image selected");
                } else {
                    ui.colored_label(egui::Color32::YELLOW, "Loading image...");
                }
            });
            
            ui.separator();
            
            // 画像が読み込まれていない場合でも、ManualGridモードのみ使用可能
            match puzzle_config.piece_mode {
                PieceMode::TargetCount => {
                    ui.add_enabled_ui(false, |ui| {
                        ui.horizontal(|ui| {
                            ui.label("Target Piece Count:");
                            ui.add(egui::Slider::new(&mut puzzle_config.target_piece_count, 4..=10000).text("pieces"));
                        });
                    });
                    ui.colored_label(egui::Color32::GRAY, "Please select an image first to calculate optimal grid size.");
                }
                
                PieceMode::ManualGrid => {
                    // 手動グリッドサイズモードは画像なしでも使用可能
                    ui.horizontal(|ui| {
                        ui.label("Grid Size:");
                        ui.add(egui::Slider::new(&mut puzzle_config.grid_size.0, 2..=1000).text("Width"));
                        ui.add(egui::Slider::new(&mut puzzle_config.grid_size.1, 2..=1000).text("Height"));
                    });
                    
                    let total_pieces = puzzle_config.grid_size.0 * puzzle_config.grid_size.1;
                    ui.horizontal(|ui| {
                        ui.label("Total pieces:");
                        ui.colored_label(egui::Color32::BLUE, format!("{}", total_pieces));
                    });
                }
                
                PieceMode::SquarePieces => {
                    ui.add_enabled_ui(false, |ui| {
                        ui.horizontal(|ui| {
                            ui.label("Grid Scale:");
                            ui.add(egui::Slider::new(&mut puzzle_config.target_piece_size, 1.0..=50.0).text("x"));
                        });
                    });
                    ui.colored_label(egui::Color32::GRAY, "Please select an image first to calculate aspect ratio-based grid.");
                }
            }
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
                    .add_filter("Image files", &["png", "jpg", "jpeg", "bmp", "gif", "webp"])
                    .pick_file()
                {
                    let original_path = path.to_string_lossy().to_string();
                    println!("Selected image: {}", original_path);
                    println!("File exists: {}", std::path::Path::new(&original_path).exists());
                    println!("File metadata: {:?}", std::fs::metadata(&original_path));
                    
                    // 画像をassetsフォルダにコピー
                    if let Some(filename) = path.file_name() {
                        let assets_path = format!("assets/{}", filename.to_string_lossy());
                        
                        // assetsディレクトリが存在しない場合は作成
                        std::fs::create_dir_all("assets").ok();
                        
                        // ファイルをコピー
                        if let Err(e) = std::fs::copy(&original_path, &assets_path) {
                            println!("Failed to copy image to assets folder: {}", e);
                            puzzle_config.image_path = original_path; // オリジナルパスを使用
                        } else {
                            puzzle_config.image_path = filename.to_string_lossy().to_string(); // assets内の相対パスを使用
                            println!("Image copied to: {}", assets_path);
                        }
                    } else {
                        puzzle_config.image_path = original_path;
                    }
                    
                    // 画像選択時に即座にロードを開始
                    let image_handle = asset_server.load(&puzzle_config.image_path);
                    
                    // PuzzleImageリソースを作成または更新
                    commands.insert_resource(PuzzleImage {
                        handle: image_handle,
                        size: Vec2::new(1.0, 1.0), // 小さな値で初期化、読み込み中を示す
                    });
                }
            }
            #[cfg(not(target_os = "windows"))]
            {
                // テスト用の固定パス（実際のプロジェクトでは適切な画像ファイルを指定）
                puzzle_config.image_path = "test_image.png".to_string();
                println!("Using test image: {}", puzzle_config.image_path);
                
                // 画像選択時に即座にロードを開始
                let image_handle = asset_server.load(&puzzle_config.image_path);
                
                // PuzzleImageリソースを作成または更新
                commands.insert_resource(PuzzleImage {
                    handle: image_handle,
                    size: Vec2::new(1.0, 1.0), // 小さな値で初期化、読み込み中を示す
                });
            }
        }
        
        ui.separator();
        
        ui.add_enabled_ui(!puzzle_config.image_path.is_empty() && image_loaded, |ui| {
            if ui.button("Start Game").clicked() {
                // ローカルゲーム開始（ネットワーキング無効のため）
                game_state.current_screen = GameScreen::InGame;
                // 新しいゲーム開始時はリセットしない（画像設定を保持）
                // game_state.needs_reset = true; 
            }
        });
        
        // 画像が選択されていない、または読み込まれていない場合のメッセージ
        if puzzle_config.image_path.is_empty() {
            ui.colored_label(egui::Color32::RED, "Please select an image before starting the game.");
        } else if !image_loaded {
            ui.colored_label(egui::Color32::YELLOW, "Please wait for the image to load before starting the game.");
        }
        
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

pub fn draw_in_game_menu_ui(
    mut contexts: EguiContexts,
    mut game_state: ResMut<GameState>,
) {
    if game_state.current_screen != GameScreen::InGameMenu {
        return;
    }
    
    let Ok(ctx) = contexts.ctx_mut() else { return; };
    
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
                        egui::Rounding::ZERO,
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
                if ui.add_sized([200.0, 40.0], egui::Button::new("Resume Game")).clicked() {
                    game_state.current_screen = GameScreen::InGame;
                    println!("🎮 Resuming game from menu");
                }
                
                // Return to Title ボタン
                if ui.add_sized([200.0, 40.0], egui::Button::new("Return to Title")).clicked() {
                    game_state.current_screen = GameScreen::Menu;
                    game_state.puzzle_completed = false;
                    game_state.puzzle_progress = 0.0;
                    game_state.needs_reset = true; // パズルと画像設定を完全リセット
                    println!("🎮 Returning to title screen");
                }
                
                ui.separator();
                
                // Exit Game ボタン
                if ui.add_sized([200.0, 40.0], egui::Button::new("Exit Game")).clicked() {
                    println!("🎮 Exiting game");
                    std::process::exit(0);
                }
                
                ui.separator();
                
                ui.label("Press ESC to resume");
            });
        });
}