use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use crate::resources::*;
use crate::puzzle_utils::calculate_grid_from_config;

/// ホストゲーム設定UI
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
    
    let Ok(ctx) = contexts.ctx_mut() else { return; };
    
    // 背景のグラデーション
    egui::Area::new(egui::Id::new("host_setup_background"))
        .fixed_pos(egui::pos2(0.0, 0.0))
        .show(ctx, |ui| {
            let screen_rect = ctx.screen_rect();
            ui.allocate_ui_with_layout(
                screen_rect.size(),
                egui::Layout::centered_and_justified(egui::Direction::TopDown),
                |ui| {
                    ui.painter().rect_filled(
                        screen_rect,
                        egui::CornerRadius::ZERO,
                        egui::Color32::from_rgb(25, 35, 45), // 少し明るめのダークブルー
                    );
                },
            );
        });
    
    // Host Setup を画面中央に表示（スクロール可能な大きなウィンドウ）
    egui::Window::new("Host Game Setup")
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .collapsible(false)
        .resizable(false)
        .title_bar(false)
        .show(ctx, |ui| {
            ui.set_min_size(egui::vec2(600.0, 700.0));
            
            egui::ScrollArea::vertical()
                .max_height(650.0)
                .show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.spacing_mut().item_spacing.y = 15.0;
                        
                        ui.add_space(10.0);
                        
                        // タイトル
                        ui.label(
                            egui::RichText::new("🎮 Host Game Setup")
                                .size(32.0)
                                .color(egui::Color32::WHITE)
                                .strong()
                        );
                        
                        ui.add_space(5.0);
                        ui.separator();
                        ui.add_space(10.0);
                        
                        // Piece Mode Section
                        ui.group(|ui| {
                            ui.set_min_width(550.0);
                            ui.vertical(|ui| {
                                ui.label(
                                    egui::RichText::new("🎯 Piece Mode")
                                        .size(20.0)
                                        .color(egui::Color32::WHITE)
                                        .strong()
                                );
                                ui.add_space(8.0);
                                
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 20.0;
                                    
                                    if ui.add_sized([160.0, 30.0], 
                                        egui::RadioButton::new(puzzle_config.piece_mode == PieceMode::SquarePieces, "Aspect Ratio")
                                    ).clicked() {
                                        puzzle_config.piece_mode = PieceMode::SquarePieces;
                                    }
                                    
                                    if ui.add_sized([160.0, 30.0], 
                                        egui::RadioButton::new(puzzle_config.piece_mode == PieceMode::TargetCount, "Target Count")
                                    ).clicked() {
                                        puzzle_config.piece_mode = PieceMode::TargetCount;
                                    }
                                    
                                    if ui.add_sized([160.0, 30.0], 
                                        egui::RadioButton::new(puzzle_config.piece_mode == PieceMode::ManualGrid, "Manual Grid")
                                    ).clicked() {
                                        puzzle_config.piece_mode = PieceMode::ManualGrid;
                                    }
                                });
                            });
                        });
                        
                        // 下位互換性のため、piece_modeの変更をuse_target_modeに反映
                        puzzle_config.use_target_mode = puzzle_config.piece_mode == PieceMode::TargetCount;
                        
                        ui.add_space(5.0);
        
                        // 画像の読み込み状態をチェック
                        let image_loaded = puzzle_image.as_ref()
                            .map(|img| img.size.x > 10.0 && img.size.y > 10.0)
                            .unwrap_or(false);
                        
                        // Image Information Section
                        if image_loaded {
                            let puzzle_image_ref = puzzle_image.as_ref().unwrap();
                            let image_width = puzzle_image_ref.size.x;
                            let image_height = puzzle_image_ref.size.y;
                            
                            ui.group(|ui| {
                                ui.set_min_width(550.0);
                                ui.vertical(|ui| {
                                    ui.label(
                                        egui::RichText::new("📷 Image Information")
                                            .size(18.0)
                                            .color(egui::Color32::LIGHT_BLUE)
                                            .strong()
                                    );
                                    ui.add_space(5.0);
                                    
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            egui::RichText::new("Size:")
                                                .size(14.0)
                                                .color(egui::Color32::LIGHT_GRAY)
                                        );
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            ui.colored_label(
                                                egui::Color32::LIGHT_GREEN, 
                                                egui::RichText::new(format!("{:.0} x {:.0} pixels", image_width, image_height))
                                                    .size(14.0)
                                                    .strong()
                                            );
                                        });
                                    });
                                });
                            });
                            
                            ui.add_space(5.0);
                            
                            // Piece Configuration Section (With Image)
                            ui.group(|ui| {
                            ui.set_min_width(550.0);
                            ui.vertical(|ui| {
                                match puzzle_config.piece_mode {
                                    PieceMode::SquarePieces => {
                                        ui.label(
                                            egui::RichText::new("⚖️ Aspect Ratio Scale")
                                                .size(18.0)
                                                .color(egui::Color32::LIGHT_BLUE)
                                                .strong()
                                        );
                                        ui.add_space(8.0);
                                        
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                egui::RichText::new("Scale:")
                                                    .size(14.0)
                                                    .color(egui::Color32::LIGHT_GRAY)
                                            );
                                            ui.add(
                                                egui::Slider::new(&mut puzzle_config.target_piece_size, 1.0..=50.0)
                                                    .text("x")
                                                    .min_decimals(1)
                                                    .max_decimals(1)
                                            );
                                        });
                                        
                                        ui.label(
                                            egui::RichText::new("Scale 1x = 1 piece, 2x ≈ 4 pieces, 4x ≈ 16 pieces, etc.")
                                                .size(12.0)
                                                .color(egui::Color32::GRAY)
                                                .italics()
                                        );
                                        
                                        ui.add_space(5.0);
                                        
                                        // 最適なグリッドサイズを計算して表示
                                        if let Some((grid_width, grid_height, info)) = calculate_grid_from_config(&puzzle_config, puzzle_image.as_deref()) {
                                            puzzle_config.grid_size = (grid_width, grid_height);
                                            ui.horizontal(|ui| {
                                                ui.label(
                                                    egui::RichText::new("Calculated Grid:")
                                                        .size(14.0)
                                                        .color(egui::Color32::LIGHT_GRAY)
                                                );
                                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                    ui.colored_label(
                                                        egui::Color32::LIGHT_GREEN, 
                                                        egui::RichText::new(info)
                                                            .size(14.0)
                                                            .strong()
                                                    );
                                                });
                                            });
                                        }
                                    }
                                    
                                    PieceMode::TargetCount => {
                                        ui.label(
                                            egui::RichText::new("🎯 Target Piece Count")
                                                .size(18.0)
                                                .color(egui::Color32::LIGHT_BLUE)
                                                .strong()
                                        );
                                        ui.add_space(8.0);
                                        
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                egui::RichText::new("Pieces:")
                                                    .size(14.0)
                                                    .color(egui::Color32::LIGHT_GRAY)
                                            );
                                            ui.add(
                                                egui::Slider::new(&mut puzzle_config.target_piece_count, 4..=10000)
                                                    .text("pieces")
                                                    .min_decimals(0)
                                                    .max_decimals(0)
                                            );
                                        });
                                        
                                        ui.add_space(5.0);
                                        
                                        // 最適なグリッドサイズを計算して表示
                                        if let Some((grid_width, grid_height, info)) = calculate_grid_from_config(&puzzle_config, puzzle_image.as_deref()) {
                                            puzzle_config.grid_size = (grid_width, grid_height);
                                            ui.horizontal(|ui| {
                                                ui.label(
                                                    egui::RichText::new("Calculated Grid:")
                                                        .size(14.0)
                                                        .color(egui::Color32::LIGHT_GRAY)
                                                );
                                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                    ui.colored_label(
                                                        egui::Color32::LIGHT_GREEN, 
                                                        egui::RichText::new(info)
                                                            .size(14.0)
                                                            .strong()
                                                    );
                                                });
                                            });
                                        }
                                    }
                                    
                                    PieceMode::ManualGrid => {
                                        ui.label(
                                            egui::RichText::new("📐 Manual Grid Size")
                                                .size(18.0)
                                                .color(egui::Color32::LIGHT_BLUE)
                                                .strong()
                                        );
                                        ui.add_space(8.0);
                                        
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                egui::RichText::new("Width:")
                                                    .size(14.0)
                                                    .color(egui::Color32::LIGHT_GRAY)
                                            );
                                            ui.add(
                                                egui::Slider::new(&mut puzzle_config.grid_size.0, 2..=1000)
                                                    .text("pieces")
                                                    .min_decimals(0)
                                                    .max_decimals(0)
                                            );
                                        });
                                        
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                egui::RichText::new("Height:")
                                                    .size(14.0)
                                                    .color(egui::Color32::LIGHT_GRAY)
                                            );
                                            ui.add(
                                                egui::Slider::new(&mut puzzle_config.grid_size.1, 2..=1000)
                                                    .text("pieces")
                                                    .min_decimals(0)
                                                    .max_decimals(0)
                                            );
                                        });
                                        
                                        ui.add_space(5.0);
                                        
                                        let total_pieces = puzzle_config.grid_size.0 * puzzle_config.grid_size.1;
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                egui::RichText::new("Total pieces:")
                                                    .size(14.0)
                                                    .color(egui::Color32::LIGHT_GRAY)
                                            );
                                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                ui.colored_label(
                                                    egui::Color32::LIGHT_GREEN, 
                                                    egui::RichText::new(format!("{}", total_pieces))
                                                        .size(14.0)
                                                        .strong()
                                                );
                                            });
                                        });
                                    }
                                }
                            });
                        });
                        
                        ui.add_space(10.0);
                        
                        // Action Buttons Section
                        ui.vertical_centered(|ui| {
                            ui.spacing_mut().item_spacing.y = 15.0;
                            
                            // Start Game Button
                            ui.add_enabled_ui(!puzzle_config.image_path.is_empty() && image_loaded, |ui| {
                                if ui.add_sized([280.0, 50.0], 
                                    egui::Button::new(
                                        egui::RichText::new("🚀 Start Game")
                                            .size(20.0)
                                            .color(egui::Color32::WHITE)
                                    )).clicked() {
                                    // ローカルゲーム開始（ネットワーキング無効のため）
                                    game_state.current_screen = GameScreen::InGame;
                                    // 新しいゲーム開始時はリセットしない（画像設定を保持）
                                    // game_state.needs_reset = true; 
                                }
                            });
                            
                            // Status Message
                            if puzzle_config.image_path.is_empty() {
                                ui.colored_label(
                                    egui::Color32::RED, 
                                    egui::RichText::new("Please select an image before starting the game.")
                                        .size(12.0)
                                        .italics()
                                );
                            } else if !image_loaded {
                                ui.colored_label(
                                    egui::Color32::YELLOW, 
                                    egui::RichText::new("Please wait for the image to load before starting the game.")
                                        .size(12.0)
                                        .italics()
                                );
                            }
                            
                            ui.add_space(5.0);
                            ui.separator();
                            ui.add_space(5.0);
                            
                            // Back Button
                            if ui.add_sized([280.0, 45.0], 
                                egui::Button::new(
                                    egui::RichText::new("⬅️ Back to Menu")
                                        .size(16.0)
                                        .color(egui::Color32::LIGHT_GRAY)
                                )).clicked() {
                                game_state.current_screen = GameScreen::Menu;
                            }
                            
                            ui.add_space(20.0);
                        });
                        
                        } else {
                            // No image loaded: Show status and manual grid option only
                            ui.group(|ui| {
                                ui.set_min_width(550.0);
                                ui.vertical(|ui| {
                                    ui.label(
                                        egui::RichText::new("📷 Image Status")
                                            .size(18.0)
                                            .color(egui::Color32::LIGHT_BLUE)
                                            .strong()
                                    );
                                    ui.add_space(5.0);
                                    
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            egui::RichText::new("Status:")
                                                .size(14.0)
                                                .color(egui::Color32::LIGHT_GRAY)
                                        );
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            if puzzle_config.image_path.is_empty() {
                                                ui.colored_label(
                                                    egui::Color32::LIGHT_RED,
                                                    egui::RichText::new("No image selected")
                                                        .size(14.0)
                                                        .strong()
                                                );
                                            } else {
                                                ui.colored_label(
                                                    egui::Color32::YELLOW,
                                                    egui::RichText::new("Loading image...")
                                                        .size(14.0)
                                                        .strong()
                                                );
                                            }
                                        });
                                    });
                                });
                            });
                            
                            ui.add_space(5.0);
                            
                            // Piece Configuration Section (Limited without image)
                            if puzzle_config.piece_mode == PieceMode::ManualGrid {
                                ui.group(|ui| {
                                    ui.set_min_width(550.0);
                                    ui.vertical(|ui| {
                                        ui.label(
                                            egui::RichText::new("📐 Manual Grid Size")
                                                .size(18.0)
                                                .color(egui::Color32::LIGHT_BLUE)
                                                .strong()
                                        );
                                        ui.add_space(8.0);
                                        
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                egui::RichText::new("Width:")
                                                    .size(14.0)
                                                    .color(egui::Color32::LIGHT_GRAY)
                                            );
                                            ui.add(
                                                egui::Slider::new(&mut puzzle_config.grid_size.0, 2..=1000)
                                                    .text("pieces")
                                                    .min_decimals(0)
                                                    .max_decimals(0)
                                            );
                                        });
                                        
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                egui::RichText::new("Height:")
                                                    .size(14.0)
                                                    .color(egui::Color32::LIGHT_GRAY)
                                            );
                                            ui.add(
                                                egui::Slider::new(&mut puzzle_config.grid_size.1, 2..=1000)
                                                    .text("pieces")
                                                    .min_decimals(0)
                                                    .max_decimals(0)
                                            );
                                        });
                                        
                                        ui.add_space(5.0);
                                        
                                        let total_pieces = puzzle_config.grid_size.0 * puzzle_config.grid_size.1;
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                egui::RichText::new("Total pieces:")
                                                    .size(14.0)
                                                    .color(egui::Color32::LIGHT_GRAY)
                                            );
                                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                ui.colored_label(
                                                    egui::Color32::LIGHT_GREEN, 
                                                    egui::RichText::new(format!("{}", total_pieces))
                                                        .size(14.0)
                                                        .strong()
                                                );
                                            });
                                        });
                                    });
                                });
                                
                                ui.add_space(5.0);
                            } else {
                                ui.colored_label(
                                    egui::Color32::GRAY, 
                                    egui::RichText::new("Please select an image to configure puzzle settings.")
                                        .size(14.0)
                                        .italics()
                                );
                                ui.add_space(10.0);
                            }
                        }
        
                        // Network Settings Section
                        ui.group(|ui| {
                            ui.set_min_width(550.0);
                            ui.vertical(|ui| {
                                ui.label(
                                    egui::RichText::new("🌐 Network Settings")
                                        .size(18.0)
                                        .color(egui::Color32::LIGHT_BLUE)
                                        .strong()
                                );
                                ui.add_space(8.0);
                                
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new("Port:")
                                            .size(14.0)
                                            .color(egui::Color32::LIGHT_GRAY)
                                    );
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        let mut port_string = network_info.port.to_string();
                                        if ui.add_sized([100.0, 25.0], egui::TextEdit::singleline(&mut port_string)).changed() {
                                            if let Ok(port) = port_string.parse::<u16>() {
                                                network_info.port = port;
                                            }
                                        }
                                    });
                                });
                            });
                        });
                        
                        ui.add_space(5.0);
                        
                        // Image Selection Section
                        ui.group(|ui| {
                            ui.set_min_width(550.0);
                            ui.vertical(|ui| {
                                ui.label(
                                    egui::RichText::new("🖼️ Puzzle Image")
                                        .size(18.0)
                                        .color(egui::Color32::LIGHT_BLUE)
                                        .strong()
                                );
                                ui.add_space(8.0);
                                
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new("Selected:")
                                            .size(14.0)
                                            .color(egui::Color32::LIGHT_GRAY)
                                    );
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        if puzzle_config.image_path.is_empty() {
                                            ui.colored_label(
                                                egui::Color32::LIGHT_RED,
                                                egui::RichText::new("No image selected")
                                                    .size(14.0)
                                                    .strong()
                                            );
                                        } else {
                                            ui.label(
                                                egui::RichText::new(&puzzle_config.image_path)
                                                    .size(14.0)
                                                    .color(egui::Color32::LIGHT_GREEN)
                                                    .strong()
                                            );
                                        }
                                    });
                                });
                                
                                ui.add_space(8.0);
                                
                                if ui.add_sized([200.0, 35.0], 
                                    egui::Button::new(
                                        egui::RichText::new("📁 Select Puzzle Image")
                                            .size(16.0)
                                    )).clicked() {
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
                            });
                        });
                    });
                });
        });
}

/// ゲーム参加UI
pub fn draw_join_game_ui(
    mut contexts: EguiContexts,
    mut game_state: ResMut<GameState>,
    mut network_info: ResMut<NetworkInfo>,
    _commands: Commands,
) {
    if game_state.current_screen != GameScreen::JoinGame {
        return;
    }
    
    let Ok(ctx) = contexts.ctx_mut() else { return; };
    
    // 背景のグラデーション
    egui::Area::new(egui::Id::new("join_game_background"))
        .fixed_pos(egui::pos2(0.0, 0.0))
        .show(ctx, |ui| {
            let screen_rect = ctx.screen_rect();
            ui.allocate_ui_with_layout(
                screen_rect.size(),
                egui::Layout::centered_and_justified(egui::Direction::TopDown),
                |ui| {
                    ui.painter().rect_filled(
                        screen_rect,
                        egui::CornerRadius::ZERO,
                        egui::Color32::from_rgb(30, 40, 50), // ダークブルーグリーン
                    );
                },
            );
        });
    
    // Join Game を画面中央に表示
    egui::Window::new("Join Game")
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .collapsible(false)
        .resizable(false)
        .title_bar(false)
        .show(ctx, |ui| {
            ui.set_min_size(egui::vec2(500.0, 400.0));
            
            ui.vertical_centered(|ui| {
                ui.spacing_mut().item_spacing.y = 20.0;
                
                ui.add_space(20.0);
                
                // タイトル
                ui.label(
                    egui::RichText::new("🔗 Join Game")
                        .size(32.0)
                        .color(egui::Color32::WHITE)
                        .strong()
                );
                
                ui.add_space(10.0);
                ui.separator();
                ui.add_space(20.0);
                
                // Server Connection Section
                ui.group(|ui| {
                    ui.set_min_width(450.0);
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new("🌐 Server Connection")
                                .size(20.0)
                                .color(egui::Color32::LIGHT_BLUE)
                                .strong()
                        );
                        ui.add_space(10.0);
                        
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("Server Address:")
                                    .size(14.0)
                                    .color(egui::Color32::LIGHT_GRAY)
                            );
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.add_sized([200.0, 25.0], egui::TextEdit::singleline(&mut network_info.server_address));
                            });
                        });
                        
                        ui.add_space(8.0);
                        
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("Port:")
                                    .size(14.0)
                                    .color(egui::Color32::LIGHT_GRAY)
                            );
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                let mut port_string = network_info.port.to_string();
                                if ui.add_sized([100.0, 25.0], egui::TextEdit::singleline(&mut port_string)).changed() {
                                    if let Ok(port) = port_string.parse::<u16>() {
                                        network_info.port = port;
                                    }
                                }
                            });
                        });
                    });
                });
                
                ui.add_space(20.0);
                
                // Action Buttons Section
                ui.vertical_centered(|ui| {
                    ui.spacing_mut().item_spacing.y = 15.0;
                    
                    // Connect Button (Disabled due to networking being disabled)
                    ui.add_enabled_ui(false, |ui| {
                        ui.add_sized([280.0, 50.0], 
                            egui::Button::new(
                                egui::RichText::new("🔌 Connect to Server")
                                    .size(20.0)
                                    .color(egui::Color32::GRAY)
                            ))
                    });
                    
                    // Status Message
                    ui.colored_label(
                        egui::Color32::YELLOW, 
                        egui::RichText::new("Multiplayer functionality is currently disabled.")
                            .size(12.0)
                            .italics()
                    );
                    
                    ui.add_space(5.0);
                    ui.separator();
                    ui.add_space(5.0);
                    
                    // Back Button
                    if ui.add_sized([280.0, 45.0], 
                        egui::Button::new(
                            egui::RichText::new("⬅️ Back to Menu")
                                .size(16.0)
                                .color(egui::Color32::LIGHT_GRAY)
                        )).clicked() {
                        game_state.current_screen = GameScreen::Menu;
                    }
                    
                    ui.add_space(20.0);
                });
            });
        });
}