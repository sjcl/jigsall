//! Pointer gestures consume asynchronous GPU results; gameplay uses PieceCommand.
use crate::play_area::{DragValidation, PivotEnvelope};
use crate::resources::pieces::DragTransform;
use crate::{resources::*, selection::*};
use bevy::prelude::*;
use jigsall_core::protocol::{ComponentRef, PieceTarget};
use jigsall_core::*;
use jigsall_puzzle::placement::LogicalPlayArea;

#[cfg(test)]
mod drag_rotation_tests;
#[cfg(test)]
mod hover_rotation_tests;

#[derive(Default)]
enum Gesture {
    #[default]
    Idle,
    PendingPoint {
        request_id: u64,
        anchor: Vec2,
        screen_anchor: Vec2,
        current: Vec2,
        screen_current: Vec2,
        released: bool,
        original: PieceBitSet,
        ctrl: bool,
    },
    Dragging {
        members: PieceBitSet,
        anchor: Vec2,
    },
    BoxSelecting {
        screen_anchor: Vec2,
        screen_current: Vec2,
        original: PieceBitSet,
        additive: bool,
        request_id: Option<u64>,
        released: bool,
        preview_ticks: u32,
    },
}
#[derive(Resource, Default)]
pub struct PieceInteraction {
    gesture: Gesture,
    pending_rotation: Option<PendingRotation>,
    // Stable only for this gesture; delayed network ACKs cannot affect a new
    // gesture even when it selects the exact same mask.
    network_gesture: std::sync::Arc<()>,
    // The gesture has ended, but its Reliable Release still owns presentation.
    // This gates piece gestures only; camera and UI systems remain independent.
    pending_network_release: bool,
    play_area: Option<LogicalPlayArea>,
    drag_validation: Option<DragValidation>,
}

#[derive(Clone, Copy)]
struct PendingRotation {
    request_id: u64,
    screen_position: Vec2,
    quarter_turns: i8,
}

pub struct PointerFrame {
    pub position: Option<Vec2>,
    pub screen_position: Option<Vec2>,
    pub pressed: bool,
    pub just_pressed: bool,
    pub ctrl: bool,
    pub over_ui: bool,
    pub focused: bool,
}
impl PieceInteraction {
    pub(crate) fn set_play_area(&mut self, definition: Option<&PuzzleDefinition>) {
        self.play_area = definition.and_then(|d| LogicalPlayArea::from_definition(d).ok());
    }
    fn refresh_drag_validation(&mut self, store: &PieceDataStore, members: &PieceBitSet) {
        self.drag_validation = self.validation_for_members(store, members);
    }
    pub(crate) fn validation_for_members(
        &self,
        store: &PieceDataStore,
        members: &PieceBitSet,
    ) -> Option<DragValidation> {
        self.play_area.and_then(|area| {
            PivotEnvelope::from_members(store, members, area)
                .map(|pivots| DragValidation { area, pivots })
        })
    }
    pub(crate) fn clamp_drag_delta(&self, delta: Vec2) -> Vec2 {
        self.drag_validation.map_or_else(
            || if delta.is_finite() { delta } else { Vec2::ZERO },
            |v| v.pivots.clamp(v.area, delta),
        )
    }
    /// Only an explicit idle Q/E press requests a GPU point pick. Pointer frames
    /// never search pieces, build a membership mask, or refresh a hover cache.
    pub fn update_rotation(
        &mut self,
        store: &PieceDataStore,
        selection: &mut PuzzleSelection,
        screen_position: Option<Vec2>,
        quarter_turns: i8,
    ) -> Option<PieceCommand> {
        if self.pending_network_release {
            self.cancel_rotation_pick(selection);
            return None;
        }
        if quarter_turns == 0 && self.pending_rotation.is_none() {
            return None;
        }
        if !matches!(self.gesture, Gesture::Idle)
            || !store.drag.members.is_empty()
            || !store.selected_pieces.is_empty()
        {
            self.cancel_rotation_pick(selection);
            return (quarter_turns != 0)
                .then(|| self.rotation_command(store, quarter_turns))
                .flatten();
        }
        let Some(screen_position) = screen_position.filter(|p| p.is_finite()) else {
            self.cancel_rotation_pick(selection);
            return None;
        };
        if quarter_turns != 0 {
            if let Some(pending) = self.pending_rotation.as_mut().filter(|pending| {
                pending.screen_position == screen_position
                    && selection
                        .latest
                        .is_some_and(|r| r.request_id == pending.request_id)
            }) {
                // Repeated keys at the same point share one in-flight readback.
                pending.quarter_turns =
                    ((pending.quarter_turns as i16 + quarter_turns as i16).rem_euclid(4)) as i8;
                if pending.quarter_turns == 0 {
                    self.cancel_rotation_pick(selection);
                    return None;
                }
            } else {
                self.cancel_rotation_pick(selection);
                let request_id = selection.request(
                    Rect::from_corners(screen_position, screen_position),
                    SelectionMode::Point,
                );
                self.pending_rotation = Some(PendingRotation {
                    request_id,
                    screen_position,
                    quarter_turns,
                });
            }
        }
        let pending = self.pending_rotation?;
        if selection
            .latest
            .is_none_or(|r| r.request_id != pending.request_id)
        {
            self.pending_rotation = None;
            return None;
        }
        let result = selection.take_result(pending.request_id)?;
        self.cancel_rotation_pick(selection);
        if let Some(error) = result.error {
            warn!(%error, "GPU rotation point selection failed");
            return None;
        }
        let SelectionPayload::Point(Some(id)) = result.payload else {
            return None;
        };
        if result.mode != SelectionMode::Point || !store.is_selectable(id) {
            return None;
        }
        // Address the whole component directly, without expanding a selection
        // bitset. The existing Rotate authority validates all members on commit.
        let target =
            PieceTarget::Component(ComponentRef::from_member(&store.connectivity, id).ok()?);
        Some(PieceCommand::Rotate {
            target,
            quarter_turns: pending.quarter_turns,
        })
    }

    pub(crate) fn cancel_rotation_pick(&mut self, selection: &mut PuzzleSelection) {
        if let Some(pending) = self.pending_rotation.take() {
            if selection
                .latest
                .is_some_and(|r| r.request_id == pending.request_id)
            {
                selection.cancel();
            }
        }
    }

    /// Discrete rotation commands never change the pointer basis before acceptance.
    pub fn rotation_command(
        &self,
        store: &PieceDataStore,
        quarter_turns: i8,
    ) -> Option<PieceCommand> {
        if self.pending_network_release {
            return None;
        }
        if let Gesture::Dragging { members, .. } = &self.gesture {
            return Some(PieceCommand::RotateDrag {
                members: members.clone(),
                delta: store.drag.delta,
                quarter_turns,
            });
        }
        if !matches!(self.gesture, Gesture::Idle)
            || !store.drag.members.is_empty()
            || store.selected_pieces.is_empty()
        {
            return None;
        }
        let target = jigsall_core::protocol::PieceTarget::from_selection(
            &store.connectivity,
            &store.selected_pieces,
        )
        .ok()?;
        Some(PieceCommand::Rotate {
            target,
            quarter_turns,
        })
    }
    pub(crate) fn accept_drag_rotation(
        &mut self,
        command: &PieceCommand,
        pointer: Vec2,
        store: &PieceDataStore,
    ) {
        if let (
            Gesture::Dragging { members, anchor },
            PieceCommand::RotateDrag {
                members: accepted, ..
            },
        ) = (&mut self.gesture, command)
        {
            if pointer.is_finite() && members == accepted {
                *anchor = pointer;
            }
        }
        if let PieceCommand::RotateDrag { members, .. } = command {
            self.refresh_drag_validation(store, members);
        }
    }
    pub fn is_dragging(&self) -> bool {
        matches!(self.gesture, Gesture::Dragging { .. })
    }
    pub(crate) fn hold_network_release(&mut self, token: &std::sync::Arc<()>) -> bool {
        if std::sync::Arc::ptr_eq(token, &self.network_gesture) {
            self.pending_network_release = true;
            return true;
        }
        false
    }
    pub(crate) fn clear_network_release(
        &mut self,
        token: &std::sync::Arc<()>,
        store: &mut PieceDataStore,
    ) {
        if std::sync::Arc::ptr_eq(token, &self.network_gesture) {
            self.pending_network_release = false;
            store.drag = default();
        }
    }
    /// Runtime ACK reconciliation at a control boundary. A released/replaced
    /// gesture must not be resurrected by a delayed authority result.
    pub(crate) fn reconcile_network_grab(
        &mut self,
        token: &std::sync::Arc<()>,
        requested: &PieceBitSet,
        accepted: PieceBitSet,
        store: &mut PieceDataStore,
    ) {
        if !std::sync::Arc::ptr_eq(token, &self.network_gesture) {
            return;
        }
        self.refresh_drag_validation(store, &accepted);
        store.drag.delta = self.clamp_drag_delta(store.drag.delta);
        if self.pending_network_release {
            store.selected_pieces = accepted.clone();
            store.highlights_dirty = true;
            if accepted.is_empty() {
                self.pending_network_release = false;
                store.drag = default();
            }
        }
        if let Gesture::Dragging { members, .. } = &mut self.gesture {
            if members == requested {
                if accepted.is_empty() {
                    self.gesture = Gesture::Idle;
                    store.drag = default();
                    store.selected_pieces.clear();
                    store.highlights_dirty = true;
                } else {
                    store.drag.members = accepted.words().clone();
                    store.selected_pieces = accepted.clone();
                    store.highlights_dirty = true;
                    *members = accepted;
                }
            }
        }
    }
    /// Rebase only after an accepted authority commit, at the submitted pointer.
    /// Movement made while waiting for the ACK remains a presentation residual.
    pub(crate) fn rebase_network_drag(
        &mut self,
        token: &std::sync::Arc<()>,
        members: &PieceBitSet,
        pointer: Option<Vec2>,
        delta: Vec2,
        store: &mut PieceDataStore,
    ) {
        if !std::sync::Arc::ptr_eq(token, &self.network_gesture) {
            return;
        }
        self.refresh_drag_validation(store, members);
        let residual = self.clamp_drag_delta(store.drag.delta - delta);
        if let Gesture::Dragging {
            members: current,
            anchor,
        } = &mut self.gesture
        {
            if current == members {
                *anchor = pointer.filter(|p| p.is_finite()).unwrap_or(*anchor + delta);
                store.drag.delta = residual;
            }
        }
    }
    pub(crate) fn network_gesture_token(&self) -> std::sync::Arc<()> {
        self.network_gesture.clone()
    }

    pub fn screen_selection_rect(&self) -> Option<Rect> {
        if let Gesture::BoxSelecting {
            screen_anchor,
            screen_current,
            ..
        } = self.gesture
        {
            Some(Rect {
                min: screen_anchor.min(screen_current),
                max: screen_anchor.max(screen_current),
            })
        } else {
            None
        }
    }
    pub fn update(
        &mut self,
        frame: PointerFrame,
        store: &mut PieceDataStore,
        selection: &mut PuzzleSelection,
        local_player: PlayerId,
    ) -> Vec<PieceCommand> {
        if self.pending_network_release {
            return Vec::new();
        }
        if !frame.focused {
            return self.cancel(store, selection, local_player);
        }
        if frame.just_pressed {
            // A mouse gesture owns the same request channel from this point on.
            self.cancel_rotation_pick(selection);
        }
        let point = frame.position.filter(|p| p.is_finite());
        let screen = frame.screen_position.filter(|p| p.is_finite());
        let mut commands = Vec::new();
        // A new press supersedes a released gesture even if its readback is late.
        let finished = matches!(
            self.gesture,
            Gesture::PendingPoint { released: true, .. }
                | Gesture::BoxSelecting { released: true, .. }
        );
        if frame.just_pressed && finished {
            self.cancel(store, selection, local_player);
        }
        if frame.just_pressed && matches!(self.gesture, Gesture::Idle) && !frame.over_ui {
            if let (Some(point), Some(screen)) = (point, screen) {
                let request_id = selection.request(
                    Rect {
                        min: screen,
                        max: screen,
                    },
                    SelectionMode::Point,
                );
                self.gesture = Gesture::PendingPoint {
                    request_id,
                    anchor: point,
                    screen_anchor: screen,
                    current: point,
                    screen_current: screen,
                    released: false,
                    original: store.selected_pieces.clone(),
                    ctrl: frame.ctrl,
                };
            }
        }
        if let Gesture::PendingPoint {
            request_id,
            anchor,
            screen_anchor,
            current,
            screen_current,
            released,
            original,
            ctrl,
        } = &mut self.gesture
        {
            if !*released {
                if !frame.pressed && (point.is_none() || screen.is_none()) {
                    return self.cancel(store, selection, local_player);
                }
                if let Some(point) = point {
                    *current = point;
                }
                if let Some(screen) = screen {
                    *screen_current = screen;
                }
                *released = !frame.pressed;
            }
            if let Some(result) = selection.take_result(*request_id) {
                store.highlights_dirty = true;
                if let Some(error) = result.error {
                    warn!(%error, "GPU point selection failed");
                    return self.cancel(store, selection, local_player);
                }
                debug_assert_eq!(result.mode, SelectionMode::Point);
                let hit = match result.payload {
                    SelectionPayload::Point(id) => {
                        id.filter(|&id| store.component_is_selectable(id))
                    }
                    SelectionPayload::Rectangle(_) => {
                        return self.cancel(store, selection, local_player)
                    }
                };
                if let Some(id) = hit {
                    if *ctrl {
                        store.select_component(id, true);
                        self.gesture = Gesture::Idle;
                        selection.cancel();
                    } else {
                        if !store.selected_pieces.contains(&id) {
                            store.selected_pieces.clear();
                            store.select_component(id, false);
                        }
                        let members = store.selectable_members(&store.selected_pieces);
                        store.selected_pieces = members.clone();
                        let validation = self.play_area.and_then(|area| {
                            PivotEnvelope::from_members(store, &members, area)
                                .map(|pivots| DragValidation { area, pivots })
                        });
                        let delta = *current - *anchor;
                        let delta = validation.map_or(delta, |v| v.pivots.clamp(v.area, delta));
                        self.drag_validation = validation;
                        store.drag = DragTransform {
                            members: members.words().clone(),
                            delta: if delta.is_finite() { delta } else { Vec2::ZERO },
                        };
                        commands.push(PieceCommand::GrabGroup {
                            members: members.clone(),
                        });
                        self.network_gesture = std::sync::Arc::new(());
                        if *released {
                            finish_drag(members, store, &mut commands);
                            self.gesture = Gesture::Idle;
                        } else {
                            self.gesture = Gesture::Dragging {
                                members,
                                anchor: *anchor,
                            };
                        }
                        selection.cancel();
                    }
                } else {
                    if !*ctrl {
                        store.selected_pieces.clear();
                    }
                    self.gesture = Gesture::BoxSelecting {
                        screen_anchor: *screen_anchor,
                        screen_current: *screen_current,
                        original: original.clone(),
                        additive: *ctrl,
                        request_id: None,
                        released: *released,
                        preview_ticks: 0,
                    };
                }
            }
        }
        let validation = self.drag_validation;
        match &mut self.gesture {
            Gesture::Dragging { members, anchor } => {
                if let Some(point) = point {
                    let delta = point - *anchor;
                    if delta.is_finite() {
                        store.drag.delta =
                            validation.map_or(delta, |v| v.pivots.clamp(v.area, delta));
                    }
                }
                if !frame.pressed {
                    finish_drag(std::mem::take(members), store, &mut commands);
                    self.gesture = Gesture::Idle;
                }
            }
            Gesture::BoxSelecting {
                screen_anchor,
                screen_current,
                original,
                additive,
                request_id,
                released,
                preview_ticks,
                ..
            } => {
                let was_released = *released;
                if !*released {
                    if let Some(screen) = screen {
                        *screen_current = screen;
                    }
                    if !frame.pressed {
                        if point.is_none() || screen.is_none() {
                            return self.cancel(store, selection, local_player);
                        }
                        *released = true;
                    }
                }
                // Always request the release rectangle, even if a preview is in flight.
                if request_id.is_none()
                    || (!was_released && *released)
                    || (!*released && *preview_ticks >= 4)
                {
                    let rect = Rect {
                        min: *screen_anchor,
                        max: *screen_current,
                    };
                    *request_id = Some(if *released {
                        selection.request(rect, SelectionMode::Rectangle)
                    } else {
                        selection.request_preview(rect)
                    });
                    *preview_ticks = 0;
                }
                *preview_ticks = preview_ticks.saturating_add(1);
                if let Some(result) = selection.take_result(request_id.unwrap()) {
                    if let Some(error) = result.error {
                        warn!(%error, "GPU rectangle selection failed");
                        return self.cancel(store, selection, local_player);
                    }
                    debug_assert_eq!(result.mode, SelectionMode::Rectangle);
                    if *released {
                        store.highlights_dirty = true;
                        let SelectionPayload::Rectangle(members) = result.payload else {
                            return self.cancel(store, selection, local_player);
                        };
                        store.commit_selection(
                            members,
                            additive.then_some(&*original),
                            local_player,
                        );
                        self.gesture = Gesture::Idle;
                        selection.cancel();
                    }
                }
            }
            Gesture::Idle | Gesture::PendingPoint { .. } => {}
        }
        commands
    }
    pub fn cancel(
        &mut self,
        store: &mut PieceDataStore,
        selection: &mut PuzzleSelection,
        local_player: PlayerId,
    ) -> Vec<PieceCommand> {
        self.cancel_rotation_pick(selection);
        if self.pending_network_release {
            // Release was already submitted, including pause/focus loss paths.
            return Vec::new();
        }
        // Repeated unfocused idle frames must not allocate or scan an owner mask.
        if matches!(self.gesture, Gesture::Idle) && !store.held_by.has_player(local_player) {
            selection.cancel();
            store.drag = default();
            return Vec::new();
        }
        store.highlights_dirty = true;
        let gesture = std::mem::take(&mut self.gesture);
        selection.cancel();
        let mut commands = Vec::new();
        let mut held = PieceBitSet::new(store.len());
        held.extend(
            store
                .held_by
                .iter()
                .filter_map(|(id, &player)| (player == local_player).then_some(id)),
        );
        match gesture {
            Gesture::Dragging { members, .. } => {
                // Pause/focus loss commits the last displayed location once.
                held.difference(&members);
                finish_drag(members, store, &mut commands);
            }
            Gesture::BoxSelecting { original, .. } | Gesture::PendingPoint { original, .. } => {
                store.restore_selection(original, local_player)
            }
            Gesture::Idle => {}
        }
        store.drag = default();
        if !held.is_empty() {
            commands.push(PieceCommand::ReleaseGroup {
                members: held,
                delta: Vec2::ZERO,
            });
        }
        commands
    }
}

fn finish_drag(members: PieceBitSet, store: &mut PieceDataStore, commands: &mut Vec<PieceCommand>) {
    commands.push(PieceCommand::ReleaseGroup {
        members,
        delta: store.drag.delta,
    });
    store.drag = default();
}

#[cfg(test)]
#[path = "interaction_bench.rs"]
mod benchmarks;
#[cfg(test)]
#[path = "connected_interaction_tests.rs"]
mod connected_tests;
#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::Arc, time::Instant};

    fn frame(position: Vec2, pressed: bool, just_pressed: bool) -> PointerFrame {
        PointerFrame {
            position: Some(position),
            screen_position: Some(position),
            pressed,
            just_pressed,
            ctrl: false,
            over_ui: false,
            focused: true,
        }
    }
    fn apply(store: &mut PieceDataStore, commands: Vec<PieceCommand>) {
        for command in commands {
            store.apply_command(LOCAL_PLAYER, &command, None, jigsall_core::LOCAL_PLAYER);
        }
    }

    #[test]
    fn local_selection_rollback_does_not_restore_other_players_or_placed_pieces() {
        for box_selecting in [false, true] {
            let mut store = PieceDataStore::default();
            store.initialize(vec![Vec2::ZERO; 3]);
            store.selected_pieces.fill();
            let mut interaction = PieceInteraction::default();
            let mut selection = PuzzleSelection::default();
            let mut down = frame(Vec2::ZERO, true, true);
            down.ctrl = true;
            interaction.update(down, &mut store, &mut selection, jigsall_core::LOCAL_PLAYER);
            if box_selecting {
                let request = selection.latest.unwrap();
                selection.completed = Some(SelectionResult {
                    request_id: request.request_id,
                    mode: SelectionMode::Point,
                    payload: SelectionPayload::Point(None),
                    error: None,
                });
                interaction.update(
                    frame(Vec2::splat(20.0), true, false),
                    &mut store,
                    &mut selection,
                    jigsall_core::LOCAL_PLAYER,
                );
                assert!(interaction.screen_selection_rect().is_some());
            }
            store.apply_command(
                LOCAL_PLAYER,
                &PieceCommand::Grab(PieceId(0)),
                None,
                jigsall_core::LOCAL_PLAYER,
            );
            store.apply_command(
                PlayerId(1),
                &PieceCommand::Grab(PieceId(1)),
                None,
                jigsall_core::LOCAL_PLAYER,
            );
            let mut placed = store.state(PieceId(2)).unwrap();
            placed.placed = true;
            store.set_state(PieceId(2), placed, jigsall_core::LOCAL_PLAYER);
            let commands =
                interaction.cancel(&mut store, &mut selection, jigsall_core::LOCAL_PLAYER);
            assert_eq!(
                store.selected_pieces.iter().collect::<Vec<_>>(),
                [PieceId(0)]
            );
            assert!(selection.latest.is_none());
            apply(&mut store, commands);
            assert_eq!(
                store.selected_pieces.iter().collect::<Vec<_>>(),
                [PieceId(0)]
            );
        }
    }

    fn begin(count: usize) -> (PieceInteraction, PieceDataStore, PuzzleSelection) {
        let mut store = PieceDataStore::default();
        store.initialize((0..count).map(|id| Vec2::new(id as f32, 0.0)).collect());
        store.selected_pieces = (0..count).map(|id| PieceId(id as u32)).collect();
        let mut selection = PuzzleSelection::default();
        let mut interaction = PieceInteraction::default();
        interaction.update(
            frame(Vec2::ZERO, true, true),
            &mut store,
            &mut selection,
            jigsall_core::LOCAL_PLAYER,
        );
        let request = selection.latest.unwrap();
        selection.completed = Some(SelectionResult {
            request_id: request.request_id,
            mode: request.mode,
            payload: SelectionPayload::from_ids(request.mode, count, vec![PieceId(0)]),
            error: None,
        });
        let commands = interaction.update(
            frame(Vec2::ZERO, true, false),
            &mut store,
            &mut selection,
            jigsall_core::LOCAL_PLAYER,
        );
        assert_eq!(commands.len(), 1);
        assert!(commands
            .iter()
            .all(|c| matches!(c, PieceCommand::GrabGroup { .. })));
        apply(&mut store, commands);
        store.sync_highlights();
        store.dirty_pieces.clear();
        (interaction, store, selection)
    }
    #[test]
    fn million_piece_drag_only_changes_delta_until_release() {
        let (mut interaction, mut store, mut selection) = begin(1_000_000);
        let members = store.drag.members.clone();
        assert_eq!(members.len() * 4, 125_000);
        for step in 1..=100 {
            assert!(interaction
                .update(
                    frame(Vec2::splat(step as f32), true, false),
                    &mut store,
                    &mut selection,
                    jigsall_core::LOCAL_PLAYER
                )
                .is_empty());
            assert!(Arc::ptr_eq(&members, &store.drag.members));
            assert!(store.dirty_pieces.is_empty());
        }
        assert_eq!(store.states[999_999].position, Vec2::new(999_999.0, 0.0));
        let commands = interaction.update(
            frame(Vec2::new(150.0, 200.0), false, false),
            &mut store,
            &mut selection,
            jigsall_core::LOCAL_PLAYER,
        );
        assert_eq!(commands.len(), 1);
        assert!(store.drag.members.is_empty());
        apply(&mut store, commands);
        assert_eq!(
            store.states[999_999].position,
            Vec2::new(1_000_149.0, 200.0)
        );
        assert!(store.held_by.is_empty());
        assert!(interaction
            .update(
                frame(Vec2::splat(500.0), false, false),
                &mut store,
                &mut selection,
                jigsall_core::LOCAL_PLAYER
            )
            .is_empty());
    }
    #[test]
    fn cancellation_commits_last_valid_delta_once_and_releases_other_local_holds() {
        for focus_loss in [false, true] {
            let (mut interaction, mut store, mut selection) = begin(33);
            let mut states = store.states.to_vec();
            states.push(GpuPieceState::new(Vec2::ZERO, PieceId(33)));
            store.states = states.into();
            store.connectivity = PieceConnectivity::new(34);
            store.dirty_pieces = PieceBitSet::new(34);
            if let Gesture::Dragging { members, .. } = &mut interaction.gesture {
                let mut resized = PieceBitSet::new(34);
                resized.union(members);
                *members = resized;
                store.drag.members = members.words().clone();
            }
            let mut other = store.state(PieceId(33)).unwrap();
            other.held_by = Some(LOCAL_PLAYER);
            store.set_state(PieceId(33), other, jigsall_core::LOCAL_PLAYER);
            interaction.update(
                frame(Vec2::new(10.0, 20.0), true, false),
                &mut store,
                &mut selection,
                jigsall_core::LOCAL_PLAYER,
            );
            interaction.update(
                frame(Vec2::splat(f32::NAN), true, false),
                &mut store,
                &mut selection,
                jigsall_core::LOCAL_PLAYER,
            );
            let commands = if focus_loss {
                let mut lost = frame(Vec2::splat(500.0), false, false);
                lost.focused = false;
                interaction.update(lost, &mut store, &mut selection, jigsall_core::LOCAL_PLAYER)
            } else {
                interaction.cancel(&mut store, &mut selection, jigsall_core::LOCAL_PLAYER)
            };
            assert_eq!(commands.len(), 2);
            apply(&mut store, commands);
            assert_eq!(store.states[32].position, Vec2::new(42.0, 20.0));
            assert_eq!(store.states[33].position, Vec2::ZERO);
            assert!(store.held_by.is_empty());
            assert!(interaction
                .cancel(&mut store, &mut selection, jigsall_core::LOCAL_PLAYER)
                .is_empty());
        }
    }
    #[test]
    #[ignore = "release CPU benchmark"]
    fn multi_drag_cpu_benchmark() {
        if cfg!(debug_assertions) {
            panic!("run with --release");
        }
        crate::test_logging::init();
        for count in [1_000, 10_000, 100_000, 1_000_000] {
            let (mut interaction, mut store, mut selection) = begin(count);
            let start = Instant::now();
            for step in 0..100_000 {
                let commands = interaction.update(
                    frame(Vec2::splat(step as f32), true, false),
                    &mut store,
                    &mut selection,
                    jigsall_core::LOCAL_PLAYER,
                );
                assert!(std::hint::black_box(commands).is_empty());
                std::hint::black_box(&store.drag);
            }
            let elapsed = start.elapsed();
            bevy::log::info!(
                selected = count,
                pointer_frames = 100_000,
                total_us = elapsed.as_secs_f64() * 1e6,
                ns_per_frame = elapsed.as_secs_f64() * 1e9 / 100_000.0,
                "Multi-drag CPU benchmark"
            );
        }
    }
}

#[cfg(test)]
mod local_identity_tests {
    use super::*;

    #[test]
    fn cancel_and_focus_loss_release_only_nonzero_local_holds() {
        let local = PlayerId(42);
        let remote = PlayerId(0);
        for focus_loss in [false, true] {
            let mut store = PieceDataStore::default();
            store.initialize(vec![Vec2::ZERO; 2]);
            store.apply_command(local, &PieceCommand::Grab(PieceId(0)), None, local);
            store.apply_command(remote, &PieceCommand::Grab(PieceId(1)), None, local);
            let mut interaction = PieceInteraction::default();
            let mut selection = PuzzleSelection::default();
            let commands = if focus_loss {
                interaction.update(
                    PointerFrame {
                        position: None,
                        screen_position: None,
                        pressed: false,
                        just_pressed: false,
                        ctrl: false,
                        over_ui: false,
                        focused: false,
                    },
                    &mut store,
                    &mut selection,
                    local,
                )
            } else {
                interaction.cancel(&mut store, &mut selection, local)
            };
            assert_eq!(commands.len(), 1);
            let PieceCommand::ReleaseGroup { members, delta } = &commands[0] else {
                panic!()
            };
            assert_eq!(members.iter().collect::<Vec<_>>(), vec![PieceId(0)]);
            assert_eq!(*delta, Vec2::ZERO);
            store.apply_command(local, &commands[0], None, local);
            assert_eq!(store.held_by.get(&PieceId(0)), None);
            assert_eq!(store.held_by.get(&PieceId(1)), Some(&remote));
            assert!(interaction
                .cancel(&mut store, &mut selection, local)
                .is_empty());
        }
    }

    #[test]
    fn pending_gesture_cancel_restores_only_current_local_selection() {
        let local = PlayerId(42);
        let remote = PlayerId(0);
        let mut store = PieceDataStore::default();
        store.initialize(vec![Vec2::ZERO; 2]);
        let mut original = PieceBitSet::new(2);
        original.fill();
        store.selected_pieces = original;
        let mut interaction = PieceInteraction::default();
        let mut selection = PuzzleSelection::default();
        interaction.update(
            PointerFrame {
                position: Some(Vec2::ZERO),
                screen_position: Some(Vec2::ZERO),
                pressed: true,
                just_pressed: true,
                ctrl: false,
                over_ui: false,
                focused: true,
            },
            &mut store,
            &mut selection,
            local,
        );
        store.apply_command(local, &PieceCommand::Grab(PieceId(0)), None, local);
        store.apply_command(remote, &PieceCommand::Grab(PieceId(1)), None, local);
        interaction.cancel(&mut store, &mut selection, local);
        assert_eq!(
            store.selected_pieces.iter().collect::<Vec<_>>(),
            vec![PieceId(0)]
        );
    }
}
