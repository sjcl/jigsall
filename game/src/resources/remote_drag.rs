//! Reconstructible presentation cache. Never used by authority, saves or snapshots.
use bevy::prelude::*;
use puzzella_core::PieceBitSet;
#[cfg(test)]
use puzzella_core::PieceId;
use std::sync::Arc;

pub const REMOTE_DRAG_SLOTS: usize = 64;

/// Presentation tuning in real seconds/world units, independent of wire ticks.
pub(crate) struct RemoteDragSmoothing {
    pub time_constant_secs: f64,
    pub settle_secs: f64,
    pub settle_epsilon: f64,
}
pub(crate) const REMOTE_DRAG_SMOOTHING: RemoteDragSmoothing = RemoteDragSmoothing {
    time_constant_secs: 0.025,
    settle_secs: 0.250,
    settle_epsilon: 0.001,
};
// Zero means no remote translation; allocated slots are 1..=64.
#[derive(Resource)]
pub struct RemoteDragPresentation {
    pub(crate) epoch: u64,
    mapping: Vec<u32>,
    members: [Option<PieceBitSet>; REMOTE_DRAG_SLOTS],
    displayed_deltas: [[f32; 2]; REMOTE_DRAG_SLOTS],
    targets: [[f32; 2]; REMOTE_DRAG_SLOTS],
    smoothing_age: [f64; REMOTE_DRAG_SLOTS],
    smoothing_slots: u64,
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
            displayed_deltas: [[0.0; 2]; REMOTE_DRAG_SLOTS],
            targets: [[0.0; 2]; REMOTE_DRAG_SLOTS],
            smoothing_age: [0.0; REMOTE_DRAG_SLOTS],
            smoothing_slots: 0,
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
        self.displayed_deltas = [[0.0; 2]; REMOTE_DRAG_SLOTS];
        self.targets = [[0.0; 2]; REMOTE_DRAG_SLOTS];
        self.smoothing_age = [0.0; REMOTE_DRAG_SLOTS];
        self.smoothing_slots = 0;
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
        self.rebase(slot, delta);
        Some(slot)
    }
    /// Reliable initialization/rebase must never interpolate across canonical bases.
    pub(crate) fn rebase(&mut self, slot: u32, delta: Vec2) {
        let index = slot as usize - 1;
        if self.members[index].is_some() {
            self.targets[index] = delta.to_array();
            self.smoothing_age[index] = 0.0;
            self.smoothing_slots &= !(1u64 << index);
            if self.displayed_deltas[index] != delta.to_array() {
                self.displayed_deltas[index] = delta.to_array();
                self.delta_revision += 1;
            }
        }
    }
    /// Only accepted Transients reach here. No displayed/GPU or membership changes.
    pub(crate) fn set_target(&mut self, slot: u32, delta: Vec2) {
        let index = slot as usize - 1;
        if self.members[index].is_some() && self.targets[index] != delta.to_array() {
            self.targets[index] = delta.to_array();
            self.smoothing_age[index] = 0.0;
            if self.displayed_deltas[index] == self.targets[index] {
                self.smoothing_slots &= !(1u64 << index);
            } else {
                self.smoothing_slots |= 1u64 << index;
            }
        }
    }
    /// Bounded latest-target exponential convergence; never reads canonical pieces.
    pub(crate) fn advance(&mut self, dt: f64) {
        if self.smoothing_slots == 0 || dt.is_nan() || dt <= 0.0 {
            return;
        }
        let tuning = &REMOTE_DRAG_SMOOTHING;
        let alpha = -(-dt / tuning.time_constant_secs).exp_m1();
        let mut pending = self.smoothing_slots;
        let mut changed = false;
        while pending != 0 {
            let index = pending.trailing_zeros() as usize;
            pending &= pending - 1;
            self.smoothing_age[index] += dt;
            let target = self.targets[index];
            let previous = self.displayed_deltas[index];
            let mut displayed = target;
            if self.smoothing_age[index] < tuning.settle_secs {
                for axis in 0..2 {
                    // f64 intermediates also keep finite, very large f32 deltas safe.
                    let from = f64::from(previous[axis]);
                    let to = f64::from(target[axis]);
                    displayed[axis] =
                        (from + (to - from) * alpha).clamp(from.min(to), from.max(to)) as f32;
                }
                if (0..2).all(|axis| {
                    (f64::from(displayed[axis]) - f64::from(target[axis])).abs()
                        <= tuning.settle_epsilon
                }) {
                    displayed = target;
                }
            }
            if displayed == target {
                self.smoothing_slots &= !(1u64 << index);
                self.smoothing_age[index] = 0.0;
            }
            if displayed != previous {
                self.displayed_deltas[index] = displayed;
                changed = true;
            }
        }
        if changed {
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
            self.displayed_deltas[index] = [0.0; 2];
            self.targets[index] = [0.0; 2];
            self.smoothing_age[index] = 0.0;
            self.smoothing_slots &= !(1u64 << index);
            self.delta_revision += 1;
        }
    }
    #[cfg(test)]
    pub(crate) fn offset(&self, id: PieceId) -> Vec2 {
        let slot = self.mapping[id.0 as usize];
        if slot == 0 {
            Vec2::ZERO
        } else {
            Vec2::from_array(self.displayed_deltas[slot as usize - 1])
        }
    }
    #[cfg(test)]
    pub(crate) fn target(&self, id: PieceId) -> Vec2 {
        let slot = self.mapping[id.0 as usize];
        if slot == 0 {
            Vec2::ZERO
        } else {
            Vec2::from_array(self.targets[slot as usize - 1])
        }
    }
    fn prepare(&mut self, upload: &mut RemoteDragUpload) {
        upload.epoch = self.epoch;
        upload.deltas = self.displayed_deltas;
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
    time: Res<Time<Real>>,
    mut presentation: ResMut<RemoteDragPresentation>,
    mut upload: ResMut<RemoteDragUpload>,
) {
    presentation.synchronize(store.epoch, store.len());
    // Last runs after all network poll/command processing, before extraction.
    // Real time avoids virtual pause/slow-motion and is not a packet clock.
    presentation.advance(time.delta_secs_f64());
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
        let membership = state.members[0].as_ref().unwrap().words().clone();
        for i in 1..10_000 {
            let delta_revision = state.delta_revision;
            super::super::pieces::without_piece_state_access(|| {
                state.set_target(slot, Vec2::splat(i as f32));
                assert_eq!(state.delta_revision, delta_revision);
                state.advance(1.0 / 60.0);
                assert!(state.delta_revision > delta_revision);
                state.prepare(&mut upload);
            });
            assert_eq!(upload.revision, revision);
            assert!(Arc::ptr_eq(&ranges, &upload.ranges));
            assert_eq!(state.mapping.as_ptr(), mapping_ptr);
            assert_eq!(
                state.members[0].as_ref().unwrap().words().as_ptr(),
                membership_ptr
            );
            assert!(state.dirty.is_empty());
            assert!(Arc::ptr_eq(
                state.members[0].as_ref().unwrap().words(),
                &membership
            ));
        }
        state.advance(REMOTE_DRAG_SMOOTHING.settle_secs);
        assert_eq!(state.offset(PieceId(999_999)), Vec2::splat(9_999.0));
        let settled_revision = state.delta_revision;
        state.advance(1.0 / 60.0);
        state.prepare(&mut upload);
        assert_eq!(upload.delta_revision, settled_revision);
        assert_eq!(upload.revision, revision);
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
        state.set_target(first, Vec2::splat(999.0));
        state.advance(1.0 / 60.0);
        state.release(first);
        let reused = state
            .allocate(mask(1024, [1000]), Vec2::splat(17.0))
            .unwrap();
        assert_eq!(first, reused);
        state.prepare(&mut upload);
        assert_eq!(state.offset(PieceId(2)), Vec2::ZERO);
        assert_eq!(state.offset(PieceId(999)), Vec2::Y);
        assert_eq!(state.offset(PieceId(1000)), Vec2::splat(17.0));
        assert_eq!(state.target(PieceId(1000)), Vec2::splat(17.0));
        assert_eq!(state.smoothing_slots, 0);
        state.set_target(reused, Vec2::splat(-99.0));
        state.advance(1.0 / 60.0);
        state.release(second);
        state.synchronize(2, 1024);
        state.prepare(&mut upload);
        assert!(upload.ranges.is_empty());
        assert!(upload.initial.as_ref().unwrap().iter().all(|s| *s == 0));
        assert!(upload.deltas.iter().all(|d| *d == [0.0; 2]));
        assert_eq!(state.targets, [[0.0; 2]; REMOTE_DRAG_SLOTS]);
        assert_eq!(state.smoothing_slots, 0);
        assert_eq!(state.smoothing_age, [0.0; REMOTE_DRAG_SLOTS]);
        let slot = state.allocate(mask(1024, [0]), Vec2::ONE).unwrap();
        state.set_target(slot, Vec2::splat(50.0));
        state.reset(2, 1024); // teardown without changing puzzle epoch
        state.prepare(&mut upload);
        assert!(upload.initial.as_ref().unwrap().iter().all(|s| *s == 0));
        assert_eq!(state.smoothing_slots, 0);
        assert_eq!(state.targets, [[0.0; 2]; REMOTE_DRAG_SLOTS]);
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
            state.set_target(id + 1, Vec2::splat(100.0));
        }
        assert_eq!(state.smoothing_slots, u64::MAX);
        state.advance(1.0 / 60.0);
        for id in 0..64 {
            assert!(state.offset(PieceId(id)).x > id as f32);
            assert!(state.offset(PieceId(id)).x < 100.0);
        }
        state.advance(REMOTE_DRAG_SMOOTHING.settle_secs);
        assert_eq!(state.smoothing_slots, 0);
    }

    fn single(delta: Vec2) -> (RemoteDragPresentation, u32) {
        let mut state = RemoteDragPresentation::default();
        state.reset(1, 1);
        let slot = state.allocate(mask(1, [0]), delta).unwrap();
        (state, slot)
    }

    #[test]
    fn remote_smoothing_equal_elapsed_time_matches_across_frame_rates() {
        let initial = Vec2::new(-30.0, 20.0);
        let target = Vec2::new(100.0, -70.0);
        let elapsed = 0.080;
        let expected = initial
            + (target - initial)
                * (1.0 - (-elapsed / REMOTE_DRAG_SMOOTHING.time_constant_secs).exp()) as f32;
        for frames in [30, 60, 120] {
            let (mut state, slot) = single(initial);
            state.set_target(slot, target);
            for _ in 0..frames {
                state.advance(elapsed / f64::from(frames));
            }
            assert!(state.offset(PieceId(0)).abs_diff_eq(expected, 0.0001));
            assert!(state.offset(PieceId(0)).distance(target) < initial.distance(target) * 0.05);
            assert_eq!(state.target(PieceId(0)), target);
        }
    }

    #[test]
    fn remote_smoothing_latest_target_reverses_without_overshoot_or_prediction() {
        let (mut state, slot) = single(Vec2::ZERO);
        let revision = state.delta_revision;
        for target in [Vec2::splat(10.0), Vec2::splat(20.0), Vec2::new(30.0, -40.0)] {
            state.set_target(slot, target);
        }
        assert_eq!(state.offset(PieceId(0)), Vec2::ZERO);
        assert_eq!(state.target(PieceId(0)), Vec2::new(30.0, -40.0));
        assert_eq!(state.delta_revision, revision);
        for target in [Vec2::new(30.0, -40.0), Vec2::new(-20.0, 50.0), Vec2::ZERO] {
            state.set_target(slot, target);
            for _ in 0..40 {
                let previous = state.offset(PieceId(0));
                state.advance(1.0 / 120.0);
                let displayed = state.offset(PieceId(0));
                assert!(displayed.is_finite());
                for axis in 0..2 {
                    assert!(displayed[axis] >= previous[axis].min(target[axis]));
                    assert!(displayed[axis] <= previous[axis].max(target[axis]));
                }
            }
            assert_eq!(state.offset(PieceId(0)), target);
        }
        let revision = state.delta_revision;
        state.advance(1.0);
        state.set_target(slot, Vec2::ZERO);
        assert_eq!(state.delta_revision, revision);
        assert_eq!(state.smoothing_slots, 0);
    }

    #[test]
    fn remote_smoothing_exact_settle_hitch_and_invalid_frame_times() {
        let (mut state, slot) = single(Vec2::ZERO);
        state.set_target(slot, Vec2::splat(0.0005));
        state.advance(1.0 / 60.0);
        assert_eq!(state.offset(PieceId(0)), Vec2::splat(0.0005));
        assert_eq!(state.smoothing_slots, 0);
        for dt in [0.0, -1.0, f64::NAN] {
            state.set_target(slot, Vec2::ONE);
            let revision = state.delta_revision;
            state.advance(dt);
            assert_eq!(state.delta_revision, revision);
        }
        for target in [
            Vec2::splat(10_000.0),
            Vec2::splat(f32::MAX),
            Vec2::splat(-f32::MAX),
        ] {
            state.set_target(slot, target);
            for _ in 0..32 {
                // Repeated identical targets must not postpone exact settling.
                state.set_target(slot, target);
                state.advance(1.0 / 120.0);
                assert!(state.offset(PieceId(0)).is_finite());
            }
            assert_eq!(state.offset(PieceId(0)), target);
            assert_eq!(state.smoothing_slots, 0);
        }
        state.set_target(slot, Vec2::ONE);
        state.advance(30.0); // window stall / debugger pause
        assert_eq!(state.offset(PieceId(0)), Vec2::ONE);
        state.set_target(slot, Vec2::ZERO);
        state.advance(f64::INFINITY);
        assert_eq!(state.offset(PieceId(0)), Vec2::ZERO);
    }

    #[test]
    fn remote_smoothing_reliable_rebase_and_release_clear_pending_state() {
        let (mut state, slot) = single(Vec2::new(12.0, 34.0));
        assert_eq!(state.target(PieceId(0)), state.offset(PieceId(0)));
        state.set_target(slot, Vec2::new(50.0, 60.0));
        state.advance(1.0 / 60.0);
        state.rebase(slot, Vec2::new(-3.0, 5.0));
        assert_eq!(state.offset(PieceId(0)), Vec2::new(-3.0, 5.0));
        assert_eq!(state.target(PieceId(0)), state.offset(PieceId(0)));
        assert_eq!(state.smoothing_slots, 0);
        state.set_target(slot, Vec2::ONE);
        state.release(slot);
        let revision = state.delta_revision;
        state.set_target(slot, Vec2::splat(999.0));
        state.advance(1.0);
        assert_eq!(state.delta_revision, revision);
        assert_eq!(state.smoothing_slots, 0);
        assert_eq!(state.targets[0], [0.0; 2]);
        assert_eq!(state.displayed_deltas[0], [0.0; 2]);
        assert_eq!(state.smoothing_age[0], 0.0);
    }
}
