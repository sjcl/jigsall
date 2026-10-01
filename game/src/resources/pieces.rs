use bevy::prelude::*;
use bytemuck::{Pod, Zeroable};
use puzzella_core::{PieceId, PieceState, PlayerId, PuzzleDefinition};
use std::{
    collections::{HashMap, HashSet},
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
static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);
/// Dense CPU authority: no per-piece definition, transform, handle or entity.
#[derive(Resource, Default)]
pub struct PieceDataStore {
    pub states: Vec<GpuPieceState>,
    pub held_by: HashMap<PieceId, PlayerId>,
    pub selected_pieces: HashSet<PieceId>,
    pub preview_pieces: HashSet<PieceId>,
    pub dirty_pieces: HashSet<PieceId>,
    pub placed_count: usize,
    pub next_z_order: u32,
    pub epoch: u64,
    pub highlights_dirty: bool,
    previous_selected: HashSet<PieceId>,
    previous_preview: HashSet<PieceId>,
}
impl PieceDataStore {
    pub fn initialize(&mut self, positions: Vec<Vec2>) {
        *self = Self::default();
        self.epoch = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
        self.states = positions
            .into_iter()
            .enumerate()
            .map(|(i, p)| GpuPieceState::new(p, PieceId(i as u32)))
            .collect();
        self.next_z_order = self.states.len() as u32;
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
            .chain(
                self.previous_preview
                    .symmetric_difference(&self.preview_pieces),
            )
            .copied()
            .collect();
        for id in ids {
            if let Some(s) = self.states.get_mut(id.0 as usize) {
                s.flags &= !(SELECTED | PREVIEW);
                if self.selected_pieces.contains(&id) {
                    s.flags |= SELECTED;
                }
                if self.preview_pieces.contains(&id) {
                    s.flags |= PREVIEW;
                }
                self.dirty_pieces.insert(id);
            }
        }
        self.previous_selected.clone_from(&self.selected_pieces);
        self.previous_preview.clone_from(&self.preview_pieces);
    }
}
#[derive(Clone, Default)]
pub struct UploadRange {
    pub start: u32,
    pub states: Vec<GpuPieceState>,
}
#[derive(Resource, Default, Clone)]
pub struct PieceUpload {
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
    #[test]
    fn million_dense_state_and_single_dirty_upload() {
        assert_eq!(std::mem::size_of::<GpuPieceState>(), 16);
        let mut app = App::new();
        app.init_resource::<PieceDataStore>()
            .init_resource::<PieceUpload>()
            .add_systems(Update, prepare_piece_upload);
        app.world_mut()
            .resource_mut::<PieceDataStore>()
            .initialize(vec![Vec2::ZERO; 1_000_000]);
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
        let mut store = app.world_mut().resource_mut::<PieceDataStore>();
        let id = PieceId(777777);
        let mut s = store.state(id).unwrap();
        s.position = Vec2::ONE;
        store.set_state(id, s);
        store.bring_piece_to_front(id);
        assert_eq!(store.states[id.0 as usize].position, Vec2::ONE);
        assert!(store.states[id.0 as usize].z_order >= 1_000_000);
        app.update();
        let upload = app.world().resource::<PieceUpload>();
        assert!(upload.initial.is_none());
        assert_eq!(upload.ranges.len(), 1);
        assert_eq!(upload.ranges[0].start, id.0);
        assert_eq!(upload.ranges[0].states.len(), 1);
    }
}
