//! Bounded, session-local history and presentation. No puzzle/store dependency.
use std::{
    collections::{BTreeMap, VecDeque},
    time::{Duration, Instant},
};

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use chrono::{DateTime, Local};
use jigsall_game::{
    events::{GameEvent, JoinAttemptId},
    resources::GameUiPointerCapture,
};

use crate::{localization::Localization, theme};

const HISTORY_LIMIT: usize = 200;
const TOAST_LIMIT: usize = 3;
const TOAST_DURATION: Duration = Duration::from_secs(4);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Operation {
    Join(JoinAttemptId),
    Autosave,
}

#[derive(Clone)]
struct Entry {
    sequence: u64,
    at: DateTime<Local>,
    event: GameEvent,
}

struct Toast {
    entry: Entry,
    expires: Instant,
}

#[derive(Resource, Default)]
pub(crate) struct Notifications {
    history: VecDeque<Entry>,
    // Progress remains here even when a burst hides it behind newer toasts.
    ongoing: BTreeMap<Operation, Entry>,
    toasts: VecDeque<Toast>,
    sequence: u64,
    pub(crate) opened: bool,
    pub(crate) hud_bottom: f32,
}

impl Notifications {
    fn accept(&mut self, event: GameEvent, now: Instant, at: DateTime<Local>) {
        let started = match &event {
            GameEvent::SessionReset => {
                *self = Self::default();
                return;
            }
            GameEvent::PlayerJoining { connection } => Some(Operation::Join(*connection)),
            GameEvent::AutosaveStarted => Some(Operation::Autosave),
            GameEvent::PlayerJoined {
                connection: Some(connection),
                ..
            }
            | GameEvent::PlayerJoinCancelled { connection, .. } => {
                self.ongoing.remove(&Operation::Join(*connection));
                None
            }
            GameEvent::AutosaveSucceeded
            | GameEvent::AutosaveFailed { .. }
            | GameEvent::AutosaveRotationFailed { .. } => {
                self.ongoing.remove(&Operation::Autosave);
                None
            }
            _ => None,
        };
        if started.is_some_and(|operation| self.ongoing.contains_key(&operation)) {
            return;
        }
        let entry = Entry {
            sequence: self.sequence,
            at,
            event,
        };
        self.sequence = self.sequence.wrapping_add(1);
        if self.history.len() == HISTORY_LIMIT {
            self.history.pop_front();
        }
        self.history.push_back(entry.clone());
        if let Some(operation) = started {
            self.ongoing.insert(operation, entry);
        } else {
            if self.toasts.len() == TOAST_LIMIT {
                self.toasts.pop_front();
            }
            self.toasts.push_back(Toast {
                entry,
                expires: now + TOAST_DURATION,
            });
        }
    }

    fn expire(&mut self, now: Instant) {
        self.toasts.retain(|toast| toast.expires > now);
    }

    fn visible(&self) -> Vec<&Entry> {
        let mut entries: Vec<_> = self
            .ongoing
            .values()
            .chain(self.toasts.iter().map(|toast| &toast.entry))
            .collect();
        entries.sort_unstable_by_key(|entry| std::cmp::Reverse(entry.sequence));
        entries.truncate(TOAST_LIMIT);
        entries.reverse();
        entries
    }
}

pub(crate) fn collect_events(
    mut events: MessageReader<GameEvent>,
    mut state: ResMut<Notifications>,
) {
    let now = Instant::now();
    for event in events.read() {
        state.accept(event.clone(), now, Local::now());
    }
    state.expire(now);
}

pub(crate) fn history_button(ui: &mut egui::Ui, state: &mut Notifications, i18n: &Localization) {
    if ui
        .add(
            egui::Button::new(i18n.text("events-history"))
                .selected(state.opened)
                .sense(egui::Sense::CLICK),
        )
        .clicked()
    {
        state.opened = !state.opened;
    }
}

#[derive(Clone, Copy)]
enum Severity {
    Info,
    Success,
    Warning,
    Error,
}

impl Severity {
    fn for_event(event: &GameEvent) -> Self {
        match event {
            GameEvent::PlayerJoined { .. } | GameEvent::AutosaveSucceeded => Self::Success,
            GameEvent::PlayerJoinCancelled { .. } | GameEvent::AutosaveRotationFailed { .. } => {
                Self::Warning
            }
            GameEvent::AutosaveFailed { .. } => Self::Error,
            _ => Self::Info,
        }
    }
    fn color(self) -> egui::Color32 {
        match self {
            Self::Info => egui::Color32::from_rgb(155, 185, 205),
            Self::Success => theme::ACCENT,
            Self::Warning => egui::Color32::from_rgb(221, 191, 128),
            Self::Error => theme::DANGER,
        }
    }
    fn key(self) -> &'static str {
        match self {
            Self::Info => "events-info",
            Self::Success => "events-success",
            Self::Warning => "events-warning",
            Self::Error => "events-error",
        }
    }
}

fn message(event: &GameEvent, i18n: &Localization) -> String {
    match event {
        GameEvent::PlayerJoined { player, .. } | GameEvent::PlayerLeft { player } => {
            let name = player
                .display_name
                .as_ref()
                .map(|n| n.as_ref().to_owned())
                .unwrap_or_else(|| i18n.text("game-default-player"));
            let key = if matches!(event, GameEvent::PlayerJoined { .. }) {
                "events-player-joined"
            } else {
                "events-player-left"
            };
            i18n.format(key, &[("player", name.as_str().into())])
        }
        GameEvent::PlayerJoining { .. } => i18n.text("events-player-joining"),
        GameEvent::PlayerJoinCancelled { .. } => i18n.text("events-player-join-cancelled"),
        GameEvent::AutosaveStarted => i18n.text("events-autosave-started"),
        GameEvent::AutosaveSucceeded => i18n.text("events-autosave-succeeded"),
        GameEvent::AutosaveFailed { .. } => i18n.text("events-autosave-failed"),
        GameEvent::AutosaveRotationFailed { .. } => i18n.text("events-autosave-rotation-failed"),
        GameEvent::SessionReset => String::new(),
    }
}

fn kind(event: &GameEvent) -> &'static str {
    match event {
        GameEvent::AutosaveStarted
        | GameEvent::AutosaveSucceeded
        | GameEvent::AutosaveFailed { .. }
        | GameEvent::AutosaveRotationFailed { .. } => "events-autosave",
        _ => "events-multiplayer",
    }
}

pub(crate) fn draw_notifications(
    mut contexts: EguiContexts,
    mut state: ResMut<Notifications>,
    i18n: Res<Localization>,
    mut capture: ResMut<GameUiPointerCapture>,
) {
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    if state.opened {
        paint_history(ctx, &mut state, &i18n, &mut capture);
    } else {
        paint_toasts(ctx, &state, &i18n);
    }
}

fn paint_history(
    ctx: &egui::Context,
    state: &mut Notifications,
    i18n: &Localization,
    capture: &mut GameUiPointerCapture,
) {
    let mut opened = state.opened;
    let height = (ctx.content_rect().bottom() - state.hud_bottom - 64.0).clamp(80.0, 400.0);
    let window = egui::Window::new(i18n.text("events-history"))
        .id("event_history".into())
        .open(&mut opened)
        .collapsible(false)
        .default_width(360.0)
        .max_width((ctx.content_rect().width() - 32.0).max(120.0))
        .anchor(
            egui::Align2::RIGHT_TOP,
            egui::vec2(-12.0, state.hud_bottom - ctx.content_rect().top() + 12.0),
        )
        .show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .max_height(height)
                .show(ui, |ui| {
                    if state.history.is_empty() {
                        ui.weak(i18n.text("events-empty"));
                    }
                    for entry in state.history.iter().rev() {
                        let severity = Severity::for_event(&entry.event);
                        ui.horizontal_wrapped(|ui| {
                            ui.weak(entry.at.format("%H:%M:%S").to_string());
                            ui.weak(i18n.text(kind(&entry.event)));
                            ui.colored_label(severity.color(), i18n.text(severity.key()));
                        });
                        let response = ui.label(message(&entry.event, i18n));
                        if let GameEvent::AutosaveFailed { error }
                        | GameEvent::AutosaveRotationFailed { error } = &entry.event
                        {
                            response.on_hover_text(i18n.persistence_error(error));
                        }
                        ui.separator();
                    }
                });
        });
    state.opened = opened;
    // Include the title bar, scrollbar and resize grip, including a closing click.
    if let Some(window) = window {
        capture.over_hud |= ctx
            .pointer_interact_pos()
            .is_some_and(|p| window.response.rect.contains(p));
    }
}

fn paint_toasts(ctx: &egui::Context, state: &Notifications, i18n: &Localization) {
    let viewport = ctx.content_rect();
    let width = 320.0_f32.min((viewport.width() - 32.0).max(80.0));
    let mut top = state.hud_bottom.max(viewport.top()) + 12.0;
    let bottom = (top + 280.0).min(viewport.bottom() - 12.0);
    // A painter creates no Area or widgets, so even the toast's pixels pass input through.
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        "event_toasts".into(),
    ));
    for entry in state.visible() {
        let severity = Severity::for_event(&entry.event);
        let color = severity.color();
        let mut job = egui::text::LayoutJob::simple(
            message(&entry.event, i18n),
            egui::FontId::proportional(14.0),
            theme::TEXT,
            width - 24.0,
        );
        job.wrap.max_rows = 3;
        let body = painter.layout_job(job);
        let heading = painter.layout_no_wrap(
            i18n.text(severity.key()),
            egui::FontId::proportional(11.0),
            color,
        );
        let height = body.size().y + heading.size().y + 24.0;
        if top + height > bottom {
            break;
        }
        let rect = egui::Rect::from_min_size(
            egui::pos2(viewport.right() - width - 12.0, top),
            egui::vec2(width, height),
        );
        painter.rect_filled(rect, 8.0, theme::PANEL);
        painter.line_segment(
            [
                rect.left_top() + egui::vec2(2.0, 8.0),
                rect.left_bottom() + egui::vec2(2.0, -8.0),
            ],
            egui::Stroke::new(3.0, color),
        );
        let text_pos = rect.left_top() + egui::vec2(12.0, 8.0);
        let body_top = text_pos.y + heading.size().y + 4.0;
        painter.galley(text_pos, heading, color);
        painter.galley(egui::pos2(text_pos.x, body_top), body, theme::TEXT);
        top += height + 8.0;
    }
}

#[cfg(test)]
mod tests;
