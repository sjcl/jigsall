//! Pointer gestures consume asynchronous GPU results; gameplay uses PieceCommand.
use crate::{resources::*, selection::*};
use bevy::prelude::*;
use puzzella_core::*;
use std::collections::{BTreeMap, HashSet};

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
        offsets: BTreeMap<PieceId, Vec2>,
    },
    BoxSelecting {
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
fn selectable(store: &PieceDataStore, id: PieceId) -> bool {
    store
        .pieces
        .get(&id)
        .is_some_and(|piece| !piece.state.placed && piece.state.held_by.is_none())
}
impl PieceInteraction {
    pub fn is_dragging(&self) -> bool {
        matches!(self.gesture, Gesture::Dragging { .. })
    }
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
        store.selected_pieces.retain(|id| {
            store.pieces.get(id).is_some_and(|piece| {
                !piece.state.placed
                    && (piece.state.held_by.is_none() || piece.state.held_by == Some(LOCAL_PLAYER))
            })
        });
        if !frame.focused {
            return self.cancel(store, selection);
        }
        let point = frame.position.filter(|p| p.is_finite());
        let screen = frame.screen_position.filter(|p| p.is_finite());
        let mut commands = Vec::new();
        let mut started_drag = false;
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
                if let Some(error) = result.error {
                    warn!(%error, "GPU point selection failed");
                    return self.cancel(store, selection);
                }
                debug_assert_eq!(result.mode, SelectionMode::Point);
                let hit = result
                    .piece_ids
                    .first()
                    .copied()
                    .filter(|&id| selectable(store, id));
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
                            .filter(|&id| selectable(store, id))
                            .collect();
                        ids.sort_by(|a, b| {
                            store.transforms[a]
                                .translation
                                .z
                                .total_cmp(&store.transforms[b].translation.z)
                                .then(a.cmp(b))
                        });
                        let offsets: BTreeMap<_, _> = ids
                            .iter()
                            .map(|&id| (id, store.pieces[&id].state.position - *anchor))
                            .collect();
                        commands.extend(ids.into_iter().map(PieceCommand::Grab));
                        // Retain press-to-release motion while the GPU was working.
                        commands.extend(offsets.iter().map(|(&id, &offset)| PieceCommand::Move {
                            id,
                            position: *current + offset,
                        }));
                        if *released {
                            commands.extend(offsets.keys().copied().map(PieceCommand::Release));
                            self.gesture = Gesture::Idle;
                        } else {
                            self.gesture = Gesture::Dragging { offsets };
                            started_drag = true;
                        }
                        selection.cancel();
                    }
                } else {
                    if !*ctrl {
                        store.selected_pieces.clear();
                    }
                    store.preview_pieces.clear();
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
            Gesture::Dragging { offsets } => {
                if let Some(point) = point.filter(|_| !started_drag) {
                    commands.extend(offsets.iter().map(|(&id, &offset)| PieceCommand::Move {
                        id,
                        position: point + offset,
                    }));
                }
                if !frame.pressed {
                    commands.extend(offsets.keys().copied().map(PieceCommand::Release));
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
                    *request_id = Some(selection.request(
                        Rect {
                            min: *screen_anchor,
                            max: *screen_current,
                        },
                        SelectionMode::Rectangle,
                    ));
                    *preview_ticks = 0;
                }
                *preview_ticks = preview_ticks.saturating_add(1);
                if let Some(result) = selection.take_result(request_id.unwrap()) {
                    if let Some(error) = result.error {
                        warn!(%error, "GPU rectangle selection failed");
                        return self.cancel(store, selection);
                    }
                    debug_assert_eq!(result.mode, SelectionMode::Rectangle);
                    store.preview_pieces = result
                        .piece_ids
                        .into_iter()
                        .filter(|&id| selectable(store, id))
                        .collect();
                    if *released {
                        store.selected_pieces = if *additive {
                            original.clone()
                        } else {
                            HashSet::new()
                        };
                        store.selected_pieces.extend(store.preview_pieces.drain());
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
        let gesture = std::mem::take(&mut self.gesture);
        selection.cancel();
        let mut held: Vec<_> = store
            .pieces
            .iter()
            .filter_map(|(&id, p)| (p.state.held_by == Some(LOCAL_PLAYER)).then_some(id))
            .collect();
        match gesture {
            Gesture::Dragging { offsets } => held.extend(offsets.into_keys()),
            Gesture::BoxSelecting { original, .. } => store.selected_pieces = original,
            Gesture::PendingPoint { original, .. } => store.selected_pieces = original,
            Gesture::Idle => {}
        }
        held.sort_unstable();
        held.dedup();
        store.preview_pieces.clear();
        held.into_iter().map(PieceCommand::Release).collect()
    }
}
