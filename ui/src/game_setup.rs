use crate::{grid::calculate_grid_from_config, theme};
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiTextureHandle};
use puzzella_game::asset_reader::{start_thread_image_load, ExternalFileRegistry};
use puzzella_game::resources::*;

pub fn draw_game_setup_ui(
    mut contexts: EguiContexts,
    mut config: ResMut<PuzzleConfig>,
    mut commands: Commands,
    puzzle_image: Option<Res<PuzzleImage>>,
    file_registry: Res<ExternalFileRegistry>,
    image_sender: Res<ImageLoadSender>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    let texture = puzzle_image
        .as_ref()
        .map(|image| contexts.add_image(EguiTextureHandle::Weak(image.handle.id())));
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    theme::background(ctx);
    let screen = ctx.content_rect();
    let width = (screen.width() - 96.0).clamp(160.0, 960.0);
    let wide = width >= 680.0;
    let image_loaded = puzzle_image
        .as_ref()
        .is_some_and(|image| image.size.x > 10.0 && image.size.y > 10.0);
    let mut select_image = false;
    egui::Area::new("new_game_screen".into())
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            theme::frame().show(ui, |ui| {
                ui.set_width(width);
                ui.set_max_height((screen.height() - 96.0).max(120.0));
                theme::heading(
                    ui,
                    "A NEW PUZZLE",
                    "New Game",
                    "Choose an image and make it your own.",
                );
                ui.separator();
                egui::ScrollArea::vertical()
                    .max_height((screen.height() - 312.0).max(80.0))
                    .show(ui, |ui| {
                        if wide {
                            ui.columns(2, |columns| {
                                select_image = image_section(
                                    &mut columns[0],
                                    &config,
                                    puzzle_image.as_deref(),
                                    texture,
                                    &file_registry,
                                );
                                piece_section(&mut columns[1], &mut config);
                            });
                        } else {
                            select_image = image_section(
                                ui,
                                &config,
                                puzzle_image.as_deref(),
                                texture,
                                &file_registry,
                            );
                            ui.add_space(12.0);
                            piece_section(ui, &mut config);
                        }
                        ui.add_space(8.0);
                    });
                ui.separator();
                if let Some((cols, rows, _)) =
                    calculate_grid_from_config(&config, puzzle_image.as_deref())
                {
                    config.grid_size = (cols, rows);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            egui::RichText::new(format!("{} pieces", cols * rows))
                                .strong()
                                .color(theme::ACCENT),
                        );
                        theme::hint(ui, format!("{cols} x {rows} grid"));
                    });
                } else {
                    theme::hint(ui, "Select an image to start your puzzle.");
                }
                ui.horizontal(|ui| {
                    let button_width = ((ui.available_width() - 10.0) * 0.5).min(240.0);
                    if theme::button(ui, "Back to Title", button_width, false).clicked() {
                        next_state.set(AppState::Menu);
                    }
                    ui.add_enabled_ui(
                        image_loaded && !config.image_path.is_empty() && !select_image,
                        |ui| {
                            if theme::button(ui, "Start Game", button_width, true).clicked() {
                                next_state.set(AppState::InGame);
                            }
                        },
                    );
                });
            });
        });
    // Keep the existing threaded decode and original-image persistence path.
    if select_image {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Image files", &["png", "jpg", "jpeg", "bmp", "gif", "webp"])
            .pick_file()
        {
            let key = file_registry.register_file(&path);
            config.image_path = key.clone();
            start_thread_image_load(key, path, image_sender.tx_results.clone());
            commands.remove_resource::<PuzzleImage>();
            commands.remove_resource::<puzzella_game::persistence::runtime::OriginalPuzzleImage>();
        }
    }
}

fn image_section(
    ui: &mut egui::Ui,
    config: &PuzzleConfig,
    image: Option<&PuzzleImage>,
    texture: Option<egui::TextureId>,
    registry: &ExternalFileRegistry,
) -> bool {
    theme::section(ui, "01", "Puzzle Image");
    let width = ui.available_width();
    let compact = ui.ctx().content_rect().height() < 640.0;
    let label = if config.image_path.is_empty() {
        "Select Image"
    } else {
        "Change Image"
    };
    let mut selected = compact && theme::button(ui, label, width, false).clicked();
    let height = if compact {
        140.0
    } else if width >= 300.0 {
        236.0
    } else {
        160.0
    };
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    ui.painter().rect_filled(rect, 10, theme::BACKGROUND);
    ui.painter().rect_stroke(
        rect,
        10,
        egui::Stroke::new(1.0, theme::BORDER),
        egui::StrokeKind::Inside,
    );
    if let (Some(image), Some(texture)) = (image, texture) {
        let available = rect.shrink(12.0).size();
        let scale = (available.x / image.size.x).min(available.y / image.size.y);
        let image_rect = egui::Rect::from_center_size(
            rect.center(),
            egui::vec2(image.size.x * scale, image.size.y * scale),
        );
        ui.painter().image(
            texture,
            image_rect,
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    } else {
        let center = rect.center() - egui::vec2(0.0, 20.0);
        theme::piece_outline(
            ui.painter(),
            egui::Rect::from_center_size(center, egui::Vec2::splat(40.0)),
            egui::Stroke::new(1.5, theme::BORDER),
        );
        ui.painter().text(
            rect.center() + egui::vec2(0.0, 36.0),
            egui::Align2::CENTER_CENTER,
            "Your next puzzle starts here",
            egui::FontId::proportional(13.0),
            theme::MUTED,
        );
    }
    let name = registry
        .get_original_filename(&config.image_path)
        .unwrap_or_else(|| config.image_path.clone());
    if config.image_path.is_empty() {
        theme::hint(ui, "PNG / JPEG / WebP / BMP");
    } else {
        ui.add(egui::Label::new(egui::RichText::new(&name).size(13.0)).truncate())
            .on_hover_text(&name);
        if let Some(image) = image {
            theme::hint(
                ui,
                format!("{:.0} x {:.0} pixels", image.size.x, image.size.y),
            );
        } else {
            theme::hint(
                ui,
                "Preparing image. Select another image if loading fails.",
            );
        }
    }
    if !compact {
        selected |= theme::button(ui, label, width, false).clicked();
    }
    selected
}

fn piece_section(ui: &mut egui::Ui, config: &mut PuzzleConfig) {
    ui.spacing_mut().item_spacing.y = 8.0;
    theme::section(ui, "02", "Piece Configuration");
    ui.horizontal_wrapped(|ui| {
        ui.selectable_value(
            &mut config.piece_mode,
            PieceMode::SquarePieces,
            "Aspect Ratio",
        );
        ui.selectable_value(
            &mut config.piece_mode,
            PieceMode::TargetCount,
            "Target Count",
        );
        ui.selectable_value(&mut config.piece_mode, PieceMode::ManualGrid, "Manual Grid");
    });
    ui.add_space(8.0);
    ui.spacing_mut().slider_width = (ui.available_width() - 110.0).clamp(60.0, 250.0);
    match config.piece_mode {
        PieceMode::SquarePieces => {
            ui.label("Aspect ratio scale");
            ui.add(
                egui::Slider::new(&mut config.target_piece_size, 1.0..=50.0)
                    .suffix("x")
                    .fixed_decimals(1),
            );
            theme::hint(ui, "Keeps pieces balanced to match your image.");
        }
        PieceMode::TargetCount => {
            ui.label("Target piece count");
            ui.add(egui::Slider::new(&mut config.target_piece_count, 4..=10000).logarithmic(true));
            theme::hint(ui, "The final count follows your image's proportions.");
            ui.horizontal_wrapped(|ui| {
                for count in [100, 500, 1000] {
                    if ui
                        .selectable_label(config.target_piece_count == count, count.to_string())
                        .clicked()
                    {
                        config.target_piece_count = count;
                    }
                }
            });
        }
        PieceMode::ManualGrid => {
            ui.label("Columns");
            ui.add(egui::Slider::new(&mut config.grid_size.0, 2..=1000).logarithmic(true));
            ui.label("Rows");
            ui.add(egui::Slider::new(&mut config.grid_size.1, 2..=1000).logarithmic(true));
            theme::hint(
                ui,
                format!(
                    "{} pieces in your custom grid",
                    config.grid_size.0 * config.grid_size.1
                ),
            );
        }
    }
    ui.add_space(12.0);
    theme::section(ui, "03", "Fine Tuning");
    ui.horizontal(|ui| {
        ui.label("Seed");
        ui.add(egui::DragValue::new(&mut config.seed).speed(1));
    });
    theme::hint(ui, "Use the same seed to repeat the piece layout.");
    ui.add_space(4.0);
    ui.label("Snap distance");
    ui.add(egui::Slider::new(&mut config.snap_distance, 1.0..=100.0).suffix(" px"));
    theme::hint(ui, "A larger distance makes pieces easier to place.");
}
