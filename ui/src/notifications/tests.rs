use super::*;
mod native;
use crate::localization::{LanguagePreference, Locale};
use jigsall_core::{PlayerDisplayName, PlayerId};
use jigsall_game::events::JoinAttemptId as ConnectionId;
use jigsall_game::{
    network::transport::DisconnectReason, persistence::runtime::PersistenceError,
    players::RosterPlayer,
};

fn player(id: u64, name: Option<&str>) -> RosterPlayer {
    RosterPlayer {
        player: PlayerId(id),
        display_name: name.map(|name| PlayerDisplayName::from_user_input(name).unwrap()),
    }
}

fn accept(state: &mut Notifications, event: GameEvent, now: Instant) {
    state.accept(event, now, Local::now());
}

#[test]
fn concurrent_operations_survive_expiry_and_finish_independently() {
    let now = Instant::now();
    let mut state = Notifications::default();
    for id in 1..=4 {
        accept(
            &mut state,
            GameEvent::PlayerJoining {
                connection: ConnectionId(id),
            },
            now,
        );
    }
    accept(&mut state, GameEvent::AutosaveStarted, now);
    state.expire(now + Duration::from_secs(60));
    assert_eq!(state.ongoing.len(), 5);
    assert_eq!(state.visible().len(), TOAST_LIMIT);
    accept(
        &mut state,
        GameEvent::PlayerJoined {
            player: player(1, Some("Alice")),
            connection: Some(ConnectionId(1)),
        },
        now,
    );
    accept(
        &mut state,
        GameEvent::PlayerJoinCancelled {
            connection: ConnectionId(2),
            reason: DisconnectReason::RemoteClosed,
        },
        now,
    );
    accept(&mut state, GameEvent::AutosaveSucceeded, now);
    assert_eq!(state.ongoing.len(), 2);
    state.expire(now + TOAST_DURATION);
    let visible = state.visible();
    assert_eq!(visible.len(), 2);
    assert!(visible.iter().all(|e| matches!(
        e.event,
        GameEvent::PlayerJoining {
            connection: ConnectionId(3 | 4)
        }
    )));
    assert_eq!(state.history.len(), 8);
}

#[test]
fn burst_keeps_newest_three_toasts_and_two_hundred_history_entries() {
    let now = Instant::now();
    let mut state = Notifications::default();
    for id in 0..250 {
        accept(
            &mut state,
            GameEvent::PlayerLeft {
                player: player(id, Some("Alice")),
            },
            now,
        );
    }
    assert_eq!(state.history.len(), HISTORY_LIMIT);
    assert_eq!(state.history.front().unwrap().sequence, 50);
    assert_eq!(state.history.back().unwrap().sequence, 249);
    assert_eq!(
        state
            .visible()
            .iter()
            .map(|e| e.sequence)
            .collect::<Vec<_>>(),
        [247, 248, 249]
    );
    state.expire(now + TOAST_DURATION);
    assert!(state.visible().is_empty());
    assert_eq!(state.history.len(), HISTORY_LIMIT);
}

#[test]
fn reset_discards_history_progress_toasts_and_open_panel_in_message_order() {
    let mut app = App::new();
    app.add_message::<GameEvent>()
        .init_resource::<Notifications>()
        .add_systems(Update, collect_events);
    app.world_mut().resource_mut::<Notifications>().opened = true;
    app.world_mut()
        .resource_mut::<Messages<GameEvent>>()
        .write_batch([
            GameEvent::PlayerJoining {
                connection: ConnectionId(1),
            },
            GameEvent::AutosaveStarted,
            GameEvent::SessionReset,
            GameEvent::PlayerJoined {
                player: player(2, Some("Bob")),
                connection: None,
            },
        ]);
    app.update();
    let state = app.world().resource::<Notifications>();
    assert!(!state.opened);
    assert!(state.ongoing.is_empty());
    assert_eq!(state.history.len(), 1);
    assert_eq!(state.visible().len(), 1);
    app.update();
    assert_eq!(app.world().resource::<Notifications>().history.len(), 1);
}

#[test]
fn duplicate_start_does_not_add_history_or_refresh_progress() {
    let now = Instant::now();
    let mut state = Notifications::default();
    for _ in 0..2 {
        accept(
            &mut state,
            GameEvent::PlayerJoining {
                connection: ConnectionId(3),
            },
            now,
        );
        accept(&mut state, GameEvent::AutosaveStarted, now);
    }
    assert_eq!(state.history.len(), 2);
    assert_eq!(state.ongoing.len(), 2);
}

#[test]
fn autosave_issue_survives_expiry_eviction_and_retry_until_success_or_reset() {
    let now = Instant::now();
    for issue in [
        GameEvent::AutosaveFailed {
            error: PersistenceError::WorkerStopped,
        },
        GameEvent::AutosaveRotationFailed {
            error: PersistenceError::WorkerStopped,
        },
    ] {
        let mut state = Notifications::default();
        accept(&mut state, issue.clone(), now);
        for id in 0..HISTORY_LIMIT {
            accept(
                &mut state,
                GameEvent::PlayerLeft {
                    player: player(id as u64, None),
                },
                now,
            );
        }
        state.expire(now + TOAST_DURATION);
        assert!(state.visible().is_empty());
        assert!(state
            .history
            .iter()
            .all(|e| matches!(e.event, GameEvent::PlayerLeft { .. })));
        assert!(state.autosave_issue.is_some());
        state.opened = true;
        accept(&mut state, GameEvent::AutosaveStarted, now);
        assert!(state.autosave_issue.is_some());
        accept(&mut state, GameEvent::AutosaveSucceeded, now);
        assert!(state.autosave_issue.is_none());
        accept(&mut state, issue, now);
        assert!(state.autosave_issue.is_some());
        accept(&mut state, GameEvent::SessionReset, now);
        assert!(state.autosave_issue.is_none());
        assert!(state.history.is_empty());
    }
}

#[test]
fn every_join_cancellation_reason_has_localized_user_facing_details() {
    let mut i18n = crate::localization::tests::english();
    let reasons = [
        DisconnectReason::Requested,
        DisconnectReason::RemoteClosed,
        DisconnectReason::ConnectionProblem,
        DisconnectReason::InvalidMessage,
        DisconnectReason::RateLimited,
        DisconnectReason::BackendFailure,
        DisconnectReason::AuthenticationFailed,
        DisconnectReason::AuthenticationTimeout,
        DisconnectReason::AuthenticatedHandoffTimeout,
        DisconnectReason::BackendConnectionTimeout,
        DisconnectReason::JoinCapacity,
        DisconnectReason::SyncPhaseTimeout,
        DisconnectReason::SyncLifetime,
        DisconnectReason::HostCapacityTimeout,
        DisconnectReason::BulkStalled,
        DisconnectReason::ProtocolViolation,
    ];
    for locale in [Locale::EN_US, Locale::JA] {
        i18n.set_preference(LanguagePreference::Locale(locale));
        for reason in reasons {
            let text = i18n.text(join_cancel_reason(reason));
            assert!(!text.is_empty());
            assert!(!text.starts_with("events-"));
            assert!(!text.contains(&format!("{reason:?}")));
        }
        assert_eq!(
            i18n.text(join_cancel_reason(DisconnectReason::SyncPhaseTimeout)),
            if locale == Locale::JA {
                "パズルの同期がタイムアウトしました。"
            } else {
                "Puzzle synchronization timed out."
            }
        );
    }
}

#[test]
fn all_events_render_in_both_languages_and_existing_history_relocalizes() {
    let now = Instant::now();
    let mut state = Notifications::default();
    let mut i18n = crate::localization::tests::english();
    for event in [
        GameEvent::PlayerJoining {
            connection: ConnectionId(1),
        },
        GameEvent::PlayerJoined {
            player: player(1, Some("Alice 🧩")),
            connection: None,
        },
        GameEvent::PlayerLeft {
            player: player(1, Some("Alice 🧩")),
        },
        GameEvent::PlayerLeft {
            player: player(2, None),
        },
        GameEvent::AutosaveStarted,
        GameEvent::AutosaveSucceeded,
        GameEvent::AutosaveFailed {
            error: PersistenceError::WorkerStopped,
        },
        GameEvent::AutosaveRotationFailed {
            error: PersistenceError::WorkerStopped,
        },
        GameEvent::PlayerJoinCancelled {
            connection: ConnectionId(1),
            reason: DisconnectReason::RemoteClosed,
        },
    ] {
        accept(&mut state, event, now);
    }
    for locale in [Locale::EN_US, Locale::JA] {
        i18n.set_preference(LanguagePreference::Locale(locale));
        let ctx = egui::Context::default();
        state.opened = true;
        for frame in 0..3 {
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1280.0, 800.0),
                    )),
                    ..default()
                },
                |ui| {
                    paint_history(
                        ui.ctx(),
                        &mut state,
                        &i18n,
                        &mut GameUiPointerCapture::default(),
                    )
                },
            );
            let texts: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Text(text) => Some(text.galley.job.text.as_str()),
                    _ => None,
                })
                .collect();
            let name_visible = texts.iter().any(|t| t.contains("Alice 🧩"));
            let reason_visible = texts.contains(
                &i18n
                    .text(join_cancel_reason(DisconnectReason::RemoteClosed))
                    .as_str(),
            );
            output.drop_without_applying_deltas();
            if frame == 2 {
                assert!(name_visible);
                assert!(reason_visible);
            }
        }
        for entry in &state.history {
            assert!(!message(&entry.event, &i18n).starts_with("events-"));
        }
        assert_eq!(
            message(&state.history[2].event, &i18n),
            if locale == Locale::JA {
                "Alice 🧩 が退出しました"
            } else {
                "Alice 🧩 left"
            }
        );
        assert_eq!(
            message(&state.history[3].event, &i18n),
            i18n.format(
                "events-player-left",
                &[("player", i18n.text("game-default-player").as_str().into())]
            )
        );
    }
}

#[test]
fn toast_paint_passes_input_through_and_fits_below_wrapped_hud() {
    let now = Instant::now();
    let i18n = crate::localization::tests::english();
    for size in [egui::vec2(1280.0, 800.0), egui::vec2(480.0, 360.0)] {
        let mut state = Notifications {
            hud_bottom: 96.0,
            ..default()
        };
        accept(&mut state, GameEvent::AutosaveStarted, now);
        let ctx = egui::Context::default();
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(viewport),
                ..default()
            },
            |ui| paint_toasts(ui.ctx(), &state, &i18n),
        );
        let rect = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Rect(rect) => Some(rect.rect),
                _ => None,
            })
            .unwrap();
        assert!(viewport.contains_rect(rect));
        assert!(rect.top() > state.hud_bottom);
        output.drop_without_applying_deltas();
        let point = rect.center();
        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(viewport),
                events: vec![
                    egui::Event::PointerMoved(point),
                    egui::Event::PointerButton {
                        pos: point,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: default(),
                    },
                ],
                ..default()
            },
            |ui| paint_toasts(ui.ctx(), &state, &i18n),
        );
        assert!(!ctx.egui_wants_pointer_input());
        assert!(!ctx.is_pointer_over_egui());
        output.drop_without_applying_deltas();
    }
}

#[test]
fn history_captures_click_drag_and_scroll_and_shows_latest_first() {
    let i18n = crate::localization::tests::english();
    let mut state = Notifications {
        opened: true,
        hud_bottom: 48.0,
        ..default()
    };
    for id in 0..60 {
        accept(
            &mut state,
            GameEvent::PlayerLeft {
                player: player(id, Some(&format!("Player{id}"))),
            },
            Instant::now(),
        );
    }
    let ctx = egui::Context::default();
    let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0));
    let mut capture = GameUiPointerCapture::default();
    let mut point = egui::Pos2::ZERO;
    for frame in 0..3 {
        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(viewport),
                ..default()
            },
            |ui| paint_history(ui.ctx(), &mut state, &i18n, &mut capture),
        );
        let label_point = output.shapes.iter().find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.job.text == "Player59 left" => {
                Some(text.pos + text.galley.size() * 0.5)
            }
            _ => None,
        });
        output.drop_without_applying_deltas();
        if frame == 2 {
            point = label_point.expect("latest event visible after window sizing");
        }
    }
    for events in [
        vec![
            egui::Event::PointerMoved(point),
            egui::Event::PointerButton {
                pos: point,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: default(),
            },
        ],
        vec![egui::Event::PointerMoved(point + egui::vec2(10.0, 10.0))],
        vec![
            egui::Event::PointerButton {
                pos: point,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: default(),
            },
            egui::Event::MouseWheel {
                phase: egui::TouchPhase::Move,
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -120.0),
                modifiers: default(),
            },
        ],
    ] {
        capture.over_hud = false;
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(viewport),
                events,
                ..default()
            },
            |ui| paint_history(ui.ctx(), &mut state, &i18n, &mut capture),
        )
        .drop_without_applying_deltas();
        // Both existing camera and puzzle input systems gate on over_hud.
        assert!(capture.over_hud);
        assert!(ctx.is_pointer_over_egui() || ctx.egui_wants_pointer_input());
    }
}
