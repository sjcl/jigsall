use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use puzzella_game::resources::*;

/// メインメニューUI
pub fn draw_menu_ui(
    mut contexts: EguiContexts,
    mut next_state: ResMut<NextState<AppState>>,
    mut dialogs: ResMut<crate::persistence::SaveDialogs>,
    mut persistence: ResMut<puzzella_game::persistence::runtime::PersistenceState>,
    service: Res<puzzella_game::persistence::runtime::PersistenceService>,
) {
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };

    // 背景のグラデーションエフェクト
    egui::Area::new(egui::Id::new("title_background"))
        .fixed_pos(egui::pos2(0.0, 0.0))
        .show(ctx, |ui| {
            let screen_rect = ctx.content_rect();
            ui.allocate_ui_with_layout(
                screen_rect.size(),
                egui::Layout::centered_and_justified(egui::Direction::TopDown),
                |ui| {
                    // 背景全体をダークブルーのグラデーションで覆う
                    ui.painter().rect_filled(
                        screen_rect,
                        egui::CornerRadius::ZERO,
                        egui::Color32::from_rgb(20, 30, 60), // ダークブルー
                    );
                },
            );
        });

    // メインメニューを画面中央に表示
    egui::Window::new("Main Menu")
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -50.0))
        .collapsible(false)
        .resizable(false)
        .title_bar(false)
        .show(ctx, |ui| {
            ui.set_min_size(egui::vec2(400.0, 350.0));

            ui.add_enabled_ui(!persistence.busy, |ui| {
                ui.vertical_centered(|ui| {
                    ui.spacing_mut().item_spacing.y = 25.0;
                    ui.spacing_mut().button_padding = egui::vec2(20.0, 12.0);

                    ui.add_space(20.0);

                    // タイトル（大きく、目立つように）
                    ui.label(
                        egui::RichText::new("🧩 Puzzella")
                            .size(48.0)
                            .color(egui::Color32::WHITE)
                            .strong(),
                    );

                    ui.label(
                        egui::RichText::new("Jigsaw Puzzle")
                            .size(18.0)
                            .color(egui::Color32::LIGHT_GRAY),
                    );

                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(15.0);

                    // Game Setup ボタン
                    if ui
                        .add_sized(
                            [280.0, 50.0],
                            egui::Button::new(egui::RichText::new("🎮 Game Setup").size(20.0)),
                        )
                        .clicked()
                    {
                        next_state.set(AppState::GameSetup);
                    }

                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(10.0);

                    if ui
                        .add_sized([280.0, 50.0], egui::Button::new("Load Game"))
                        .clicked()
                    {
                        dialogs.load_open = true;
                        service.list(&mut persistence);
                    }
                    // Exit ボタン
                    if ui
                        .add_sized(
                            [280.0, 45.0],
                            egui::Button::new(
                                egui::RichText::new("❌ Exit")
                                    .size(16.0)
                                    .color(egui::Color32::LIGHT_RED),
                            ),
                        )
                        .clicked()
                    {
                        std::process::exit(0);
                    }

                    ui.add_space(20.0);
                });
            });
        });
}
