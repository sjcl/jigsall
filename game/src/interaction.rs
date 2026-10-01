//! Pointer gestures consume asynchronous GPU results; gameplay uses PieceCommand.
use crate::resources::pieces::DragTransform;
use crate::{resources::*, selection::*};
use bevy::prelude::*;
use puzzella_core::*;
use std::collections::HashSet;

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
        original: HashSet<PieceId>,
        ctrl: bool,
    },
    Dragging {
        ids: Vec<PieceId>,
        anchor: Vec2,
    },
    BoxSelecting {
        #[allow(dead_code)] // World-space gesture anchor is also used by CPU gesture tests.
        anchor: Vec2,
        current: Vec2,
        screen_anchor: Vec2,
        screen_current: Vec2,
        original: HashSet<PieceId>,
        additive: bool,
        request_id: Option<u64>,
        released: bool,
        preview_ticks: u32,
    },
}
#[derive(Resource, Default)]
pub struct PieceInteraction {
    gesture: Gesture,
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
    pub fn is_dragging(&self) -> bool {
        matches!(self.gesture, Gesture::Dragging { .. })
    }
    #[cfg(test)]
    pub fn selection_rect(&self) -> Option<Rect> {
        if let Gesture::BoxSelecting {
            anchor, current, ..
        } = self.gesture
        {
            Some(Rect {
                min: anchor.min(current),
                max: anchor.max(current),
            })
        } else {
            None
        }
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
    ) -> Vec<PieceCommand> {
        if !frame.focused {
            return self.cancel(store, selection);
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
            self.cancel(store, selection);
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
                    return self.cancel(store, selection);
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
                    return self.cancel(store, selection);
                }
                debug_assert_eq!(result.mode, SelectionMode::Point);
                let hit = result
                    .piece_ids
                    .first()
                    .copied()
                    .filter(|&id| store.is_selectable(id));
                if let Some(id) = hit {
                    if *ctrl {
                        if !store.selected_pieces.remove(&id) {
                            store.selected_pieces.insert(id);
                        }
                        self.gesture = Gesture::Idle;
                        selection.cancel();
                    } else {
                        if !store.selected_pieces.contains(&id) {
                            store.selected_pieces.clear();
                            store.selected_pieces.insert(id);
                        }
                        let mut ids: Vec<_> = store
                            .selected_pieces
                            .iter()
                            .copied()
                            .filter(|&id| store.is_selectable(id))
                            .collect();
                        ids.sort_by(|a, b| {
                            store.states[a.0 as usize]
                                .z_order
                                .cmp(&store.states[b.0 as usize].z_order)
                                .then(a.cmp(b))
                        });
                        let mut members = vec![0u32; store.len().div_ceil(32)];
                        for &id in &ids {
                            members[id.0 as usize / 32] |= 1 << (id.0 % 32);
                        }
                        let delta = *current - *anchor;
                        store.drag = DragTransform {
                            members: members.into(),
                            delta: if delta.is_finite() { delta } else { Vec2::ZERO },
                        };
                        commands.extend(ids.iter().copied().map(PieceCommand::Grab));
                        if *released {
                            finish_drag(&ids, store, &mut commands);
                            self.gesture = Gesture::Idle;
                        } else {
                            self.gesture = Gesture::Dragging {
                                ids,
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
                        anchor: *anchor,
                        current: *current,
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
        match &mut self.gesture {
            Gesture::Dragging { ids, anchor } => {
                if let Some(point) = point {
                    let delta = point - *anchor;
                    if delta.is_finite() {
                        store.drag.delta = delta;
                    }
                }
                if !frame.pressed {
                    finish_drag(ids, store, &mut commands);
                    self.gesture = Gesture::Idle;
                }
            }
            Gesture::BoxSelecting {
                current,
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
                    if let Some(point) = point {
                        *current = point;
                    }
                    if let Some(screen) = screen {
                        *screen_current = screen;
                    }
                    if !frame.pressed {
                        if point.is_none() || screen.is_none() {
                            return self.cancel(store, selection);
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
                        return self.cancel(store, selection);
                    }
                    debug_assert_eq!(result.mode, SelectionMode::Rectangle);
                    if *released {
                        store.highlights_dirty = true;
                        store.selected_pieces = if *additive {
                            original.clone()
                        } else {
                            HashSet::new()
                        };
                        let ids = result
                            .piece_ids
                            .into_iter()
                            .filter(|&id| store.is_selectable(id))
                            .collect::<Vec<_>>();
                        store.selected_pieces.extend(ids);
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
    ) -> Vec<PieceCommand> {
        store.highlights_dirty = true;
        let gesture = std::mem::take(&mut self.gesture);
        selection.cancel();
        let mut commands = Vec::new();
        let mut held: Vec<_> = store
            .held_by
            .iter()
            .filter_map(|(&id, &player)| (player == LOCAL_PLAYER).then_some(id))
            .collect();
        match gesture {
            Gesture::Dragging { ids, .. } => {
                // Pause/focus loss commits the last displayed location before snap.
                let members = store.drag.members.clone();
                finish_drag(&ids, store, &mut commands);
                held.retain(|id| {
                    members
                        .get(id.0 as usize / 32)
                        .is_none_or(|word| word & (1 << (id.0 % 32)) == 0)
                });
            }
            Gesture::BoxSelecting { original, .. } => store.selected_pieces = original,
            Gesture::PendingPoint { original, .. } => store.selected_pieces = original,
            Gesture::Idle => {}
        }
        held.sort_unstable();
        held.dedup();
        store.drag = default();
        commands.extend(held.into_iter().map(PieceCommand::Release));
        commands
    }
}

fn finish_drag(ids: &[PieceId], store: &mut PieceDataStore, commands: &mut Vec<PieceCommand>) {
    let delta = store.drag.delta;
    commands.reserve(ids.len() * 2);
    commands.extend(ids.iter().copied().map(|id| PieceCommand::Move {
        id,
        position: store.states[id.0 as usize].position + delta,
    }));
    commands.extend(ids.iter().copied().map(PieceCommand::Release));
    store.drag = default();
}

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
            let id = command.piece_id();
            let mut state = store.state(id).unwrap();
            if let Some(outcome) = apply_piece_command(&mut state, LOCAL_PLAYER, &command) {
                store.set_state(id, state);
                if outcome == CommandOutcome::Grabbed {
                    store.bring_piece_to_front(id);
                }
            }
        }
    }
    fn begin(count: usize) -> (PieceInteraction, PieceDataStore, PuzzleSelection) {
        let mut store = PieceDataStore::default();
        store.initialize((0..count).map(|id| Vec2::new(id as f32, 0.0)).collect());
        store.selected_pieces = (0..count).map(|id| PieceId(id as u32)).collect();
        let mut selection = PuzzleSelection::default();
        let mut interaction = PieceInteraction::default();
        interaction.update(frame(Vec2::ZERO, true, true), &mut store, &mut selection);
        let request = selection.latest.unwrap();
        selection.completed = Some(SelectionResult {
            request_id: request.request_id,
            mode: request.mode,
            piece_ids: vec![PieceId(0)],
            entities: vec![],
            error: None,
        });
        let commands =
            interaction.update(frame(Vec2::ZERO, true, false), &mut store, &mut selection);
        assert_eq!(commands.len(), count);
        assert!(commands.iter().all(|c| matches!(c, PieceCommand::Grab(_))));
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
                    &mut selection
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
        );
        assert_eq!(commands.len(), 2_000_000);
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
                &mut selection
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
            let mut other = store.state(PieceId(33)).unwrap();
            other.held_by = Some(LOCAL_PLAYER);
            store.set_state(PieceId(33), other);
            interaction.update(
                frame(Vec2::new(10.0, 20.0), true, false),
                &mut store,
                &mut selection,
            );
            interaction.update(
                frame(Vec2::splat(f32::NAN), true, false),
                &mut store,
                &mut selection,
            );
            let commands = if focus_loss {
                let mut lost = frame(Vec2::splat(500.0), false, false);
                lost.focused = false;
                interaction.update(lost, &mut store, &mut selection)
            } else {
                interaction.cancel(&mut store, &mut selection)
            };
            assert_eq!(commands.len(), 67);
            apply(&mut store, commands);
            assert_eq!(store.states[32].position, Vec2::new(42.0, 20.0));
            assert_eq!(store.states[33].position, Vec2::ZERO);
            assert!(store.held_by.is_empty());
            assert!(interaction.cancel(&mut store, &mut selection).is_empty());
        }
    }
    #[test]
    #[ignore = "release CPU benchmark"]
    fn multi_drag_cpu_benchmark() {
        if cfg!(debug_assertions) {
            panic!("run with --release");
        }
        println!("selected,pointer_frames,total_us,ns_per_frame");
        for count in [1_000, 10_000, 100_000, 1_000_000] {
            let (mut interaction, mut store, mut selection) = begin(count);
            let start = Instant::now();
            for step in 0..100_000 {
                let commands = interaction.update(
                    frame(Vec2::splat(step as f32), true, false),
                    &mut store,
                    &mut selection,
                );
                assert!(std::hint::black_box(commands).is_empty());
                std::hint::black_box(&store.drag);
            }
            let elapsed = start.elapsed();
            println!(
                "{count},100000,{:.3},{:.3}",
                elapsed.as_secs_f64() * 1e6,
                elapsed.as_secs_f64() * 1e9 / 100_000.0
            );
        }
    }
}
