//! One translation decision, followed by unions at that fixed offset only.
use super::*;
use puzzella_core::{matches_translation, PuzzleGeometry, SnapCandidate};

#[derive(Clone, Copy)]
struct CorrectBounds {
    min: Vec2,
    max: Vec2,
}
impl CorrectBounds {
    fn fits(self, offset: Vec2) -> bool {
        (self.min + offset).is_finite() && (self.max + offset).is_finite()
    }
}

pub(super) struct SnapScratch {
    geometry: PuzzleGeometry,
    snap_distance: f32,
    #[cfg(test)]
    pub(super) boundary_members: usize,
    seen_targets: PieceScratchSet,
    validated: PieceScratchSet,
    eligible: PieceScratchSet,
    // Completed components never move again or rescan their exhausted boundary
    // during this Release, including a growing board cluster.
    pub(super) resolved: PieceScratchSet,
    members: Vec<PieceId>,
    // Keep the fixed logical translation only when representative subtraction
    // rounds it differently. Keys are queried directly, never used for tie order.
    rounded_offsets: HashMap<PieceId, Vec2>,
}
impl SnapScratch {
    pub(super) fn new(count: usize, definition: &PuzzleDefinition) -> Self {
        Self {
            geometry: definition.geometry(),
            snap_distance: definition.snap_distance,
            #[cfg(test)]
            boundary_members: 0,
            seen_targets: PieceScratchSet::new(count),
            validated: PieceScratchSet::new(count),
            eligible: PieceScratchSet::new(count),
            resolved: PieceScratchSet::new(count),
            members: Vec::new(),
            rounded_offsets: HashMap::new(),
        }
    }

    #[cfg(test)]
    pub(super) fn mask_heap_bytes(&self) -> usize {
        self.seen_targets.heap_bytes()
            + self.validated.heap_bytes()
            + self.eligible.heap_bytes()
            + self.resolved.heap_bytes()
    }

    fn reset_targets(&mut self) {
        self.seen_targets.clear();
    }

    fn first_visit(&mut self, target: PieceId) -> bool {
        self.seen_targets.insert(target)
    }
}

impl PieceDataStore {
    /// Called only for correct grid neighbors known to share the authoritative root.
    /// Reuse the closure's edge visits, including cycle edges with no new DSU union.
    fn cache_connected_edge(&mut self, member: PieceId, neighbor: PieceId, direction: usize) {
        let (edge, opposite) = CONNECTED_EDGE_PAIRS[direction];
        for (id, bit) in [(member, edge), (neighbor, opposite)] {
            if self.states[id.0 as usize].flags & bit == 0 {
                self.states[id.0 as usize].flags |= bit;
                self.dirty_pieces.insert(id);
            }
        }
    }

    fn target_offset(&self, target: PieceId, scratch: &SnapScratch) -> Vec2 {
        if let Some(&offset) = scratch.rounded_offsets.get(&target) {
            return offset;
        }
        let geometry = &scratch.geometry;
        let representative = self.connectivity.minimum_member(target);
        self.states[representative.0 as usize].position - geometry.correct_position(representative)
    }

    fn snap_target_is_eligible(
        &self,
        target: PieceId,
        translation: Vec2,
        scratch: &mut SnapScratch,
    ) -> bool {
        if scratch.validated.insert(target) {
            let placed = self.states[target.0 as usize].flags & PLACED != 0;
            if translation.is_finite()
                && self.connectivity.iter_component(target).all(|id| {
                    let state = &self.states[id.0 as usize];
                    let correct = scratch.geometry.correct_position(id);
                    self.held_by.get(&id).is_none()
                        && state.flags & (HELD | ENABLED) == ENABLED
                        && (state.flags & PLACED != 0) == placed
                        && matches_translation(state.position, correct, translation)
                        && (!placed || state.position == correct)
                })
            {
                scratch.eligible.insert(target);
            }
        }
        scratch.eligible.contains(&target)
    }

    fn best_neighbor(
        &self,
        moving: PieceId,
        translation: Vec2,
        scratch: &mut SnapScratch,
    ) -> Option<SnapCandidate> {
        let geometry = scratch.geometry;
        // Disconnected singletons and an already-connected puzzle usually have
        // no candidate. Compute reconstruction bounds only if one is in range.
        let mut bounds: Option<CorrectBounds> = None;
        let singleton = self.connectivity.component_size(moving) == 1;
        let mut singleton_targets = [PieceId(0); 4];
        let mut target_count = 0;
        let mut best: Option<SnapCandidate> = None;
        for member in self.connectivity.iter_component(moving) {
            for neighbor in geometry.neighbors(member).into_iter().flatten() {
                let target = self.connectivity.find_root(neighbor);
                if target == moving {
                    continue;
                }
                if singleton {
                    // At most four roots: no heap allocation or scratch bit writes
                    // for the million-disconnected-singleton hot path.
                    if singleton_targets[..target_count].contains(&target) {
                        continue;
                    }
                    singleton_targets[target_count] = target;
                    target_count += 1;
                } else if !scratch.first_visit(target) {
                    continue;
                }
                let offset = self.target_offset(target, scratch);
                // Minimum member is stable across union histories and snapshot restore.
                let representative = self.connectivity.minimum_member(target);
                let Some(candidate) = SnapCandidate::new(
                    translation,
                    offset,
                    Some(representative),
                    scratch.snap_distance,
                ) else {
                    continue;
                };
                if best.is_some_and(|best| !candidate.precedes(&best)) {
                    continue;
                }
                let bounds = bounds.get_or_insert_with(|| {
                    let correct = geometry.correct_position(moving);
                    let mut bounds = CorrectBounds {
                        min: correct,
                        max: correct,
                    };
                    for member in self.connectivity.iter_component(moving) {
                        let correct = geometry.correct_position(member);
                        bounds.min = bounds.min.min(correct);
                        bounds.max = bounds.max.max(correct);
                    }
                    bounds
                });
                if bounds.fits(offset) && self.snap_target_is_eligible(target, offset, scratch) {
                    best = Some(candidate);
                }
            }
        }
        best
    }

    fn normalize_component(
        &mut self,
        moving: PieceId,
        offset: Vec2,
        placed: bool,
        geometry: &PuzzleGeometry,
    ) -> usize {
        let mut newly_placed = 0;
        for member in self.connectivity.iter_component(moving) {
            let state = &mut self.states[member.0 as usize];
            let position = geometry.correct_position(member) + offset;
            debug_assert!(position.is_finite());
            let flags = (state.flags & !HELD) | if placed { PLACED } else { 0 };
            let z_order = if placed { 0 } else { state.z_order };
            if state.position != position || state.flags != flags || state.z_order != z_order {
                self.dirty_pieces.insert(member);
                newly_placed += usize::from(placed && state.flags & PLACED == 0);
                state.position = position;
                state.flags = flags;
                state.z_order = z_order;
            }
            if placed {
                self.selected_pieces.remove(&member);
            }
        }
        newly_placed
    }

    pub(super) fn resolve_component_snap(
        &mut self,
        moving: PieceId,
        scratch: &mut SnapScratch,
    ) -> usize {
        scratch.reset_targets();
        scratch.members.clear();
        let mut moving = self.connectivity.find_root(moving);
        let translation = self.target_offset(moving, scratch);
        if !translation.is_finite() {
            scratch.resolved.insert(moving);
            return 0;
        }
        // Board priority is unconditional inside the strict threshold. No neighbor
        // search or distance comparison runs until the board has been ruled out.
        let board = SnapCandidate::new(translation, Vec2::ZERO, None, scratch.snap_distance);
        let candidate = board.or_else(|| self.best_neighbor(moving, translation, scratch));
        let Some(candidate) = candidate else {
            scratch.resolved.insert(moving);
            return 0;
        };
        let final_offset = candidate.offset;
        let placed = final_offset == Vec2::ZERO;
        let geometry = scratch.geometry;
        let mut newly_placed = self.normalize_component(moving, final_offset, placed, &geometry);
        scratch
            .members
            .extend(self.connectivity.iter_component(moving));
        scratch.reset_targets();
        let mut cursor = 0;
        while cursor < scratch.members.len() {
            let member = scratch.members[cursor];
            cursor += 1;
            #[cfg(test)]
            {
                scratch.boundary_members += 1;
            }
            for (direction, neighbor) in geometry.neighbors(member).into_iter().enumerate() {
                let Some(neighbor) = neighbor else { continue };
                let target = self.connectivity.find_root(neighbor);
                if target == moving {
                    self.cache_connected_edge(member, neighbor, direction);
                    continue;
                }
                if !scratch.first_visit(target) {
                    continue;
                }
                let offset = self.target_offset(target, scratch);
                let representative = self.connectivity.minimum_member(target);
                // Recognize the same logical translation despite f32 add/sub
                // rounding. This is not another distance-based snap decision.
                if (offset != final_offset
                    && !matches_translation(
                        self.states[representative.0 as usize].position,
                        geometry.correct_position(representative),
                        final_offset,
                    ))
                    || !self.snap_target_is_eligible(target, offset, scratch)
                {
                    continue;
                }
                // Validate against ONE fixed offset, not accumulated edge error.
                // Cached resolved offsets avoid rescanning growing aligned clusters.
                if offset != final_offset
                    && !self.connectivity.iter_component(target).all(|id| {
                        matches_translation(
                            self.states[id.0 as usize].position,
                            geometry.correct_position(id),
                            final_offset,
                        )
                    })
                {
                    continue;
                }
                // Copy the incoming list BEFORE union; never rescan the growing list.
                if !scratch.resolved.contains(&target) {
                    scratch
                        .members
                        .extend(self.connectivity.iter_component(target));
                }
                if placed {
                    if self.states[target.0 as usize].flags & PLACED == 0 {
                        newly_placed +=
                            self.normalize_component(target, final_offset, true, &geometry);
                    }
                } else {
                    let moving_selected = self
                        .selected_pieces
                        .contains(&self.connectivity.minimum_member(moving));
                    let target_selected = self
                        .selected_pieces
                        .contains(&self.connectivity.minimum_member(target));
                    if moving_selected != target_selected {
                        let unselected = if moving_selected { target } else { moving };
                        for id in self.connectivity.iter_component(unselected) {
                            self.selected_pieces.insert(id);
                        }
                    }
                }
                scratch.rounded_offsets.remove(&moving);
                scratch.rounded_offsets.remove(&target);
                moving = self.connectivity.union(moving, target);
                self.cache_connected_edge(member, neighbor, direction);
            }
        }
        let representative = self.connectivity.minimum_member(moving);
        let recovered = self.states[representative.0 as usize].position
            - geometry.correct_position(representative);
        if recovered != final_offset {
            scratch.rounded_offsets.insert(moving, final_offset);
        }
        self.placed_count += newly_placed;
        scratch.resolved.insert(moving);
        scratch.validated.insert(moving);
        scratch.eligible.insert(moving);
        self.highlights_dirty = true;
        newly_placed
    }
}
