use bevy::prelude::*;
use bytemuck::{Pod, Zeroable};
use puzzella_core::{PieceId, PieceState, PlayerId, PuzzleDefinition};
use std::{
    collections::{HashMap, HashSet},
    mem::MaybeUninit,
    ops::{Deref, DerefMut},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

pub const PLACED: u32 = 1;
pub const SELECTED: u32 = 2;
pub const PREVIEW: u32 = 4;
pub const HELD: u32 = 8;
pub const ENABLED: u32 = 16;
pub const MAX_Z: u32 = (1 << 24) - 2;
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
/// Dense CPU authority: no per-piece definition, transform, handle or entity.
#[derive(Resource, Default)]
pub struct PieceDataStore {
    pub drag: DragTransform,
    pub states: DensePieceStates,
    pub held_by: HashMap<PieceId, PlayerId>,
    pub selected_pieces: HashSet<PieceId>,
    pub dirty_pieces: HashSet<PieceId>,
    pub placed_count: usize,
    pub next_z_order: u32,
    pub epoch: u64,
    pub highlights_dirty: bool,
    previous_selected: HashSet<PieceId>,
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
        self.next_z_order = self.states.len() as u32;
    }
    /// Rare snapshot restore. Reuses the local upload/readback lifecycle counter.
    pub(crate) fn replace_snapshot_states(
        &mut self,
        states: Vec<GpuPieceState>,
        next_z_order: u32,
    ) {
        *self = Self::default();
        self.epoch = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
        self.placed_count = states
            .iter()
            .filter(|state| state.flags & PLACED != 0)
            .count();
        self.states = states.into();
        self.next_z_order = next_z_order;
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
            self.held_by.insert(id, player);
        } else {
            self.held_by.remove(&id);
        }
        self.dirty_pieces.insert(id);
    }
    pub(crate) fn is_selectable(&self, id: PieceId) -> bool {
        self.states
            .get(id.0 as usize)
            .is_some_and(|s| s.flags & (PLACED | HELD) == 0 && s.flags & ENABLED != 0)
    }
    pub fn bring_piece_to_front(&mut self, id: PieceId) {
        if self.next_z_order >= MAX_Z {
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
        self.states[id.0 as usize].z_order = self.next_z_order;
        self.next_z_order += 1;
        self.dirty_pieces.insert(id);
    }
    pub fn sync_highlights(&mut self) {
        if !self.highlights_dirty {
            return;
        }
        self.highlights_dirty = false;
        let ids: HashSet<_> = self
            .previous_selected
            .symmetric_difference(&self.selected_pieces)
            .copied()
            .collect();
        for id in ids {
            if let Some(s) = self.states.get_mut(id.0 as usize) {
                s.flags &= !SELECTED;
                if self.selected_pieces.contains(&id) {
                    s.flags |= SELECTED;
                }
                self.dirty_pieces.insert(id);
            }
        }
        self.previous_selected.clone_from(&self.selected_pieces);
    }
}
#[derive(Clone, Default)]
pub struct UploadRange {
    pub start: u32,
    pub states: Vec<GpuPieceState>,
}
#[derive(Resource, Default, Clone)]
pub struct PieceUpload {
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
    upload.drag = store.drag.clone();
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
        let mut ids: Vec<_> = store.dirty_pieces.drain().collect();
        ids.sort_unstable();
        let mut ranges: Vec<UploadRange> = Vec::new();
        for id in ids {
            if let Some(range) = ranges
                .last_mut()
                .filter(|r| r.start + r.states.len() as u32 == id.0)
            {
                range.states.push(store.states[id.0 as usize]);
            } else {
                ranges.push(UploadRange {
                    start: id.0,
                    states: vec![store.states[id.0 as usize]],
                });
            }
        }
        upload.ranges = ranges.into();
    }
}
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
