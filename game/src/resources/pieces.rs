use bevy::prelude::*;
use bytemuck::{Pod, Zeroable};
use puzzella_core::{
    PieceBitSet, PieceCommand, PieceConnectivity, PieceId, PieceScratchSet, PieceState, PlayerId,
    PuzzleDefinition,
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
pub const MAX_Z: u32 = (1 << 24) - 2;
mod snapping;
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct GpuPieceState {
    pub position: Vec2,
    pub z_order: u32,
    pub flags: u32,
}
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

impl DensePieceStates {
    /// Build the final 16-byte states directly in their worker-owned allocation.
    pub fn generate(definition: &PuzzleDefinition) -> Self {
        let mut states = Arc::<[GpuPieceState]>::new_uninit_slice(definition.piece_count());
        let display_size = definition.image_size.as_vec2();
        puzzella_puzzle::placement::fill_placement_grid(
            Arc::get_mut(&mut states).unwrap(),
            display_size / definition.grid_size.as_vec2(),
            display_size,
            definition.seed,
            |position| MaybeUninit::new(GpuPieceState::new(position, PieceId(0))),
        );
        // SAFETY: fill_placement_grid writes every slot before shuffling it.
        // GpuPieceState has no drop glue; a panic before this point is also safe.
        let mut states = unsafe { states.assume_init() };
        for (id, state) in Arc::get_mut(&mut states).unwrap().iter_mut().enumerate() {
            state.z_order = id as u32;
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
    fn ensure_len(&mut self, count: usize) {
        assert!(count <= puzzella_core::MAX_PIECES);
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
}
/// Dense CPU authority: no per-piece definition, transform, handle or entity.
#[derive(Resource, Default)]
pub struct PieceDataStore {
    pub connectivity: PieceConnectivity,
    pub drag: DragTransform,
    pub states: DensePieceStates,
    pub held_by: PieceOwners,
    pub selected_pieces: PieceBitSet,
    pub dirty_pieces: PieceBitSet,
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
        *self = Self::default();
        self.epoch = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
        self.states = states;
        self.connectivity = PieceConnectivity::new(self.len());
        self.next_z_order = self.states.len() as u32;
        self.selected_pieces = PieceBitSet::new(self.len());
        self.dirty_pieces = PieceBitSet::new(self.len());
    }
    /// Rare snapshot restore. Reuses the local upload/readback lifecycle counter.
    pub(crate) fn replace_snapshot_states(
        &mut self,
        states: Vec<GpuPieceState>,
        next_z_order: u32,
        connectivity: PieceConnectivity,
    ) {
        *self = Self::default();
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
    }
    pub fn len(&self) -> usize {
        self.states.len()
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
    pub fn set_state(&mut self, id: PieceId, state: PieceState) {
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
        if state.held_by != Some(puzzella_core::LOCAL_PLAYER) {
            self.drag.remove(id);
        }
        // This setter is singleton-only; connected authority changes use bulk commands.
        if !self.is_valid_local_selection(id) && self.selected_pieces.remove(&id) {
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
    pub(crate) fn is_valid_local_selection(&self, id: PieceId) -> bool {
        self.states
            .get(id.0 as usize)
            .is_some_and(|s| s.flags & PLACED == 0 && s.flags & ENABLED != 0)
            && self
                .held_by
                .get(&id)
                .is_none_or(|owner| *owner == puzzella_core::LOCAL_PLAYER)
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
    pub fn commit_selection(&mut self, members: PieceBitSet, original: Option<&PieceBitSet>) {
        let mut selected = self.selectable_members(&members);
        if let Some(original) = original {
            let original = self.canonical_members(original, |id| self.is_valid_local_selection(id));
            selected.union(&original);
        }
        self.selected_pieces = selected;
        self.highlights_dirty = true;
    }

    /// Revalidate rollback against current ownership, allowing complete local holds.
    pub(crate) fn restore_selection(&mut self, original: PieceBitSet) {
        self.selected_pieces =
            self.canonical_members(&original, |id| self.is_valid_local_selection(id));
        self.highlights_dirty = true;
    }

    /// One dispatch per bulk operation. Pointer frames keep changing only drag.delta.
    pub fn apply_command(
        &mut self,
        player: PlayerId,
        command: &PieceCommand,
        definition: Option<&PuzzleDefinition>,
    ) -> AppliedCommand {
        let definition =
            definition.filter(|d| d.piece_count() == self.len() && d.validate().is_ok());
        match command {
            PieceCommand::GrabGroup { members } if members.bit_len() == self.len() => {
                self.grab_components(player, members)
            }
            PieceCommand::ReleaseGroup { members, delta }
                if members.bit_len() == self.len() && delta.is_finite() =>
            {
                self.release_components(player, members, *delta, definition)
            }
            PieceCommand::Grab(id) if self.contains(*id) => {
                let mut members = PieceBitSet::new(self.len());
                members.insert(*id);
                self.grab_components(player, &members)
            }
            PieceCommand::Release(id) if self.contains(*id) => self.release_roots(
                player,
                vec![self.connectivity.minimum_member(*id)],
                Vec2::ZERO,
                definition,
            ),
            PieceCommand::Move { id, position } if self.contains(*id) && position.is_finite() => {
                self.move_component(player, *id, *position, definition);
                AppliedCommand::default()
            }
            _ => AppliedCommand::default(),
        }
    }

    fn grab_components(&mut self, player: PlayerId, requested: &PieceBitSet) -> AppliedCommand {
        let accepted = self.selectable_members(requested);
        // Only this transition allocates sorted IDs; preserve relative Z across components too.
        let mut ids: Vec<_> = accepted.iter().collect();
        ids.sort_unstable_by_key(|id| (self.states[id.0 as usize].z_order, *id));
        if ids.is_empty() {
            if player == puzzella_core::LOCAL_PLAYER
                && Arc::ptr_eq(&self.drag.members, requested.words())
            {
                self.drag = DragTransform::default();
            }
            return AppliedCommand::default();
        }
        if self.next_z_order.saturating_add(ids.len() as u32) > MAX_Z {
            self.compact_z();
        }
        self.held_by.ensure_len(self.len());
        let states = &mut *self.states;
        for (rank, &id) in ids.iter().enumerate() {
            let state = &mut states[id.0 as usize];
            state.flags |= HELD;
            state.z_order = self.next_z_order + rank as u32;
            self.held_by.owners[id.0 as usize] = player;
            if player != puzzella_core::LOCAL_PLAYER && self.selected_pieces.remove(&id) {
                self.highlights_dirty = true;
            }
        }
        self.held_by.occupied.union(&accepted);
        *self.held_by.counts.entry(player).or_default() += ids.len();
        self.dirty_pieces.union(&accepted);
        self.next_z_order += ids.len() as u32;
        if player == puzzella_core::LOCAL_PLAYER
            && Arc::ptr_eq(&self.drag.members, requested.words())
        {
            self.drag.members = accepted.words().clone();
        } else if player != puzzella_core::LOCAL_PLAYER {
            self.drag.exclude(&accepted);
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
        let offset = definition.map(|d| position - d.correct_position(id));
        let translated = |member: PieceId| {
            if member == id {
                return position;
            }
            if let (Some(d), Some(offset)) = (definition, offset) {
                d.correct_position(member) + offset
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
        let connectivity = &self.connectivity;
        let states = &mut *self.states;
        for member in connectivity.iter_component(id) {
            let position = if member == id {
                position
            } else if let (Some(d), Some(offset)) = (definition, offset) {
                d.correct_position(member) + offset
            } else {
                states[member.0 as usize].position + delta
            };
            if states[member.0 as usize].position != position {
                states[member.0 as usize].position = position;
                self.dirty_pieces.insert(member);
            }
        }
    }

    fn release_components(
        &mut self,
        player: PlayerId,
        requested: &PieceBitSet,
        delta: Vec2,
        definition: Option<&PuzzleDefinition>,
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
        self.release_roots(player, roots, delta, definition)
    }

    fn release_roots(
        &mut self,
        player: PlayerId,
        mut roots: Vec<PieceId>,
        delta: Vec2,
        definition: Option<&PuzzleDefinition>,
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
        let mut released = 0;
        let clear_drag = player == puzzella_core::LOCAL_PLAYER && !self.drag.members.is_empty();
        // Commit ALL released translations before resolving any snap. A sibling
        // component in this same gesture is a target at its final release position.
        let geometry = definition.map(PuzzleDefinition::geometry);
        let states = &mut *self.states;
        for &root in &roots {
            let connected = self.connectivity.component_size(root) > 1;
            let offset = geometry
                .as_ref()
                .filter(|_| connected)
                .map(|d| states[root.0 as usize].position - d.correct_position(root) + delta);
            let translated = |id: PieceId, position: Vec2| {
                if connected {
                    if let (Some(d), Some(offset)) = (geometry.as_ref(), offset) {
                        return d.correct_position(id) + offset;
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

    /// Compatibility release notification producers use the same component resolver.
    pub(crate) fn snap_unheld_component(&mut self, id: PieceId, definition: &PuzzleDefinition) {
        if self.contains(id)
            && self.connectivity.iter_component(id).all(|member| {
                self.states[member.0 as usize].flags & PLACED == 0
                    && self.held_by.get(&member).is_none()
            })
        {
            self.resolve_component_snap(
                id,
                &mut snapping::SnapScratch::new(self.len(), definition),
            );
        }
    }
}
#[cfg(test)]
#[path = "pieces/connected_tests.rs"]
mod connected_tests;
#[derive(Clone, Default)]
pub struct UploadRange {
    pub start: u32,
    pub states: Vec<GpuPieceState>,
}
#[derive(Resource, Default, Clone)]
pub struct PieceUpload {
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
    store.sync_highlights();
    if store.epoch != upload.epoch {
        upload.epoch = store.epoch;
        upload.revision += 1;
        upload.definition = definition.as_deref().cloned();
        upload.initial = Some(store.states.clone().into());
        upload.ranges = Arc::default();
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
                states: store.states.to_vec(),
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
                range.states.push(store.states[id.0 as usize]);
            } else {
                // Bound fragmented bulk updates too. After 128 separate spans,
                // upload their enclosing span (possibly including unchanged gaps).
                // Single/small edits still upload only exact dirty ranges.
                if ranges.len() == 128 {
                    let start = ranges[0].start;
                    let end = dirty.iter().last().unwrap().0;
                    ranges.clear();
                    ranges.push(UploadRange {
                        start,
                        states: store.states[start as usize..=end as usize].to_vec(),
                    });
                    break;
                }
                ranges.push(UploadRange {
                    start: id.0,
                    states: vec![store.states[id.0 as usize]],
                });
            }
        }
        upload.ranges = ranges.into();
        store.dirty_pieces.clear();
    }
}
#[cfg(test)]
#[path = "pieces_tests.rs"]
mod bulk_tests;
#[cfg(test)]
mod tests {
    use super::*;
    use puzzella_core::GENERATOR_VERSION;

    fn definition(grid_size: UVec2, seed: u64) -> PuzzleDefinition {
        PuzzleDefinition {
            generator_version: GENERATOR_VERSION,
            seed,
            grid_size,
            image_size: UVec2::new(4096, 2048),
            snap_distance: 5.0,
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
                let expected = puzzella_puzzle::placement::generate_placement_grid(
                    grid.x as usize,
                    grid.y as usize,
                    size.x,
                    size.y,
                    def.image_size.x as f32,
                    def.image_size.y as f32,
                    seed,
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
        drop(extracted);
        let mut store = app.world_mut().resource_mut::<PieceDataStore>();
        let id = PieceId(777777);
        let mut s = store.state(id).unwrap();
        s.position = Vec2::ONE;
        store.set_state(id, s);
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
        store.set_state(PieceId(0), PieceState::new(Vec2::ONE));
        assert_eq!(extracted[0], before);
        assert_eq!(store.states[0].position, Vec2::ONE);
        assert_ne!(store.states.as_ptr(), extracted.as_ptr());
    }
}
