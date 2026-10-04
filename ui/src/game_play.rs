use crate::localization::Localization;
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use jigsall_game::keybindings::{KeyAction, KeyBindingsState};
use jigsall_game::persistence::runtime::PersistenceState;
use jigsall_game::resources::*;

/// インゲームUI（プレイ中のUI）
#[allow(clippy::too_many_arguments)]
pub fn draw_game_ui(
    i18n: Res<Localization>,
    mut contexts: EguiContexts,
    game_state: Res<GameData>,
    definition: Option<Res<jigsall_core::PuzzleDefinition>>,
    roster: Res<PlayerRoster>,
    bindings: Res<KeyBindingsState>,
    mut capture: ResMut<GameUiPointerCapture>,
    persistence: Res<PersistenceState>,
    network: Res<jigsall_game::network::runtime::NetworkStatus>,
    sub_state: Res<State<GameSubState>>,
    mut next_sub_state: ResMut<NextState<GameSubState>>,
) {
    let _span = info_span!("draw_game_ui").entered();

    capture.over_hud = false;
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let mut viewport_ui = egui::Ui::new(
        ctx.clone(),
        "game_info_viewport".into(),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.viewport_rect()),
    );
    let panel = egui::Panel::top("game_info").show(&mut viewport_ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                i18n.format(
                    "game-progress",
                    &[(
                        "percent",
                        format!("{:.1}", game_state.puzzle_progress * 100.0)
                            .as_str()
                            .into(),
                    )],
                ),
            );
            if roster.len() > 1 || network.role.is_some() {
                ui.separator();
                paint_player_count(ui, &roster, &i18n);
            }
            match network.role {
                Some(jigsall_game::network::runtime::RuntimeRole::Host) => {
                    ui.label(i18n.text("multiplayer-hosting"));
                    crate::multiplayer::paint_room_code(ui, &network, &i18n);
                }
                Some(jigsall_game::network::runtime::RuntimeRole::Client)
                    if network.phase == jigsall_game::network::runtime::RuntimePhase::Ready =>
                {
                    ui.label(i18n.text("multiplayer-connected"));
                }
                _ => {}
            }
            crate::multiplayer::paint_control_warning(ui, &network, &i18n);
            paint_autosave_status(ui, &persistence, &i18n);

            ui.separator();
            // HUD buttons must not capture Tab or block gameplay key bindings.
            egui::containers::menu::MenuButton::from_button(
                egui::Button::new(i18n.text("game-controls")).sense(egui::Sense::CLICK),
            )
            .ui(ui, |ui| {
                paint_controls(
                    ui,
                    &bindings,
                    &i18n,
                    definition.as_ref().is_some_and(|d| d.rotation_enabled),
                );
            });
            if ui
                .add_enabled(
                    *sub_state.get() == GameSubState::Playing,
                    egui::Button::new(i18n.text("game-menu")).sense(egui::Sense::CLICK),
                )
                .clicked()
            {
                next_sub_state.set(GameSubState::Paused);
            }
        });
    });
    capture.over_hud = ctx
        .pointer_interact_pos()
        .is_some_and(|point| panel.response.rect.contains(point));
}

fn paint_controls(
    ui: &mut egui::Ui,
    bindings: &KeyBindingsState,
    i18n: &Localization,
    rotation_enabled: bool,
) {
    ui.set_max_width(360.0_f32.min(ui.ctx().content_rect().width() - 32.0));
    let binding_label = |action| {
        let label = bindings.current.binding(action).label();
        if label.is_empty() {
            i18n.text("keys-unassigned")
        } else {
            label
        }
    };
    if rotation_enabled {
        ui.label(i18n.format(
            "game-drag-hint",
            &[
                ("left", binding_label(KeyAction::RotateLeft).as_str().into()),
                (
                    "right",
                    binding_label(KeyAction::RotateRight).as_str().into(),
                ),
            ],
        ));
    } else {
        ui.label(i18n.text("game-drag-only-hint"));
    }
    ui.label(i18n.text("completion-navigation"));
    ui.label(i18n.format(
        "game-select-hint",
        &[(
            "keys",
            binding_label(KeyAction::MultiSelect).as_str().into(),
        )],
    ));
    ui.label(i18n.format(
        "game-tab-hint",
        &[(
            "keys",
            binding_label(KeyAction::ShowPlayers).as_str().into(),
        )],
    ));
}

fn paint_autosave_status(ui: &mut egui::Ui, state: &PersistenceState, i18n: &Localization) {
    if state.autosaving {
        ui.separator();
        ui.spinner();
        ui.label(i18n.text("game-autosaving"));
    } else if let Some(error) = &state.autosave_error {
        ui.separator();
        let key = if matches!(
            error,
            jigsall_game::persistence::runtime::PersistenceError::AutosaveRotation(_)
        ) {
            "game-autosave-rotation-failed"
        } else {
            "game-autosave-failed"
        };
        ui.colored_label(crate::theme::DANGER, i18n.text(key))
            .on_hover_text(i18n.persistence_error(error));
    }
}

/// プレイヤー一覧オーバーレイ（割り当てキーを押している間表示）
pub fn draw_players_overlay(
    i18n: Res<Localization>,
    mut contexts: EguiContexts,
    roster: Res<PlayerRoster>,
) {
    let _span = info_span!("draw_players_overlay").entered();

    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };

    // 半透明の背景を表示
    egui::Area::new(egui::Id::new("players_overlay_background"))
        .order(egui::Order::Background)
        .interactable(false)
        .fixed_pos(egui::pos2(0.0, 0.0))
        .show(ctx, |ui| {
            let screen_rect = ctx.content_rect();
            ui.allocate_ui_with_layout(
                screen_rect.size(),
                egui::Layout::centered_and_justified(egui::Direction::TopDown),
                |ui| {
                    // 背景全体を半透明の黒で覆う
                    ui.painter().rect_filled(
                        screen_rect,
                        egui::CornerRadius::ZERO,
                        egui::Color32::from_black_alpha(100), // 少し薄めの半透明
                    );
                },
            );
        });

    // プレイヤー一覧を画面上部中央に表示（上部UIの下に配置）
    egui::Window::new(i18n.text("game-players"))
        .id("players_window".into())
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 120.0)) // 上部UIを避けて配置
        .collapsible(false)
        .resizable(false)
        .title_bar(true)
        .show(ctx, |ui| {
            ui.set_min_width(400.0);

            // ウィンドウの高さを画面の半分に制限
            let screen_height = ctx.content_rect().height();
            let max_height = screen_height * 0.5;

            egui::ScrollArea::vertical()
                .max_height(max_height)
                .show(ui, |ui| {
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 10.0;

                        paint_players(ui, &roster, &i18n);
                    });
                });
        });
}

fn paint_player_count(ui: &mut egui::Ui, roster: &PlayerRoster, i18n: &Localization) {
    ui.label(i18n.format("game-player-count", &[("count", roster.len().into())]));
}

fn paint_players(ui: &mut egui::Ui, roster: &PlayerRoster, i18n: &Localization) {
    if roster.is_empty() {
        ui.centered_and_justified(|ui| {
            ui.label(i18n.text("game-no-players"));
        });
    } else {
        // プレイヤー一覧
        for player in roster.players() {
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    if let Some(name) = &player.display_name {
                        ui.label(name.as_ref());
                    } else {
                        ui.label(i18n.text("game-default-player"));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(i18n.format("game-score", &[("score", player.score.into())]));
                    });
                });
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hud_and_overlay_render_roster_count_duplicate_names_default_and_scores() {
        use jigsall_core::{PlayerDisplayName, PlayerId};
        use jigsall_game::players::{RosterPlayer, RosterSnapshot};
        let mut roster = PlayerRoster::default();
        roster
            .install_snapshot(
                RosterSnapshot {
                    revision: 2,
                    players: (0..3)
                        .map(|id| RosterPlayer {
                            player: PlayerId(id),
                            display_name: (id != 0)
                                .then(|| PlayerDisplayName::from_user_input("Alice").unwrap()),
                        })
                        .collect(),
                },
                PlayerId(0),
                PlayerId(1),
            )
            .unwrap();
        let i18n = crate::localization::tests::english();
        let ctx = egui::Context::default();
        let output = ctx.run_ui(default(), |ui| {
            paint_player_count(ui, &roster, &i18n);
            paint_players(ui, &roster, &i18n);
        });
        let texts: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|s| {
                if let egui::Shape::Text(text) = &s.shape {
                    Some(text.galley.job.text.as_str())
                } else {
                    None
                }
            })
            .collect();
        assert!(texts.contains(
            &i18n
                .format("game-player-count", &[("count", 3usize.into())])
                .as_str()
        ));
        assert_eq!(texts.iter().filter(|t| **t == "Alice").count(), 2);
        assert!(texts.contains(&i18n.text("game-default-player").as_str()));
        assert_eq!(
            texts
                .iter()
                .filter(|t| **t == i18n.format("game-score", &[("score", 0u32.into())]))
                .count(),
            3
        );
        output.drop_without_applying_deltas();
    }

    #[test]
    fn autosave_status_shows_work_and_failure_and_hides_after_completion() {
        for locale in [
            crate::localization::Locale::EN_US,
            crate::localization::Locale::JA,
        ] {
            let mut i18n = crate::localization::tests::english();
            i18n.set_preference(crate::localization::LanguagePreference::Locale(locale));
            for (saving, failed) in [(true, false), (false, true), (false, false)] {
                let mut state = PersistenceState::default();
                state.autosaving = saving;
                state.autosave_error = failed
                    .then_some(jigsall_game::persistence::runtime::PersistenceError::WorkerStopped);
                let ctx = egui::Context::default();
                let output = ctx.run_ui(default(), |ui| {
                    ui.horizontal(|ui| paint_autosave_status(ui, &state, &i18n));
                });
                let text: Vec<_> = output
                    .shapes
                    .iter()
                    .filter_map(|shape| match &shape.shape {
                        egui::Shape::Text(text) => Some(text.galley.job.text.as_str()),
                        _ => None,
                    })
                    .collect();
                assert_eq!(
                    text.contains(&i18n.text("game-autosaving").as_str()),
                    saving
                );
                assert_eq!(
                    text.contains(&i18n.text("game-autosave-failed").as_str()),
                    failed
                );
                output.drop_without_applying_deltas();
            }
        }
    }
}
