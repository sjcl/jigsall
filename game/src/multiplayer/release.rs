//! Shared host/replica release preparation, gameplay and result verification.
use crate::resources::{
    pieces::{AppliedCommand, CONNECTED_EDGES, ENABLED, HELD, PLACED},
    PieceDataStore,
};
use bevy::math::Vec2;
use puzzella_core::{
    protocol::{ActiveDragTarget, RejectedComponentRef, ReleaseResultFingerprint, TargetError},
    PieceId, PlayerId, PuzzleDefinition,
};
use sha2::{Digest, Sha256};

pub(super) fn release_drag(
    store: &mut PieceDataStore,
    player: PlayerId,
    target: &ActiveDragTarget,
    delta: Vec2,
    definition: Option<&PuzzleDefinition>,
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
    let applied = store.release_roots(player, roots.clone(), delta, definition);
    let result = result_fingerprint(store, &roots, definition, &applied);
    Ok((applied, rejected, result))
}

/// Hash ONLY the final components reached by released roots (including absorbed
/// neighbors). Sort stable minima and members, never DSU roots or linked-list order.
pub(super) fn result_fingerprint(
    store: &PieceDataStore,
    released_roots: &[PieceId],
    definition: Option<&PuzzleDefinition>,
    applied: &AppliedCommand,
) -> ReleaseResultFingerprint {
    let mut minima: Vec<_> = released_roots
        .iter()
        .map(|&id| store.connectivity.minimum_member(id))
        .collect();
    minima.sort_unstable();
    minima.dedup();
    let mut hash = Sha256::new();
    hash.update(b"puzzella/release-result/v1\0");
    hash.update((minima.len() as u32).to_le_bytes());
    for minimum in minima {
        let state = &store.states[minimum.0 as usize];
        let offset =
            state.position - definition.map_or(Vec2::ZERO, |d| d.correct_position(minimum));
        hash.update(minimum.0.to_le_bytes());
        hash.update((store.connectivity.component_size(minimum) as u32).to_le_bytes());
        hash.update(offset.x.to_bits().to_le_bytes());
        hash.update(offset.y.to_bits().to_le_bytes());
        hash.update([u8::from(state.flags & PLACED != 0)]);
        let mut members: Vec<_> = store.connectivity.iter_component(minimum).collect();
        members.sort_unstable();
        for id in members {
            let state = &store.states[id.0 as usize];
            hash.update(id.0.to_le_bytes());
            hash.update(state.position.x.to_bits().to_le_bytes());
            hash.update(state.position.y.to_bits().to_le_bytes());
            hash.update(state.z_order.to_le_bytes());
            hash.update((state.flags & (PLACED | HELD | ENABLED | CONNECTED_EDGES)).to_le_bytes());
            let owner = store.held_by.get(&id);
            hash.update([u8::from(owner.is_some())]);
            hash.update(owner.map_or(0, |p| p.0).to_le_bytes());
        }
    }
    for count in [applied.released, applied.placed, store.placed_count] {
        hash.update((count as u32).to_le_bytes());
    }
    hash.update(store.next_z_order.to_le_bytes());
    let bytes = hash.finalize();
    ReleaseResultFingerprint(u128::from_le_bytes(bytes[..16].try_into().unwrap()))
}
