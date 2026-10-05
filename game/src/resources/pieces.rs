use bevy::prelude::*;
use bytemuck::{Pod, Zeroable};
use jigsall_core::{
    decode_rotation, rotate_quarter, with_rotation, PieceBitSet, PieceCommand, PieceConnectivity,
    PieceId, PieceScratchSet, PieceState, PlayerId, PuzzleDefinition,
};
use std::{
    collections::HashMap,
    mem::MaybeUninit,
    ops::{Deref, DerefMut},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

pub const PLACED: u32 = 1;
#[cfg(test)]
pub const SELECTED: u32 = 2;
#[cfg(test)]
pub const PREVIEW: u32 = 4;
pub const HELD: u32 = 8;
pub const ENABLED: u32 = 16;
// Presentation cache only; PieceConnectivity remains the connectivity authority.
// Directions refer to canonical piece edges and never change when rotating.
// Keep in sync with the highlight boundary mask in puzzle_render.wgsl.
pub const CONNECTED_TOP: u32 = 1 << 5;
pub const CONNECTED_RIGHT: u32 = 1 << 6;
pub const CONNECTED_BOTTOM: u32 = 1 << 7;
pub const CONNECTED_LEFT: u32 = 1 << 8;
pub const CONNECTED_EDGES: u32 =
    CONNECTED_TOP | CONNECTED_RIGHT | CONNECTED_BOTTOM | CONNECTED_LEFT;
// PuzzleGeometry::neighbors order: left, right, up, down.
pub(crate) const CONNECTED_EDGE_PAIRS: [(u32, u32); 4] = [
    (CONNECTED_LEFT, CONNECTED_RIGHT),
    (CONNECTED_RIGHT, CONNECTED_LEFT),
    (CONNECTED_TOP, CONNECTED_BOTTOM),
    (CONNECTED_BOTTOM, CONNECTED_TOP),
];
pub const MAX_Z: u32 = (1 << 24) - 2;
mod cancellation;
pub(crate) mod local_rotation;
mod rotation;
mod snapping;
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct GpuPieceState {
    pub position: Vec2,
    pub z_order: u32,
    pub flags: u32,
}
const _: () = assert!(std::mem::size_of::<GpuPieceState>() == 16);
impl GpuPieceState {
    pub fn new(position: Vec2, id: PieceId) -> Self {
        Self {
            position,
            z_order: id.0,
            flags: ENABLED,
        }
    }
}

/// One fixed-size allocation shared only while the initial upload is in flight.
/// Early mutations use copy-on-write so an extracted upload stays immutable.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DensePieceStates(Arc<[GpuPieceState]>);

#[cfg(test)]
thread_local! {
    static FORBID_PIECE_STATE_ACCESS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Test guard for presentation paths that must never inspect canonical pieces.
#[cfg(test)]
pub(crate) fn without_piece_state_access<R>(f: impl FnOnce() -> R) -> R {
    struct Reset(bool);
    impl Drop for Reset {
        fn drop(&mut self) {
            FORBID_PIECE_STATE_ACCESS.set(self.0);
        }
    }
    let _reset = Reset(FORBID_PIECE_STATE_ACCESS.replace(true));
    f()
}

impl DensePieceStates {
    /// Build the final 16-byte states directly in their worker-owned allocation.
    pub fn generate(definition: &PuzzleDefinition) -> Self {
        let mut states = Arc::<[GpuPieceState]>::new_uninit_slice(definition.piece_count());
        let display_size = definition.image_size.as_vec2();
        jigsall_puzzle::placement::fill_placement_grid(
            Arc::get_mut(&mut states).unwrap(),
            jigsall_puzzle::placement::placement_piece_size(definition),
            display_size,
            definition.seed,
            |position| MaybeUninit::new(GpuPieceState::new(position, PieceId(0))),
        );
        // SAFETY: fill_placement_grid writes every slot before shuffling it.
        // GpuPieceState has no drop glue; a panic before this point is also safe.
        let mut states = unsafe { states.assume_init() };
        for (id, state) in Arc::get_mut(&mut states).unwrap().iter_mut().enumerate() {
            state.z_order = id as u32;
            state.flags = with_rotation(
                state.flags,
                jigsall_puzzle::placement::initial_rotation(definition, PieceId(id as u32)),
            );
        }
        Self(states)
    }

    /// Fixed-size storage has no spare capacity.
    pub fn capacity(&self) -> usize {
        self.len()
    }
}

impl Deref for DensePieceStates {
    type Target = [GpuPieceState];
    fn deref(&self) -> &Self::Target {
        #[cfg(test)]
        assert!(
            !FORBID_PIECE_STATE_ACCESS.get(),
            "pending frame accessed canonical pieces"
        );
        &self.0
    }
}

impl<'a> IntoIterator for &'a DensePieceStates {
    type Item = &'a GpuPieceState;
    type IntoIter = std::slice::Iter<'a, GpuPieceState>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl DerefMut for DensePieceStates {
    fn deref_mut(&mut self) -> &mut Self::Target {
        #[cfg(test)]
        assert!(
            !FORBID_PIECE_STATE_ACCESS.get(),
            "pending frame mutated canonical pieces"
        );
        Arc::make_mut(&mut self.0)
    }
}

impl From<Vec<GpuPieceState>> for DensePieceStates {
    fn from(states: Vec<GpuPieceState>) -> Self {
        Self(states.into())
    }
}

impl From<DensePieceStates> for Arc<[GpuPieceState]> {
    fn from(states: DensePieceStates) -> Self {
        states.0
    }
}

static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);
/// Local presentation only. Membership is frozen once; pointer frames change delta.
#[derive(Clone, Default)]
pub struct DragTransform {
    pub members: Arc<[u32]>,
    pub delta: Vec2,
}
impl DragTransform {
    fn remove(&mut self, id: PieceId) {
        let index = id.0 as usize / 32;
        let bit = 1 << (id.0 % 32);
        if self.members.get(index).is_some_and(|word| word & bit != 0) {
            Arc::make_mut(&mut self.members)[index] &= !bit;
        }
    }
    fn exclude(&mut self, members: &PieceBitSet) {
        if self.members.is_empty() {
            return;
        }
        for index in 0..self.members.len() {
            let old = self.members[index];
            let word = old & !members.words().get(index).copied().unwrap_or(0);
            if old != word {
                Arc::make_mut(&mut self.members)[index] = word;
            }
        }
    }
}
/// Dense owner IDs plus occupancy: 8.125 MB / 1M, allocated only on first hold.
/// All u64 PlayerIds are valid; occupancy avoids reserving a sentinel identity.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PieceOwners {
    owners: Vec<PlayerId>,
    occupied: PieceBitSet,
    counts: HashMap<PlayerId, usize>,
}
impl PieceOwners {
    pub fn len(&self) -> usize {
        self.occupied.count()
    }
    pub fn is_empty(&self) -> bool {
        self.occupied.is_empty()
    }
    pub fn get(&self, id: &PieceId) -> Option<&PlayerId> {
        self.occupied
            .contains(id)
            .then(|| &self.owners[id.0 as usize])
    }
    pub fn has_player(&self, player: PlayerId) -> bool {
        self.counts.contains_key(&player)
    }
    pub(crate) fn count_for(&self, player: PlayerId) -> usize {
        self.counts.get(&player).copied().unwrap_or(0)
    }
    fn ensure_len(&mut self, count: usize) {
        assert!(count <= jigsall_core::MAX_PIECES);
        if self.owners.len() < count {
            self.owners.resize(count, PlayerId(0));
            let mut occupied = PieceBitSet::new(count);
            occupied.union(&self.occupied);
            self.occupied = occupied;
        }
    }
    pub fn insert(&mut self, id: PieceId, player: PlayerId) {
        if self.get(&id) == Some(&player) {
            return;
        }
        self.remove(&id);
        self.ensure_len(id.0 as usize + 1);
        self.owners[id.0 as usize] = player;
        self.occupied.insert(id);
        *self.counts.entry(player).or_default() += 1;
    }
    pub fn remove(&mut self, id: &PieceId) {
        if let Some(player) = self.get(id).copied() {
            let count = self.counts.get_mut(&player).unwrap();
            *count -= 1;
            if *count == 0 {
                self.counts.remove(&player);
            }
        }
        self.occupied.remove(id);
    }
    pub fn iter(&self) -> impl Iterator<Item = (PieceId, &PlayerId)> + '_ {
        self.occupied
            .iter()
            .map(|id| (id, &self.owners[id.0 as usize]))
    }
}

#[derive(Default, Debug, PartialEq, Eq)]
pub struct AppliedCommand {
    pub grabbed: usize,
    pub released: usize,
    pub placed: usize,
    pub rotated: usize,
    pub drag_rebased: bool,
}
/// Operation-local accepted IDs and an optional shared presentation/bulk mask.
/// Small authority-only grabs never construct membership words.
struct GrabPlan {
    ids: Vec<PieceId>,
    members: Option<PieceBitSet>,
}

/// Dense CPU authority: no per-piece definition, transform, handle or entity.
#[derive(Resource, Default)]
pub struct PieceDataStore {
    pub connectivity: PieceConnectivity,
    pub drag: DragTransform,
    /// Local uncommitted rotation only; excluded from canonical state/capture.
    pub local_rotation: local_rotation::LocalRotationPresentation,
    /// Continuous visual residuals; lifetime is independent of prediction ACKs.
    pub rotation_visual: super::rotation_visual::RotationVisual,
    pub states: DensePieceStates,
    pub held_by: PieceOwners,
    pub selected_pieces: PieceBitSet,
    pub dirty_pieces: PieceBitSet,
    /// Discrete rotation requires exact dirty spans, even for fragmented selections.
    exact_dirty_ranges: bool,
    /// Only absorbed members; roots are derived at upload time, with no CPU mirror.
    pub component_root_dirty: PieceBitSet,
    pub placed_count: usize,
    pub next_z_order: u32,
    pub epoch: u64,
    pub highlights_dirty: bool,
}
impl PieceDataStore {
    /// Convenience for explicit positions; runtime generation uses initialize_dense.
    pub fn initialize(&mut self, positions: Vec<Vec2>) {
        self.initialize_dense(
            positions
                .into_iter()
                .enumerate()
                .map(|(i, p)| GpuPieceState::new(p, PieceId(i as u32)))
                .collect::<Vec<_>>()
                .into(),
        );
    }
    /// Adopt the worker result without visiting or copying any piece states.
    pub fn initialize_dense(&mut self, states: DensePieceStates) {
        let presentation_clock = self.rotation_visual.clock;
        *self = Self::default();
        self.rotation_visual.clock = presentation_clock;
        self.epoch = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
        self.states = states;
        self.connectivity = PieceConnectivity::new(self.len());
        self.next_z_order = self.states.len() as u32;
        self.selected_pieces = PieceBitSet::new(self.len());
        self.dirty_pieces = PieceBitSet::new(self.len());
        self.component_root_dirty = PieceBitSet::new(self.len());
    }
    /// Rare snapshot restore. Reuses the local upload/readback lifecycle counter.
    pub(crate) fn replace_snapshot_states(
        &mut self,
        states: Vec<GpuPieceState>,
        next_z_order: u32,
        connectivity: PieceConnectivity,
    ) {
        let presentation_clock = self.rotation_visual.clock;
        *self = Self::default();
        self.rotation_visual.clock = presentation_clock;
        self.epoch = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
        self.placed_count = states
            .iter()
            .filter(|state| state.flags & PLACED != 0)
            .count();
        self.states = states.into();
        self.connectivity = connectivity;
        self.next_z_order = next_z_order;
        self.selected_pieces = PieceBitSet::new(self.len());
        self.dirty_pieces = PieceBitSet::new(self.len());
        self.component_root_dirty = PieceBitSet::new(self.len());
    }
    pub fn len(&self) -> usize {
        self.states.len()
    }

    /// Join-only commit step after snapshot/overlay validation and snapshot restore.
    /// Membership is canonical, disjoint, nonempty and unplaced; target is unheld.
    /// Changes only HELD/owners, with one player-count map write per drag. The new
    /// snapshot epoch already requires a full upload, so no dirty mask is needed.
    pub(crate) fn restore_baseline_holds(
        &mut self,
        player: PlayerId,
        target: &jigsall_core::protocol::ActiveDragTarget,
    ) {
        use jigsall_core::protocol::ActiveDragTarget;
        self.held_by.ensure_len(self.len());
        let mut count = 0;
        let states = &mut *self.states;
        let owners = &mut self.held_by;
        match target {
            ActiveDragTarget::Sparse(refs) => {
                for reference in refs {
                    for id in self.connectivity.iter_component(reference.member) {
                        states[id.0 as usize].flags |= HELD;
                        owners.owners[id.0 as usize] = player;
                        owners.occupied.insert(id);
                        count += 1;
                    }
                }
            }
            ActiveDragTarget::Dense(dense) => {
                for id in dense.members.iter() {
                    states[id.0 as usize].flags |= HELD;
                    owners.owners[id.0 as usize] = player;
                }
                owners.occupied.union(&dense.members);
                count = dense.members.count();
            }
        }
        owners.counts.insert(player, count);
    }
    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }
    pub fn contains(&self, id: PieceId) -> bool {
        (id.0 as usize) < self.len()
    }
    pub fn state(&self, id: PieceId) -> Option<PieceState> {
        self.states.get(id.0 as usize).map(|s| PieceState {
            position: s.position,
            placed: s.flags & PLACED != 0,
            held_by: self.held_by.get(&id).copied(),
        })
    }
    pub fn set_state(&mut self, id: PieceId, state: PieceState, local_player: PlayerId) {
        assert_eq!(
            self.connectivity.component_size(id),
            1,
            "Use component commands for connected pieces"
        );
        let s = &mut self.states[id.0 as usize];
        if state.placed && s.flags & PLACED == 0 {
            self.placed_count += 1;
        }
        if !state.placed && s.flags & PLACED != 0 {
            self.placed_count -= 1;
        }
        s.position = state.position;
        s.flags = (s.flags & !(PLACED | HELD))
            | if state.placed { PLACED } else { 0 }
            | if state.held_by.is_some() { HELD } else { 0 };
        if state.placed {
            s.flags = with_rotation(s.flags, 0);
            s.z_order = 0;
        }
        if let Some(player) = state.held_by {
            self.held_by.ensure_len(self.len());
            self.held_by.insert(id, player);
        } else {
            self.held_by.remove(&id);
        }
        // Explicit authority changes can end ownership before a gesture finishes.
        // Never present another player's subsequent grab with our local delta.
        if state.held_by != Some(local_player) {
            self.drag.remove(id);
        }
        // This setter is singleton-only; connected authority changes use bulk commands.
        if !self.is_valid_local_selection(id, local_player) && self.selected_pieces.remove(&id) {
            self.highlights_dirty = true;
        }
        self.dirty_pieces.insert(id);
    }
    pub(crate) fn is_selectable(&self, id: PieceId) -> bool {
        self.states
            .get(id.0 as usize)
            .is_some_and(|s| s.flags & (PLACED | HELD) == 0 && s.flags & ENABLED != 0)
            && self.held_by.get(&id).is_none()
    }
    /// Existing selections may include our own drag, but never another player's hold.
    pub(crate) fn is_valid_local_selection(&self, id: PieceId, local_player: PlayerId) -> bool {
        self.states
            .get(id.0 as usize)
            .is_some_and(|s| s.flags & PLACED == 0 && s.flags & ENABLED != 0)
            && self
                .held_by
                .get(&id)
                .is_none_or(|owner| *owner == local_player)
    }
    fn compact_z(&mut self) {
        // Extremely rare slow path after ~16M front operations.
        let mut ids: Vec<_> = (0..self.len())
            .filter(|&i| self.states[i].flags & PLACED == 0)
            .collect();
        ids.sort_unstable_by_key(|&i| (self.states[i].z_order, i));
        for (rank, i) in ids.into_iter().enumerate() {
            self.states[i].z_order = rank as u32;
            self.dirty_pieces.insert(PieceId(i as u32));
        }
        self.next_z_order = self.len() as u32;
    }
    pub fn bring_piece_to_front(&mut self, id: PieceId) {
        let mut ids: Vec<_> = self.connectivity.iter_component(id).collect();
        ids.sort_unstable_by_key(|id| (self.states[id.0 as usize].z_order, *id));
        if self.next_z_order.saturating_add(ids.len() as u32) > MAX_Z {
            self.compact_z();
        }
        let states = &mut *self.states;
        for (rank, id) in ids.iter().enumerate() {
            states[id.0 as usize].z_order = self.next_z_order + rank as u32;
            self.dirty_pieces.insert(*id);
        }
        self.next_z_order += ids.len() as u32;
    }
    /// Selection is presentation-only: never mutate dense flags or dirty states.
    pub fn sync_highlights(&mut self) {
        self.highlights_dirty = false;
    }

    /// Expand first, then accept/reject each entire component against current authority.
    pub(crate) fn canonical_members(
        &self,
        requested: &PieceBitSet,
        mut acceptable: impl FnMut(PieceId) -> bool,
    ) -> PieceBitSet {
        let mut members = self.connectivity.expand(requested);
        let mut seen = PieceScratchSet::new(self.len());
        for id in requested.iter().filter(|id| self.contains(*id)) {
            if self.connectivity.component_size(id) == 1 {
                if !acceptable(id) {
                    members.remove(&id);
                }
                continue;
            }
            let root = self.connectivity.find_root(id);
            if !seen.contains(&root) {
                seen.insert(root);
                if !self.connectivity.iter_component(root).all(&mut acceptable) {
                    for member in self.connectivity.iter_component(root) {
                        members.remove(&member);
                    }
                }
            }
        }
        members
    }

    pub(crate) fn selectable_members(&self, requested: &PieceBitSet) -> PieceBitSet {
        self.canonical_members(requested, |id| self.is_selectable(id))
    }

    pub(crate) fn component_is_selectable(&self, id: PieceId) -> bool {
        self.contains(id)
            && self
                .connectivity
                .iter_component(id)
                .all(|m| self.is_selectable(m))
    }

    pub(crate) fn select_component(&mut self, id: PieceId, toggle: bool) {
        let remove = toggle && self.selected_pieces.contains(&id);
        for member in self.connectivity.iter_component(id) {
            if remove {
                self.selected_pieces.remove(&member);
            } else {
                self.selected_pieces.insert(member);
            }
        }
        self.highlights_dirty = true;
    }

    /// GPU rectangle readback stays a bitset; expansion happens only on final commit.
    pub fn commit_selection(
        &mut self,
        members: PieceBitSet,
        original: Option<&PieceBitSet>,
        local_player: PlayerId,
    ) {
        let mut selected = self.selectable_members(&members);
        if let Some(original) = original {
            let original = self.canonical_members(original, |id| {
                self.is_valid_local_selection(id, local_player)
            });
            selected.union(&original);
        }
        self.selected_pieces = selected;
        self.highlights_dirty = true;
    }

    /// Revalidate rollback against current ownership, allowing complete local holds.
    pub(crate) fn restore_selection(&mut self, original: PieceBitSet, local_player: PlayerId) {
        self.selected_pieces = self.canonical_members(&original, |id| {
            self.is_valid_local_selection(id, local_player)
        });
        self.highlights_dirty = true;
    }

    /// One dispatch per bulk operation. Pointer frames keep changing only drag.delta.
    pub fn apply_command(
        &mut self,
        player: PlayerId,
        command: &PieceCommand,
        definition: Option<&PuzzleDefinition>,
        local_player: PlayerId,
    ) -> AppliedCommand {
        let definition =
            definition.filter(|d| d.piece_count() == self.len() && d.validate().is_ok());
        match command {
            PieceCommand::RotateDrag {
                members,
                delta,
                quarter_turns,
            } => definition
                .and_then(|d| {
                    self.rotate_local_drag(player, members, *delta, *quarter_turns, d, local_player)
                })
                .unwrap_or_default(),
            PieceCommand::Rotate {
                target,
                quarter_turns,
            } => definition
                .and_then(|d| self.rotate_target(target, *quarter_turns, d).ok())
                .map_or_else(AppliedCommand::default, |result| result.applied),
            PieceCommand::GrabGroup { members } if members.bit_len() == self.len() => {
                self.grab_components(player, members, local_player)
            }
            PieceCommand::ReleaseGroup { members, delta }
                if members.bit_len() == self.len() && delta.is_finite() =>
            {
                self.release_components(player, members, *delta, definition, local_player)
            }
            PieceCommand::Grab(id) if self.contains(*id) => {
                let plan = self.grab_roots([self.connectivity.minimum_member(*id)], None);
                self.apply_grab(player, plan, false, local_player)
            }
            PieceCommand::Release(id) if self.contains(*id) => self.release_roots(
                player,
                vec![self.connectivity.minimum_member(*id)],
                Vec2::ZERO,
                definition,
                local_player,
            ),
            PieceCommand::Move { id, position } if self.contains(*id) && position.is_finite() => {
                self.move_component(player, *id, *position, definition);
                AppliedCommand::default()
            }
            _ => AppliedCommand::default(),
        }
    }

    /// Protocol-only adapter for already resolved, unique component minima.
    /// Shares the relative-Z/compaction implementation without constructing a mask.
    pub(crate) fn grab_resolved_components(
        &mut self,
        player: PlayerId,
        minima: impl IntoIterator<Item = PieceId>,
        local_player: PlayerId,
    ) -> AppliedCommand {
        let plan = self.grab_roots(minima, None);
        self.apply_grab(player, plan, false, local_player)
    }

    /// Protocol-only bulk adapter. The caller has validated canonical membership
    /// and selectability. Reuse the existing temporary relative-Z sort/bulk path.
    pub(crate) fn grab_accepted_members(
        &mut self,
        player: PlayerId,
        members: &PieceBitSet,
        local_player: PlayerId,
    ) -> AppliedCommand {
        let plan = GrabPlan {
            ids: members.iter().collect(),
            members: Some(members.clone()),
        };
        self.apply_grab(player, plan, false, local_player)
    }

    /// Replica-only semantic apply after full consistency validation. Do not run
    /// authority acceptance again or silently omit any accepted component.
    pub(crate) fn grab_authority_components(
        &mut self,
        player: PlayerId,
        minima: impl IntoIterator<Item = PieceId>,
        local_player: PlayerId,
    ) -> AppliedCommand {
        let ids = minima
            .into_iter()
            .flat_map(|minimum| self.connectivity.iter_component(minimum))
            .collect();
        self.apply_grab(player, GrabPlan { ids, members: None }, false, local_player)
    }

    /// Full masks emit the minimum member once; partial masks deduplicate only
    /// roots whose minimum is absent. Small partial sets stay on the stack.
    fn grab_component_roots<'a>(
        &'a self,
        requested: &'a PieceBitSet,
        partial_roots: &'a mut PieceScratchSet,
    ) -> impl Iterator<Item = PieceId> + 'a {
        requested.iter().filter_map(|id| {
            if self.connectivity.component_size(id) == 1 {
                return Some(id);
            }
            let root = self.connectivity.minimum_member(id);
            (id == root || (!requested.contains(&root) && partial_roots.insert(root)))
                .then_some(root)
        })
    }

    fn grab_components(
        &mut self,
        player: PlayerId,
        requested: &PieceBitSet,
        local_player: PlayerId,
    ) -> AppliedCommand {
        let sync_drag =
            player == local_player && Arc::ptr_eq(&self.drag.members, requested.words());
        if requested.count() > self.len().div_ceil(32) {
            // Preserve the established dense path: canonical component validation,
            // shared all-valid mask, and ID-ordered input to the relative-Z sort.
            let accepted = self.selectable_members(requested);
            let plan = GrabPlan {
                ids: accepted.iter().collect(),
                members: Some(accepted),
            };
            return self.apply_grab(player, plan, sync_drag, local_player);
        }
        let mut partial_roots = PieceScratchSet::new(self.len());
        let roots = self.grab_component_roots(requested, &mut partial_roots);
        let plan = self.grab_roots(roots, sync_drag.then_some(requested));
        self.apply_grab(player, plan, sync_drag, local_player)
    }

    /// Validate every member before accepting any part of a component. Owner
    /// occupancy and the HELD mirror must both say unheld, even for this player.
    fn grab_roots(
        &self,
        roots: impl IntoIterator<Item = PieceId>,
        mask: Option<&PieceBitSet>,
    ) -> GrabPlan {
        let mut plan = GrabPlan {
            ids: Vec::new(),
            members: mask.cloned(),
        };
        for root in roots {
            if self.connectivity.component_size(root) == 1 {
                if self.is_selectable(root) {
                    plan.ids.push(root);
                } else if let Some(members) = &mut plan.members {
                    members.remove(&root);
                }
                continue;
            }
            if self
                .connectivity
                .iter_component(root)
                .all(|id| self.is_selectable(id))
            {
                for id in self.connectivity.iter_component(root) {
                    plan.ids.push(id);
                    if let Some(members) = &mut plan.members {
                        // Avoid COW when full membership already shares the mask.
                        if !members.contains(&id) {
                            members.insert(id);
                        }
                    }
                }
            } else if let Some(members) = &mut plan.members {
                for id in self.connectivity.iter_component(root) {
                    members.remove(&id);
                }
            }
        }
        plan
    }

    fn apply_grab(
        &mut self,
        player: PlayerId,
        plan: GrabPlan,
        sync_drag: bool,
        local_player: PlayerId,
    ) -> AppliedCommand {
        // Compile separate ID/word update loops: dense grabs should not pay
        // small-operation branches once per member. Scalar never needs a mask.
        if plan.ids.len() > self.len().div_ceil(32) && plan.members.is_some() {
            self.apply_grab_inner::<true>(player, plan, sync_drag, local_player)
        } else {
            self.apply_grab_inner::<false>(player, plan, sync_drag, local_player)
        }
    }

    fn apply_grab_inner<const BULK: bool>(
        &mut self,
        player: PlayerId,
        mut plan: GrabPlan,
        sync_drag: bool,
        local_player: PlayerId,
    ) -> AppliedCommand {
        let ids = &mut plan.ids;
        // One deterministic ordering across ALL accepted components, before compaction.
        ids.sort_unstable_by_key(|id| (self.states[id.0 as usize].z_order, *id));
        if ids.is_empty() {
            if sync_drag {
                self.drag = DragTransform::default();
            }
            return AppliedCommand::default();
        }
        if self.next_z_order.saturating_add(ids.len() as u32) > MAX_Z {
            self.compact_z();
        }
        self.held_by.ensure_len(self.len());
        let remote = player != local_player;
        let states = &mut *self.states;
        for (rank, &id) in ids.iter().enumerate() {
            let state = &mut states[id.0 as usize];
            state.flags |= HELD;
            state.z_order = self.next_z_order + rank as u32;
            self.held_by.owners[id.0 as usize] = player;
            if !BULK {
                self.held_by.occupied.insert(id);
                self.dirty_pieces.insert(id);
            }
            if remote {
                if self.selected_pieces.remove(&id) {
                    self.highlights_dirty = true;
                }
                if !BULK {
                    self.drag.remove(id);
                }
            }
        }
        if BULK {
            let members = plan.members.as_ref().unwrap();
            self.held_by.occupied.union(members);
            self.dirty_pieces.union(members);
            if remote {
                self.drag.exclude(members);
            }
        }
        *self.held_by.counts.entry(player).or_default() += ids.len();
        self.next_z_order += ids.len() as u32;
        if sync_drag {
            self.drag.members = plan.members.as_ref().unwrap().words().clone();
        }
        AppliedCommand {
            grabbed: ids.len(),
            ..default()
        }
    }

    fn move_component(
        &mut self,
        player: PlayerId,
        id: PieceId,
        position: Vec2,
        definition: Option<&PuzzleDefinition>,
    ) {
        if !self.connectivity.iter_component(id).all(|member| {
            self.states[member.0 as usize].flags & PLACED == 0
                && self.held_by.get(&member) == Some(&player)
        }) {
            return;
        }
        let delta = position - self.states[id.0 as usize].position;
        let rotation = decode_rotation(self.states[id.0 as usize].flags);
        let offset =
            definition.map(|d| position - rotate_quarter(d.correct_position(id), rotation));
        let translated = |member: PieceId| {
            if member == id {
                return position;
            }
            if let (Some(d), Some(offset)) = (definition, offset) {
                rotate_quarter(d.correct_position(member), rotation) + offset
            } else {
                self.states[member.0 as usize].position + delta
            }
        };
        if !self
            .connectivity
            .iter_component(id)
            .all(|member| translated(member).is_finite())
        {
            return;
        }
        if let Some(area) = definition
            .and_then(|d| jigsall_puzzle::placement::LogicalPlayArea::from_definition(d).ok())
        {
            let pivot = crate::play_area::component_center(
                self.connectivity.iter_component(id).map(translated),
            );
            if pivot.is_none_or(|p| !area.contains(p)) {
                return;
            }
        }
        let connectivity = &self.connectivity;
        let states = &mut *self.states;
        for member in connectivity.iter_component(id) {
            let position = if member == id {
                position
            } else if let (Some(d), Some(offset)) = (definition, offset) {
                rotate_quarter(d.correct_position(member), rotation) + offset
            } else {
                states[member.0 as usize].position + delta
            };
            if states[member.0 as usize].position != position {
                states[member.0 as usize].position = position;
                self.dirty_pieces.insert(member);
            }
        }
    }

    pub(crate) fn release_components(
        &mut self,
        player: PlayerId,
        requested: &PieceBitSet,
        delta: Vec2,
        definition: Option<&PuzzleDefinition>,
        local_player: PlayerId,
    ) -> AppliedCommand {
        let mut roots = Vec::new();
        let mut partial_roots = PieceScratchSet::new(self.len());
        for id in requested.iter() {
            let root = self.connectivity.minimum_member(id);
            // Full masks already contain the minimum: emit that member only.
            // Deduplication scratch is needed only for partial component masks.
            if id == root || (!requested.contains(&root) && partial_roots.insert(root)) {
                roots.push(root);
            }
        }
        roots.sort_unstable();
        self.release_roots(player, roots, delta, definition, local_player)
    }

    pub(crate) fn release_roots(
        &mut self,
        player: PlayerId,
        mut roots: Vec<PieceId>,
        delta: Vec2,
        definition: Option<&PuzzleDefinition>,
        local_player: PlayerId,
    ) -> AppliedCommand {
        roots.retain(|&root| {
            self.connectivity.iter_component(root).all(|id| {
                self.states[id.0 as usize].flags & PLACED == 0
                    && self.held_by.get(&id) == Some(&player)
            })
        });
        if roots.is_empty() {
            return AppliedCommand::default();
        }
        let area = definition
            .and_then(|d| jigsall_puzzle::placement::LogicalPlayArea::from_definition(d).ok());
        if let Some(area) = area {
            if !self.release_pivots_fit(roots.iter().copied(), delta, definition, area) {
                return AppliedCommand::default();
            }
        }
        let mut released = 0;
        let clear_drag = player == local_player && !self.drag.members.is_empty();
        // Commit ALL released translations before resolving any snap. A sibling
        // component in this same gesture is a target at its final release position.
        let geometry = definition.map(PuzzleDefinition::geometry);
        for &root in &roots {
            let connected = self.connectivity.component_size(root) > 1;
            let rotation = decode_rotation(self.states[root.0 as usize].flags);
            let offset = geometry.as_ref().filter(|_| connected).map(|d| {
                self.connected_release_offset(root, delta, d, area)
                    .expect("release preflight validated the normalized pivot")
            });
            let states = &mut *self.states;
            let translated = |id: PieceId, position: Vec2| {
                if connected {
                    if let (Some(d), Some(offset)) = (geometry.as_ref(), offset) {
                        return rotate_quarter(d.correct_position(id), rotation) + offset;
                    }
                }
                position + delta
            };
            // Overflow ignores the move for the WHOLE component, but releases its hold.
            let finite = self
                .connectivity
                .iter_component(root)
                .all(|id| translated(id, states[id.0 as usize].position).is_finite());
            for id in self.connectivity.iter_component(root) {
                let position = if finite {
                    translated(id, states[id.0 as usize].position)
                } else {
                    states[id.0 as usize].position
                };
                let state = &mut states[id.0 as usize];
                state.position = position;
                state.flags &= !HELD;
                self.held_by.occupied.remove(&id);
                self.dirty_pieces.insert(id);
                released += 1;
                if clear_drag {
                    self.drag.remove(id);
                }
            }
        }
        let count = self.held_by.counts.get_mut(&player).unwrap();
        *count -= released;
        if *count == 0 {
            self.held_by.counts.remove(&player);
        }
        let mut placed = 0;
        if let Some(definition) = definition {
            let mut scratch = snapping::SnapScratch::new(self.len(), definition);
            for root in roots {
                let current = self.connectivity.find_root(root);
                if self.states[root.0 as usize].flags & PLACED == 0
                    && !scratch.resolved.contains(&current)
                {
                    placed += self.resolve_component_snap(current, &mut scratch);
                }
            }
        }
        AppliedCommand {
            released,
            placed,
            ..default()
        }
    }

    /// Disconnect repairs even contradictory partial holds as a complete ownership unit.
    pub(crate) fn clear_player_holds(&mut self, player: PlayerId) -> Vec<PieceId> {
        let mut requested = PieceBitSet::new(self.len());
        requested.extend(
            self.held_by
                .iter()
                .filter_map(|(id, &owner)| (owner == player).then_some(id)),
        );
        let canonical = self.connectivity.expand(&requested);
        let ids: Vec<_> = canonical.iter().collect();
        let states = &mut *self.states;
        for &id in &ids {
            self.held_by.remove(&id);
            states[id.0 as usize].flags &= !HELD;
            self.dirty_pieces.insert(id);
        }
        self.drag.exclude(&canonical);
        ids
    }
}
// Fixtures exercise the production snap resolver independently of grab-time Z updates.
#[cfg(test)]
impl PieceDataStore {
    pub(crate) fn snap_fixture_component(&mut self, id: PieceId, definition: &PuzzleDefinition) {
        self.resolve_component_snap(id, &mut snapping::SnapScratch::new(self.len(), definition));
    }
}
#[cfg(test)]
#[path = "pieces/connected_tests.rs"]
mod connected_tests;
#[cfg(test)]
#[path = "pieces/local_identity_tests.rs"]
mod local_identity_tests;
#[derive(Clone, Default)]
pub struct UploadRange {
    pub start: u32,
    pub states: Vec<GpuPieceState>,
}
#[derive(Clone, Default)]
pub struct ComponentRootRange {
    pub start: u32,
    pub roots: Vec<u32>,
}
#[derive(Resource, Default, Clone)]
pub struct PieceUpload {
    pub root_revision: u64,
    pub initial_roots: Option<Arc<[u32]>>,
    pub root_ranges: Arc<[ComponentRootRange]>,
    pub selected: Arc<[u32]>,
    pub drag: DragTransform,
    pub epoch: u64,
    pub revision: u64,
    pub definition: Option<PuzzleDefinition>,
    pub initial: Option<Arc<[GpuPieceState]>>,
    pub ranges: Arc<[UploadRange]>,
}
pub fn prepare_piece_upload(
    mut store: ResMut<PieceDataStore>,
    mut upload: ResMut<PieceUpload>,
    definition: Option<Res<PuzzleDefinition>>,
) {
    // Idle frame: O(1) Arc sharing/count checks, independent of selected count.
    upload.drag = store.drag.clone();
    upload.selected = store.selected_pieces.words().clone();
    let exact_dirty_ranges = std::mem::take(&mut store.exact_dirty_ranges);
    store.sync_highlights();
    prepare_component_root_upload(&mut store, &mut upload);
    if store.epoch != upload.epoch {
        upload.epoch = store.epoch;
        upload.revision += 1;
        upload.definition = definition.as_deref().cloned();
        upload.initial = Some(store.states.clone().into());
        // A fresh GPU buffer starts canonical, then receives sparse overrides.
        let mut ids: Vec<_> = store.local_rotation.poses.keys().copied().collect();
        ids.sort_unstable();
        upload.ranges = presentation_ranges(&store, ids.into_iter());
        store.dirty_pieces.clear();
        return;
    }
    if upload.initial.is_some() || !store.dirty_pieces.is_empty() {
        upload.initial = None;
        upload.revision += 1;
        let count = store.len();
        // Reuse the mask allocation after range construction, even for small edits.
        let dirty = &store.dirty_pieces;
        let mut ranges: Vec<UploadRange> = Vec::new();
        if dirty.count() == count && count != 0 {
            // A dense bulk commit is one exact allocation/copy, not a million
            // pushes or individual upload records.
            upload.ranges = vec![UploadRange {
                start: 0,
                states: store.presentation_slice(0, count),
            }]
            .into();
            store.dirty_pieces.clear();
            return;
        }
        for id in dirty.iter() {
            if let Some(range) = ranges
                .last_mut()
                .filter(|r| r.start + r.states.len() as u32 == id.0)
            {
                range.states.push(store.presentation_state(id));
            } else {
                // Bound fragmented bulk updates too. After 128 separate spans,
                // upload their enclosing span (possibly including unchanged gaps).
                // Single/small edits still upload only exact dirty ranges.
                if ranges.len() == 128 && !exact_dirty_ranges {
                    let start = ranges[0].start;
                    let end = dirty.iter().last().unwrap().0;
                    ranges.clear();
                    ranges.push(UploadRange {
                        start,
                        states: store.presentation_slice(start as usize, end as usize + 1),
                    });
                    break;
                }
                ranges.push(UploadRange {
                    start: id.0,
                    states: vec![store.presentation_state(id)],
                });
            }
        }
        upload.ranges = ranges.into();
        store.dirty_pieces.clear();
    }
}

fn presentation_ranges(
    store: &PieceDataStore,
    ids: impl Iterator<Item = PieceId>,
) -> Arc<[UploadRange]> {
    let mut ranges: Vec<UploadRange> = Vec::new();
    for id in ids {
        if let Some(range) = ranges
            .last_mut()
            .filter(|r| r.start + r.states.len() as u32 == id.0)
        {
            range.states.push(store.presentation_state(id));
        } else {
            ranges.push(UploadRange {
                start: id.0,
                states: vec![store.presentation_state(id)],
            });
        }
    }
    ranges.into()
}

fn prepare_component_root_upload(store: &mut PieceDataStore, upload: &mut PieceUpload) {
    if store.epoch != upload.epoch {
        upload.root_revision += 1;
        upload.initial_roots = Some(
            (0..store.len() as u32)
                .map(|id| store.connectivity.find_root(PieceId(id)).0)
                .collect::<Vec<_>>()
                .into(),
        );
        upload.root_ranges = Arc::default();
        store.component_root_dirty.clear();
        return;
    }
    if upload.initial_roots.is_none() && store.component_root_dirty.is_empty() {
        return; // O(1) idle; no bitmap scan, root lookup or allocation.
    }
    upload.initial_roots = None;
    upload.root_revision += 1;
    let dirty = &store.component_root_dirty;
    let root = |id: PieceId| store.connectivity.find_root(id).0;
    let mut ranges: Vec<ComponentRootRange> = Vec::new();
    for id in dirty.iter() {
        if let Some(range) = ranges
            .last_mut()
            .filter(|r| r.start + r.roots.len() as u32 == id.0)
        {
            range.roots.push(root(id));
        } else {
            if ranges.len() == 128 {
                let start = ranges[0].start;
                let end = dirty.iter().last().unwrap().0;
                ranges.clear();
                ranges.push(ComponentRootRange {
                    start,
                    roots: (start..=end).map(|id| root(PieceId(id))).collect(),
                });
                break;
            }
            ranges.push(ComponentRootRange {
                start: id.0,
                roots: vec![root(id)],
            });
        }
    }
    upload.root_ranges = ranges.into();
    store.component_root_dirty.clear();
}
#[cfg(test)]
#[path = "pieces_tests.rs"]
mod bulk_tests;
#[cfg(test)]
mod tests {
    use super::*;
    use jigsall_core::GENERATOR_VERSION;

    fn definition(grid_size: UVec2, seed: u64) -> PuzzleDefinition {
        PuzzleDefinition {
            generator_version: GENERATOR_VERSION,
            seed,
            grid_size,
            image_size: UVec2::new(4096, 2048),
            snap_distance: 5.0,
            rotation_enabled: false,
        }
    }

    #[test]
    fn worker_states_preserve_seeded_positions_and_piece_order() {
        for grid in [
            UVec2::ONE,
            UVec2::new(40, 25),
            UVec2::new(1000, 1),
            UVec2::new(1, 1000),
        ] {
            for seed in [42, 42 | (1 << 63)] {
                let def = definition(grid, seed);
                let size = def.image_size.as_vec2() / grid.as_vec2();
                let mut expected = vec![Vec2::ZERO; def.piece_count()];
                jigsall_puzzle::placement::fill_placement_grid(
                    &mut expected,
                    size,
                    def.image_size.as_vec2(),
                    seed,
                    |p| p,
                );
                let states = DensePieceStates::generate(&def);
                assert_eq!(states.len(), expected.len());
                for (id, (state, position)) in states.iter().zip(expected).enumerate() {
                    assert_eq!(*state, GpuPieceState::new(position, PieceId(id as u32)));
                }
            }
        }
    }

    #[test]
    fn worker_random_rotation_is_reproducible_and_rectangular_slots_remain_disjoint() {
        use jigsall_puzzle::placement::{initial_rotation, LogicalPlayArea};
        for grid in [UVec2::new(40, 25), UVec2::new(1000, 1), UVec2::new(1, 1000)] {
            let mut def = definition(grid, 42);
            def.rotation_enabled = true;
            let states = DensePieceStates::generate(&def);
            assert_eq!(states, DensePieceStates::generate(&def));
            let size = def.image_size.as_vec2() / grid.as_vec2();
            let area = LogicalPlayArea::from_definition(&def).unwrap();
            let mut seen = [false; 4];
            for (index, state) in states.iter().enumerate() {
                let rotation = decode_rotation(state.flags);
                assert_eq!(rotation, initial_rotation(&def, PieceId(index as u32)));
                assert_eq!(state.z_order, index as u32);
                assert_eq!(state.flags & !jigsall_core::ROTATION_MASK, ENABLED);
                assert!(area.contains(state.position.as_dvec2()));
                seen[rotation as usize] = true;
                // The procedural quad includes every tab. Swapping axes at 90°
                // must keep it outside the board and disjoint from other quads.
                let half = if rotation & 1 == 0 {
                    size
                } else {
                    Vec2::new(size.y, size.x)
                } * 0.72;
                assert!((state.position.abs() - half)
                    .cmpgt(def.image_size.as_vec2() * 0.5)
                    .any());
                for other in &states[..index] {
                    let other_half = if decode_rotation(other.flags) & 1 == 0 {
                        size
                    } else {
                        Vec2::new(size.y, size.x)
                    } * 0.72;
                    assert!((state.position - other.position)
                        .abs()
                        .cmpgt(half + other_half)
                        .any());
                }
            }
            assert!(seen.into_iter().all(|v| v));
        }
    }

    #[test]
    fn million_dense_state_and_single_dirty_upload() {
        assert_eq!(std::mem::size_of::<GpuPieceState>(), 16);
        let mut app = App::new();
        app.init_resource::<PieceDataStore>()
            .init_resource::<PieceUpload>()
            .add_systems(Update, prepare_piece_upload);
        let states = DensePieceStates::generate(&definition(UVec2::splat(1000), 42));
        let allocation = states.as_ptr();
        app.world_mut()
            .resource_mut::<PieceDataStore>()
            .initialize_dense(states);
        assert_eq!(
            app.world().resource::<PieceDataStore>().states.as_ptr(),
            allocation
        );
        app.update();
        let roots = app
            .world()
            .resource::<PieceUpload>()
            .initial_roots
            .as_ref()
            .unwrap();
        assert_eq!(roots.len() * std::mem::size_of::<u32>(), 4_000_000);
        assert_eq!((roots[0], roots[999_999]), (0, 999_999));
        assert_eq!(
            app.world()
                .resource::<PieceUpload>()
                .initial
                .as_ref()
                .unwrap()
                .len(),
            1_000_000
        );
        assert_eq!(
            app.world()
                .resource::<PieceUpload>()
                .initial
                .as_ref()
                .unwrap()
                .as_ptr(),
            allocation,
        );
        // Extraction shares the same snapshot. The following frame releases it.
        let extracted = app.world().resource::<PieceUpload>().clone();
        app.update();
        assert!(app.world().resource::<PieceUpload>().initial.is_none());
        assert!(app
            .world()
            .resource::<PieceUpload>()
            .initial_roots
            .is_none());
        let root_revision = app.world().resource::<PieceUpload>().root_revision;
        drop(extracted);
        let mut store = app.world_mut().resource_mut::<PieceDataStore>();
        let id = PieceId(777777);
        let mut s = store.state(id).unwrap();
        s.position = Vec2::ONE;
        store.set_state(id, s, jigsall_core::LOCAL_PLAYER);
        store.bring_piece_to_front(id);
        assert_eq!(store.states[id.0 as usize].position, Vec2::ONE);
        assert!(store.states[id.0 as usize].z_order >= 1_000_000);
        assert_eq!(
            store.states.as_ptr(),
            allocation,
            "ordinary edits must not clone the initial state"
        );
        app.update();
        let upload = app.world().resource::<PieceUpload>();
        assert!(upload.initial.is_none());
        assert_eq!(upload.ranges.len(), 1);
        assert_eq!(upload.ranges[0].start, id.0);
        assert_eq!(upload.ranges[0].states.len(), 1);
        assert_eq!(
            upload.root_revision, root_revision,
            "singleton move never uploads roots"
        );
        assert!(upload.root_ranges.is_empty());
    }

    #[test]
    fn early_mutation_preserves_the_extracted_initial_snapshot() {
        let mut store = PieceDataStore::default();
        store.initialize_dense(DensePieceStates::generate(&definition(
            UVec2::new(2, 2),
            42,
        )));
        let extracted: Arc<[GpuPieceState]> = store.states.clone().into();
        let before = extracted[0];
        store.set_state(
            PieceId(0),
            PieceState {
                position: Vec2::ONE,
                placed: false,
                held_by: None,
            },
            jigsall_core::LOCAL_PLAYER,
        );
        assert_eq!(extracted[0], before);
        assert_eq!(store.states[0].position, Vec2::ONE);
        assert_ne!(store.states.as_ptr(), extracted.as_ptr());
    }
}
