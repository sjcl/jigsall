//! Shared host/replica release preparation, gameplay and result verification.
use crate::resources::{
    pieces::{AppliedCommand, CONNECTED_EDGES, ENABLED, HELD, PLACED},
    PieceDataStore,
};
use bevy::math::Vec2;
use jigsall_core::{
    decode_rotation,
    protocol::{
        ActiveDrag, ActiveDragTarget, RejectedComponentRef, ReleaseResultFingerprint, TargetError,
    },
    rotate_quarter, PieceBitSet, PieceId, PlayerId, PuzzleDefinition, ROTATION_MASK,
};
use sha2::{Digest, Sha256};

pub(super) fn release_drag(
    store: &mut PieceDataStore,
    player: PlayerId,
    target: &ActiveDragTarget,
    delta: Vec2,
    definition: Option<&PuzzleDefinition>,
    local_player: PlayerId,
) -> Result<
    (
        AppliedCommand,
        Vec<RejectedComponentRef>,
        ReleaseResultFingerprint,
    ),
    TargetError,
> {
    let definition = definition.filter(|d| d.validate().is_ok() && d.piece_count() == store.len());
    let acceptable = |id: PieceId| {
        store.states[id.0 as usize].flags & (PLACED | ENABLED | HELD) == (ENABLED | HELD)
            && store.held_by.get(&id) == Some(&player)
    };
    let mut roots = Vec::new();
    let mut rejected = Vec::new();
    match target {
        ActiveDragTarget::Sparse(refs) => {
            for &reference in refs {
                match reference.resolve(&store.connectivity) {
                    Ok(minimum) if store.connectivity.iter_component(minimum).all(acceptable) => {
                        roots.push(minimum)
                    }
                    Ok(_) => {}
                    Err(reason) => rejected.push(RejectedComponentRef { reference, reason }),
                }
            }
        }
        ActiveDragTarget::Dense(dense) => {
            let canonical = dense.resolve(&store.connectivity)?;
            let accepted = store.canonical_members(&canonical, acceptable);
            roots.extend(
                accepted
                    .iter()
                    .filter(|&id| store.connectivity.minimum_member(id) == id),
            );
        }
    }
    roots.sort_unstable();
    let applied = store.release_roots(player, roots.clone(), delta, definition, local_player);
    let result = result_fingerprint(store, &roots, definition, &applied);
    Ok((applied, rejected, result))
}

/// Ascending, deduplicated IDs with bounded sparse sort/storage. Dense ordering
/// scans bitset words instead of sorting a puzzle-sized vector. Storage is reused
/// across components, and is never retained by a drag/replication context.
struct FingerprintOrder {
    sparse: Vec<PieceId>,
    dense: Option<PieceBitSet>,
    use_dense: bool,
    bit_len: usize,
    sort_limit: usize,
}

impl FingerprintOrder {
    fn new(bit_len: usize) -> Self {
        Self {
            sparse: Vec::new(),
            dense: None,
            use_dense: false,
            bit_len,
            // Above this size a sorted ID vector costs at least as much storage
            // as mask words. The floor keeps small operations on the sparse path.
            sort_limit: bit_len.div_ceil(32).max(128),
        }
    }

    fn set(&mut self, ids: impl Iterator<Item = PieceId>, upper_bound: usize) {
        self.sparse.clear();
        self.use_dense = upper_bound > self.sort_limit;
        if self.use_dense {
            let mask = self
                .dense
                .get_or_insert_with(|| PieceBitSet::new(self.bit_len));
            mask.clear();
            mask.extend(ids);
        } else {
            self.sparse.reserve(upper_bound);
            self.sparse.extend(ids);
            self.sparse.sort_unstable();
            self.sparse.dedup();
        }
    }

    fn len(&self) -> usize {
        if self.use_dense {
            self.dense.as_ref().unwrap().count()
        } else {
            self.sparse.len()
        }
    }

    fn iter(&self) -> impl Iterator<Item = PieceId> + '_ {
        self.sparse.iter().copied().chain(
            self.dense
                .as_ref()
                .filter(|_| self.use_dense)
                .into_iter()
                .flat_map(|mask| mask.iter()),
        )
    }
}

struct FingerprintScratch {
    minima: FingerprintOrder,
    members: FingerprintOrder,
}

impl FingerprintScratch {
    fn new(bit_len: usize) -> Self {
        Self {
            minima: FingerprintOrder::new(bit_len),
            members: FingerprintOrder::new(bit_len),
        }
    }
}

/// Hash ONLY the final components reached by released roots (including absorbed
/// neighbors). Stable ascending IDs preserve the exact v1 byte stream regardless
/// of sparse/dense ordering, DSU roots or linked-list order.
pub(super) fn result_fingerprint(
    store: &PieceDataStore,
    released_roots: &[PieceId],
    definition: Option<&PuzzleDefinition>,
    applied: &AppliedCommand,
) -> ReleaseResultFingerprint {
    let mut scratch = FingerprintScratch::new(store.len());
    fingerprint_with_scratch(store, released_roots, definition, applied, &mut scratch)
}

/// Include the retained context/basis as well as canonical geometry and holds.
pub(super) fn drag_rotation_fingerprint(
    store: &PieceDataStore,
    roots: &[PieceId],
    definition: &PuzzleDefinition,
    applied: &AppliedCommand,
    player: PlayerId,
    drag: &ActiveDrag,
) -> ReleaseResultFingerprint {
    let result = result_fingerprint(store, roots, Some(definition), applied);
    let mut hash = Sha256::new();
    hash.update(b"jigsall/drag-rotation/v1\0");
    hash.update(result.0.to_le_bytes());
    hash.update(player.0.to_le_bytes());
    hash.update(drag.grab_sequence.to_le_bytes());
    hash.update(drag.basis_sequence.to_le_bytes());
    hash.update([u8::from(drag.last_tick.is_some())]);
    hash.update(drag.last_tick.unwrap_or(0).to_le_bytes());
    hash.update(drag.delta.x.to_bits().to_le_bytes());
    hash.update(drag.delta.y.to_bits().to_le_bytes());
    let bytes = hash.finalize();
    ReleaseResultFingerprint(u128::from_le_bytes(bytes[..16].try_into().unwrap()))
}

fn fingerprint_with_scratch(
    store: &PieceDataStore,
    released_roots: &[PieceId],
    definition: Option<&PuzzleDefinition>,
    applied: &AppliedCommand,
    scratch: &mut FingerprintScratch,
) -> ReleaseResultFingerprint {
    let FingerprintScratch { minima, members } = scratch;
    minima.set(
        released_roots
            .iter()
            .map(|&id| store.connectivity.minimum_member(id)),
        released_roots.len(),
    );
    let mut hash = Sha256::new();
    hash.update(b"jigsall/release-result/v1\0");
    hash.update((minima.len() as u32).to_le_bytes());
    for minimum in minima.iter() {
        let size = store.connectivity.component_size(minimum);
        let state = &store.states[minimum.0 as usize];
        let offset = state.position
            - definition.map_or(Vec2::ZERO, |d| {
                rotate_quarter(d.correct_position(minimum), decode_rotation(state.flags))
            });
        hash.update(minimum.0.to_le_bytes());
        hash.update((size as u32).to_le_bytes());
        hash.update(offset.x.to_bits().to_le_bytes());
        hash.update(offset.y.to_bits().to_le_bytes());
        hash.update([u8::from(state.flags & PLACED != 0)]);
        if size == 1 {
            // A million singleton results allocate no member vectors or masks.
            hash_member(&mut hash, store, minimum);
        } else {
            members.set(store.connectivity.iter_component(minimum), size);
            for id in members.iter() {
                hash_member(&mut hash, store, id);
            }
        }
    }
    for count in [applied.released, applied.placed, store.placed_count] {
        hash.update((count as u32).to_le_bytes());
    }
    hash.update(store.next_z_order.to_le_bytes());
    let bytes = hash.finalize();
    ReleaseResultFingerprint(u128::from_le_bytes(bytes[..16].try_into().unwrap()))
}

fn hash_member(hash: &mut Sha256, store: &PieceDataStore, id: PieceId) {
    let state = &store.states[id.0 as usize];
    hash.update(id.0.to_le_bytes());
    hash.update(state.position.x.to_bits().to_le_bytes());
    hash.update(state.position.y.to_bits().to_le_bytes());
    hash.update(state.z_order.to_le_bytes());
    hash.update(
        (state.flags & (PLACED | HELD | ENABLED | CONNECTED_EDGES | ROTATION_MASK)).to_le_bytes(),
    );
    let owner = store.held_by.get(&id);
    hash.update([u8::from(owner.is_some())]);
    hash.update(owner.map_or(0, |p| p.0).to_le_bytes());
}

#[cfg(test)]
#[path = "release_tests.rs"]
mod tests;
