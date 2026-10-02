use super::*;
use puzzella_core::{GENERATOR_VERSION, LOCAL_PLAYER, ROTATION_MASK};

fn fixture() -> (PuzzleDefinition, PieceDataStore, PieceBitSet) {
    let d = PuzzleDefinition {
        generator_version: GENERATOR_VERSION,
        seed: 42,
        grid_size: UVec2::new(4, 1),
        image_size: UVec2::new(80, 30),
        snap_distance: 5.0,
    };
    let mut store = PieceDataStore::default();
    store.initialize(
        (0..4)
            .map(|id| d.correct_position(PieceId(id)) + Vec2::splat(1000.))
            .collect(),
    );
    store.connectivity.union(PieceId(0), PieceId(1));
    let mut members = PieceBitSet::new(4);
    members.extend([PieceId(0), PieceId(1), PieceId(2)]);
    store.drag.members = members.words().clone();
    store.selected_pieces = members.clone();
    store.apply_command(
        LOCAL_PLAYER,
        &PieceCommand::GrabGroup {
            members: members.clone(),
        },
        Some(&d),
        puzzella_core::LOCAL_PLAYER,
    );
    store.drag.delta = Vec2::new(20., 30.);
    store.dirty_pieces.clear();
    (d, store, members)
}

fn rotate_drag(
    store: &mut PieceDataStore,
    d: &PuzzleDefinition,
    members: &PieceBitSet,
    turns: i8,
) -> AppliedCommand {
    store.apply_command(
        LOCAL_PLAYER,
        &PieceCommand::RotateDrag {
            members: members.clone(),
            delta: store.drag.delta,
            quarter_turns: turns,
        },
        Some(d),
        puzzella_core::LOCAL_PLAYER,
    )
}

#[test]
fn drag_rotation_commits_displayed_positions_with_independent_pivots_and_preserves_metadata() {
    let (d, mut store, members) = fixture();
    let before = store.states.to_vec();
    let drag_members = store.drag.members.clone();
    let selected = store.selected_pieces.words().clone();
    let root_dirty = store.component_root_dirty.clone();
    let roots: Vec<_> = (0..4)
        .map(|id| store.connectivity.find_root(PieceId(id)))
        .collect();
    let next_z = store.next_z_order;
    let result = rotate_drag(&mut store, &d, &members, 1);
    assert!(result.drag_rebased);
    assert_eq!(result.rotated, 3);
    assert_eq!(store.states[0].position, Vec2::new(1000., 1020.));
    assert_eq!(store.states[1].position, Vec2::new(1000., 1040.));
    assert_eq!(store.states[2].position, Vec2::new(1030., 1030.));
    assert_eq!(store.states[3], before[3]);
    for id in members.iter() {
        let state = store.states[id.0 as usize];
        assert_eq!(decode_rotation(state.flags), 1);
        assert_eq!(
            state.flags & !ROTATION_MASK,
            before[id.0 as usize].flags & !ROTATION_MASK
        );
        assert_eq!(state.z_order, before[id.0 as usize].z_order);
        assert_eq!(store.held_by.get(&id), Some(&LOCAL_PLAYER));
    }
    assert_eq!(store.drag.delta, Vec2::ZERO);
    assert!(Arc::ptr_eq(&drag_members, &store.drag.members));
    assert!(Arc::ptr_eq(&selected, store.selected_pieces.words()));
    assert_eq!(store.component_root_dirty, root_dirty);
    assert_eq!(store.next_z_order, next_z);
    assert_eq!(store.dirty_pieces, members);
    assert_eq!(
        (0..4)
            .map(|id| store.connectivity.find_root(PieceId(id)))
            .collect::<Vec<_>>(),
        roots
    );
    assert_eq!(store.placed_count, 0);
}

#[test]
fn repeated_drag_turns_reconstruct_fractional_l_shape_without_accumulated_drift() {
    let (mut d, mut store, _) = fixture();
    d.grid_size = UVec2::new(7, 3);
    d.image_size = UVec2::new(123, 101);
    store.initialize(
        (0..21)
            .map(|id| d.correct_position(PieceId(id)) + Vec2::splat(1000.))
            .collect(),
    );
    store.connectivity.union(PieceId(0), PieceId(1));
    store.connectivity.union(PieceId(0), PieceId(7));
    let mut members = PieceBitSet::new(21);
    members.extend([PieceId(0), PieceId(1), PieceId(7)]);
    store.drag.members = members.words().clone();
    store.apply_command(
        LOCAL_PLAYER,
        &PieceCommand::GrabGroup {
            members: members.clone(),
        },
        Some(&d),
        puzzella_core::LOCAL_PLAYER,
    );
    let before = store.states.to_vec();
    for turn in 0..400 {
        assert!(rotate_drag(&mut store, &d, &members, 1).drag_rebased);
        let rotation = (turn + 1) % 4;
        let translation =
            store.states[0].position - rotate_quarter(d.correct_position(PieceId(0)), rotation);
        for id in members.iter() {
            assert!(matches_transform(
                store.states[id.0 as usize].position,
                d.correct_position(id),
                rotation,
                translation
            ));
            if rotation == 0 {
                assert!(
                    store.states[id.0 as usize]
                        .position
                        .distance(before[id.0 as usize].position)
                        < 0.001
                );
            }
        }
    }
}

#[test]
fn invalid_drag_sibling_rejects_the_whole_operation_before_mutation() {
    for case in 0..11 {
        let (d, mut store, mut members) = fixture();
        match case {
            0 => store.states[1].flags |= PLACED,
            1 => store.states[1].flags &= !ENABLED,
            2 => store.held_by.insert(PieceId(1), PlayerId(99)),
            3 => store.states[1].flags &= !HELD,
            4 => store.states[1].flags = with_rotation(store.states[1].flags, 1),
            5 => store.states[1].position.x += 1.,
            6 => {
                members.remove(&PieceId(0));
                store.drag.members = members.words().clone();
            }
            7 => store.held_by.remove(&PieceId(1)),
            8 => {}
            9 => {
                store.states[2].position = Vec2::splat(f32::MAX);
                store.drag.delta = Vec2::splat(f32::MAX);
            }
            10 => {
                members.remove(&PieceId(1));
                store.drag.members = members.words().clone();
            }
            _ => unreachable!(),
        }
        let before = store.states.to_vec();
        let dirty = store.dirty_pieces.clone();
        let delta = store.drag.delta;
        let roots = store.component_root_dirty.clone();
        let command = PieceCommand::RotateDrag {
            members,
            delta: if case == 8 { Vec2::NAN } else { delta },
            quarter_turns: 1,
        };
        assert!(
            !store
                .apply_command(
                    LOCAL_PLAYER,
                    &command,
                    Some(&d),
                    puzzella_core::LOCAL_PLAYER
                )
                .drag_rebased,
            "case {case}"
        );
        assert_eq!(store.states.to_vec(), before);
        assert_eq!(store.drag.delta, delta);
        assert_eq!(store.dirty_pieces, dirty);
        assert_eq!(store.component_root_dirty, roots);
    }
}

#[test]
fn singleton_drag_rotation_preserves_tiny_and_signed_zero_centers() {
    for position in [Vec2::new(1e-20, -1e-20), Vec2::new(-0.0, 0.0)] {
        let (d, mut store, _) = fixture();
        let mut members = PieceBitSet::new(4);
        members.insert(PieceId(2));
        store.states[2].position = position;
        store.drag.members = members.words().clone();
        store.drag.delta = Vec2::ZERO;
        assert!(rotate_drag(&mut store, &d, &members, -1).drag_rebased);
        assert_eq!(
            store.states[2].position.to_array().map(f32::to_bits),
            position.to_array().map(f32::to_bits)
        );
    }
}

#[test]
fn drag_rotation_defers_same_rotation_neighbor_and_board_snapping_until_release() {
    for mode in 0..3 {
        let board = mode != 0;
        let (d, mut store, _) = fixture();
        store.initialize(
            (0..4)
                .map(|id| d.correct_position(PieceId(id)) + Vec2::splat(500.))
                .collect(),
        );
        let mut members = PieceBitSet::new(4);
        members.insert(PieceId(0));
        store.drag.members = members.words().clone();
        store.apply_command(
            LOCAL_PLAYER,
            &PieceCommand::GrabGroup {
                members: members.clone(),
            },
            Some(&d),
            puzzella_core::LOCAL_PLAYER,
        );
        let desired = if board {
            d.correct_position(PieceId(0))
        } else {
            rotate_quarter(d.correct_position(PieceId(0)), 1) + Vec2::splat(100.)
        };
        store.states[1].position =
            rotate_quarter(d.correct_position(PieceId(1)), 1) + Vec2::splat(100.);
        store.states[1].flags = with_rotation(store.states[1].flags, 1);
        store.drag.delta = desired - store.states[0].position;
        assert!(rotate_drag(&mut store, &d, &members, 1).drag_rebased);
        assert_eq!(store.connectivity.component_size(PieceId(0)), 1);
        assert_eq!(store.placed_count, 0);
        if mode == 1 {
            store.apply_command(
                LOCAL_PLAYER,
                &PieceCommand::ReleaseGroup {
                    members: members.clone(),
                    delta: Vec2::ZERO,
                },
                Some(&d),
                puzzella_core::LOCAL_PLAYER,
            );
            assert_eq!(store.placed_count, 0);
            store.drag.members = members.words().clone();
            store.apply_command(
                LOCAL_PLAYER,
                &PieceCommand::GrabGroup {
                    members: members.clone(),
                },
                Some(&d),
                puzzella_core::LOCAL_PLAYER,
            );
            assert!(rotate_drag(&mut store, &d, &members, -1).drag_rebased);
        }
        if mode == 2 {
            // Return to zero inside the same held gesture, then board snap on Release.
            assert!(rotate_drag(&mut store, &d, &members, -1).drag_rebased);
            assert_eq!(store.held_by.get(&PieceId(0)), Some(&LOCAL_PLAYER));
        }
        store.apply_command(
            LOCAL_PLAYER,
            &PieceCommand::ReleaseGroup {
                members,
                delta: Vec2::ZERO,
            },
            Some(&d),
            puzzella_core::LOCAL_PLAYER,
        );
        if board {
            assert_eq!(store.placed_count, 1);
        } else {
            assert_eq!(store.connectivity.component_size(PieceId(0)), 2);
        }
    }
}

#[test]
fn drag_rotation_upload_is_exact_even_for_fragmented_members_and_pointer_frames_reuse_membership() {
    let (mut d, mut store, _) = fixture();
    d.grid_size = UVec2::new(1000, 1);
    d.image_size = UVec2::new(20000, 30);
    store.initialize(
        (0..1000)
            .map(|id| d.correct_position(PieceId(id)) + Vec2::splat(1000.))
            .collect(),
    );
    let mut members = PieceBitSet::new(1000);
    members.extend((0..300).map(|id| PieceId(id * 2)));
    store.drag.members = members.words().clone();
    store.apply_command(
        LOCAL_PLAYER,
        &PieceCommand::GrabGroup {
            members: members.clone(),
        },
        Some(&d),
        puzzella_core::LOCAL_PLAYER,
    );
    let mut app = App::new();
    app.insert_resource(store)
        .insert_resource(d.clone())
        .init_resource::<PieceUpload>()
        .add_systems(Update, prepare_piece_upload);
    app.update();
    app.update();
    let root_revision = app.world().resource::<PieceUpload>().root_revision;
    assert!(
        rotate_drag(
            &mut app.world_mut().resource_mut::<PieceDataStore>(),
            &d,
            &members,
            1
        )
        .drag_rebased
    );
    app.update();
    let upload = app.world().resource::<PieceUpload>();
    assert_eq!(upload.ranges.len(), 300);
    assert_eq!(
        upload.ranges.iter().map(|r| r.states.len()).sum::<usize>(),
        300
    );
    assert_eq!(upload.root_revision, root_revision);
    let revision = upload.revision;
    let mask = upload.drag.members.clone();
    for tick in 0..100 {
        app.world_mut().resource_mut::<PieceDataStore>().drag.delta = Vec2::splat(tick as f32);
        app.update();
        let upload = app.world().resource::<PieceUpload>();
        assert_eq!(upload.revision, revision);
        assert_eq!(upload.root_revision, root_revision);
        assert!(Arc::ptr_eq(&mask, &upload.drag.members));
        assert!(app
            .world()
            .resource::<PieceDataStore>()
            .dirty_pieces
            .is_empty());
    }
}
