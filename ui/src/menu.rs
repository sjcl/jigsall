use crate::localization::Localization;
use crate::multiplayer::{self, MenuScreen, MultiplayerUi};
use crate::theme;
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use jigsall_game::resources::*;

#[allow(clippy::too_many_arguments)]
pub fn draw_menu_ui(
    i18n: Res<Localization>,
    mut contexts: EguiContexts,
    mut next_state: ResMut<NextState<AppState>>,
    mut dialogs: ResMut<crate::persistence::SaveDialogs>,
    mut persistence: ResMut<jigsall_game::persistence::runtime::PersistenceState>,
    service: Res<jigsall_game::persistence::runtime::PersistenceService>,
    mut exit: MessageWriter<AppExit>,
    mut settings_dialog: ResMut<crate::settings::SettingsDialog>,
    display_settings: Res<jigsall_game::settings::DisplaySettingsState>,
    mut profile: ResMut<jigsall_game::player_settings::PlayerSettingsState>,
    status: Res<jigsall_game::network::runtime::NetworkStatus>,
    mut multiplayer: ResMut<MultiplayerUi>,
    #[cfg(feature = "rendezvous")] rendezvous_config: Option<
        Res<jigsall_game::network::runtime::RendezvousRuntimeConfig>,
    >,
) {
    #[cfg(feature = "rendezvous")]
    let internet_available = rendezvous_config.is_some();
    #[cfg(not(feature = "rendezvous"))]
    let internet_available = false;
    multiplayer.configure_connection_methods(internet_available);
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
                                egui::RichText::new("Jigsall")
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
                                                multiplayer.host_settings_tab = false;
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
                                        if let Some(screen) =
                                            paint_multiplayer_choices(ui, width, &i18n)
                                        {
                                            multiplayer.navigate(screen);
                                        }
                                    }
                                    MenuScreen::Join => {
                                        multiplayer::paint_method(
                                            ui,
                                            &mut multiplayer,
                                            internet_available,
                                            &i18n,
                                        );
                                        if !cfg!(feature = "gns") {
                                            theme::hint(ui, i18n.text("multiplayer-unavailable"));
                                        }
                                        multiplayer.paint_password_notice(ui, &i18n);
                                        multiplayer::paint_join(
                                            ui,
                                            &mut multiplayer,
                                            &mut profile,
                                            &i18n,
                                        );
                                    }
                                    MenuScreen::HostLoadSettings => {
                                        multiplayer::paint_method(
                                            ui,
                                            &mut multiplayer,
                                            internet_available,
                                            &i18n,
                                        );
                                        if let Some((_, title)) = &multiplayer.selected_save {
                                            ui.label(title);
                                        }
                                        multiplayer.paint_password_notice(ui, &i18n);
                                        multiplayer::paint_connection_fields(
                                            ui,
                                            &mut multiplayer.host,
                                            true,
                                            &mut profile,
                                            &i18n,
                                        );
                                        let valid =
                                            multiplayer.host_available() && !multiplayer.submitted;
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
                                if matches!(
                                    multiplayer.screen,
                                    MenuScreen::Title | MenuScreen::SinglePlayer
                                ) && theme::button(ui, i18n.text("menu-settings"), width, false)
                                    .clicked()
                                {
                                    multiplayer.clear_passwords_for_settings();
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
                                if multiplayer.screen == MenuScreen::Title {
                                    ui.add_space(4.0);
                                    if theme::danger_button(ui, i18n.text("menu-exit"), width)
                                        .clicked()
                                    {
                                        exit.write(AppExit::Success);
                                    }
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
        jigsall_game::build_info::version_label(),
        egui::FontId::proportional(10.0),
        theme::MUTED,
    );
}

fn paint_multiplayer_choices(
    ui: &mut egui::Ui,
    width: f32,
    i18n: &Localization,
) -> Option<MenuScreen> {
    theme::heading(ui, i18n.text("menu-multiplayer"));
    let mut selected = None;
    if theme::button(ui, i18n.text("multiplayer-host"), width, true).clicked() {
        selected = Some(MenuScreen::Host);
    }
    if theme::button(ui, i18n.text("multiplayer-join"), width, false).clicked() {
        selected = Some(MenuScreen::Join);
    }
    selected
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::localization::{LanguagePreference, Locale};

    #[test]
    fn multiplayer_entry_shows_purpose_choices_without_connection_methods() {
        let mut i18n = crate::localization::tests::english();
        for locale in [Locale::EN_US, Locale::JA] {
            i18n.set_preference(LanguagePreference::Locale(locale));
            let ctx = egui::Context::default();
            let render = |events| {
                let mut selected = None;
                let output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(400.0, 600.0),
                        )),
                        events,
                        ..default()
                    },
                    |ui| {
                        theme::prepare(ui.ctx());
                        ui.set_width(320.0);
                        selected = paint_multiplayer_choices(ui, 320.0, &i18n);
                    },
                );
                (output, selected)
            };
            render(vec![]).0.drop_without_applying_deltas();
            let (output, selected) = render(vec![]);
            assert!(selected.is_none());
            let labels: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) => Some(text.galley.job.text.as_str()),
                    _ => None,
                })
                .collect();
            for key in ["menu-multiplayer", "multiplayer-host", "multiplayer-join"] {
                assert!(labels.contains(&i18n.text(key).as_str()));
            }
            for key in ["multiplayer-internet", "multiplayer-direct-ip"] {
                assert!(!labels.contains(&i18n.text(key).as_str()));
            }
            let choices: Vec<_> = [
                ("multiplayer-host", MenuScreen::Host),
                ("multiplayer-join", MenuScreen::Join),
            ]
            .into_iter()
            .map(|(key, screen)| {
                let label = i18n.text(key);
                let position = output
                    .shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        egui::Shape::Text(text) if text.galley.job.text == label => {
                            Some(text.pos + text.galley.size() * 0.5)
                        }
                        _ => None,
                    })
                    .expect("localized multiplayer choice");
                (position, screen)
            })
            .collect();
            output.drop_without_applying_deltas();
            for (position, screen) in choices {
                for pressed in [true, false] {
                    let (output, selected) = render(vec![
                        egui::Event::PointerMoved(position),
                        egui::Event::PointerButton {
                            pos: position,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: default(),
                        },
                    ]);
                    if !pressed {
                        assert!(selected == Some(screen));
                    }
                    output.drop_without_applying_deltas();
                }
            }
        }
    }
}
