//! Reconstructible interaction feedback; never part of gameplay or wire state.
use super::{remote_drag::RemoteDragPresentation, PieceDataStore};
use bevy::prelude::*;
use bytemuck::{Pod, Zeroable};
use std::{collections::BTreeMap, sync::Arc};

/// Real-time smoothstep duration for both grab and release.
pub(crate) const DRAG_ELEVATION_SECONDS: f64 = 0.080;

#[derive(Clone, Copy, Default, Pod, Zeroable)]
#[repr(C)]
pub(crate) struct GpuDragElevation {
    pub from: f32,
    pub to: f32,
    pub start: f32,
    pub duration: f32,
}

struct Envelope {
    members: Arc<[u32]>,
    from: f32,
    to: f32,
    start: f64,
}
impl Envelope {
    fn value(&self, now: f64) -> f32 {
        let t = ((now - self.start) / DRAG_ELEVATION_SECONDS).clamp(0.0, 1.0) as f32;
        self.from + (self.to - self.from) * t * t * (3.0 - 2.0 * t)
    }
}

#[derive(Clone)]
pub(crate) struct DragElevationRange {
    pub start: u32,
    pub slots: Vec<u32>,
}
#[derive(Resource, Clone, Default)]
pub(crate) struct DragElevationUpload {
    pub epoch: u64,
    pub revision: u64,
    pub records: Arc<[GpuDragElevation]>,
    pub ranges: Arc<[DragElevationRange]>,
    pub time: f32,
    pub active: bool,
}

#[derive(Resource, Default)]
pub(crate) struct DragElevationPresentation {
    epoch: u64,
    count: usize,
    local: Arc<[u32]>,
    remote_version: Option<Arc<()>>,
    sources: Vec<Option<u32>>,
    mapping: Vec<u32>,
    records: Vec<Envelope>,
    free: Vec<u32>,
    fades: Vec<(u32, f64)>,
    next_expiry: f64,
    held: usize,
    origin: f64,
    dirty: BTreeMap<u32, u32>,
}

// Only used at membership/control boundaries, never in ordinary pointer frames.
fn members(words: &[u32]) -> impl Iterator<Item = u32> + '_ {
    words.iter().enumerate().flat_map(|(index, &word)| {
        let mut remaining = word;
        std::iter::from_fn(move || {
            if remaining == 0 {
                return None;
            }
            let bit = remaining.trailing_zeros();
            remaining &= remaining - 1;
            Some(index as u32 * 32 + bit)
        })
    })
}

impl DragElevationPresentation {
    fn change_source(&mut self, source: usize, mask: Option<Arc<[u32]>>, now: f64) -> bool {
        if let Some(slot) = self.sources[source] {
            if mask
                .as_ref()
                .is_some_and(|mask| Arc::ptr_eq(mask, &self.records[slot as usize - 1].members))
            {
                return false;
            }
            let record = &mut self.records[slot as usize - 1];
            record.from = record.value(now);
            record.to = 0.0;
            record.start = now;
            self.fades.push((slot, now + DRAG_ELEVATION_SECONDS));
            self.held -= 1;
            self.sources[source] = None;
        } else if mask.is_none() {
            return false;
        }
        if let Some(mask) = mask {
            if self.mapping.is_empty() {
                self.mapping.resize(self.count, 0);
            }
            // Regrabbing an envelope in flight starts at its displayed value.
            // One common start value also keeps every component member together.
            let from = members(&mask)
                .map(|id| self.value(id, now))
                .fold(0.0, f32::max);
            let slot = if let Some(slot) = self.free.pop() {
                for id in members(&self.records[slot as usize - 1].members) {
                    if self.mapping[id as usize] == slot {
                        self.mapping[id as usize] = 0;
                        self.dirty.insert(id, 0);
                    }
                }
                self.records[slot as usize - 1] = Envelope {
                    members: mask.clone(),
                    from,
                    to: 1.0,
                    start: now,
                };
                slot
            } else {
                self.records.push(Envelope {
                    members: mask.clone(),
                    from,
                    to: 1.0,
                    start: now,
                });
                self.records.len() as u32
            };
            for id in members(&mask) {
                self.mapping[id as usize] = slot;
                self.dirty.insert(id, slot);
            }
            self.sources[source] = Some(slot);
            self.held += 1;
        }
        true
    }

    fn value(&self, id: u32, now: f64) -> f32 {
        let slot = self.mapping.get(id as usize).copied().unwrap_or(0);
        if slot == 0 {
            0.0
        } else {
            self.records[slot as usize - 1].value(now)
        }
    }

    fn expire(&mut self, now: f64) -> bool {
        if self.fades.is_empty() || now < self.next_expiry {
            return false;
        }
        self.fades.retain(|&(slot, end)| {
            if now < end {
                return true;
            }
            let record = &mut self.records[slot as usize - 1];
            record.from = 0.0;
            record.to = 0.0;
            self.free.push(slot);
            false
        });
        // Stale mappings safely point to zero records. Clear them only on slot
        // reuse, avoiding a second component walk when a fade finishes.
        true
    }

    fn publish(&mut self, now: f64, upload: &mut DragElevationUpload) {
        self.origin = now;
        self.next_expiry = self
            .fades
            .iter()
            .map(|&(_, end)| end)
            .fold(f64::INFINITY, f64::min);
        upload.records = self
            .records
            .iter()
            .map(|record| GpuDragElevation {
                from: record.from,
                to: record.to,
                start: (record.start - now) as f32,
                duration: DRAG_ELEVATION_SECONDS as f32,
            })
            .collect();
        let mut ranges: Vec<DragElevationRange> = Vec::new();
        for (&id, &slot) in &self.dirty {
            if let Some(range) = ranges
                .last_mut()
                .filter(|r| r.start + r.slots.len() as u32 == id)
            {
                range.slots.push(slot);
            } else {
                if ranges.len() == 128 {
                    let start = ranges[0].start;
                    let end = *self.dirty.last_key_value().unwrap().0 as usize + 1;
                    ranges = vec![DragElevationRange {
                        start,
                        slots: self.mapping[start as usize..end].to_vec(),
                    }];
                    break;
                }
                ranges.push(DragElevationRange {
                    start: id,
                    slots: vec![slot],
                });
            }
        }
        self.dirty.clear();
        upload.ranges = ranges.into();
        upload.revision += 1;
    }

    fn prepare(
        &mut self,
        store: &PieceDataStore,
        remote: &RemoteDragPresentation,
        upload: &mut DragElevationUpload,
    ) {
        let now = store.rotation_visual.clock;
        if self.epoch != store.epoch || self.sources.is_empty() {
            *self = Self {
                epoch: store.epoch,
                count: store.len(),
                sources: vec![None; 1 + super::remote_drag::REMOTE_DRAG_SLOTS],
                next_expiry: f64::INFINITY,
                ..default()
            };
            *upload = DragElevationUpload {
                epoch: store.epoch,
                ..default()
            };
        }
        let mut changed = self.expire(now);
        if !Arc::ptr_eq(&self.local, &store.drag.members) {
            self.local = store.drag.members.clone();
            let mask = self
                .local
                .iter()
                .any(|&word| word != 0)
                .then(|| self.local.clone());
            changed |= self.change_source(0, mask, now);
        }
        if !self
            .remote_version
            .as_ref()
            .is_some_and(|version| Arc::ptr_eq(version, remote.membership_version()))
        {
            self.remote_version = Some(remote.membership_version().clone());
            for (index, mask) in remote.held_members().enumerate() {
                changed |= self.change_source(index + 1, mask, now);
            }
        }
        if changed {
            self.publish(now, upload);
        }
        upload.time = (now - self.origin) as f32;
        upload.active = self.held != 0 || !self.fades.is_empty();
    }
}

pub(crate) fn prepare_drag_elevation_upload(
    store: Res<PieceDataStore>,
    remote: Res<RemoteDragPresentation>,
    mut presentation: ResMut<DragElevationPresentation>,
    mut upload: ResMut<DragElevationUpload>,
) {
    presentation.prepare(&store, &remote, &mut upload);
}

#[cfg(test)]
mod tests;
