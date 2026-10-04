use crate::localization::Localization;
use crate::multiplayer::{self, MenuScreen, MultiplayerUi};
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
    profile: Res<puzzella_game::player_settings::PlayerSettingsState>,
    status: Res<puzzella_game::network::runtime::NetworkStatus>,
    mut multiplayer: ResMut<MultiplayerUi>,
) {
    if multiplayer.connection_screen(&status) {
        return;
    }
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
                        if !matches!(
                            multiplayer.screen,
                            MenuScreen::Join | MenuScreen::HostLoadSettings
                        ) {
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
                        }
                        ui.add_enabled_ui(
                            !persistence.busy && !dialogs.load_open && !settings_dialog.open,
                            |ui| {
                                ui.spacing_mut().item_spacing.y = 8.0;
                                match multiplayer.screen {
                                    MenuScreen::Title => {
                                        if theme::button(
                                            ui,
                                            i18n.text("menu-singleplayer"),
                                            width,
                                            true,
                                        )
                                        .clicked()
                                        {
                                            multiplayer.navigate(MenuScreen::SinglePlayer);
                                        }
                                        if theme::button(
                                            ui,
                                            i18n.text("menu-multiplayer"),
                                            width,
                                            false,
                                        )
                                        .clicked()
                                        {
                                            multiplayer.navigate(MenuScreen::Multiplayer);
                                        }
                                    }
                                    MenuScreen::SinglePlayer | MenuScreen::Host => {
                                        let host = multiplayer.screen == MenuScreen::Host;
                                        theme::heading(
                                            ui,
                                            i18n.text(if host {
                                                "multiplayer-host"
                                            } else {
                                                "menu-singleplayer"
                                            }),
                                        );
                                        if host {
                                            theme::hint(ui, i18n.text("multiplayer-host-hint"));
                                            if !cfg!(feature = "gns") {
                                                theme::hint(
                                                    ui,
                                                    i18n.text("multiplayer-unavailable"),
                                                );
                                            }
                                        }
                                        ui.add_enabled_ui(!host || cfg!(feature = "gns"), |ui| {
                                            if theme::button(
                                                ui,
                                                i18n.text("menu-new-game"),
                                                width,
                                                true,
                                            )
                                            .clicked()
                                            {
                                                multiplayer.host_setup = host;
                                                multiplayer.host_settings_tab = true;
                                                persistence.retain_image_for_host = host;
                                                next_state.set(AppState::GameSetup);
                                            }
                                            if theme::button(
                                                ui,
                                                i18n.text("menu-load-game"),
                                                width,
                                                false,
                                            )
                                            .clicked()
                                            {
                                                dialogs.host_load = host;
                                                dialogs.load_open = true;
                                                service.list(&mut persistence);
                                            }
                                        });
                                    }
                                    MenuScreen::Multiplayer => {
                                        theme::heading(ui, i18n.text("menu-multiplayer"));
                                        if theme::button(
                                            ui,
                                            i18n.text("multiplayer-host"),
                                            width,
                                            true,
                                        )
                                        .clicked()
                                        {
                                            multiplayer.navigate(MenuScreen::Host);
                                        }
                                        if theme::button(
                                            ui,
                                            i18n.text("multiplayer-join"),
                                            width,
                                            false,
                                        )
                                        .clicked()
                                        {
                                            multiplayer.navigate(MenuScreen::Join);
                                        }
                                    }
                                    MenuScreen::Join => {
                                        if !cfg!(feature = "gns") {
                                            theme::hint(ui, i18n.text("multiplayer-unavailable"));
                                        }
                                        multiplayer::paint_join(
                                            ui,
                                            &mut multiplayer,
                                            &profile,
                                            &i18n,
                                        );
                                    }
                                    MenuScreen::HostLoadSettings => {
                                        if let Some((_, title)) = &multiplayer.selected_save {
                                            ui.label(title);
                                        }
                                        multiplayer::paint_connection_fields(
                                            ui,
                                            &mut multiplayer.host,
                                            true,
                                            &profile,
                                            &i18n,
                                        );
                                        let valid = multiplayer.host.valid(true)
                                            && !multiplayer.submitted
                                            && cfg!(feature = "gns");
                                        ui.add_enabled_ui(valid, |ui| {
                                            if theme::button(
                                                ui,
                                                i18n.text("multiplayer-start-host"),
                                                width,
                                                true,
                                            )
                                            .clicked()
                                            {
                                                multiplayer.submit_host();
                                            }
                                        });
                                    }
                                }
                                if theme::button(ui, i18n.text("menu-settings"), width, false)
                                    .clicked()
                                {
                                    multiplayer.host.clear_password();
                                    multiplayer.join.clear_password();
                                    settings_dialog.open(&display_settings);
                                }
                                if multiplayer.screen != MenuScreen::Title
                                    && theme::button(ui, i18n.text("common-back"), width, false)
                                        .clicked()
                                {
                                    let parent = match multiplayer.screen {
                                        MenuScreen::Host | MenuScreen::Join => {
                                            MenuScreen::Multiplayer
                                        }
                                        MenuScreen::HostLoadSettings => MenuScreen::Host,
                                        _ => MenuScreen::Title,
                                    };
                                    multiplayer.navigate(parent);
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
