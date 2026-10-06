//! Continuous pose residuals, independent of logical rotation and prediction ACKs.
//! Member work is confined to control boundaries. Frames only advance the clock.
use super::pieces::{local_rotation::PresentationPose, PieceDataStore, ENABLED, HELD, PLACED};
use bevy::prelude::*;
use bytemuck::{Pod, Zeroable};
use jigsall_core::{decode_rotation, rotate_quarter, PieceConnectivity, PieceId};
use std::{collections::HashMap, sync::Arc};

pub(crate) const ROTATION_SECONDS_PER_QUARTER: f64 = 0.120;
const QUARTER: f32 = std::f32::consts::FRAC_PI_2;

fn rotate(p: Vec2, angle: f32) -> Vec2 {
    let (s, c) = angle.sin_cos();
    Vec2::new(c * p.x - s * p.y, s * p.x + c * p.y)
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct RotationAnimation {
    pub pivot: Vec2,
    pub offset: Vec2,
    pub residual: f32,
    pub target_angle: f32,
    pub start: f64,
    pub duration: f64,
    pub start_elevation: f32,
}
impl RotationAnimation {
    pub fn progress(self, now: f64) -> f32 {
        ((now - self.start) / self.duration).clamp(0.0, 1.0) as f32
    }
    fn remaining(self, now: f64) -> f32 {
        let p = self.progress(now);
        1.0 - p * p * (3.0 - 2.0 * p)
    }
    pub fn angle(self, now: f64) -> f32 {
        self.residual * self.remaining(now)
    }
    /// Normalized visual lift only. Keep this polynomial in sync with
    /// rotation_elevation in presentation.wgsl; no gameplay height is implied.
    pub fn elevation(self, now: f64) -> f32 {
        let p = self.progress(now);
        if p >= 1.0 {
            return 0.0;
        }
        let peak = self
            .start_elevation
            .max((self.residual.abs() / QUARTER).clamp(0.0, 1.0));
        if p < 0.5 {
            let t = p * 2.0;
            let eased = t * t * (3.0 - 2.0 * t);
            self.start_elevation + (peak - self.start_elevation) * eased
        } else {
            let t = p * 2.0 - 1.0;
            peak * (1.0 - t * t * (3.0 - 2.0 * t))
        }
    }
    pub fn position(self, base: Vec2, now: f64) -> Vec2 {
        let remaining = self.remaining(now);
        if remaining == 0.0 {
            return base;
        }
        self.pivot + rotate(base - self.pivot, self.residual * remaining) + self.offset * remaining
    }
}

/// Extensible presentation record; no field belongs to the 16-byte piece state.
#[repr(C)]
#[derive(Clone, Copy, Default, Pod, Zeroable)]
pub(crate) struct GpuRotationAnimation {
    pub pivot: Vec2,
    pub offset: Vec2,
    pub residual: f32,
    pub start: f32,
    pub duration: f32,
    pub start_elevation: f32,
}
const _: () = assert!(std::mem::size_of::<GpuRotationAnimation>() == 32);

#[derive(Clone, Default)]
pub(crate) struct RotationSlotRange {
    pub start: u32,
    pub slots: Vec<u32>,
}

#[derive(Default)]
pub struct RotationVisual {
    pub(crate) clock: f64,
    origin: f64,
    active_until: f64,
    animations: HashMap<PieceId, RotationAnimation>,
    slots: HashMap<PieceId, u32>,
    dirty_slots: HashMap<PieceId, u32>,
    records: Arc<[GpuRotationAnimation]>,
    revision: u64,
}

#[derive(Clone, Copy)]
struct Before {
    pose: PresentationPose,
    delta: Vec2,
    displayed: Vec2,
    angle: f32,
    elevation: f32,
    size: usize,
    animation: Option<RotationAnimation>,
    intent: Option<(i32, Vec2)>,
}
pub(crate) struct RotationBoundary {
    epoch: u64,
    items: HashMap<PieceId, Before>,
}

impl PieceDataStore {
    fn visual_delta(&self, id: PieceId) -> Vec2 {
        if self.states[id.0 as usize].flags & HELD != 0
            && self
                .drag
                .members
                .get(id.0 as usize / 32)
                .is_some_and(|word| word & (1 << (id.0 % 32)) != 0)
        {
            self.drag.delta
        } else {
            Vec2::ZERO
        }
    }
    fn visual_before(&self, root: PieceId) -> Before {
        let state = self.presentation_state(root);
        let pose = PresentationPose {
            position: state.position,
            rotation: decode_rotation(state.flags),
        };
        let delta = self.visual_delta(root);
        let animation = self
            .rotation_visual
            .animations
            .get(&root)
            .copied()
            .filter(|a| self.rotation_visual.clock < a.start + a.duration);
        Before {
            pose,
            delta,
            displayed: animation.map_or(pose.position, |a| {
                a.position(pose.position, self.rotation_visual.clock)
            }) + delta,
            angle: animation.map_or(pose.rotation as f32 * QUARTER, |a| {
                a.target_angle + a.angle(self.rotation_visual.clock)
            }),
            elevation: animation.map_or(0.0, |a| a.elevation(self.rotation_visual.clock)),
            size: self.connectivity.component_size(root),
            animation,
            intent: None,
        }
    }
    /// Only call at a logical control boundary, never on idle/pointer frames.
    pub(crate) fn capture_rotation_boundary(&self) -> RotationBoundary {
        if self.rotation_visual.clock >= self.rotation_visual.active_until {
            return self.empty_rotation_boundary();
        }
        let items = self
            .rotation_visual
            .animations
            .keys()
            .copied()
            .filter(|&id| self.contains(id))
            .map(|id| (id, self.visual_before(id)))
            .collect();
        RotationBoundary {
            epoch: self.epoch,
            items,
        }
    }
    pub(crate) fn empty_rotation_boundary(&self) -> RotationBoundary {
        RotationBoundary {
            epoch: self.epoch,
            items: HashMap::new(),
        }
    }
    pub(crate) fn add_rotation_intent(
        &self,
        boundary: &mut RotationBoundary,
        root: PieceId,
        turns: i8,
        pivot: Vec2,
    ) {
        let before = boundary
            .items
            .entry(root)
            .or_insert_with(|| self.visual_before(root));
        let turns = i32::from(turns) + before.intent.map_or(0, |i| i.0);
        before.intent = Some((turns, pivot));
    }
    pub(crate) fn preserve_release_delta(
        &self,
        boundary: &mut RotationBoundary,
        members: &jigsall_core::PieceBitSet,
        delta: Vec2,
    ) {
        // Input ends the gesture before offline dispatch. Release still carries
        // the final pointer delta whose displayed basis must be preserved.
        for (&root, before) in &mut boundary.items {
            if members.contains(&root) {
                before.displayed += delta - before.delta;
                before.delta = delta;
            }
        }
    }
    /// Rebase final logical/predicted bases without feeding the visual pose back
    /// into the planner, pointer basis, protocol, snapshot or authority.
    pub(crate) fn finish_rotation_boundary(&mut self, boundary: RotationBoundary) {
        if boundary.epoch != self.epoch {
            return;
        }
        let now = self.rotation_visual.clock;
        let mut changed = false;
        for (root, before) in boundary.items {
            if !self.contains(root)
                || self.connectivity.minimum_member(root) != root
                || self.connectivity.component_size(root) != before.size
                || self.states[root.0 as usize].flags & (ENABLED | PLACED) != ENABLED
            {
                changed |= self.rotation_visual.animations.remove(&root).is_some();
                continue;
            }
            let state = self.presentation_state(root);
            let pose = PresentationPose {
                position: state.position,
                rotation: decode_rotation(state.flags),
            };
            let delta = self.visual_delta(root);
            let quarter_delta = (pose.rotation as i32 - before.pose.rotation as i32).rem_euclid(4);
            let intent = before.intent.filter(|(turns, _)| {
                turns.rem_euclid(4) == quarter_delta && turns.rem_euclid(4) != 0
            });
            if intent.is_none()
                && pose.rotation == before.pose.rotation
                && pose.position == before.pose.position
                && delta == before.delta
            {
                continue;
            }
            // No new accepted/predicted rotation and no running visual to correct.
            if intent.is_none() && before.animation.is_none() {
                continue;
            }
            let turns = intent.map_or_else(
                || {
                    if quarter_delta > 2 {
                        quarter_delta - 4
                    } else {
                        quarter_delta
                    }
                },
                |i| i.0,
            );
            let old_target = before
                .animation
                .map_or(before.pose.rotation as f32 * QUARTER, |a| a.target_angle);
            let target_angle = old_target + turns as f32 * QUARTER;
            let residual = before.angle - target_angle;
            let pivot = intent.map_or_else(
                || {
                    let a = before.animation.expect("running rebase");
                    pose.position
                        + rotate_quarter(a.pivot - before.pose.position, quarter_delta as u32)
                },
                |(_, pivot)| pivot - delta,
            );
            let offset =
                before.displayed - delta - (pivot + rotate(pose.position - pivot, residual));
            // ACK basis changes which leave the world-space final pose identical
            // translate the existing residual without restarting its easing/time.
            let animation = if intent.is_none()
                && pose.rotation == before.pose.rotation
                && (pose.position + delta).distance(before.pose.position + before.delta) < 0.0001
            {
                let mut a = before.animation.unwrap();
                a.pivot += before.delta - delta;
                a
            } else {
                RotationAnimation {
                    pivot,
                    offset,
                    residual,
                    target_angle,
                    start: now,
                    duration: ROTATION_SECONDS_PER_QUARTER
                        * if residual.abs() > f32::EPSILON {
                            f64::from((residual / QUARTER).abs()).max(0.001)
                        } else {
                            1.0 // Translation-only correction at a logical boundary.
                        },
                    start_elevation: before.elevation,
                }
            };
            self.rotation_visual.animations.insert(root, animation);
            changed = true;
        }
        if changed {
            self.rotation_visual.publish(&self.connectivity);
        }
    }
    pub(crate) fn clear_rotation_visual(&mut self) {
        if !self.rotation_visual.animations.is_empty() {
            self.rotation_visual.animations.clear();
            self.rotation_visual.publish(&self.connectivity);
        }
    }
}

impl RotationVisual {
    fn publish(&mut self, connectivity: &PieceConnectivity) {
        // O(animation/component count), only at explicit control boundaries.
        self.animations
            .retain(|_, a| self.clock < a.start + a.duration);
        self.origin = self.clock;
        self.active_until = self.clock;
        let mut roots: Vec<_> = self.animations.keys().copied().collect();
        roots.sort_unstable();
        let mut slots = HashMap::with_capacity(roots.len());
        let mut records = Vec::with_capacity(roots.len());
        for root in roots {
            let a = self.animations[&root];
            self.active_until = self.active_until.max(a.start + a.duration);
            // The GPU buffer stores DSU roots, which need not be stable minima.
            slots.insert(connectivity.find_root(root), records.len() as u32 + 1);
            records.push(GpuRotationAnimation {
                pivot: a.pivot,
                offset: a.offset,
                residual: a.residual,
                start: (a.start - self.origin) as f32,
                duration: a.duration as f32,
                start_elevation: a.start_elevation,
            });
        }
        for &root in self.slots.keys().chain(slots.keys()) {
            let slot = slots.get(&root).copied().unwrap_or(0);
            if self.slots.get(&root).copied().unwrap_or(0) != slot {
                self.dirty_slots.insert(root, slot);
            }
        }
        self.slots = slots;
        self.records = records.into();
        self.revision += 1;
    }
    #[cfg(test)]
    pub(crate) fn animation(&self, root: PieceId) -> Option<RotationAnimation> {
        self.animations.get(&root).copied()
    }
}

#[derive(Resource, Clone, Default)]
pub(crate) struct RotationVisualUpload {
    pub epoch: u64,
    pub revision: u64,
    pub records: Arc<[GpuRotationAnimation]>,
    pub ranges: Arc<[RotationSlotRange]>,
    pub time: f32,
    pub active: bool,
}

pub(crate) fn update_rotation_clock(time: Res<Time<Real>>, store: Option<ResMut<PieceDataStore>>) {
    if let Some(mut store) = store {
        store.rotation_visual.clock = time.elapsed_secs_f64();
    }
}

pub(crate) fn prepare_rotation_visual_upload(
    mut store: ResMut<PieceDataStore>,
    mut upload: ResMut<RotationVisualUpload>,
) {
    if upload.epoch != store.epoch {
        *upload = RotationVisualUpload {
            epoch: store.epoch,
            ..default()
        };
    }
    let visual = &mut store.rotation_visual;
    if upload.revision != visual.revision {
        let mut dirty: Vec<_> = visual.dirty_slots.drain().collect();
        dirty.sort_unstable_by_key(|(root, _)| *root);
        let mut ranges: Vec<RotationSlotRange> = Vec::new();
        for (root, slot) in dirty {
            if let Some(range) = ranges
                .last_mut()
                .filter(|r| r.start + r.slots.len() as u32 == root.0)
            {
                range.slots.push(slot);
            } else {
                ranges.push(RotationSlotRange {
                    start: root.0,
                    slots: vec![slot],
                });
            }
        }
        upload.records = visual.records.clone();
        upload.ranges = ranges.into();
        upload.revision = visual.revision;
    }
    upload.time = (visual.clock - visual.origin) as f32;
    upload.active = visual.clock < visual.active_until;
}

#[cfg(test)]
#[path = "rotation_visual_tests.rs"]
mod tests;
