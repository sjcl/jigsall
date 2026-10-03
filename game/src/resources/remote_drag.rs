//! Reconstructible presentation cache. Never used by authority, saves or snapshots.
use bevy::prelude::*;
use puzzella_core::PieceBitSet;
#[cfg(test)]
use puzzella_core::PieceId;
use std::sync::Arc;

pub const REMOTE_DRAG_SLOTS: usize = 64;
// Zero means no remote translation; allocated slots are 1..=64.
#[derive(Resource)]
pub struct RemoteDragPresentation {
    pub(crate) epoch: u64,
    mapping: Vec<u32>,
    members: [Option<PieceBitSet>; REMOTE_DRAG_SLOTS],
    deltas: [[f32; 2]; REMOTE_DRAG_SLOTS],
    dirty: PieceBitSet,
    dense_pending: bool,
    revision: u64,
    delta_revision: u64,
}
impl Default for RemoteDragPresentation {
    fn default() -> Self {
        Self {
            epoch: 0,
            mapping: vec![],
            members: std::array::from_fn(|_| None),
            deltas: [[0.0; 2]; REMOTE_DRAG_SLOTS],
            dirty: PieceBitSet::default(),
            dense_pending: false,
            revision: 0,
            delta_revision: 0,
        }
    }
}

impl RemoteDragPresentation {
    pub(crate) fn synchronize(&mut self, epoch: u64, count: usize) -> bool {
        if self.epoch == epoch && self.mapping.len() == count {
            return false;
        }
        self.reset(epoch, count);
        true
    }
    pub(crate) fn reset(&mut self, epoch: u64, count: usize) {
        self.epoch = epoch;
        self.mapping = vec![0; count];
        self.members = std::array::from_fn(|_| None);
        self.deltas = [[0.0; 2]; REMOTE_DRAG_SLOTS];
        self.dirty = PieceBitSet::new(count);
        self.dense_pending = true;
        self.delta_revision += 1;
    }
    pub(crate) fn allocate(&mut self, members: PieceBitSet, delta: Vec2) -> Option<u32> {
        if members.is_empty() {
            return None;
        }
        let index = self.members.iter().position(Option::is_none)?;
        let slot = index as u32 + 1;
        for id in members.iter() {
            self.mapping[id.0 as usize] = slot;
            self.dirty.insert(id);
        }
        self.members[index] = Some(members);
        self.set_delta(slot, delta);
        Some(slot)
    }
    // Only called after the protocol has authenticated and accepted the scalar.
    pub(crate) fn set_delta(&mut self, slot: u32, delta: Vec2) {
        let index = slot as usize - 1;
        if self.members[index].is_some() && self.deltas[index] != delta.to_array() {
            self.deltas[index] = delta.to_array();
            self.delta_revision += 1;
        }
    }
    pub(crate) fn release(&mut self, slot: u32) {
        let index = slot as usize - 1;
        if let Some(members) = self.members[index].take() {
            for id in members.iter() {
                self.mapping[id.0 as usize] = 0;
                self.dirty.insert(id);
            }
            self.deltas[index] = [0.0; 2];
            self.delta_revision += 1;
        }
    }
    #[cfg(test)]
    pub(crate) fn offset(&self, id: PieceId) -> Vec2 {
        let slot = self.mapping[id.0 as usize];
        if slot == 0 {
            Vec2::ZERO
        } else {
            Vec2::from_array(self.deltas[slot as usize - 1])
        }
    }
    fn prepare(&mut self, upload: &mut RemoteDragUpload) {
        upload.epoch = self.epoch;
        upload.deltas = self.deltas;
        upload.delta_revision = self.delta_revision;
        if self.dense_pending {
            self.dense_pending = false;
            self.revision += 1;
            upload.initial = Some(self.mapping.clone().into());
            upload.ranges = Arc::default();
            self.dirty.clear();
        } else if upload.initial.is_some() || !self.dirty.is_empty() {
            upload.initial = None;
            let mut ranges: Vec<RemoteSlotRange> = Vec::new();
            for id in self.dirty.iter() {
                if let Some(range) = ranges
                    .last_mut()
                    .filter(|r| r.start + r.slots.len() as u32 == id.0)
                {
                    range.slots.push(self.mapping[id.0 as usize]);
                } else {
                    if ranges.len() == 128 {
                        // Bound fragmented queue writes, including unchanged gaps.
                        let start = ranges[0].start;
                        let end = self.dirty.iter().last().unwrap().0 as usize + 1;
                        ranges = vec![RemoteSlotRange {
                            start,
                            slots: self.mapping[start as usize..end].to_vec(),
                        }];
                        break;
                    }
                    ranges.push(RemoteSlotRange {
                        start: id.0,
                        slots: vec![self.mapping[id.0 as usize]],
                    });
                }
            }
            upload.ranges = ranges.into();
            self.revision += 1;
            self.dirty.clear();
        }
        upload.revision = self.revision;
    }
}

#[derive(Clone)]
pub(crate) struct RemoteSlotRange {
    pub start: u32,
    pub slots: Vec<u32>,
}
#[derive(Resource, Clone)]
pub(crate) struct RemoteDragUpload {
    pub epoch: u64,
    pub revision: u64,
    pub initial: Option<Arc<[u32]>>,
    pub ranges: Arc<[RemoteSlotRange]>,
    pub deltas: [[f32; 2]; REMOTE_DRAG_SLOTS],
    pub delta_revision: u64,
}
impl Default for RemoteDragUpload {
    fn default() -> Self {
        Self {
            epoch: 0,
            revision: 0,
            initial: None,
            ranges: Arc::default(),
            deltas: [[0.0; 2]; REMOTE_DRAG_SLOTS],
            delta_revision: 0,
        }
    }
}
pub(crate) fn prepare_remote_drag_upload(
    store: Res<super::PieceDataStore>,
    mut presentation: ResMut<RemoteDragPresentation>,
    mut upload: ResMut<RemoteDragUpload>,
) {
    presentation.synchronize(store.epoch, store.len());
    presentation.prepare(&mut upload);
}

#[cfg(test)]
mod tests {
    use super::*;
    fn mask(count: usize, ids: impl IntoIterator<Item = u32>) -> PieceBitSet {
        let mut members = PieceBitSet::new(count);
        members.extend(ids.into_iter().map(PieceId));
        members
    }
    #[test]
    fn remote_drag_million_scalar_updates_leave_mapping_and_upload_ranges_untouched() {
        let mut state = RemoteDragPresentation::default();
        state.reset(1, 1_000_000);
        let mut upload = RemoteDragUpload::default();
        state.prepare(&mut upload);
        state.prepare(&mut upload);
        let slot = state
            .allocate(mask(1_000_000, 0..1_000_000), Vec2::ZERO)
            .unwrap();
        state.prepare(&mut upload);
        assert_eq!(upload.ranges.len(), 1);
        assert_eq!(upload.ranges[0].slots.len(), 1_000_000);
        let ranges = upload.ranges.clone();
        let revision = upload.revision;
        let mapping_ptr = state.mapping.as_ptr();
        let membership_ptr = state.members[0].as_ref().unwrap().words().as_ptr();
        for i in 1..100 {
            state.set_delta(slot, Vec2::splat(i as f32));
            state.prepare(&mut upload);
            assert_eq!(upload.revision, revision);
            assert!(Arc::ptr_eq(&ranges, &upload.ranges));
            assert_eq!(state.mapping.as_ptr(), mapping_ptr);
            assert_eq!(
                state.members[0].as_ref().unwrap().words().as_ptr(),
                membership_ptr
            );
            assert!(state.dirty.is_empty());
        }
        assert_eq!(state.offset(PieceId(999_999)), Vec2::splat(99.0));
        state.release(slot);
        state.prepare(&mut upload);
        assert!(upload.ranges[0].slots.iter().all(|s| *s == 0));
    }
    #[test]
    fn remote_drag_upload_bounds_fragmentation_and_clears_reused_slots_and_epochs() {
        let mut state = RemoteDragPresentation::default();
        state.reset(1, 1024);
        let mut upload = RemoteDragUpload::default();
        state.prepare(&mut upload);
        state.prepare(&mut upload);
        let first = state
            .allocate(mask(1024, (0..600).step_by(2)), Vec2::X)
            .unwrap();
        let second = state.allocate(mask(1024, [999]), Vec2::Y).unwrap();
        state.prepare(&mut upload);
        assert_eq!(upload.ranges.len(), 1);
        assert_eq!(state.offset(PieceId(2)), Vec2::X);
        assert_eq!(state.offset(PieceId(999)), Vec2::Y);
        state.release(first);
        let reused = state
            .allocate(mask(1024, [1000]), Vec2::splat(17.0))
            .unwrap();
        assert_eq!(first, reused);
        state.prepare(&mut upload);
        assert_eq!(state.offset(PieceId(2)), Vec2::ZERO);
        assert_eq!(state.offset(PieceId(999)), Vec2::Y);
        assert_eq!(state.offset(PieceId(1000)), Vec2::splat(17.0));
        state.release(second);
        state.synchronize(2, 1024);
        state.prepare(&mut upload);
        assert!(upload.ranges.is_empty());
        assert!(upload.initial.as_ref().unwrap().iter().all(|s| *s == 0));
        assert!(upload.deltas.iter().all(|d| *d == [0.0; 2]));
        state.reset(2, 1024); // teardown without changing puzzle epoch
        state.prepare(&mut upload);
        assert!(upload.initial.as_ref().unwrap().iter().all(|s| *s == 0));
    }
    #[test]
    fn remote_drag_slots_are_bounded_and_all_64_are_distinct() {
        let mut state = RemoteDragPresentation::default();
        state.reset(1, 65);
        for id in 0..64 {
            assert_eq!(
                state.allocate(mask(65, [id]), Vec2::splat(id as f32)),
                Some(id + 1)
            );
        }
        assert_eq!(state.allocate(mask(65, [64]), Vec2::ONE), None);
        for id in 0..64 {
            assert_eq!(state.offset(PieceId(id)), Vec2::splat(id as f32));
        }
    }
}
