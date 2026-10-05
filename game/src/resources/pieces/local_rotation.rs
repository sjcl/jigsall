//! Sparse, presentation-only pose overrides. Never used by authority or capture.
use super::*;

#[derive(Clone, Copy, Debug)]
pub(crate) struct PresentationPose {
    pub position: Vec2,
    pub rotation: u32,
}

pub(crate) struct PredictionDrag<'a> {
    pub members: &'a PieceBitSet,
    pub player: PlayerId,
    pub optimistic_grab: bool,
}

#[derive(Default)]
pub struct LocalRotationPresentation {
    pub(crate) poses: HashMap<PieceId, PresentationPose>,
}

impl PieceDataStore {
    /// Compose only pose bits; ownership, Z and every other flag stay canonical.
    pub(crate) fn presentation_state(&self, id: PieceId) -> GpuPieceState {
        let mut state = self.states[id.0 as usize];
        if !self.local_rotation.poses.is_empty() {
            if let Some(pose) = self.local_rotation.poses.get(&id) {
                state.position = pose.position;
                state.flags = with_rotation(state.flags, pose.rotation);
            }
        }
        state
    }

    pub(crate) fn presentation_slice(&self, start: usize, end: usize) -> Vec<GpuPieceState> {
        if self.local_rotation.poses.is_empty() {
            // Preserve the existing bulk memcpy path without prediction.
            self.states[start..end].to_vec()
        } else {
            (start..end)
                .map(|id| self.presentation_state(PieceId(id as u32)))
                .collect()
        }
    }

    /// Explicit control boundary only. Dirty both old and new membership so
    /// retired/rejected overrides restore canonical GPU state in the same upload.
    pub(crate) fn set_local_rotation(&mut self, poses: HashMap<PieceId, PresentationPose>) {
        let visual = self.capture_rotation_boundary();
        for &id in self.local_rotation.poses.keys().chain(poses.keys()) {
            self.dirty_pieces.insert(id);
        }
        if !self.local_rotation.poses.is_empty() || !poses.is_empty() {
            self.exact_dirty_ranges = true;
        }
        self.local_rotation.poses = poses;
        self.finish_rotation_boundary(visual);
    }

    pub(crate) fn clear_local_rotation(&mut self) {
        if !self.local_rotation.poses.is_empty() {
            self.set_local_rotation(HashMap::new());
        }
    }
}
