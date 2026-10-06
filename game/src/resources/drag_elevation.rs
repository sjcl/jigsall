//! Reconstructible interaction feedback; never part of gameplay or wire state.
use super::{remote_drag::RemoteDragPresentation, PieceDataStore};
use bevy::prelude::*;
use bytemuck::{Pod, Zeroable};
use jigsall_core::{PieceBitSet, PieceId};
use std::sync::Arc;

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

enum EnvelopeMembers {
    Shared(Arc<[u32]>),
    Partition(Vec<u32>),
}
impl EnvelopeMembers {
    fn iter(&self) -> impl Iterator<Item = u32> + '_ {
        let (words, ids): (&[u32], &[u32]) = match self {
            Self::Shared(words) => (words, &[]),
            Self::Partition(ids) => (&[], ids),
        };
        members(words).chain(ids.iter().copied())
    }
}
struct Envelope {
    members: EnvelopeMembers,
    from: f32,
    to: f32,
    start: f64,
}

#[derive(Default)]
struct DragSource {
    members: Option<Arc<[u32]>>,
    slots: Vec<u32>,
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
    sources: Vec<DragSource>,
    mapping: Vec<u32>,
    records: Vec<Envelope>,
    free: Vec<u32>,
    fades: Vec<(u32, f64)>,
    next_expiry: f64,
    held: usize,
    origin: f64,
    dirty: PieceBitSet,
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
        let previous = &self.sources[source];
        if match (&previous.members, &mask) {
            (Some(previous), Some(mask)) => Arc::ptr_eq(previous, mask),
            (None, None) => true,
            _ => false,
        } {
            return false;
        }
        // A gesture may own several envelopes after a mixed re-grab. Release
        // every one from its own current value, without walking their members.
        let previous = std::mem::take(&mut self.sources[source]);
        self.held -= previous.slots.len();
        for slot in previous.slots {
            let record = &mut self.records[slot as usize - 1];
            record.from = record.value(now);
            record.to = 0.0;
            record.start = now;
            self.fades.push((slot, now + DRAG_ELEVATION_SECONDS));
        }
        if let Some(mask) = mask {
            if self.mapping.is_empty() {
                self.mapping.resize(self.count, 0);
                self.dirty = PieceBitSet::new(self.count);
            }
            // Index by old presentation slot, never by piece/component/DSU.
            // Count first so the common one-envelope grab shares the original
            // bitset and mixed grabs allocate exactly one ID list per old slot.
            let mut indices = vec![usize::MAX; self.records.len() + 1];
            let mut groups: Vec<(u32, usize)> = Vec::new();
            for id in members(&mask) {
                let old = self.previous_slot(id);
                let index = &mut indices[old as usize];
                if *index == usize::MAX {
                    *index = groups.len();
                    groups.push((old, 0));
                }
                groups[*index].1 += 1;
            }
            let partitions = if groups.len() == 1 {
                vec![EnvelopeMembers::Shared(mask.clone())]
            } else {
                let mut ids: Vec<Vec<u32>> = groups
                    .iter()
                    .map(|&(_, count)| Vec::with_capacity(count))
                    .collect();
                for id in members(&mask) {
                    ids[indices[self.previous_slot(id) as usize]].push(id);
                }
                ids.into_iter().map(EnvelopeMembers::Partition).collect()
            };
            // Capture all starts before any free-slot reuse can overwrite a
            // previous record. Every partition keeps its own displayed height.
            let starts: Vec<f32> = groups
                .iter()
                .map(|&(old, _)| {
                    if old == 0 {
                        0.0
                    } else {
                        self.records[old as usize - 1].value(now)
                    }
                })
                .collect();
            let mut slots = Vec::with_capacity(groups.len());
            for (members, from) in partitions.into_iter().zip(starts) {
                slots.push(self.allocate(Envelope {
                    members,
                    from,
                    to: 1.0,
                    start: now,
                }));
            }
            self.held += slots.len();
            self.sources[source] = DragSource {
                members: Some(mask),
                slots,
            };
        }
        true
    }

    fn previous_slot(&self, id: u32) -> u32 {
        let slot = self.mapping[id as usize];
        // Expired mappings are deliberately retained until reuse. All zero
        // records belong to the idle partition, so they need no separate list.
        if slot != 0
            && (self.records[slot as usize - 1].from != 0.0
                || self.records[slot as usize - 1].to != 0.0)
        {
            slot
        } else {
            0
        }
    }

    fn allocate(&mut self, record: Envelope) -> u32 {
        let slot = if let Some(slot) = self.free.pop() {
            for id in self.records[slot as usize - 1].members.iter() {
                if self.mapping[id as usize] == slot {
                    self.mapping[id as usize] = 0;
                    self.dirty.insert(PieceId(id));
                }
            }
            self.records[slot as usize - 1] = record;
            slot
        } else {
            self.records.push(record);
            self.records.len() as u32
        };
        for id in self.records[slot as usize - 1].members.iter() {
            self.mapping[id as usize] = slot;
            self.dirty.insert(PieceId(id));
        }
        slot
    }

    #[cfg(test)]
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
        let mut spans: Vec<(usize, usize)> = Vec::new();
        // Empty release/expiry updates never scan the piece-sized bitset.
        for id in self.dirty.iter().take(self.dirty.len()) {
            let id = id.0 as usize;
            if let Some(span) = spans.last_mut().filter(|(_, end)| *end == id) {
                span.1 += 1;
            } else {
                if spans.len() == 128 {
                    let start = spans[0].0;
                    let end = self.dirty.iter().last().unwrap().0 as usize + 1;
                    spans = vec![(start, end)];
                    break;
                }
                spans.push((id, id + 1));
            }
        }
        self.dirty.clear();
        upload.ranges = spans
            .into_iter()
            .map(|(start, end)| DragElevationRange {
                start: start as u32,
                slots: self.mapping[start..end].to_vec(),
            })
            .collect();
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
                sources: (0..=super::remote_drag::REMOTE_DRAG_SLOTS)
                    .map(|_| DragSource::default())
                    .collect(),
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
