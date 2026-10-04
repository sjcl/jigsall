use super::*;
use bevy::math::UVec2;
use jigsall_core::{GENERATOR_VERSION, MAX_PIECES};

/// Frozen pre-optimization v1 encoding. Keep independent of the new ordering and
/// member-hashing helpers; only modest fixtures use this allocation-heavy oracle.
fn legacy_fingerprint(
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
    hash.update(b"jigsall/release-result/v1\0");
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

#[test]
fn sparse_and_dense_ordering_match_v1_for_duplicates_merges_and_float_bits() {
    let mut store = PieceDataStore::default();
    store.initialize(
        (0..4096)
            .map(|id| {
                Vec2::new(
                    if id % 2 == 0 { -0.0 } else { id as f32 * 0.25 },
                    id as f32 * -0.125,
                )
            })
            .collect(),
    );
    // A huge component, several small components, and unaffected singletons.
    // Descending unions produce a member traversal different from ID ordering.
    for id in (1..257).rev() {
        store.connectivity.union(PieceId(0), PieceId(id));
    }
    for start in (300..500).step_by(4) {
        for id in start + 1..start + 4 {
            store.connectivity.union(PieceId(start), PieceId(id));
        }
    }
    for (id, state) in store.states.iter_mut().enumerate() {
        state.z_order = 4096 - id as u32;
        state.flags |= if id % 3 == 0 { CONNECTED_EDGES } else { 0 };
        state.flags |= if id % 5 == 0 { PLACED } else { 0 };
        state.flags |= 0xffff_0000; // Local/unknown presentation bits stay excluded.
    }
    store.held_by.insert(PieceId(0), PlayerId(0));
    store.held_by.insert(PieceId(315), PlayerId(u64::MAX));
    store.states[0].flags |= HELD;
    store.states[315].flags |= HELD;
    store.placed_count = store
        .states
        .iter()
        .filter(|state| state.flags & PLACED != 0)
        .count();
    let definition = PuzzleDefinition {
        generator_version: GENERATOR_VERSION,
        seed: 42,
        grid_size: UVec2::splat(64),
        image_size: UVec2::new(2048, 4096),
        snap_distance: 5.0,
        rotation_enabled: true,
    };
    let applied = AppliedCommand {
        released: 1000,
        placed: 100,
        ..Default::default()
    };
    let limit = FingerprintOrder::new(store.len()).sort_limit;
    for roots in [
        vec![],
        vec![
            PieceId(256),
            PieceId(17),
            PieceId(315),
            PieceId(305),
            PieceId(315),
        ],
        (0..limit as u32)
            .rev()
            .map(|id| PieceId(600 + id))
            .collect(),
        (0..=limit as u32)
            .rev()
            .map(|id| PieceId(600 + id))
            .collect(),
        (0..1000).rev().map(PieceId).collect(),
    ] {
        for definition in [None, Some(&definition)] {
            assert_eq!(
                result_fingerprint(&store, &roots, definition, &applied),
                legacy_fingerprint(&store, &roots, definition, &applied)
            );
        }
    }
}

#[test]
fn ordering_thresholds_dedup_and_scratch_reuse_preserve_ascending_ids() {
    for bit_len in [4096, MAX_PIECES] {
        let mut order = FingerprintOrder::new(bit_len);
        let limit = order.sort_limit;
        for count in [0, 1, limit - 1, limit, limit + 1] {
            order.set((0..count as u32).rev().map(|id| PieceId(id / 2)), count);
            assert_eq!(order.use_dense, count > limit);
            assert_eq!(order.len(), count.div_ceil(2));
            assert_eq!(
                order.iter().collect::<Vec<_>>(),
                (0..count.div_ceil(2) as u32)
                    .map(PieceId)
                    .collect::<Vec<_>>()
            );
        }
        let dense_pointer = order.dense.as_ref().unwrap().words().as_ptr();
        let sparse_pointer = order.sparse.as_ptr();
        let sparse_capacity = order.sparse.capacity();
        order.set((0..32).rev().map(PieceId), 32);
        assert_eq!(order.sparse.as_ptr(), sparse_pointer);
        assert_eq!(order.sparse.capacity(), sparse_capacity);
        assert_eq!(
            order.iter().collect::<Vec<_>>(),
            (0..32).map(PieceId).collect::<Vec<_>>()
        );
        order.set((32..limit as u32 + 33).rev().map(PieceId), limit + 1);
        assert_eq!(
            order.dense.as_ref().unwrap().words().as_ptr(),
            dense_pointer
        );
        assert_eq!(
            order.iter().collect::<Vec<_>>(),
            (32..limit as u32 + 33).map(PieceId).collect::<Vec<_>>()
        );
        order.set(std::iter::empty(), 0);
        assert_eq!(order.len(), 0);
        assert_eq!(order.iter().count(), 0); // No stale dense members on sparse reuse.
    }
}

#[test]
fn many_small_components_reuse_one_member_vector() {
    let mut store = PieceDataStore::default();
    store.initialize(vec![Vec2::ZERO; 4096]);
    for id in (0..4096).step_by(2) {
        store.connectivity.union(PieceId(id), PieceId(id + 1));
    }
    let roots: Vec<_> = (0..4096).rev().map(PieceId).collect();
    let applied = AppliedCommand {
        released: 4096,
        ..Default::default()
    };
    let mut scratch = FingerprintScratch::new(store.len());
    let expected = legacy_fingerprint(&store, &roots, None, &applied);
    assert_eq!(
        fingerprint_with_scratch(&store, &roots, None, &applied, &mut scratch),
        expected
    );
    assert!(scratch.members.dense.is_none());
    assert!(scratch.members.sparse.capacity() < 128);
    let pointer = scratch.members.sparse.as_ptr();
    let capacity = scratch.members.sparse.capacity();
    assert_eq!(
        fingerprint_with_scratch(&store, &roots, None, &applied, &mut scratch),
        expected
    );
    assert_eq!(scratch.members.sparse.as_ptr(), pointer);
    assert_eq!(scratch.members.sparse.capacity(), capacity);
}

#[test]
fn million_singletons_and_one_million_member_component_keep_v1_without_id_vectors() {
    let mut store = PieceDataStore::default();
    store.initialize(vec![Vec2::ZERO; MAX_PIECES]);
    let roots: Vec<_> = (0..MAX_PIECES as u32).rev().map(PieceId).collect();
    let applied = AppliedCommand {
        released: MAX_PIECES,
        ..Default::default()
    };
    let mut scratch = FingerprintScratch::new(MAX_PIECES);
    let fingerprint = fingerprint_with_scratch(&store, &roots, None, &applied, &mut scratch);
    // Independently encoded from the frozen v1 fields using Python hashlib/struct.
    assert_eq!(
        fingerprint,
        ReleaseResultFingerprint(0xb58e0f0a4eaba3fc44913a67e9720e37)
    );
    assert_eq!(scratch.minima.sparse.capacity(), 0);
    assert_eq!(
        scratch.minima.dense.as_ref().unwrap().words().len() * 4,
        125_000
    );
    assert_eq!(scratch.members.sparse.capacity(), 0);
    assert!(scratch.members.dense.is_none()); // Zero member ordering allocations.
    let minima_pointer = scratch.minima.dense.as_ref().unwrap().words().as_ptr();
    for id in 1..MAX_PIECES as u32 {
        store.connectivity.union(PieceId(0), PieceId(id));
    }
    let fingerprint = fingerprint_with_scratch(&store, &roots, None, &applied, &mut scratch);
    assert_eq!(
        fingerprint,
        ReleaseResultFingerprint(0x2809af6b1513005cf9e776b25a657ae0)
    );
    assert_eq!(scratch.minima.len(), 1);
    assert_eq!(
        scratch.minima.dense.as_ref().unwrap().words().as_ptr(),
        minima_pointer
    );
    assert_eq!(scratch.minima.sparse.capacity(), 0);
    assert_eq!(scratch.members.sparse.capacity(), 0);
    assert_eq!(
        scratch.members.dense.as_ref().unwrap().words().len() * 4,
        125_000
    );
}
