use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use puzzella_game::resources::*;

const PANEL: egui::Color32 = egui::Color32::from_rgb(24, 33, 45);
const BORDER: egui::Color32 = egui::Color32::from_rgb(58, 73, 87);
const TEXT: egui::Color32 = egui::Color32::from_rgb(235, 239, 241);
const MUTED: egui::Color32 = egui::Color32::from_rgb(155, 171, 183);
const ACCENT: egui::Color32 = egui::Color32::from_rgb(172, 197, 174);

/// A quiet result card over the completed puzzle, without clearing its session.
pub fn draw_completion_ui(
    mut contexts: EguiContexts,
    store: Res<PieceDataStore>,
    mut next_state: ResMut<NextState<AppState>>,
    mut next_completion_state: ResMut<NextState<GameCompleteSubState>>,
) {
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let width = (ctx.content_rect().width() - 72.0).clamp(160.0, 360.0);
    let height = (ctx.content_rect().height() - 72.0).max(120.0);
    let compact = ctx.content_rect().height() < 520.0;
    egui::Modal::new(egui::Id::new("puzzle_complete"))
        .backdrop_color(egui::Color32::from_black_alpha(160))
        .frame(
            egui::Frame::new()
                .fill(PANEL)
                .stroke(egui::Stroke::new(1.0, BORDER))
                .corner_radius(16)
                .inner_margin(24),
        )
        .show(ctx, |ui| {
            ui.set_width(width);
            egui::ScrollArea::vertical()
                .max_height(height)
                .show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.spacing_mut().item_spacing.y = if compact { 6.0 } else { 8.0 };
                        ui.visuals_mut().override_text_color = Some(TEXT);
                        ui.add_space(if compact { 0.0 } else { 8.0 });

                        // Draw the completion mark explicitly so it needs no emoji font.
                        let size = if compact { 44.0 } else { 56.0 };
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
                        let center = rect.center();
                        ui.painter().circle_filled(
                            center,
                            size / 2.0 - 1.0,
                            egui::Color32::from_rgb(41, 58, 52),
                        );
                        ui.painter().circle_stroke(
                            center,
                            size / 2.0 - 1.0,
                            egui::Stroke::new(1.0, ACCENT.gamma_multiply(0.5)),
                        );
                        ui.painter().add(egui::Shape::line(
                            vec![
                                center + egui::vec2(-11.0, 0.0),
                                center + egui::vec2(-3.0, 8.0),
                                center + egui::vec2(12.0, -9.0),
                            ],
                            egui::Stroke::new(3.0, ACCENT),
                        ));

                        ui.add_space(if compact { 0.0 } else { 8.0 });
                        ui.label(egui::RichText::new("WELL DONE").size(11.0).color(ACCENT));
                        ui.label(
                            egui::RichText::new("Puzzle Complete")
                                .size(if compact { 26.0 } else { 30.0 })
                                .strong(),
                        );
                        ui.label(
                            egui::RichText::new("Every piece is in its place.")
                                .size(14.0)
                                .color(MUTED),
                        );
                        ui.add_space(if compact { 8.0 } else { 12.0 });

                        egui::Frame::new()
                            .fill(egui::Color32::from_rgb(30, 43, 52))
                            .corner_radius(8)
                            .inner_margin(egui::Margin::symmetric(16, 10))
                            .show(ui, |ui| {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{} pieces  /  100% complete",
                                        store.len()
                                    ))
                                    .size(13.0)
                                    .color(ACCENT),
                                );
                            });
                        ui.add_space(if compact { 8.0 } else { 16.0 });

                        if ui
                            .add_sized(
                                [ui.available_width(), 46.0],
                                egui::Button::new(
                                    egui::RichText::new("View Completed Puzzle")
                                        .size(15.0)
                                        .strong()
                                        .color(PANEL),
                                )
                                .fill(ACCENT)
                                .corner_radius(8),
                            )
                            .clicked()
                        {
                            next_completion_state.set(GameCompleteSubState::Viewing);
                        }
                        if ui
                            .add_sized(
                                [ui.available_width(), 42.0],
                                egui::Button::new(
                                    egui::RichText::new("Return to Title")
                                        .size(14.0)
                                        .color(TEXT),
                                )
                                .fill(PANEL)
                                .stroke(egui::Stroke::new(1.0, BORDER))
                                .corner_radius(8),
                            )
                            .clicked()
                        {
                            next_state.set(AppState::Menu);
                        }
                        ui.add_space(4.0);
                    });
                });
        });
}

/// A small HUD leaves the finished canvas available for pan and zoom.
pub fn draw_completed_puzzle_ui(
    mut contexts: EguiContexts,
    mut capture: ResMut<GameUiPointerCapture>,
    mut next: ResMut<NextState<GameCompleteSubState>>,
) {
    capture.over_hud = false;
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let mut viewport_ui = egui::Ui::new(
        ctx.clone(),
        "completed_puzzle_viewport".into(),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.viewport_rect()),
    );
    let panel = egui::Panel::top("completed_puzzle_info")
        .frame(
            egui::Frame::new()
                .fill(PANEL)
                .inner_margin(egui::Margin::symmetric(16, 10)),
        )
        .show(&mut viewport_ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Completed Puzzle")
                        .color(ACCENT)
                        .strong(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("ESC  Menu").clicked() {
                        next.set(GameCompleteSubState::Paused);
                    }
                    if ui.available_width() >= 240.0 {
                        ui.label(
                            egui::RichText::new("Scroll to zoom / Right-drag to pan")
                                .size(12.0)
                                .color(MUTED),
                        );
                    }
                });
            });
        });
    capture.over_hud = ctx
        .pointer_interact_pos()
        .is_some_and(|point| panel.response.rect.contains(point));
}
