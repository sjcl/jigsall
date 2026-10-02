use crate::localization::Localization;
use crate::theme;
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use puzzella_game::resources::*;

#[allow(clippy::too_many_arguments)]
pub fn draw_menu_ui(
    i18n: Res<Localization>,
    mut contexts: EguiContexts,
    mut next_state: ResMut<NextState<AppState>>,
    mut dialogs: ResMut<crate::persistence::SaveDialogs>,
    mut persistence: ResMut<puzzella_game::persistence::runtime::PersistenceState>,
    service: Res<puzzella_game::persistence::runtime::PersistenceService>,
    mut exit: MessageWriter<AppExit>,
    mut settings_dialog: ResMut<crate::settings::SettingsDialog>,
    display_settings: Res<puzzella_game::settings::DisplaySettingsState>,
) {
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    theme::background(ctx);
    let screen = ctx.content_rect();
    let width = (screen.width() - 64.0).clamp(160.0, 360.0);
    let compact = screen.height() < 660.0;
    let logo = theme::logo(ctx);
    egui::Area::new("title_menu".into())
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -12.0))
        .show(ctx, |ui| {
            ui.set_width(width);
            ui.set_max_height((screen.height() - 88.0).max(120.0));
            egui::ScrollArea::vertical()
                .max_height((screen.height() - 88.0).max(120.0))
                .show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        let size = if screen.height() < 540.0 {
                            44.0
                        } else if compact {
                            72.0
                        } else {
                            104.0
                        };
                        ui.add(egui::Image::new((logo.id(), egui::vec2(size, size))));
                        ui.label(
                            egui::RichText::new("Puzzella")
                                .size(if compact { 34.0 } else { 54.0 })
                                .strong(),
                        );
                        ui.add_space(if compact { 12.0 } else { 24.0 });
                        ui.add_enabled_ui(
                            !persistence.busy && !dialogs.load_open && !settings_dialog.open,
                            |ui| {
                                ui.spacing_mut().item_spacing.y = 8.0;
                                if theme::button(ui, i18n.text("menu-new-game"), width, true)
                                    .clicked()
                                {
                                    next_state.set(AppState::GameSetup);
                                }
                                if theme::button(ui, i18n.text("menu-load-game"), width, false)
                                    .clicked()
                                {
                                    dialogs.load_open = true;
                                    service.list(&mut persistence);
                                }
                                // Multiplayer is reserved until network play is implemented.
                                theme::button(ui, i18n.text("menu-multiplayer"), width, false);
                                if theme::button(ui, i18n.text("menu-settings"), width, false)
                                    .clicked()
                                {
                                    settings_dialog.open(&display_settings);
                                }
                                ui.add_space(4.0);
                                if theme::danger_button(ui, i18n.text("menu-exit"), width).clicked()
                                {
                                    exit.write(AppExit::Success);
                                }
                            },
                        );
                    });
                });
        });
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Background,
        "title_footer".into(),
    ));
    painter.text(
        egui::pos2(screen.left() + 32.0, screen.bottom() - 22.0),
        egui::Align2::LEFT_CENTER,
        i18n.text("menu-subtitle"),
        egui::FontId::proportional(10.0),
        theme::MUTED,
    );
    painter.text(
        egui::pos2(screen.right() - 32.0, screen.bottom() - 22.0),
        egui::Align2::RIGHT_CENTER,
        concat!("v", env!("CARGO_PKG_VERSION")),
        egui::FontId::proportional(10.0),
        theme::MUTED,
    );
}
