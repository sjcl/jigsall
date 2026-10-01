//! Release-local boundary frontier and translation index. Nothing runs on idle/pointer frames.
use super::*;
use puzzella_core::{matches_translation, SnapCandidate};
use rstar::RTree;
use std::collections::{BTreeMap, BTreeSet};

type OffsetKey = (u32, u32);
fn key(offset: Vec2) -> OffsetKey {
    // Treat negative zero as the same translation.
    let bits = |v: f32| if v == 0.0 { 0 } else { v.to_bits() };
    (bits(offset.x), bits(offset.y))
}
fn offset(key: OffsetKey) -> Vec2 {
    Vec2::new(f32::from_bits(key.0), f32::from_bits(key.1))
}
fn point(offset: Vec2) -> [f64; 2] {
    [f64::from(offset.x), f64::from(offset.y)]
}

/// Identical offsets share ONE point and ordered boundary PieceIds. A million
/// already-aligned targets therefore do not cause repeated million-way tie scans.
#[derive(Default)]
struct CandidateIndex {
    tree: RTree<[f64; 2]>,
    targets: BTreeMap<OffsetKey, BTreeSet<PieceId>>,
    by_root: BTreeMap<PieceId, (PieceId, Vec2)>,
}
impl CandidateIndex {
    fn insert(&mut self, root: PieceId, target: PieceId, translation: Vec2) {
        if let Some(&(previous, offset)) = self.by_root.get(&root) {
            if previous <= target {
                return;
            }
            self.remove_boundary(previous, offset);
        }
        self.by_root.insert(root, (target, translation));
        let key = key(translation);
        let targets = self.targets.entry(key).or_insert_with(|| {
            self.tree.insert(point(offset(key)));
            BTreeSet::new()
        });
        targets.insert(target);
    }

    fn remove_boundary(&mut self, target: PieceId, translation: Vec2) {
        let key = key(translation);
        let targets = self.targets.get_mut(&key).unwrap();
        targets.remove(&target);
        if targets.is_empty() {
            self.targets.remove(&key);
            self.tree.remove(&point(offset(key)));
        }
    }

    fn remove_component(&mut self, root: PieceId) {
        let (boundary, translation) = self.by_root.remove(&root).unwrap();
        self.remove_boundary(boundary, translation);
    }

    fn nearest(&self, moving: Vec2, distance: f32, bounds: CorrectBounds) -> Option<SnapCandidate> {
        let mut best: Option<SnapCandidate> = None;
        for (p, squared) in self
            .tree
            .nearest_neighbor_iter_with_distance_2(&point(moving))
        {
            if squared >= f64::from(distance).powi(2) {
                break;
            }
            if best.is_some_and(|best| squared > best.distance_squared) {
                break;
            }
            let translation = Vec2::new(p[0] as f32, p[1] as f32);
            if !bounds.fits(translation) {
                continue;
            }
            let target = *self.targets[&key(translation)].first().unwrap();
            let Some(candidate) = SnapCandidate::new(moving, translation, Some(target), distance)
            else {
                break;
            };
            if best.is_none_or(|best| candidate.precedes(&best)) {
                best = Some(candidate);
            }
        }
        best
    }
}

#[derive(Clone, Copy)]
struct CorrectBounds {
    min: Vec2,
    max: Vec2,
}
impl CorrectBounds {
    fn empty() -> Self {
        Self {
            min: Vec2::splat(f32::INFINITY),
            max: Vec2::splat(f32::NEG_INFINITY),
        }
    }
    fn include(&mut self, correct: Vec2) {
        self.min = self.min.min(correct);
        self.max = self.max.max(correct);
    }
    fn merge(&mut self, other: Self) {
        self.include(other.min);
        self.include(other.max);
    }
    fn fits(self, offset: Vec2) -> bool {
        (self.min + offset).is_finite() && (self.max + offset).is_finite()
    }
}

pub(super) struct SnapScratch {
    #[cfg(test)]
    pub(super) boundary_members: usize,
    seen_targets: PieceBitSet,
    touched: Vec<PieceId>,
    candidates: Vec<(PieceId, PieceId, Vec2)>,
    validated: PieceBitSet,
    eligible: PieceBitSet,
    completed_placed: PieceBitSet,
    pending_members: Vec<PieceId>,
}
impl SnapScratch {
    pub(super) fn new(count: usize) -> Self {
        Self {
            #[cfg(test)]
            boundary_members: 0,
            seen_targets: PieceBitSet::new(count),
            touched: Vec::new(),
            candidates: Vec::new(),
            validated: PieceBitSet::new(count),
            eligible: PieceBitSet::new(count),
            completed_placed: PieceBitSet::new(count),
            pending_members: Vec::new(),
        }
    }

    fn reset(&mut self) {
        // Never zero an N-bit scratch allocation once per released singleton.
        for id in self.touched.drain(..) {
            self.seen_targets.remove(&id);
        }
        self.candidates.clear();
        self.pending_members.clear();
    }
}

impl PieceDataStore {
    fn discover_boundary(
        &self,
        frontier: PieceId,
        moving: PieceId,
        definition: &PuzzleDefinition,
        scratch: &mut SnapScratch,
        collect_pending: bool,
    ) -> CorrectBounds {
        let mut bounds = CorrectBounds::empty();
        let frontier_root = self.connectivity.find_root(frontier);
        let moving_root = self.connectivity.find_root(moving);
        for member in self.connectivity.iter_component(frontier) {
            if collect_pending {
                scratch.pending_members.push(member);
            }
            #[cfg(test)]
            {
                scratch.boundary_members += 1;
            }
            bounds.include(definition.correct_position(member));
            for neighbor in definition.neighbors(member).into_iter().flatten() {
                let target = self.connectivity.find_root(neighbor);
                if target == frontier_root
                    || target == moving_root
                    || (scratch.validated.contains(&target) && !scratch.eligible.contains(&target))
                    || scratch.seen_targets.contains(&neighbor)
                {
                    continue;
                }
                scratch.seen_targets.insert(neighbor);
                scratch.touched.push(neighbor);
                let representative = self.connectivity.minimum_member(target);
                let target_state = &self.states[representative.0 as usize];
                let translation =
                    target_state.position - definition.correct_position(representative);
                // Defer whole-target validation until it is actually a snap candidate.
                // Merely touching a distant huge neighbor must not scan its members.
                if translation.is_finite()
                    && self.held_by.get(&target).is_none()
                    && target_state.flags & (HELD | ENABLED) == ENABLED
                {
                    // Stable boundary PieceId, independent of DSU union history or restore roots.
                    scratch.candidates.push((target, neighbor, translation));
                }
            }
        }
        bounds
    }

    fn snap_target_is_eligible(
        &self,
        target: PieceId,
        translation: Vec2,
        definition: &PuzzleDefinition,
        scratch: &mut SnapScratch,
    ) -> bool {
        let target = self.connectivity.find_root(target);
        if scratch.validated.insert(target) {
            let placed = self.states[target.0 as usize].flags & PLACED != 0;
            if self.connectivity.iter_component(target).all(|id| {
                let state = &self.states[id.0 as usize];
                self.held_by.get(&id).is_none()
                    && state.flags & (HELD | ENABLED) == ENABLED
                    && (state.flags & PLACED != 0) == placed
                    && matches_translation(
                        state.position,
                        definition.correct_position(id),
                        translation,
                    )
                    && (!placed || state.position == definition.correct_position(id))
            }) {
                scratch.eligible.insert(target);
            }
        }
        scratch.eligible.contains(&target)
    }

    pub(super) fn resolve_component_snap(
        &mut self,
        moving: PieceId,
        definition: &PuzzleDefinition,
        scratch: &mut SnapScratch,
    ) -> usize {
        scratch.reset();
        let representative = self.connectivity.minimum_member(moving);
        let mut translation = self.states[representative.0 as usize].position
            - definition.correct_position(representative);
        if !translation.is_finite() {
            return 0;
        }
        let mut bounds = self.discover_boundary(moving, moving, definition, scratch, false);
        let mut best = SnapCandidate::new(translation, Vec2::ZERO, None, definition.snap_distance);
        for &(_, target, target_offset) in &scratch.candidates {
            if !bounds.fits(target_offset) {
                continue;
            }
            if let Some(candidate) = SnapCandidate::new(
                translation,
                target_offset,
                Some(target),
                definition.snap_distance,
            ) {
                if best.is_none_or(|best| candidate.precedes(&best)) {
                    best = Some(candidate);
                }
            }
        }
        // Most disconnected releases do not snap: no tree/map allocation in that path.
        let Some(mut candidate) = best else { return 0 };
        scratch
            .pending_members
            .extend(self.connectivity.iter_component(moving));
        let mut index = CandidateIndex::default();
        for (root, target, translation) in scratch.candidates.drain(..) {
            index.insert(root, target, translation);
        }
        let mut placed = false;
        loop {
            if let Some(target) = candidate.target {
                let target_root = self.connectivity.find_root(target);
                index.remove_component(target_root);
                if self.snap_target_is_eligible(target, candidate.offset, definition, scratch) {
                    translation = candidate.offset;
                    let target_placed = self.states[target.0 as usize].flags & PLACED != 0;
                    placed |= target_placed;
                    // Scan BEFORE the list splice, so growing components are never rescanned.
                    // Reusing a placed target exhausted in this Release must not
                    // rescan a growing board cluster for every independent singleton.
                    if !target_placed || !scratch.completed_placed.contains(&target_root) {
                        bounds.merge(self.discover_boundary(
                            target,
                            moving,
                            definition,
                            scratch,
                            !target_placed,
                        ));
                    }
                    for (root, target, translation) in scratch.candidates.drain(..) {
                        index.insert(root, target, translation);
                    }
                    self.connectivity.union(moving, target);
                }
            } else {
                translation = Vec2::ZERO;
                placed = true;
            }
            let board = (!placed)
                .then(|| {
                    SnapCandidate::new(translation, Vec2::ZERO, None, definition.snap_distance)
                })
                .flatten();
            let neighbor = index.nearest(translation, definition.snap_distance, bounds);
            best = match (board, neighbor) {
                (Some(board), Some(neighbor)) => Some(if board.precedes(&neighbor) {
                    board
                } else {
                    neighbor
                }),
                (board, neighbor) => board.or(neighbor),
            };
            let Some(next) = best else { break };
            // Once on the board, accept only zero-offset neighbors. They remain at
            // correct_position; near loose neighbors snap to the placed component
            // when THEY release, so a fixed board component never moves off-board.
            if placed && next.offset != Vec2::ZERO {
                break;
            }
            candidate = next;
        }
        let selected = !placed
            && scratch
                .pending_members
                .iter()
                .any(|id| self.selected_pieces.contains(id));
        let mut newly_placed = 0;
        let states = &mut *self.states;
        // Placed targets remain canonical, so only new unplaced members need writes.
        for &member in &scratch.pending_members {
            let state = &mut states[member.0 as usize];
            let position = definition.correct_position(member) + translation;
            // Bounds checked in O(1) before every union, without rescanning the growing list.
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
            } else if selected && !self.selected_pieces.contains(&member) {
                self.selected_pieces.insert(member);
            }
        }
        self.placed_count += newly_placed;
        if placed {
            let root = self.connectivity.find_root(moving);
            scratch.completed_placed.insert(root);
            scratch.validated.insert(root);
            scratch.eligible.insert(root);
        }
        self.highlights_dirty = true;
        newly_placed
    }
}
