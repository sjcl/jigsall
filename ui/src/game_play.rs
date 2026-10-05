use crate::localization::Localization;
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use jigsall_core::PlayerId;
use jigsall_game::keybindings::{KeyAction, KeyBindingsState};
use jigsall_game::persistence::runtime::PersistenceState;
use jigsall_game::resources::*;

/// Click-to-pin is independent of the momentary ShowPlayers key binding.
#[derive(Resource, Default)]
pub(crate) struct PlayersOverlayState {
    opened_by_click: bool,
}

pub(crate) fn reset_players_overlay(mut state: ResMut<PlayersOverlayState>) {
    *state = default();
}

pub(crate) fn players_overlay_pinned(state: Res<PlayersOverlayState>) -> bool {
    state.opened_by_click
}

/// インゲームUI（プレイ中のUI）
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_game_ui(
    i18n: Res<Localization>,
    mut contexts: EguiContexts,
    game_state: Res<GameData>,
    definition: Option<Res<jigsall_core::PuzzleDefinition>>,
    roster: Res<PlayerRoster>,
    mut players_overlay: ResMut<PlayersOverlayState>,
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
                paint_player_count(ui, &roster, &i18n, &mut players_overlay);
            }
            match network.role {
                Some(jigsall_game::network::runtime::RuntimeRole::Host) => {
                    if let Some(key) = crate::multiplayer::host_status_key(&network) {
                        ui.label(i18n.text(key));
                    }
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

/// プレイヤー一覧オーバーレイ（割り当てキーの長押し、または人数のクリックで表示）
pub(crate) fn draw_players_overlay(
    i18n: Res<Localization>,
    mut contexts: EguiContexts,
    roster: Res<PlayerRoster>,
    local: Res<LocalPlayerId>,
    network: Res<jigsall_game::network::runtime::NetworkStatus>,
    mut state: ResMut<PlayersOverlayState>,
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
            ui.set_min_width((ctx.content_rect().width() - 48.0).clamp(160.0, 400.0));

            // ウィンドウの高さを画面の半分に制限
            let screen_height = ctx.content_rect().height();
            let max_height = screen_height * 0.5;

            egui::ScrollArea::vertical()
                .max_height(max_height)
                .show(ui, |ui| {
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 10.0;

                        paint_players(ui, &roster, local.0, network.host, &i18n);
                    });
                });
            if state.opened_by_click {
                ui.separator();
                if ui
                    .add(egui::Button::new(i18n.text("common-close")).sense(egui::Sense::CLICK))
                    .clicked()
                {
                    state.opened_by_click = false;
                }
            }
        });
}

fn paint_player_count(
    ui: &mut egui::Ui,
    roster: &PlayerRoster,
    i18n: &Localization,
    state: &mut PlayersOverlayState,
) {
    if ui
        .add(
            egui::Button::new(i18n.format("game-player-count", &[("count", roster.len().into())]))
                .selected(state.opened_by_click)
                .sense(egui::Sense::CLICK),
        )
        .on_hover_text(i18n.text("game-player-list-toggle"))
        .clicked()
    {
        state.opened_by_click = !state.opened_by_click;
    }
}

fn paint_players(
    ui: &mut egui::Ui,
    roster: &PlayerRoster,
    local: PlayerId,
    host: Option<PlayerId>,
    i18n: &Localization,
) {
    if roster.is_empty() {
        ui.centered_and_justified(|ui| {
            ui.label(i18n.text("game-no-players"));
        });
    } else {
        // プレイヤー一覧
        for player in roster.players() {
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    let color = jigsall_game::render::remote_cursor::player_color_srgb(player.id);
                    let (marker, _) =
                        ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                    ui.painter().circle_filled(
                        marker.center(),
                        5.0,
                        egui::Color32::from_rgb(color[0], color[1], color[2]),
                    );
                    if let Some(name) = &player.display_name {
                        ui.label(name.as_ref());
                    } else {
                        ui.label(i18n.text("game-default-player"));
                    }
                    if player.id == local {
                        ui.weak(i18n.text("game-player-you"));
                    }
                    if Some(player.id) == host {
                        ui.weak(i18n.text("game-player-host"));
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
    use jigsall_game::network::runtime::{
        NetworkStatus, RendezvousControlStatus, RuntimeConnectionMethod, RuntimePhase, RuntimeRole,
    };

    fn hud_app() -> (App, egui::Context) {
        let mut app = App::new();
        let entity = app
            .world_mut()
            .spawn((
                bevy_egui::EguiContext::default(),
                bevy_egui::PrimaryEguiContext,
            ))
            .id();
        let ctx = app
            .world_mut()
            .get_mut::<bevy_egui::EguiContext>(entity)
            .unwrap()
            .get_mut()
            .clone();
        app.init_resource::<bevy_egui::EguiUserTextures>()
            .insert_resource(crate::localization::tests::english())
            .insert_resource(KeyBindingsState::load(None))
            .init_resource::<GameData>()
            .init_resource::<GameUiPointerCapture>()
            .init_resource::<PlayersOverlayState>()
            .init_resource::<LocalPlayerId>()
            .init_resource::<PersistenceState>()
            .init_resource::<ButtonInput<KeyCode>>()
            .insert_resource(PlayerRoster::host_only(PlayerId(0), None))
            .insert_resource(NetworkStatus {
                role: Some(RuntimeRole::Host),
                phase: RuntimePhase::Hosting,
                host: Some(PlayerId(0)),
                ..default()
            })
            .insert_resource(State::new(GameSubState::Playing))
            .init_resource::<NextState<GameSubState>>()
            .add_systems(
                bevy_egui::EguiPrimaryContextPass,
                (
                    draw_game_ui,
                    draw_players_overlay
                        .after(draw_game_ui)
                        .run_if(players_overlay_pinned.or_else(crate::players_key_pressed)),
                ),
            );
        (app, ctx)
    }

    fn render_hud(
        app: &mut App,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 800.0),
                )),
                events,
                ..default()
            },
            |_| {
                app.world_mut()
                    .run_schedule(bevy_egui::EguiPrimaryContextPass)
            },
        )
    }

    fn click_hud_label(app: &mut App, ctx: &egui::Context, label: &str) {
        render_hud(app, ctx, vec![]).drop_without_applying_deltas();
        let output = render_hud(app, ctx, vec![]);
        let point = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.job.text == label => {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing HUD label: {label}"));
        output.drop_without_applying_deltas();
        for pressed in [true, false] {
            render_hud(
                app,
                ctx,
                vec![
                    egui::Event::PointerMoved(point),
                    egui::Event::PointerButton {
                        pos: point,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: default(),
                    },
                ],
            )
            .drop_without_applying_deltas();
        }
    }

    fn assert_roster_visible(app: &mut App, ctx: &egui::Context, visible: bool) {
        render_hud(app, ctx, vec![]).drop_without_applying_deltas();
        let output = render_hud(app, ctx, vec![]);
        assert_eq!(
            output.shapes.iter().any(|shape| matches!(
                &shape.shape,
                egui::Shape::Text(text) if text.galley.job.text == "Players"
            )),
            visible
        );
        output.drop_without_applying_deltas();
    }

    #[test]
    fn player_count_clicks_toggle_and_close_roster_without_stealing_gameplay_keys() {
        let (mut app, ctx) = hud_app();
        assert_roster_visible(&mut app, &ctx, false);
        for _ in 0..3 {
            click_hud_label(&mut app, &ctx, "Players: 1");
            assert!(
                app.world()
                    .resource::<PlayersOverlayState>()
                    .opened_by_click
            );
            assert_roster_visible(&mut app, &ctx, true);
            assert!(app.world().resource::<GameUiPointerCapture>().over_hud);
            assert!(ctx.memory(|memory| memory.focused().is_none()));
            assert!(!ctx.egui_wants_keyboard_input());
            click_hud_label(&mut app, &ctx, "Players: 1");
            assert_roster_visible(&mut app, &ctx, false);
        }
        click_hud_label(&mut app, &ctx, "Players: 1");
        click_hud_label(&mut app, &ctx, "Close");
        assert_roster_visible(&mut app, &ctx, false);
        assert!(ctx.memory(|memory| memory.focused().is_none()));
        assert!(!ctx.egui_wants_keyboard_input());
    }

    #[test]
    fn held_players_key_and_click_pin_remain_independent() {
        let (mut app, ctx) = hud_app();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Tab);
        render_hud(
            &mut app,
            &ctx,
            vec![egui::Event::Key {
                key: egui::Key::Tab,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: default(),
            }],
        )
        .drop_without_applying_deltas();
        assert!(ctx.memory(|memory| memory.focused().is_none()));
        assert!(!ctx.egui_wants_keyboard_input());
        assert_roster_visible(&mut app, &ctx, true);
        assert!(
            !app.world()
                .resource::<PlayersOverlayState>()
                .opened_by_click
        );
        click_hud_label(&mut app, &ctx, "Players: 1");
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::Tab);
        assert_roster_visible(&mut app, &ctx, true);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Tab);
        click_hud_label(&mut app, &ctx, "Close");
        assert!(
            !app.world()
                .resource::<PlayersOverlayState>()
                .opened_by_click
        );
        assert_roster_visible(&mut app, &ctx, true);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::Tab);
        assert_roster_visible(&mut app, &ctx, false);
    }

    #[test]
    fn menu_and_new_game_reset_click_pinned_roster() {
        let mut app = App::new();
        crate::register_screens(&mut app);
        for state in [AppState::Menu, AppState::InGame] {
            app.world_mut()
                .resource_mut::<PlayersOverlayState>()
                .opened_by_click = true;
            app.world_mut().run_schedule(OnEnter(state));
            assert!(
                !app.world()
                    .resource::<PlayersOverlayState>()
                    .opened_by_click
            );
        }
    }

    #[test]
    fn held_players_key_is_hidden_while_the_window_is_unfocused() {
        let (mut app, ctx) = hud_app();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Tab);
        app.world_mut().spawn((
            Window {
                focused: false,
                ..default()
            },
            bevy::window::PrimaryWindow,
        ));
        assert_roster_visible(&mut app, &ctx, false);
    }

    #[test]
    fn hud_stops_advertising_open_room_when_room_service_is_unavailable() {
        let (mut app, ctx) = hud_app();
        for control in [
            RendezvousControlStatus::Available,
            RendezvousControlStatus::Unavailable,
            RendezvousControlStatus::Connecting,
        ] {
            app.world_mut().insert_resource(NetworkStatus {
                role: Some(RuntimeRole::Host),
                phase: RuntimePhase::Hosting,
                connection_method: Some(RuntimeConnectionMethod::Internet),
                room_code: Some("ABCDE234".into()),
                rendezvous_control: Some(control),
                ..default()
            });
            let output = render_hud(&mut app, &ctx, vec![]);
            let i18n = app.world().resource::<Localization>();
            let texts: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) => Some(text.galley.job.text.as_str()),
                    _ => None,
                })
                .collect();
            assert_eq!(
                texts.contains(&i18n.text("multiplayer-hosting").as_str()),
                control == RendezvousControlStatus::Available,
            );
            assert_eq!(
                texts.contains(&i18n.text("multiplayer-host-not-accepting").as_str()),
                control != RendezvousControlStatus::Available,
            );
            output.drop_without_applying_deltas();
        }
    }

    #[test]
    fn roster_never_guesses_host_identity_and_can_label_self_as_host() {
        let roster = PlayerRoster::host_only(PlayerId(17), None);
        let mut i18n = crate::localization::tests::english();
        for locale in [
            crate::localization::Locale::EN_US,
            crate::localization::Locale::JA,
        ] {
            i18n.set_preference(crate::localization::LanguagePreference::Locale(locale));
            for host in [None, Some(PlayerId(0)), Some(PlayerId(17))] {
                let ctx = egui::Context::default();
                let output = ctx.run_ui(default(), |ui| {
                    paint_players(ui, &roster, PlayerId(17), host, &i18n);
                });
                let texts: Vec<_> = output
                    .shapes
                    .iter()
                    .filter_map(|shape| match &shape.shape {
                        egui::Shape::Text(text) => Some(text.galley.job.text.as_str()),
                        _ => None,
                    })
                    .collect();
                assert!(texts.contains(&i18n.text("game-player-you").as_str()));
                assert_eq!(
                    texts.contains(&i18n.text("game-player-host").as_str()),
                    host == Some(PlayerId(17)),
                );
                output.drop_without_applying_deltas();
            }
        }
    }

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
            paint_player_count(ui, &roster, &i18n, &mut PlayersOverlayState::default());
            paint_players(ui, &roster, PlayerId(1), Some(PlayerId(0)), &i18n);
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
        for key in ["game-player-you", "game-player-host"] {
            assert_eq!(
                texts.iter().filter(|text| **text == i18n.text(key)).count(),
                1
            );
        }
        for player in roster.players() {
            let [r, g, b] = jigsall_game::render::remote_cursor::player_color_srgb(player.id);
            assert!(output.shapes.iter().any(|shape| matches!(
                &shape.shape,
                egui::Shape::Circle(circle) if circle.fill == egui::Color32::from_rgb(r, g, b)
            )));
        }
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
