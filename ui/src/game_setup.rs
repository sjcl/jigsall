use crate::localization::Localization;
use crate::{grid::calculate_grid_from_config, theme};
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiTextureHandle};
use jigsall_game::asset_reader::ExternalFileRegistry;
use jigsall_game::resources::*;

pub(crate) mod image_picker;

pub(crate) fn randomize_seed(mut config: ResMut<PuzzleConfig>) {
    config.seed = rand::random();
}

#[allow(clippy::too_many_arguments)] // Explicit ECS resources include image load failures.
pub fn draw_game_setup_ui(
    i18n: Res<Localization>,
    mut contexts: EguiContexts,
    mut config: ResMut<PuzzleConfig>,
    mut image_picker: ResMut<image_picker::ImagePicker>,
    puzzle_image: Option<Res<PuzzleImage>>,
    image_error: Option<Res<ImageLoadError>>,
    file_registry: Res<ExternalFileRegistry>,
    mut next_state: ResMut<NextState<AppState>>,
    mut multiplayer: ResMut<crate::multiplayer::MultiplayerUi>,
    status: Res<jigsall_game::network::runtime::NetworkStatus>,
    profile: Res<jigsall_game::player_settings::PlayerSettingsState>,
    original: Option<Res<jigsall_game::persistence::runtime::OriginalPuzzleImage>>,
    mut settings: ResMut<crate::settings::SettingsDialog>,
    display: Res<jigsall_game::settings::DisplaySettingsState>,
    #[cfg(feature = "rendezvous")] rendezvous_config: Option<
        Res<jigsall_game::network::runtime::RendezvousRuntimeConfig>,
    >,
) {
    #[cfg(feature = "rendezvous")]
    let internet_available = rendezvous_config.is_some();
    #[cfg(not(feature = "rendezvous"))]
    let internet_available = false;
    if multiplayer.connection_screen(&status) {
        return;
    }
    let image_error = image_error
        .as_deref()
        .filter(|error| error.virtual_key == config.image_path);
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
    let image_loaded = image_error.is_none()
        && puzzle_image
            .as_ref()
            .is_some_and(|image| image.logical_size.min_element() > 10);
    let mut select_image = false;
    egui::Area::new("new_game_screen".into())
        .enabled(!image_picker.is_open() && !settings.open)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            theme::frame().show(ui, |ui| {
                ui.set_width(width);
                ui.set_max_height((screen.height() - 96.0).max(120.0));
                theme::heading(
                    ui,
                    i18n.text(if multiplayer.host_setup {
                        "multiplayer-new-game"
                    } else {
                        "menu-new-game"
                    }),
                );
                if multiplayer.host_setup {
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .selectable_label(
                                !multiplayer.host_settings_tab,
                                i18n.text("multiplayer-puzzle-settings"),
                            )
                            .clicked()
                        {
                            multiplayer.host_settings_tab = false;
                        }
                        if ui
                            .selectable_label(
                                multiplayer.host_settings_tab,
                                i18n.text("multiplayer-host-settings"),
                            )
                            .clicked()
                        {
                            multiplayer.host_settings_tab = true;
                        }
                    });
                }
                ui.separator();
                egui::ScrollArea::vertical()
                    .max_height((screen.height() - 312.0).max(80.0))
                    .show(ui, |ui| {
                        if multiplayer.host_setup && multiplayer.host_settings_tab {
                            crate::multiplayer::paint_method(
                                ui,
                                &mut multiplayer,
                                internet_available,
                                &i18n,
                            );
                            crate::multiplayer::paint_connection_fields(
                                ui,
                                &mut multiplayer.host,
                                true,
                                &profile,
                                &i18n,
                            );
                            if ui
                                .small_button(i18n.text("multiplayer-name-settings"))
                                .clicked()
                            {
                                multiplayer.host.clear_password();
                                settings.open(&display);
                            }
                            if image_loaded
                                && original
                                    .as_ref()
                                    .is_none_or(|image| image.encoded.is_none())
                            {
                                ui.colored_label(
                                    theme::DANGER,
                                    i18n.text("multiplayer-error-image"),
                                );
                            }
                        } else if wide {
                            ui.columns(2, |columns| {
                                select_image = image_section(
                                    &mut columns[0],
                                    &config,
                                    puzzle_image.as_deref(),
                                    image_error,
                                    texture,
                                    &file_registry,
                                    &i18n,
                                );
                                piece_section(
                                    &mut columns[1],
                                    &mut config,
                                    puzzle_image.as_deref().filter(|_| image_error.is_none()),
                                    &i18n,
                                );
                            });
                        } else {
                            select_image = image_section(
                                ui,
                                &config,
                                puzzle_image.as_deref(),
                                image_error,
                                texture,
                                &file_registry,
                                &i18n,
                            );
                            ui.add_space(12.0);
                            piece_section(
                                ui,
                                &mut config,
                                puzzle_image.as_deref().filter(|_| image_error.is_none()),
                                &i18n,
                            );
                        }
                        ui.add_space(8.0);
                    });
                ui.separator();
                if multiplayer.host_setup
                    && multiplayer.host_settings_tab
                    && !paint_piece_summary(
                        ui,
                        &mut config,
                        puzzle_image.as_deref().filter(|_| image_error.is_none()),
                        &i18n,
                    )
                {
                    theme::hint(ui, i18n.text("multiplayer-puzzle-tab-hint"));
                }
                if multiplayer.host_setup
                    && !multiplayer.host_settings_tab
                    && !multiplayer.host.valid(true)
                {
                    theme::hint(ui, i18n.text("multiplayer-setup-required"));
                }
                ui.horizontal(|ui| {
                    let button_width = ((ui.available_width() - 10.0) * 0.5).min(240.0);
                    if theme::button(ui, i18n.text("common-back-title"), button_width, false)
                        .clicked()
                    {
                        multiplayer.navigate(crate::multiplayer::MenuScreen::Title);
                        next_state.set(AppState::Menu);
                    }
                    ui.add_enabled_ui(
                        image_loaded
                            && !config.image_path.is_empty()
                            && !select_image
                            && !multiplayer.submitted
                            && (!multiplayer.host_setup
                                || (multiplayer.host.valid(true)
                                    && original
                                        .as_ref()
                                        .is_some_and(|image| image.encoded.is_some())
                                    && cfg!(feature = "gns"))),
                        |ui| {
                            if theme::button(
                                ui,
                                i18n.text(if multiplayer.host_setup {
                                    "multiplayer-start-host"
                                } else {
                                    "setup-start-game"
                                }),
                                button_width,
                                true,
                            )
                            .clicked()
                            {
                                if multiplayer.host_setup {
                                    multiplayer.submit_host();
                                } else {
                                    multiplayer.submitted = true;
                                    next_state.set(AppState::InGame);
                                }
                            }
                        },
                    );
                });
            });
        });
    if select_image {
        image_picker.open(i18n.text("setup-image-filter"));
    }
}

fn image_section(
    ui: &mut egui::Ui,
    config: &PuzzleConfig,
    image: Option<&PuzzleImage>,
    error: Option<&ImageLoadError>,
    texture: Option<egui::TextureId>,
    registry: &ExternalFileRegistry,
    i18n: &Localization,
) -> bool {
    theme::section(ui, i18n.text("setup-image"));
    let width = ui.available_width();
    let compact = ui.ctx().content_rect().height() < 640.0;
    let label = if config.image_path.is_empty() {
        i18n.text("setup-select-image")
    } else {
        i18n.text("setup-change-image")
    };
    let mut selected = compact && theme::button(ui, label.as_str(), width, false).clicked();
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
        let size = image.logical_size.as_vec2();
        let scale = (available.x / size.x).min(available.y / size.y);
        let image_rect =
            egui::Rect::from_center_size(rect.center(), egui::vec2(size.x * scale, size.y * scale));
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
            i18n.text("setup-image-placeholder"),
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
        if let Some(error) = error {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(i18n.format(
                        "setup-image-load-failed",
                        &[("reason", error.reason.as_str().into())],
                    ))
                    .color(theme::DANGER),
                )
                .wrap(),
            );
            theme::hint(ui, i18n.text("setup-image-load-retry"));
        } else if let Some(image) = image {
            theme::hint(
                ui,
                i18n.format(
                    "setup-image-pixels",
                    &[
                        ("width", image.logical_size.x.into()),
                        ("height", image.logical_size.y.into()),
                    ],
                ),
            );
        } else {
            theme::hint(ui, i18n.text("setup-preparing-image"));
        }
    }
    if !compact {
        selected |= theme::button(ui, label, width, false).clicked();
    }
    selected
}

#[cfg(test)]
mod tests;

fn piece_section(
    ui: &mut egui::Ui,
    config: &mut PuzzleConfig,
    image: Option<&PuzzleImage>,
    i18n: &Localization,
) {
    ui.spacing_mut().item_spacing.y = 8.0;
    theme::section(ui, i18n.text("setup-pieces"));
    ui.horizontal_wrapped(|ui| {
        ui.selectable_value(
            &mut config.piece_mode,
            PieceMode::SquarePieces,
            i18n.text("setup-aspect-ratio"),
        );
        ui.selectable_value(
            &mut config.piece_mode,
            PieceMode::TargetCount,
            i18n.text("setup-target-count"),
        );
        ui.selectable_value(
            &mut config.piece_mode,
            PieceMode::ManualGrid,
            i18n.text("setup-manual-grid"),
        );
    });
    ui.add_space(8.0);
    ui.spacing_mut().slider_width = (ui.available_width() - 110.0).clamp(60.0, 250.0);
    match config.piece_mode {
        PieceMode::SquarePieces | PieceMode::TargetCount => {
            ui.label(i18n.text(if config.piece_mode == PieceMode::SquarePieces {
                "setup-target-pieces"
            } else {
                "setup-pieces"
            }));
            ui.add(egui::Slider::new(&mut config.target_piece_count, 4..=10000).logarithmic(true));
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
            ui.label(i18n.text("setup-columns"));
            ui.add(egui::Slider::new(&mut config.grid_size.0, 2..=1000).logarithmic(true));
            ui.label(i18n.text("setup-rows"));
            ui.add(egui::Slider::new(&mut config.grid_size.1, 2..=1000).logarithmic(true));
        }
    }
    ui.add_space(4.0);
    paint_piece_summary(ui, config, image, i18n);
    ui.add_space(12.0);
    ui.checkbox(&mut config.rotation_enabled, i18n.text("setup-rotation"))
        .on_hover_text(i18n.text("setup-rotation-hint"));
    if config.rotation_enabled && pieces_are_rectangular(config, image) {
        ui.add(
            egui::Label::new(
                egui::RichText::new(i18n.text("setup-rotation-warning")).color(theme::DANGER),
            )
            .wrap(),
        );
    }
    ui.add_space(4.0);
    egui::CollapsingHeader::new(i18n.text("setup-tuning"))
        .id_salt("puzzle_tuning")
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(i18n.text("setup-seed"));
                seed_input(ui, &mut config.seed);
            });
            theme::hint(ui, i18n.text("setup-seed-hint"));
        });
}

fn paint_piece_summary(
    ui: &mut egui::Ui,
    config: &mut PuzzleConfig,
    image: Option<&PuzzleImage>,
    i18n: &Localization,
) -> bool {
    let Some((columns, rows)) = calculate_grid_from_config(config, image) else {
        return false;
    };
    config.grid_size = (columns, rows);
    ui.horizontal_wrapped(|ui| {
        ui.label(
            egui::RichText::new(
                i18n.format("setup-piece-count", &[("count", (columns * rows).into())]),
            )
            .strong()
            .color(theme::ACCENT),
        );
        theme::hint(
            ui,
            i18n.format(
                "setup-grid",
                &[("columns", columns.into()), ("rows", rows.into())],
            ),
        );
    });
    let size = image.unwrap().logical_size;
    let pixels = |edge: u32, divisions: usize| {
        let text = format!("{:.2}", f64::from(edge) / divisions as f64);
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    };
    ui.label(i18n.format(
        "setup-piece-size",
        &[
            ("width", pixels(size.x, columns).as_str().into()),
            ("height", pixels(size.y, rows).as_str().into()),
        ],
    ))
    .on_hover_text(i18n.text("setup-piece-size-hint"));
    true
}

fn pieces_are_rectangular(config: &PuzzleConfig, image: Option<&PuzzleImage>) -> bool {
    let Some((columns, rows)) = calculate_grid_from_config(config, image) else {
        return false;
    };
    let size = image.unwrap().logical_size;
    u64::from(size.x) * rows as u64 != u64::from(size.y) * columns as u64
}

fn seed_input(ui: &mut egui::Ui, seed: &mut u64) -> egui::Response {
    // DragValue converts through f64, which rounds most randomly generated u64 seeds.
    let id = ui.make_persistent_id("puzzle_seed");
    let mut text = ui
        .data_mut(|data| data.remove_temp::<(u64, String)>(id))
        .filter(|(value, _)| *value == *seed)
        .map_or_else(|| seed.to_string(), |(_, text)| text);
    let response = ui.add(
        egui::TextEdit::singleline(&mut text)
            .id(id)
            .font(egui::TextStyle::Monospace)
            .desired_width(190.0_f32.min(ui.available_width())),
    );
    if response.changed() {
        if let Ok(value) = text.trim().parse::<u64>() {
            *seed = value;
        }
    }
    // Keep incomplete edits while focused; leaving the field restores the last valid seed.
    if !response.has_focus() {
        text = seed.to_string();
    }
    ui.data_mut(|data| data.insert_temp(id, (*seed, text)));
    response
}
