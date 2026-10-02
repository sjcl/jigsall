use super::*;
use crate::checkpoint::{CheckpointError, PuzzleCheckpoint, SnapshotPieceState, SNAPSHOT_PLACED};
use puzzella_core::{
    protocol::{ComponentRef, DenseTarget, TargetError},
    session::ImageHash,
    GENERATOR_VERSION, LOCAL_PLAYER,
};

fn fixture(grid: UVec2) -> (PuzzleDefinition, PieceDataStore) {
    let definition = PuzzleDefinition {
        generator_version: GENERATOR_VERSION,
        seed: 42,
        grid_size: grid,
        image_size: grid * UVec2::new(20, 30),
        snap_distance: 5.0,
    };
    let mut store = PieceDataStore::default();
    store.initialize(
        (0..definition.piece_count() as u32)
            .map(|id| definition.correct_position(PieceId(id)) + Vec2::splat(100.0))
            .collect(),
    );
    (definition, store)
}

fn target(store: &PieceDataStore, ids: &[u32]) -> PieceTarget {
    let mut mask = PieceBitSet::new(store.len());
    mask.extend(ids.iter().copied().map(PieceId));
    PieceTarget::from_selection(&store.connectivity, &mask).unwrap()
}

fn rotate(store: &mut PieceDataStore, d: &PuzzleDefinition, ids: &[u32], turns: i8) -> usize {
    store
        .apply_command(
            LOCAL_PLAYER,
            &PieceCommand::Rotate {
                target: target(store, ids),
                quarter_turns: turns,
            },
            Some(d),
            puzzella_core::LOCAL_PLAYER,
        )
        .rotated
}

fn set_transform(
    store: &mut PieceDataStore,
    d: &PuzzleDefinition,
    id: u32,
    rotation: u32,
    translation: Vec2,
) {
    let state = &mut store.states[id as usize];
    state.position = rotate_quarter(d.correct_position(PieceId(id)), rotation) + translation;
    state.flags = with_rotation(state.flags, rotation);
}

fn assert_rigid(store: &PieceDataStore, d: &PuzzleDefinition, id: u32, rotation: u32) {
    let representative = store.connectivity.minimum_member(PieceId(id));
    let translation = store.states[representative.0 as usize].position
        - rotate_quarter(d.correct_position(representative), rotation);
    for member in store.connectivity.iter_component(representative) {
        let state = store.states[member.0 as usize];
        assert_eq!(decode_rotation(state.flags), rotation);
        assert!(
            matches_transform(
                state.position,
                d.correct_position(member),
                rotation,
                translation
            ),
            "member {member} drifted at rotation {rotation}"
        );
    }
}

#[test]
fn singleton_four_turns_and_many_cycles_keep_exact_center_and_flags() {
    let (d, mut store) = fixture(UVec2::splat(3));
    store.states[0].position = Vec2::new(0.031_415_9, -173.123_45);
    store.states[0].flags |= CONNECTED_EDGES;
    let original = store.states[0];
    for step in 0..4000 {
        assert_eq!(rotate(&mut store, &d, &[0], 1), 1);
        assert_eq!(store.states[0].position, original.position);
        assert_eq!(store.states[0].flags & CONNECTED_EDGES, CONNECTED_EDGES);
        assert_eq!(decode_rotation(store.states[0].flags), (step + 1) % 4);
    }
    assert_eq!(store.states[0], original);
    store.dirty_pieces.clear();
    assert_eq!(rotate(&mut store, &d, &[0], -128), 0);
    assert!(store.dirty_pieces.is_empty());
    assert!(store.component_root_dirty.is_empty());
}

#[test]
fn horizontal_pair_and_l_body_preserve_relative_geometry_at_every_quarter_turn() {
    for members in [vec![0, 1], vec![0, 1, 3]] {
        let (d, mut store) = fixture(UVec2::splat(3));
        for &id in &members[1..] {
            store.connectivity.union(PieceId(0), PieceId(id));
        }
        let before: Vec<_> = members
            .iter()
            .map(|&id| store.states[id as usize].position)
            .collect();
        let pivot = (before
            .iter()
            .copied()
            .fold(Vec2::splat(f32::INFINITY), Vec2::min)
            + before
                .iter()
                .copied()
                .fold(Vec2::splat(f32::NEG_INFINITY), Vec2::max))
            * 0.5;
        for rotation in 1..=4 {
            assert_eq!(rotate(&mut store, &d, &[members[0]], 1), members.len());
            assert_rigid(&store, &d, 0, rotation % 4);
            for (&id, &position) in members.iter().zip(&before) {
                assert_eq!(
                    store.states[id as usize].position,
                    pivot + rotate_quarter(position - pivot, rotation)
                );
            }
        }
    }
}

#[test]
fn canonical_reconstruction_does_not_accumulate_fractional_grid_drift() {
    let (mut d, mut store) = fixture(UVec2::new(7, 3));
    d.image_size = UVec2::new(123, 101);
    for id in 0..21 {
        set_transform(&mut store, &d, id, 0, Vec2::new(173.123_45, -81.031_25));
    }
    store.connectivity.union(PieceId(0), PieceId(1));
    store.connectivity.union(PieceId(0), PieceId(7));
    for _ in 0..4 {
        rotate(&mut store, &d, &[0], 1);
    }
    let baseline = store.states.to_vec();
    for step in 0..4000 {
        rotate(&mut store, &d, &[0], 1);
        assert_rigid(&store, &d, 0, (step + 1) % 4);
        if step % 4 == 3 {
            assert_eq!(&*store.states, &baseline);
        }
    }
}

#[test]
fn multiselection_rotates_each_component_about_its_own_center_once() {
    let (d, mut store) = fixture(UVec2::new(4, 1));
    store.connectivity.union(PieceId(0), PieceId(1));
    set_transform(&mut store, &d, 2, 0, Vec2::new(1000.0, -1000.0));
    let singleton = store.states[2].position;
    let pair_pivot = (store.states[0].position + store.states[1].position) * 0.5;
    let untouched = store.states[3];
    assert_eq!(rotate(&mut store, &d, &[0, 1, 2], -1), 3);
    assert_eq!(store.states[2].position, singleton);
    assert_eq!(
        (store.states[0].position + store.states[1].position) * 0.5,
        pair_pivot
    );
    assert_eq!(store.states[3], untouched);
    assert_rigid(&store, &d, 0, 3);
}

#[test]
fn authority_rejects_placed_disabled_hold_and_invalid_rigid_components_atomically() {
    for invalid in 0..7 {
        let (d, mut store) = fixture(UVec2::new(3, 1));
        store.connectivity.union(PieceId(0), PieceId(1));
        match invalid {
            0 => store.states[1].flags |= PLACED,
            1 => store.states[1].flags &= !ENABLED,
            2 | 3 => {
                store.held_by.insert(
                    PieceId(1),
                    if invalid == 2 {
                        PlayerId(9)
                    } else {
                        LOCAL_PLAYER
                    },
                );
            }
            4 => store.states[1].flags = with_rotation(store.states[1].flags, 1),
            5 => store.states[1].position.x += 1.0,
            6 => {
                let mut mask = PieceBitSet::new(3);
                mask.insert(PieceId(1));
                store.drag.members = mask.words().clone();
            }
            _ => unreachable!(),
        }
        let before = store.states.to_vec();
        assert_eq!(rotate(&mut store, &d, &[0], 1), 0);
        assert_eq!(&*store.states, &before);
        assert!(store.dirty_pieces.is_empty());
    }
}

#[test]
fn stale_sparse_and_dense_topology_cannot_expand_rotation_targets() {
    let (d, mut store) = fixture(UVec2::new(40, 1));
    let sparse = target(&store, &[0]);
    let mut members = PieceBitSet::new(40);
    members.fill();
    let dense =
        PieceTarget::Dense(DenseTarget::from_selection(&store.connectivity, &members).unwrap());
    store.connectivity.union(PieceId(0), PieceId(1));
    let before = store.states.to_vec();
    let result = store.rotate_target(&sparse, 1, &d).unwrap();
    assert_eq!(result.applied.rotated, 0);
    assert_eq!(result.rejected[0].reason, TargetError::StaleComponent);
    assert!(matches!(
        store.rotate_target(&dense, 1, &d),
        Err(TargetError::StaleTopology)
    ));
    assert_eq!(&*store.states, &before);
    let r = ComponentRef::from_member(&store.connectivity, PieceId(0)).unwrap();
    assert_eq!(
        store
            .rotate_target(&PieceTarget::Components(vec![r, r]), 1, &d)
            .unwrap()
            .applied
            .rotated,
        2
    );
}

#[test]
fn matching_rotation_neighbors_snap_and_can_rotate_after_union() {
    let (d, mut store) = fixture(UVec2::new(2, 1));
    set_transform(&mut store, &d, 0, 1, Vec2::new(101.0, 100.0));
    set_transform(&mut store, &d, 1, 1, Vec2::splat(100.0));
    store.snap_unheld_component(PieceId(0), &d);
    assert_eq!(store.connectivity.component_size(PieceId(0)), 2);
    assert_rigid(&store, &d, 0, 1);
    assert_eq!(store.states[0].flags & CONNECTED_RIGHT, CONNECTED_RIGHT);
    assert_eq!(store.states[1].flags & CONNECTED_LEFT, CONNECTED_LEFT);
    assert_eq!(rotate(&mut store, &d, &[1], 1), 2);
    assert_rigid(&store, &d, 0, 2);
    assert_eq!(store.placed_count, 0);
}

#[test]
fn rotated_board_and_different_rotation_neighbors_never_snap() {
    let (d, mut store) = fixture(UVec2::new(2, 1));
    for rotation in 1..4 {
        set_transform(&mut store, &d, 0, rotation, Vec2::ZERO);
        set_transform(&mut store, &d, 1, 0, Vec2::ZERO);
        store.snap_unheld_component(PieceId(0), &d);
        assert_eq!(store.connectivity.component_size(PieceId(0)), 1);
        assert_eq!(store.placed_count, 0);
        // Even occupying the exact completed position cannot place a rotated piece.
        store.states[0].position = d.correct_position(PieceId(0));
        store.snap_unheld_component(PieceId(0), &d);
        assert_eq!(store.placed_count, 0);
    }
    set_transform(&mut store, &d, 0, 0, Vec2::X);
    store.snap_unheld_component(PieceId(0), &d);
    assert_eq!(store.placed_count, 2);
    assert_eq!(rotate(&mut store, &d, &[0], 1), 0);
}

#[test]
fn closure_requires_same_rotation_and_one_fixed_logical_translation() {
    let (d, mut store) = fixture(UVec2::new(5, 1));
    for (id, rotation, x) in [
        (0, 1, 100.0),
        (1, 1, 104.0),
        (2, 1, 104.0),
        (3, 2, 104.0),
        (4, 1, 104.0),
    ] {
        set_transform(&mut store, &d, id, rotation, Vec2::new(x, 100.0));
    }
    store.snap_unheld_component(PieceId(0), &d);
    assert_eq!(store.connectivity.component_size(PieceId(0)), 3);
    assert_eq!(store.connectivity.component_size(PieceId(3)), 1);
    assert_eq!(store.connectivity.component_size(PieceId(4)), 1);
    assert_rigid(&store, &d, 0, 1);
    let (d, mut store) = fixture(UVec2::new(3, 1));
    for id in 0..3 {
        set_transform(
            &mut store,
            &d,
            id,
            1,
            Vec2::new(100.0 + id as f32 * 4.0, 100.0),
        );
    }
    store.snap_unheld_component(PieceId(0), &d);
    assert_eq!(store.connectivity.component_size(PieceId(0)), 2);
    assert_eq!(store.connectivity.component_size(PieceId(2)), 1);
}

#[test]
fn rotated_moves_and_release_preserve_rigid_transform_and_pointer_upload_contract() {
    let (d, mut store) = fixture(UVec2::new(3, 1));
    store.connectivity.union(PieceId(0), PieceId(1));
    rotate(&mut store, &d, &[0], 1);
    let roots_dirty = store.component_root_dirty.clone();
    store.apply_command(
        LOCAL_PLAYER,
        &PieceCommand::Grab(PieceId(1)),
        Some(&d),
        puzzella_core::LOCAL_PLAYER,
    );
    let position = store.states[1].position + Vec2::new(10.25, -6.5);
    store.apply_command(
        LOCAL_PLAYER,
        &PieceCommand::Move {
            id: PieceId(1),
            position,
        },
        Some(&d),
        puzzella_core::LOCAL_PLAYER,
    );
    assert_rigid(&store, &d, 0, 1);
    let before = store.states.to_vec();
    store.dirty_pieces.clear();
    let mut members = PieceBitSet::new(3);
    members.extend([PieceId(0), PieceId(1)]);
    store.drag.members = members.words().clone();
    for step in 0..100 {
        store.drag.delta = Vec2::splat(step as f32);
        assert!(store.dirty_pieces.is_empty());
        assert_eq!(&*store.states, &before);
        assert_eq!(rotate(&mut store, &d, &[0], 1), 0);
    }
    store.apply_command(
        LOCAL_PLAYER,
        &PieceCommand::ReleaseGroup {
            members,
            delta: Vec2::new(0.25, 5.5),
        },
        Some(&d),
        puzzella_core::LOCAL_PLAYER,
    );
    assert_rigid(&store, &d, 0, 1);
    assert_eq!(store.component_root_dirty, roots_dirty);
    assert!(store.held_by.is_empty());
}

#[test]
fn rotated_checkpoint_roundtrip_restores_geometry_flags_and_canonical_edges() {
    let (d, mut store) = fixture(UVec2::splat(2));
    store.snap_unheld_component(PieceId(0), &d);
    assert_eq!(rotate(&mut store, &d, &[0], -1), 4);
    let checkpoint = PuzzleCheckpoint::capture(&store, &d, ImageHash([7; 32])).unwrap();
    assert_eq!(std::mem::size_of::<SnapshotPieceState>(), 16);
    let mut restored = PieceDataStore::default();
    checkpoint.install(&mut restored).unwrap();
    assert_eq!(&*restored.states, &*store.states);
    assert_rigid(&restored, &d, 0, 3);
    assert_eq!(
        PuzzleCheckpoint::capture(&restored, &d, checkpoint.image_hash).unwrap(),
        checkpoint
    );
    let mut mixed = checkpoint.clone();
    mixed.pieces[1].flags = with_rotation(mixed.pieces[1].flags, 2);
    assert_eq!(
        mixed.validate(),
        Err(CheckpointError::InconsistentComponent(PieceId(1)))
    );
    let mut drift = checkpoint;
    drift.pieces[1].position.x += 1.0;
    assert_eq!(
        drift.validate(),
        Err(CheckpointError::InconsistentComponent(PieceId(1)))
    );
}

#[test]
fn placed_snapshot_rotation_and_unknown_flag_bits_are_rejected() {
    let (d, mut store) = fixture(UVec2::ONE);
    set_transform(&mut store, &d, 0, 0, Vec2::ZERO);
    store.states[0].flags |= PLACED;
    let mut checkpoint = PuzzleCheckpoint::capture(&store, &d, ImageHash([0; 32])).unwrap();
    checkpoint.pieces[0].flags = with_rotation(SNAPSHOT_PLACED, 1);
    assert_eq!(
        checkpoint.validate(),
        Err(CheckpointError::InvalidPlacedPosition(PieceId(0)))
    );
    checkpoint.pieces[0].flags = 1 << 11;
    assert_eq!(
        checkpoint.validate(),
        Err(CheckpointError::InvalidFlags(PieceId(0)))
    );
}

#[test]
fn rotated_fractional_closure_keeps_fixed_translation_and_scans_each_boundary_once() {
    for rotation in 1..4 {
        let (mut d, mut store) = fixture(UVec2::splat(100));
        d.image_size = UVec2::splat(4096);
        for id in 0..10_000 {
            // Rotate the complete canonical translation fixture as well. This
            // exercises the existing rounding asymmetry on both world axes.
            let translation = Vec2::new(
                if (id % 100 + id / 100) % 2 == 0 {
                    100.37
                } else {
                    104.37
                },
                1000.37,
            );
            set_transform(
                &mut store,
                &d,
                id,
                rotation,
                rotate_quarter(translation, rotation),
            );
        }
        let mut scratch = super::super::snapping::SnapScratch::new(store.len(), &d);
        for id in 0..10_000 {
            let root = store.connectivity.find_root(PieceId(id));
            if !scratch.resolved.contains(&root) {
                store.resolve_component_snap(root, &mut scratch);
            }
        }
        assert_eq!(store.connectivity.component_size(PieceId(0)), 10_000);
        assert_eq!(scratch.boundary_members, 10_000);
        assert_eq!(store.placed_count, 0);
        assert_rigid(&store, &d, 0, rotation);
    }
}

#[test]
fn million_dense_rotation_uploads_only_changed_members_and_no_component_roots() {
    let (d, store) = fixture(UVec2::splat(1000));
    let mut app = App::new();
    app.insert_resource(d.clone())
        .insert_resource(store)
        .init_resource::<PieceUpload>()
        .add_systems(Update, prepare_piece_upload);
    app.update();
    app.update();
    let root_revision = app.world().resource::<PieceUpload>().root_revision;
    let state_allocation = app.world().resource::<PieceDataStore>().states.as_ptr();
    rotate(
        &mut app.world_mut().resource_mut::<PieceDataStore>(),
        &d,
        &[777_777],
        1,
    );
    app.update();
    let upload = app.world().resource::<PieceUpload>();
    assert_eq!(upload.ranges.len(), 1);
    assert_eq!(upload.ranges[0].start, 777_777);
    assert_eq!(
        upload.ranges[0].states.len() * std::mem::size_of::<GpuPieceState>(),
        16
    );
    assert_eq!(upload.root_revision, root_revision);
    assert!(upload.root_ranges.is_empty());
    assert_eq!(
        app.world().resource::<PieceDataStore>().states.as_ptr(),
        state_allocation
    );
    let revision = upload.revision;
    let ranges = upload.ranges.clone();
    app.update();
    let idle = app.world().resource::<PieceUpload>();
    assert_eq!(idle.revision, revision);
    assert!(Arc::ptr_eq(&idle.ranges, &ranges));
    assert!(app
        .world()
        .resource::<PieceDataStore>()
        .dirty_pieces
        .is_empty());
}

#[test]
fn singleton_centers_preserve_tiny_offsets_signed_zero_and_large_float_bits() {
    let (d, mut store) = fixture(UVec2::new(3, 1));
    for position in [
        Vec2::new(1e-20, -1e-20),
        Vec2::new(-0.0, 0.0),
        Vec2::new(1e30, -1e30),
    ] {
        store.states[0].position = position;
        for _ in 0..4 {
            assert_eq!(rotate(&mut store, &d, &[0], 1), 1);
            assert_eq!(
                store.states[0].position.to_array().map(f32::to_bits),
                position.to_array().map(f32::to_bits)
            );
        }
    }
}
