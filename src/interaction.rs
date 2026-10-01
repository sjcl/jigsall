//! Local pointer gestures. Gameplay changes only through PieceCommand.
use crate::{gameplay::*, resources::*};
use bevy::prelude::*;
use std::collections::{BTreeMap, HashSet};

#[derive(Default)]
enum Gesture {
    #[default]
    Idle,
    Dragging {
        offsets: BTreeMap<PieceId, Vec2>,
    },
    BoxSelecting {
        anchor: Vec2,
        current: Vec2,
        original: HashSet<PieceId>,
        additive: bool,
    },
}

#[derive(Resource, Default)]
pub struct PieceInteraction {
    gesture: Gesture,
}

pub struct PointerFrame {
    pub position: Option<Vec2>,
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

    pub fn update(
        &mut self,
        frame: PointerFrame,
        store: &mut PieceDataStore,
        collision: &mut PieceCollisionSystem,
    ) -> Vec<PieceCommand> {
        // Placement or another player's ownership can invalidate a selection.
        store.selected_pieces.retain(|id| {
            store.pieces.get(id).is_some_and(|piece| {
                !piece.state.placed
                    && (piece.state.held_by.is_none() || piece.state.held_by == Some(LOCAL_PLAYER))
            })
        });
        if !frame.focused {
            return self.cancel(store);
        }
        let point = frame.position.filter(|point| point.is_finite());
        let mut commands = Vec::new();
        if frame.just_pressed && matches!(self.gesture, Gesture::Idle) && !frame.over_ui {
            if let Some(point) = point {
                let hit = collision
                    .find_piece_at_position(point)
                    .filter(|&id| selectable(store, id));
                if let Some(id) = hit {
                    if frame.ctrl {
                        if !store.selected_pieces.remove(&id) {
                            store.selected_pieces.insert(id);
                        }
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
                        // Raising a group preserves its internal stacking order.
                        ids.sort_by(|a, b| {
                            store.transforms[a]
                                .translation
                                .z
                                .total_cmp(&store.transforms[b].translation.z)
                                .then(a.cmp(b))
                        });
                        let offsets = ids
                            .iter()
                            .map(|&id| (id, store.pieces[&id].state.position - point))
                            .collect();
                        commands.extend(ids.into_iter().map(PieceCommand::Grab));
                        self.gesture = Gesture::Dragging { offsets };
                    }
                } else {
                    let original = store.selected_pieces.clone();
                    if !frame.ctrl {
                        store.selected_pieces.clear();
                    }
                    store.preview_pieces.clear();
                    self.gesture = Gesture::BoxSelecting {
                        anchor: point,
                        current: point,
                        original,
                        additive: frame.ctrl,
                    };
                }
            }
        }

        match &mut self.gesture {
            Gesture::Dragging { offsets } => {
                // The release frame contributes its final position too. While
                // outside the viewport, retain the last valid piece positions.
                if let Some(point) = point {
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
                anchor,
                current,
                original,
                additive,
            } => {
                if let Some(point) = point {
                    *current = point;
                    let rect = Rect {
                        min: anchor.min(point),
                        max: anchor.max(point),
                    };
                    store.preview_pieces = if rect.width() > 0.0 && rect.height() > 0.0 {
                        collision
                            .find_pieces_with_detailed_rect_intersection(rect)
                            .into_iter()
                            .filter(|&id| selectable(store, id))
                            .collect()
                    } else {
                        HashSet::new()
                    };
                }
                if !frame.pressed {
                    if point.is_none() {
                        store.selected_pieces = original.clone();
                    } else if *additive {
                        store.selected_pieces = original.clone();
                        store
                            .selected_pieces
                            .extend(store.preview_pieces.iter().copied());
                    } else {
                        store.selected_pieces = store.preview_pieces.clone();
                    }
                    store.preview_pieces.clear();
                    self.gesture = Gesture::Idle;
                }
            }
            Gesture::Idle => {}
        }
        commands
    }

    /// Pause/focus loss ends ownership and discards an unfinished selection box.
    pub fn cancel(&mut self, store: &mut PieceDataStore) -> Vec<PieceCommand> {
        let gesture = std::mem::take(&mut self.gesture);
        let mut held: Vec<_> = store
            .pieces
            .iter()
            .filter_map(|(&id, piece)| (piece.state.held_by == Some(LOCAL_PLAYER)).then_some(id))
            .collect();
        match gesture {
            Gesture::Dragging { offsets } => held.extend(offsets.into_keys()),
            Gesture::BoxSelecting { original, .. } => store.selected_pieces = original,
            Gesture::Idle => {}
        }
        held.sort_unstable();
        held.dedup();
        store.preview_pieces.clear();
        held.into_iter().map(PieceCommand::Release).collect()
    }
}
