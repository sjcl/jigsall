use crate::grid::calculate_grid_from_config;
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use puzzella_game::asset_reader::ExternalFileRegistry;
use puzzella_game::resources::*;

/// ローカルゲーム設定UI
pub fn draw_game_setup_ui(
    mut contexts: EguiContexts,
    mut puzzle_config: ResMut<PuzzleConfig>,
    mut commands: Commands,
    puzzle_image: Option<Res<PuzzleImage>>,
    file_registry: Res<ExternalFileRegistry>,
    image_sender: Res<ImageLoadSender>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };

    // 背景のグラデーション
    egui::Area::new(egui::Id::new("game_setup_background"))
        .fixed_pos(egui::pos2(0.0, 0.0))
        .show(ctx, |ui| {
            let screen_rect = ctx.content_rect();
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

    // ゲーム設定を画面中央に表示（スクロール可能な大きなウィンドウ）
    egui::Window::new("Game Setup")
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
                            egui::RichText::new("🎮 Game Setup")
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


                        ui.add_space(5.0);

                        // 画像の読み込み状態をチェック
                        let image_loaded = puzzle_image.as_ref()
                            .map(|img| img.size.x > 10.0 && img.size.y > 10.0)
                            .unwrap_or(false);


                        // 画像読み込み状態の表示（簡略化）
                        if puzzle_image.is_none() && !puzzle_config.image_path.is_empty() {
                            ui.colored_label(
                                egui::Color32::YELLOW,
                                egui::RichText::new("Loading image...")
                                    .size(14.0)
                                    .italics()
                            );
                            ui.add_space(5.0);
                        }

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
                                    next_state.set(AppState::InGame);
                                    // 新しいゲーム開始時はリセットしない（画像設定を保持）
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
                                next_state.set(AppState::Menu);
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

                        ui.horizontal(|ui| {
                            ui.label("Seed:");
                            ui.add(egui::DragValue::new(&mut puzzle_config.seed));
                        });

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
                                            // 外部ファイルの場合はオリジナルのファイル名を表示
                                            let display_name = if file_registry.is_external_image_path(&puzzle_config.image_path) {
                                                file_registry.get_original_filename(&puzzle_config.image_path)
                                                    .unwrap_or_else(|| puzzle_config.image_path.clone())
                                            } else {
                                                puzzle_config.image_path.clone()
                                            };

                                            ui.label(
                                                egui::RichText::new(&display_name)
                                                    .size(14.0)
                                                    .color(egui::Color32::LIGHT_GREEN)
                                                    .strong()
                                            );
                                        }
                                    });
                                });

                                ui.add_space(8.0);

                                // シンプルなボタン表示
                                let button_text = "📁 Select Puzzle Image";

                                {
                                    if ui.add_sized([200.0, 35.0],
                                        egui::Button::new(
                                            egui::RichText::new(button_text)
                                                .size(16.0)
                                        )
                                    ).clicked() {
                                    // 同期的ファイルダイアログを開く
                                    {
                                        println!("🔍 MAIN THREAD [{:?}]: About to open file dialog...", std::thread::current().id());
                                        let result = rfd::FileDialog::new()
                                            .add_filter("Image files", &["png", "jpg", "jpeg", "bmp", "gif", "webp"])
                                            .pick_file();
                                        println!("🔍 MAIN THREAD [{:?}]: File dialog returned result", std::thread::current().id());

                                        if let Some(file_path) = result {
                                            println!("✅ MAIN THREAD [{:?}]: File selected: {}", std::thread::current().id(), file_path.display());

                                            // 外部ファイルとして登録
                                            println!("🔍 MAIN THREAD [{:?}]: About to register file...", std::thread::current().id());
                                            let virtual_path = file_registry.register_file(&file_path);
                                            println!("🔍 MAIN THREAD [{:?}]: File registered as: {}", std::thread::current().id(), virtual_path);
                                            puzzle_config.image_path = virtual_path.clone();

                                            // 画像読み込みをネイティブスレッドで開始
                                            println!("🔍 MAIN THREAD [{:?}]: About to start worker thread...", std::thread::current().id());
                                            use puzzella_game::asset_reader::start_thread_image_load;
                                            start_thread_image_load(
                                                virtual_path.clone(),
                                                file_path,
                                                image_sender.tx_results.clone(),
                                            );
                                            println!("🔍 MAIN THREAD [{:?}]: Worker thread started", std::thread::current().id());

                                            // PuzzleImageリソースを削除（読み込み完了時に再作成される）
                                            commands.remove_resource::<PuzzleImage>();
                                            commands.remove_resource::<puzzella_game::persistence::runtime::OriginalPuzzleImage>();

                                            println!("🚀 MAIN THREAD [{:?}]: Started async image loading: {}", std::thread::current().id(), virtual_path);
                                        } else {
                                            println!("🚫 MAIN THREAD [{:?}]: File dialog was cancelled", std::thread::current().id());
                                        }
                                    }
                                    }
                                }
                            });
                        });
                    });
                });
        });
}
