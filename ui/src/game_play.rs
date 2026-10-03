use crate::localization::Localization;
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use puzzella_game::keybindings::{KeyAction, KeyBindingsState};
use puzzella_game::persistence::runtime::PersistenceState;
use puzzella_game::resources::*;

/// インゲームUI（プレイ中のUI）
pub fn draw_game_ui(
    i18n: Res<Localization>,
    mut contexts: EguiContexts,
    game_state: Res<GameData>,
    bindings: Res<KeyBindingsState>,
    mut capture: ResMut<GameUiPointerCapture>,
    persistence: Res<PersistenceState>,
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
            ui.separator();
            ui.label(i18n.format(
                "game-player-count",
                &[("count", game_state.players.len().into())],
            ));
            paint_autosave_status(ui, &persistence, &i18n);

            ui.separator();
            let binding_label = |action| {
                let label = bindings.current.binding(action).label();
                if label.is_empty() {
                    i18n.text("keys-unassigned")
                } else {
                    label
                }
            };
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
            ui.separator();
            ui.label(i18n.format(
                "game-tab-hint",
                &[(
                    "keys",
                    binding_label(KeyAction::ShowPlayers).as_str().into(),
                )],
            ));
        });
    });
    capture.over_hud = ctx
        .pointer_interact_pos()
        .is_some_and(|point| panel.response.rect.contains(point));
}

fn paint_autosave_status(ui: &mut egui::Ui, state: &PersistenceState, i18n: &Localization) {
    if state.autosaving {
        ui.separator();
        ui.spinner();
        ui.label(i18n.text("game-autosaving"));
    } else if let Some(error) = &state.autosave_error {
        ui.separator();
        ui.colored_label(crate::theme::DANGER, i18n.text("game-autosave-failed"))
            .on_hover_text(i18n.persistence_error(error));
    }
}

/// プレイヤー一覧オーバーレイ（割り当てキーを押している間表示）
pub fn draw_players_overlay(
    i18n: Res<Localization>,
    mut contexts: EguiContexts,
    game_state: Res<GameData>,
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

                        if game_state.players.is_empty() {
                            ui.centered_and_justified(|ui| {
                                ui.label(i18n.text("game-no-players"));
                            });
                        } else {
                            // プレイヤー一覧
                            for player in &game_state.players {
                                ui.group(|ui| {
                                    ui.horizontal(|ui| {
                                        if let Some(name) = &player.name {
                                            ui.label(name);
                                        } else {
                                            ui.label(i18n.text("game-default-player"));
                                        }
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                ui.label(i18n.format(
                                                    "game-score",
                                                    &[("score", player.score.into())],
                                                ));
                                            },
                                        );
                                    });
                                });
                            }
                        }
                    });
                });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

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
                state.autosave_error = failed.then_some(
                    puzzella_game::persistence::runtime::PersistenceError::WorkerStopped,
                );
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
