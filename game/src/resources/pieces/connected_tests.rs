use super::*;
use puzzella_core::{matches_translation, GENERATOR_VERSION, LOCAL_PLAYER};

fn fixture(
    grid: UVec2,
    offsets: impl IntoIterator<Item = Vec2>,
) -> (PuzzleDefinition, PieceDataStore) {
    let definition = PuzzleDefinition {
        generator_version: GENERATOR_VERSION,
        seed: 42,
        grid_size: grid,
        image_size: puzzella_core::fit_image_size(
            grid * 20,
            puzzella_core::MAX_PUZZLE_IMAGE_DIMENSION,
        ),
        snap_distance: 5.0,
    };
    let mut store = PieceDataStore::default();
    store.initialize(
        offsets
            .into_iter()
            .enumerate()
            .map(|(id, offset)| definition.correct_position(PieceId(id as u32)) + offset)
            .collect(),
    );
    assert_eq!(store.len(), definition.piece_count());
    (definition, store)
}
fn mask(count: usize, ids: &[u32]) -> PieceBitSet {
    let mut mask = PieceBitSet::new(count);
    mask.extend(ids.iter().copied().map(PieceId));
    mask
}
fn release(
    store: &mut PieceDataStore,
    d: &PuzzleDefinition,
    ids: &[u32],
    delta: Vec2,
) -> AppliedCommand {
    let members = mask(store.len(), ids);
    store.apply_command(
        LOCAL_PLAYER,
        &PieceCommand::GrabGroup {
            members: members.clone(),
        },
        Some(d),
        puzzella_core::LOCAL_PLAYER,
    );
    store.apply_command(
        LOCAL_PLAYER,
        &PieceCommand::ReleaseGroup { members, delta },
        Some(d),
        puzzella_core::LOCAL_PLAYER,
    )
}
fn assert_offset(store: &PieceDataStore, d: &PuzzleDefinition, id: u32, expected: Vec2) {
    for member in store.connectivity.iter_component(PieceId(id)) {
        assert_eq!(
            store.states[member.0 as usize].position,
            d.correct_position(member) + expected
        );
        assert!(matches_translation(
            store.states[member.0 as usize].position,
            d.correct_position(member),
            expected
        ));
    }
}

#[test]
fn component_root_dirty_tracks_only_absorbed_members_and_coalesces_final_roots() {
    let (d, mut s) = fixture(UVec2::new(4, 1), [Vec2::splat(100.0); 4]);
    // Three-member target wins over the released singleton, even with a larger ID.
    s.connectivity.union(PieceId(1), PieceId(2));
    s.connectivity.union(PieceId(1), PieceId(3));
    s.snap_unheld_component(PieceId(0), &d);
    assert_eq!(
        s.component_root_dirty.iter().collect::<Vec<_>>(),
        [PieceId(0)]
    );
    assert_eq!(s.connectivity.find_root(PieceId(0)), PieceId(1));

    let (d, s) = fixture(UVec2::splat(3), [Vec2::splat(100.0); 9]);
    let mut app = App::new();
    app.insert_resource(s)
        .insert_resource(d.clone())
        .init_resource::<PieceUpload>()
        .add_systems(Update, prepare_piece_upload);
    app.update();
    assert_eq!(
        &**app
            .world()
            .resource::<PieceUpload>()
            .initial_roots
            .as_ref()
            .unwrap(),
        &(0..9).collect::<Vec<_>>()
    );
    app.update();
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .snap_unheld_component(PieceId(0), &d);
    app.update();
    let upload = app.world().resource::<PieceUpload>();
    assert_eq!(upload.root_ranges.len(), 1);
    assert_eq!(upload.root_ranges[0].start, 1);
    assert_eq!(upload.root_ranges[0].roots, vec![0; 8]);
    assert!(app
        .world()
        .resource::<PieceDataStore>()
        .component_root_dirty
        .is_empty());
    let revision = upload.root_revision;
    for step in 0..16 {
        app.world_mut().resource_mut::<PieceDataStore>().drag.delta = Vec2::splat(step as f32);
        app.update();
        assert_eq!(
            app.world().resource::<PieceUpload>().root_revision,
            revision
        );
    }
}

#[test]
fn fragmented_component_root_uploads_are_bounded_and_snapshot_epoch_rebuilds_roots() {
    let (_, mut store) = fixture(UVec2::new(300, 1), [Vec2::splat(100.0); 300]);
    let mut upload = PieceUpload::default();
    prepare_component_root_upload(&mut store, &mut upload);
    assert_eq!(upload.initial_roots.as_ref().unwrap().len(), 300);
    upload.epoch = store.epoch;
    prepare_component_root_upload(&mut store, &mut upload);
    store
        .component_root_dirty
        .extend((1..300).step_by(2).map(PieceId));
    prepare_component_root_upload(&mut store, &mut upload);
    assert_eq!(upload.root_ranges.len(), 1);
    assert_eq!(upload.root_ranges[0].start, 1);
    assert_eq!(upload.root_ranges[0].roots, (1..300).collect::<Vec<_>>());
    let mut connectivity = PieceConnectivity::new(300);
    connectivity.union(PieceId(3), PieceId(7));
    store.replace_snapshot_states(store.states.to_vec(), store.next_z_order, connectivity);
    prepare_component_root_upload(&mut store, &mut upload);
    let roots = upload.initial_roots.as_ref().unwrap();
    assert_eq!((roots[3], roots[7], roots[8]), (3, 3, 8));
    assert!(upload.root_ranges.is_empty());
}

#[test]
fn same_release_root_changes_upload_the_final_winner_once_per_dirty_member() {
    let (d, mut store) = fixture(UVec2::new(6, 1), [Vec2::splat(100.0); 6]);
    store.connectivity.union(PieceId(2), PieceId(3));
    store.connectivity.union(PieceId(2), PieceId(4));
    let mut upload = PieceUpload::default();
    prepare_component_root_upload(&mut store, &mut upload);
    upload.epoch = store.epoch;
    prepare_component_root_upload(&mut store, &mut upload);
    store.snap_unheld_component(PieceId(0), &d);
    assert_eq!(
        store.component_root_dirty.iter().collect::<Vec<_>>(),
        [PieceId(0), PieceId(1), PieceId(5)]
    );
    prepare_component_root_upload(&mut store, &mut upload);
    assert_eq!(upload.root_ranges.len(), 2);
    assert_eq!(
        (upload.root_ranges[0].start, &upload.root_ranges[0].roots),
        (0, &vec![2, 2])
    );
    assert_eq!(
        (upload.root_ranges[1].start, &upload.root_ranges[1].roots),
        (5, &vec![2])
    );
}

fn assert_render_edges(store: &PieceDataStore, d: &PuzzleDefinition) {
    for (index, state) in store.states.iter().enumerate() {
        let id = PieceId(index as u32);
        let expected = d
            .neighbors(id)
            .into_iter()
            .zip(CONNECTED_EDGE_PAIRS)
            .filter_map(|(neighbor, (edge, _))| {
                neighbor
                    .filter(|&n| store.connectivity.same_component(id, n))
                    .map(|_| edge)
            })
            .fold(0, |flags, edge| flags | edge);
        assert_eq!(state.flags & CONNECTED_EDGES, expected, "piece {index}");
    }
}

#[test]
fn render_edge_cache_is_symmetric_and_dirties_only_the_joined_pair() {
    assert_eq!(std::mem::size_of::<GpuPieceState>(), 16);
    for (grid, edges) in [
        (UVec2::new(3, 1), [CONNECTED_RIGHT, CONNECTED_LEFT]),
        (UVec2::new(1, 3), [CONNECTED_BOTTOM, CONNECTED_TOP]),
    ] {
        let (d, mut s) = fixture(grid, [100.0, 100.0, 300.0].map(Vec2::splat));
        let before = s.states.to_vec();
        s.snap_unheld_component(PieceId(0), &d);
        assert_eq!(
            s.dirty_pieces.iter().collect::<Vec<_>>(),
            [PieceId(0), PieceId(1)]
        );
        for (index, edge) in edges.into_iter().enumerate() {
            assert_eq!(s.states[index].flags, before[index].flags | edge);
            assert_eq!(s.states[index].position, before[index].position);
            assert_eq!(s.states[index].z_order, before[index].z_order);
        }
        assert_eq!(s.states[2], before[2]);
        assert_render_edges(&s, &d);
        s.dirty_pieces.clear();
        s.snap_unheld_component(PieceId(0), &d);
        assert!(
            s.dirty_pieces.is_empty(),
            "cached edges never dirty the states again"
        );
    }
}

#[test]
fn render_edge_cache_records_cycle_edges_l_shapes_and_holes() {
    for members in [
        vec![0],
        vec![0, 1],
        vec![0, 3],
        vec![0, 1, 3, 4],
        vec![0, 1, 3],
        vec![0, 1, 2, 3, 5, 6, 7, 8],
        (0..9).collect(),
    ] {
        let (d, mut s) = fixture(
            UVec2::splat(3),
            (0..9).map(|id| Vec2::splat(if members.contains(&id) { 100.0 } else { 500.0 })),
        );
        s.snap_unheld_component(PieceId(0), &d);
        assert_eq!(s.connectivity.component_size(PieceId(0)), members.len());
        assert_render_edges(&s, &d);
    }
}

#[test]
fn connected_outline_uploads_only_changed_states_and_stays_idle_during_drag() {
    let (d, s) = fixture(UVec2::new(3, 1), [100.0, 100.0, 300.0].map(Vec2::splat));
    let mut app = App::new();
    app.insert_resource(s)
        .insert_resource(d.clone())
        .init_resource::<PieceUpload>()
        .add_systems(Update, prepare_piece_upload);
    app.update();
    app.update(); // Drop the initial shared upload before incremental edits.
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .snap_unheld_component(PieceId(0), &d);
    app.update();
    let upload = app.world().resource::<PieceUpload>();
    assert_eq!(upload.ranges.len(), 1);
    assert_eq!(upload.ranges[0].start, 0);
    assert_eq!(
        upload.ranges[0].states.len() * std::mem::size_of::<GpuPieceState>(),
        32
    );
    let revision = upload.revision;
    for _ in 0..8 {
        app.update();
        assert_eq!(app.world().resource::<PieceUpload>().revision, revision);
    }
    {
        let mut store = app.world_mut().resource_mut::<PieceDataStore>();
        let members = mask(3, &[0, 1]);
        store.apply_command(
            LOCAL_PLAYER,
            &PieceCommand::GrabGroup {
                members: members.clone(),
            },
            Some(&d),
            puzzella_core::LOCAL_PLAYER,
        );
        store.drag.members = members.words().clone();
    }
    app.update();
    let revision = app.world().resource::<PieceUpload>().revision;
    let before = app.world().resource::<PieceDataStore>().states.to_vec();
    let allocation = app.world().resource::<PieceDataStore>().states.as_ptr();
    let members = app
        .world()
        .resource::<PieceDataStore>()
        .drag
        .members
        .clone();
    for step in 0..64 {
        app.world_mut().resource_mut::<PieceDataStore>().drag.delta = Vec2::splat(step as f32);
        app.update();
        let store = app.world().resource::<PieceDataStore>();
        assert!(store.dirty_pieces.is_empty());
        assert_eq!(&*store.states, before.as_slice());
        assert_eq!(store.states.as_ptr(), allocation);
        assert!(Arc::ptr_eq(&store.drag.members, &members));
        assert_eq!(app.world().resource::<PieceUpload>().revision, revision);
    }
}

#[test]
fn correct_neighbors_connect_off_board_with_strict_threshold() {
    for (distance, connected) in [(5.0, false), (4.99, true)] {
        let (d, mut s) = fixture(
            UVec2::new(2, 1),
            [Vec2::new(100.0, 100.0), Vec2::new(100.0 + distance, 100.0)],
        );
        let result = release(&mut s, &d, &[0], Vec2::ZERO);
        assert_eq!(result.released, 1);
        assert_eq!(result.placed, 0);
        assert_eq!(
            s.connectivity.same_component(PieceId(0), PieceId(1)),
            connected
        );
        if connected {
            assert_offset(&s, &d, 0, Vec2::new(104.99, 100.0));
        }
    }
}

#[test]
fn physically_close_wrong_neighbors_cannot_connect_or_wrap_rows() {
    let (d, mut s) = fixture(
        UVec2::splat(2),
        [
            Vec2::splat(100.0),
            Vec2::splat(500.0),
            Vec2::splat(700.0),
            Vec2::splat(100.0),
        ],
    );
    s.states[3].position = s.states[0].position + Vec2::X;
    release(&mut s, &d, &[0], Vec2::ZERO);
    assert_eq!(s.connectivity.component_size(PieceId(0)), 1);
    assert_render_edges(&s, &d);
    let (d, mut s) = fixture(
        UVec2::splat(2),
        [
            Vec2::splat(500.0),
            Vec2::splat(100.0),
            Vec2::splat(100.0),
            Vec2::splat(700.0),
        ],
    );
    release(&mut s, &d, &[1], Vec2::ZERO);
    assert!(!s.connectivity.same_component(PieceId(1), PieceId(2)));
    assert_render_edges(&s, &d);
}

#[test]
fn single_component_and_component_component_merges_share_the_resolver() {
    let (d, mut s) = fixture(UVec2::new(6, 1), [Vec2::splat(100.0); 6]);
    s.connectivity.union(PieceId(1), PieceId(2));
    s.connectivity.union(PieceId(3), PieceId(4));
    s.connectivity.union(PieceId(4), PieceId(5));
    release(&mut s, &d, &[0], Vec2::ZERO);
    assert_eq!(s.connectivity.component_size(PieceId(5)), 6);
    assert_offset(&s, &d, 0, Vec2::splat(100.0));
}

#[test]
fn one_release_never_moves_the_absorbed_component_to_another_offset() {
    let (d, mut s) = fixture(
        UVec2::new(6, 1),
        [100.0, 100.0, 104.0, 104.0, 108.0, 108.0].map(|x| Vec2::new(x, 100.0)),
    );
    for id in [0, 2, 4] {
        s.connectivity.union(PieceId(id), PieceId(id + 1));
    }
    let mut scratch = snapping::SnapScratch::new(s.len(), &d);
    s.resolve_component_snap(PieceId(0), &mut scratch);
    assert_eq!(scratch.boundary_members, 4);
    assert_eq!(s.connectivity.component_size(PieceId(0)), 4);
    assert!(!s.connectivity.same_component(PieceId(0), PieceId(4)));
    assert_offset(&s, &d, 0, Vec2::new(104.0, 100.0));
    assert_offset(&s, &d, 4, Vec2::new(108.0, 100.0));
}

#[test]
fn large_same_offset_closure_scans_each_boundary_member_once() {
    let (d, mut s) = fixture(
        UVec2::splat(100),
        std::iter::repeat_n(Vec2::splat(1000.0), 10_000),
    );
    let mut scratch = snapping::SnapScratch::new(s.len(), &d);
    s.resolve_component_snap(PieceId(0), &mut scratch);
    assert_eq!(scratch.boundary_members, 10_000);
    assert_eq!(s.connectivity.component_size(PieceId(0)), 10_000);
    assert_offset(&s, &d, 0, Vec2::splat(1000.0));
    assert_render_edges(&s, &d);
}

#[test]
fn multiple_disconnected_components_commit_delta_once_and_stay_independent() {
    let (d, mut s) = fixture(
        UVec2::new(4, 1),
        [100.0, 100.0, 200.0, 200.0].map(Vec2::splat),
    );
    s.connectivity.union(PieceId(0), PieceId(1));
    s.connectivity.union(PieceId(2), PieceId(3));
    let result = release(&mut s, &d, &[0, 3], Vec2::new(17.0, 19.0));
    assert_eq!(result.released, 4);
    assert_eq!(s.connectivity.component_size(PieceId(0)), 2);
    assert_offset(&s, &d, 0, Vec2::new(117.0, 119.0));
    assert_offset(&s, &d, 3, Vec2::new(217.0, 219.0));
}

#[test]
fn simultaneous_releases_see_other_components_after_their_delta_commit() {
    let (d, mut s) = fixture(UVec2::new(2, 1), [Vec2::splat(50.0), Vec2::splat(53.0)]);
    let result = release(&mut s, &d, &[0, 1], Vec2::new(50.0, 60.0));
    assert_eq!(result.released, 2);
    assert_offset(&s, &d, 0, Vec2::new(103.0, 113.0));
}

#[test]
fn partial_bulk_masks_and_scalar_commands_move_only_complete_components() {
    let (d, mut s) = fixture(UVec2::new(4, 1), [Vec2::splat(100.0); 4]);
    for id in 1..4 {
        s.connectivity.union(PieceId(id - 1), PieceId(id));
    }
    let members = mask(4, &[2]);
    assert_eq!(
        s.apply_command(
            LOCAL_PLAYER,
            &PieceCommand::GrabGroup {
                members: members.clone()
            },
            Some(&d),
            puzzella_core::LOCAL_PLAYER
        )
        .grabbed,
        4
    );
    assert_eq!(
        s.apply_command(
            PlayerId(1),
            &PieceCommand::Grab(PieceId(1)),
            Some(&d),
            puzzella_core::LOCAL_PLAYER
        )
        .grabbed,
        0
    );
    s.apply_command(
        PlayerId(1),
        &PieceCommand::Move {
            id: PieceId(1),
            position: Vec2::ZERO,
        },
        Some(&d),
        puzzella_core::LOCAL_PLAYER,
    );
    assert_offset(&s, &d, 0, Vec2::splat(100.0));
    s.dirty_pieces.clear();
    s.apply_command(
        LOCAL_PLAYER,
        &PieceCommand::Move {
            id: PieceId(2),
            position: d.correct_position(PieceId(2)) + Vec2::splat(200.0),
        },
        Some(&d),
        puzzella_core::LOCAL_PLAYER,
    );
    assert_offset(&s, &d, 0, Vec2::splat(200.0));
    assert_eq!(s.dirty_pieces.count(), 4);
    s.dirty_pieces.clear();
    // Duplicate scalar coordinates must not schedule a dense-state upload.
    s.apply_command(
        LOCAL_PLAYER,
        &PieceCommand::Move {
            id: PieceId(2),
            position: d.correct_position(PieceId(2)) + Vec2::splat(200.0),
        },
        Some(&d),
        puzzella_core::LOCAL_PLAYER,
    );
    assert!(s.dirty_pieces.is_empty());
    assert_eq!(
        s.apply_command(
            LOCAL_PLAYER,
            &PieceCommand::ReleaseGroup {
                members,
                delta: Vec2::ONE
            },
            Some(&d),
            puzzella_core::LOCAL_PLAYER
        )
        .released,
        4
    );
    assert_offset(&s, &d, 0, Vec2::splat(201.0));
    assert!(s.held_by.is_empty());
    assert_eq!(
        s.apply_command(
            LOCAL_PLAYER,
            &PieceCommand::Grab(PieceId(1)),
            Some(&d),
            puzzella_core::LOCAL_PLAYER
        )
        .grabbed,
        4
    );
    assert_eq!(
        s.apply_command(
            LOCAL_PLAYER,
            &PieceCommand::Release(PieceId(3)),
            Some(&d),
            puzzella_core::LOCAL_PLAYER
        )
        .released,
        4
    );
    assert!(s.held_by.is_empty());
}

#[test]
fn contradictory_partial_ownership_rejects_whole_component() {
    let (d, mut s) = fixture(UVec2::new(2, 1), [Vec2::splat(100.0); 2]);
    s.connectivity.union(PieceId(0), PieceId(1));
    s.held_by.insert(PieceId(1), PlayerId(1)); // Deliberately stale HELD mirror.
    assert_eq!(
        s.apply_command(
            LOCAL_PLAYER,
            &PieceCommand::Grab(PieceId(0)),
            Some(&d),
            puzzella_core::LOCAL_PLAYER
        )
        .grabbed,
        0
    );
    s.held_by.insert(PieceId(0), LOCAL_PLAYER);
    assert_eq!(
        s.apply_command(
            LOCAL_PLAYER,
            &PieceCommand::Release(PieceId(0)),
            Some(&d),
            puzzella_core::LOCAL_PLAYER
        )
        .released,
        0
    );
    assert_eq!(s.held_by.len(), 2);
    assert_eq!(
        crate::multiplayer::release_player_holds(&mut s, LOCAL_PLAYER),
        [PieceId(0), PieceId(1)]
    );
    assert!(s.held_by.is_empty());
    assert_offset(&s, &d, 0, Vec2::splat(100.0));
}

#[test]
fn held_neighbor_components_are_never_absorbed() {
    let (d, mut s) = fixture(UVec2::new(3, 1), [Vec2::splat(100.0); 3]);
    s.connectivity.union(PieceId(1), PieceId(2));
    s.apply_command(
        PlayerId(1),
        &PieceCommand::Grab(PieceId(2)),
        Some(&d),
        puzzella_core::LOCAL_PLAYER,
    );
    release(&mut s, &d, &[0], Vec2::ZERO);
    assert_eq!(s.connectivity.component_size(PieceId(0)), 1);
    assert_eq!(s.held_by.len(), 2);
}

#[test]
fn two_and_hundred_piece_components_place_atomically() {
    for grid in [UVec2::new(2, 1), UVec2::splat(10)] {
        let count = (grid.x * grid.y) as usize;
        let (d, mut s) = fixture(grid, std::iter::repeat_n(Vec2::new(4.0, 0.0), count));
        for id in 1..count as u32 {
            s.connectivity.union(PieceId(0), PieceId(id));
        }
        let result = release(&mut s, &d, &[count as u32 - 1], Vec2::ZERO);
        assert_eq!(result.placed, count);
        assert_eq!(s.placed_count, count);
        assert!(s.states.iter().all(|state| state.flags & PLACED != 0));
        assert!(s.held_by.is_empty());
        assert_offset(&s, &d, 0, Vec2::ZERO);
    }
}

#[test]
fn snapping_to_placed_target_places_all_absorbed_members() {
    let (d, mut s) = fixture(
        UVec2::new(3, 1),
        [Vec2::new(4.0, 0.0), Vec2::new(4.0, 0.0), Vec2::ZERO],
    );
    let mut state = s.state(PieceId(2)).unwrap();
    state.placed = true;
    s.set_state(PieceId(2), state, puzzella_core::LOCAL_PLAYER);
    s.connectivity.union(PieceId(0), PieceId(1));
    let result = release(&mut s, &d, &[0], Vec2::ZERO);
    assert_eq!(result.placed, 2);
    assert_eq!(s.placed_count, 3);
    assert_eq!(s.connectivity.component_size(PieceId(0)), 3);
    assert_offset(&s, &d, 0, Vec2::ZERO);
}

#[test]
fn nearest_candidate_and_minimum_member_ties_are_deterministic() {
    for _ in 0..20 {
        // Middle piece has equally distant correct neighbors; stable PieceId 0 wins.
        let (d, mut s) = fixture(
            UVec2::new(3, 1),
            [100.0, 102.0, 104.0].map(|x| Vec2::new(x, 100.0)),
        );
        release(&mut s, &d, &[1], Vec2::ZERO);
        assert_eq!(s.connectivity.component_size(PieceId(1)), 2);
        assert!(!s.connectivity.same_component(PieceId(1), PieceId(2)));
        assert_offset(&s, &d, 1, Vec2::new(100.0, 100.0));
        let (d, mut s) = fixture(
            UVec2::new(3, 1),
            [98.0, 102.0, 106.0].map(|x| Vec2::new(x, 100.0)),
        );
        release(&mut s, &d, &[1], Vec2::ZERO);
        assert!(s.connectivity.same_component(PieceId(1), PieceId(0)));
        assert!(!s.connectivity.same_component(PieceId(1), PieceId(2)));
        assert_offset(&s, &d, 1, Vec2::new(98.0, 100.0));
    }
}

#[test]
fn union_expands_selection_and_only_changed_dense_members_become_dirty() {
    let (d, mut s) = fixture(UVec2::new(2, 1), [Vec2::splat(100.0); 2]);
    s.selected_pieces.insert(PieceId(0));
    s.apply_command(
        LOCAL_PLAYER,
        &PieceCommand::Grab(PieceId(0)),
        Some(&d),
        puzzella_core::LOCAL_PLAYER,
    );
    s.dirty_pieces.clear();
    s.apply_command(
        LOCAL_PLAYER,
        &PieceCommand::Release(PieceId(0)),
        Some(&d),
        puzzella_core::LOCAL_PLAYER,
    );
    assert_eq!(s.selected_pieces.count(), 2);
    assert_eq!(
        s.dirty_pieces.iter().collect::<Vec<_>>(),
        [PieceId(0), PieceId(1)]
    );
}

#[test]
fn component_front_preserves_relative_z_including_compaction() {
    let (_, mut s) = fixture(UVec2::new(3, 1), [Vec2::splat(100.0); 3]);
    s.connectivity.union(PieceId(0), PieceId(1));
    s.states[0].z_order = 10;
    s.states[1].z_order = 5;
    s.next_z_order = MAX_Z;
    s.bring_piece_to_front(PieceId(0));
    assert!(s.states[0].z_order > s.states[1].z_order);
    assert!(s.states[1].z_order > s.states[2].z_order);
    assert!(s.next_z_order <= MAX_Z);
}

#[test]
fn million_member_component_partial_grab_release_and_expansion_complete() {
    let (d, mut s) = fixture(
        UVec2::splat(1000),
        std::iter::repeat_n(Vec2::splat(10_000.0), 1_000_000),
    );
    for id in 1..1_000_000 {
        s.connectivity.union(PieceId(id - 1), PieceId(id));
    }
    assert_eq!(s.connectivity.storage_bytes(), 8_000_000);
    assert_eq!(
        s.connectivity.expand(&mask(s.len(), &[777_777])).count(),
        1_000_000
    );
    assert_eq!(
        s.connectivity.iter_component(PieceId(777_777)).count(),
        1_000_000
    );
    let result = release(&mut s, &d, &[777_777], Vec2::ONE);
    assert_eq!(result.released, 1_000_000);
    assert!(s.held_by.is_empty());
    assert_offset(&s, &d, 0, Vec2::splat(10_001.0));
}

#[test]
fn connected_board_threshold_is_strict_and_disconnect_does_not_snap() {
    let (d, mut s) = fixture(UVec2::new(2, 1), [Vec2::new(5.0, 0.0); 2]);
    s.connectivity.union(PieceId(0), PieceId(1));
    release(&mut s, &d, &[1], Vec2::ZERO);
    assert_eq!(s.placed_count, 0);
    s.apply_command(
        LOCAL_PLAYER,
        &PieceCommand::Grab(PieceId(0)),
        Some(&d),
        puzzella_core::LOCAL_PLAYER,
    );
    let before = s.states.clone();
    assert_eq!(
        crate::multiplayer::release_player_holds(&mut s, LOCAL_PLAYER),
        [PieceId(0), PieceId(1)]
    );
    assert!(s.held_by.is_empty());
    for id in 0..2 {
        assert_eq!(s.states[id].position, before[id].position);
        assert_eq!(s.states[id].z_order, before[id].z_order);
    }
    assert_eq!(s.placed_count, 0);
    assert_eq!(s.connectivity.component_size(PieceId(0)), 2);
}

#[test]
fn overflowing_release_rejects_gameplay_translation_and_keeps_numeric_fallback_without_definition()
{
    let (d, mut s) = fixture(UVec2::new(2, 1), [Vec2::splat(f32::MAX); 2]);
    s.connectivity.union(PieceId(0), PieceId(1));
    let result = release(&mut s, &d, &[0], Vec2::splat(f32::MAX));
    assert_eq!(result.released, 0);
    assert_eq!(s.held_by.len(), 2);
    let result = s.release_roots(
        LOCAL_PLAYER,
        vec![PieceId(0)],
        Vec2::splat(f32::MAX),
        None,
        LOCAL_PLAYER,
    );
    assert_eq!(result.released, 2);
    assert!(s.held_by.is_empty());
    assert!(s
        .states
        .iter()
        .all(|state| state.position == Vec2::splat(f32::MAX)));
}

#[test]
fn nearby_boundary_at_another_offset_is_not_reconsidered_after_snap() {
    let (d, mut s) = fixture(
        UVec2::splat(2),
        [108.0, 100.0, 104.0, 100.0].map(|x| Vec2::new(x, 100.0)),
    );
    s.connectivity.union(PieceId(1), PieceId(3));
    release(&mut s, &d, &[1], Vec2::ZERO);
    assert_eq!(s.connectivity.component_size(PieceId(1)), 3);
    assert!(!s.connectivity.same_component(PieceId(1), PieceId(0)));
    assert_offset(&s, &d, 1, Vec2::new(104.0, 100.0));
}

#[test]
fn fully_selected_component_merge_preserves_shared_mask_when_membership_is_unchanged() {
    let (d, mut s) = fixture(
        UVec2::new(4, 1),
        [100.0, 100.0, 102.0, 102.0].map(Vec2::splat),
    );
    s.connectivity.union(PieceId(0), PieceId(1));
    s.connectivity.union(PieceId(2), PieceId(3));
    s.selected_pieces.fill();
    let selected = s.selected_pieces.words().clone();
    release(&mut s, &d, &[0], Vec2::ZERO);
    assert_eq!(s.connectivity.component_size(PieceId(0)), 4);
    assert!(Arc::ptr_eq(&selected, s.selected_pieces.words()));
}

#[test]
fn finite_scalar_singleton_move_still_accepts_an_overflowing_difference() {
    let mut s = PieceDataStore::default();
    s.initialize(vec![Vec2::splat(f32::MAX)]);
    s.apply_command(
        LOCAL_PLAYER,
        &PieceCommand::Grab(PieceId(0)),
        None,
        puzzella_core::LOCAL_PLAYER,
    );
    s.apply_command(
        LOCAL_PLAYER,
        &PieceCommand::Move {
            id: PieceId(0),
            position: Vec2::splat(-f32::MAX),
        },
        None,
        puzzella_core::LOCAL_PLAYER,
    );
    assert_eq!(s.states[0].position, Vec2::splat(-f32::MAX));
    assert_eq!(
        s.apply_command(
            LOCAL_PLAYER,
            &PieceCommand::Release(PieceId(0)),
            None,
            puzzella_core::LOCAL_PLAYER
        )
        .released,
        1
    );
}

#[test]
fn alternating_board_releases_union_without_rescanning_the_growing_cluster() {
    let grid = UVec2::splat(100);
    let (d, mut s) = fixture(
        grid,
        (0..10_000).map(|id| {
            Vec2::new(
                if (id % 100 + id / 100) % 2 == 0 {
                    4.0
                } else {
                    -4.0
                },
                0.0,
            )
        }),
    );
    let mut all = PieceBitSet::new(s.len());
    all.fill();
    s.apply_command(
        LOCAL_PLAYER,
        &PieceCommand::GrabGroup {
            members: all.clone(),
        },
        Some(&d),
        puzzella_core::LOCAL_PLAYER,
    );
    let result = s.apply_command(
        LOCAL_PLAYER,
        &PieceCommand::ReleaseGroup {
            members: all,
            delta: Vec2::ZERO,
        },
        Some(&d),
        puzzella_core::LOCAL_PLAYER,
    );
    assert_eq!((result.released, result.placed), (10_000, 10_000));
    assert_eq!(s.connectivity.component_size(PieceId(0)), 10_000);
    assert_eq!(s.placed_count, 10_000);
    assert_offset(&s, &d, 0, Vec2::ZERO);
    assert_render_edges(&s, &d);
}

#[test]
fn no_chained_translation_from_zero_four_eight() {
    // A common y offset keeps the board out of range; x offsets are 0, 4, 8.
    let (d, mut s) = fixture(
        UVec2::new(3, 1),
        [0.0, 4.0, 8.0].map(|x| Vec2::new(x, 100.0)),
    );
    release(&mut s, &d, &[0], Vec2::ZERO);
    assert!(s.connectivity.same_component(PieceId(0), PieceId(1)));
    assert!(!s.connectivity.same_component(PieceId(0), PieceId(2)));
    assert_offset(&s, &d, 0, Vec2::new(4.0, 100.0));
    assert_offset(&s, &d, 2, Vec2::new(8.0, 100.0));
}

#[test]
fn same_final_offset_closure_includes_newly_exposed_neighbors() {
    let (d, mut s) = fixture(
        UVec2::new(4, 1),
        [0.0, 4.0, 4.0, 4.0].map(|x| Vec2::new(x, 100.0)),
    );
    release(&mut s, &d, &[0], Vec2::ZERO);
    assert_eq!(s.connectivity.component_size(PieceId(0)), 4);
    assert_offset(&s, &d, 0, Vec2::new(4.0, 100.0));
}

#[test]
fn board_has_priority_even_when_neighbor_is_closer() {
    let (d, mut s) = fixture(UVec2::new(2, 1), [Vec2::new(3.0, 0.0), Vec2::new(4.0, 0.0)]);
    release(&mut s, &d, &[0], Vec2::ZERO);
    assert_eq!(s.placed_count, 1);
    assert_offset(&s, &d, 0, Vec2::ZERO);
    assert_offset(&s, &d, 1, Vec2::new(4.0, 0.0));
    assert!(!s.connectivity.same_component(PieceId(0), PieceId(1)));
}

#[test]
fn final_offset_union_rejects_offsets_beyond_arithmetic_rounding() {
    let (d, mut s) = fixture(
        UVec2::splat(2),
        [100.0, 104.0, 105.0, 108.0].map(|x| Vec2::new(x, 50.0)),
    );
    release(&mut s, &d, &[0], Vec2::ZERO);
    assert_eq!(s.connectivity.component_size(PieceId(0)), 2);
    assert_offset(&s, &d, 0, Vec2::new(104.0, 50.0));
    assert_offset(&s, &d, 2, Vec2::new(105.0, 50.0));
    assert_offset(&s, &d, 3, Vec2::new(108.0, 50.0));
}

#[test]
fn multiple_released_components_snap_independently_and_never_move_resolved_targets() {
    let (d, mut s) = fixture(
        UVec2::new(4, 1),
        [100.0, 104.0, 108.0, 112.0].map(|x| Vec2::new(x, 100.0)),
    );
    let result = release(&mut s, &d, &[0, 1, 2, 3], Vec2::ZERO);
    assert_eq!(result.released, 4);
    assert_eq!(s.connectivity.component_size(PieceId(0)), 3);
    assert_offset(&s, &d, 0, Vec2::new(104.0, 100.0));
    assert_offset(&s, &d, 3, Vec2::new(112.0, 100.0));

    let (d, mut s) = fixture(
        UVec2::new(6, 1),
        [100.0, 104.0, 150.0, 200.0, 204.0, 260.0].map(|x| Vec2::new(x, 100.0)),
    );
    release(&mut s, &d, &[0, 3], Vec2::ZERO);
    assert_offset(&s, &d, 0, Vec2::new(104.0, 100.0));
    assert_offset(&s, &d, 3, Vec2::new(204.0, 100.0));
    assert_eq!(s.connectivity.component_size(PieceId(0)), 2);
    assert_eq!(s.connectivity.component_size(PieceId(3)), 2);
}

#[test]
fn invalid_nearest_component_is_rejected_as_a_whole_before_choosing_valid_target() {
    for invalid in ["owner", "translation", "placed", "disabled"] {
        let (d, mut s) = fixture(
            UVec2::splat(2),
            [98.0, 100.0, 98.0, 103.0].map(|x| Vec2::new(x, 100.0)),
        );
        s.connectivity.union(PieceId(0), PieceId(2));
        match invalid {
            "owner" => {
                s.held_by.insert(PieceId(2), PlayerId(1));
            }
            "translation" => {
                s.states[2].position += Vec2::splat(20.0);
            }
            "placed" => {
                s.states[2].flags |= PLACED;
            }
            "disabled" => {
                s.states[2].flags &= !ENABLED;
            }
            _ => unreachable!(),
        }
        release(&mut s, &d, &[1], Vec2::ZERO);
        assert!(
            s.connectivity.same_component(PieceId(1), PieceId(3)),
            "{invalid}"
        );
        assert!(
            !s.connectivity.same_component(PieceId(1), PieceId(0)),
            "{invalid}"
        );
        assert_offset(&s, &d, 1, Vec2::new(103.0, 100.0));
    }
}

#[test]
fn fractional_closure_accepts_rounding_in_both_axes_without_moving_targets() {
    for (grid, image, offset, delta) in [
        (
            UVec2::new(3, 1),
            UVec2::new(4096, 20),
            Vec2::new(100.37, 0.0),
            Vec2::new(4.0, 0.0),
        ),
        (
            UVec2::new(1, 3),
            UVec2::new(20, 4096),
            Vec2::new(0.0, 100.37),
            Vec2::new(0.0, 4.0),
        ),
    ] {
        let d = PuzzleDefinition {
            generator_version: GENERATOR_VERSION,
            seed: 42,
            grid_size: grid,
            image_size: image,
            snap_distance: 5.0,
        };
        let mut s = PieceDataStore::default();
        s.initialize(
            (0..3)
                .map(|id| {
                    d.correct_position(PieceId(id)) + if id == 0 { offset - delta } else { offset }
                })
                .collect(),
        );
        let recovered = |id: u32| s.states[id as usize].position - d.correct_position(PieceId(id));
        assert_ne!(recovered(1), recovered(2));
        let targets = s.states[1..].to_vec();
        let result = release(&mut s, &d, &[0], Vec2::ZERO);
        assert_eq!((result.released, result.placed), (1, 0));
        assert_eq!(s.connectivity.component_size(PieceId(0)), 3);
        assert_eq!(
            s.states[0].position,
            d.correct_position(PieceId(0)) + offset
        );
        for (state, before) in s.states[1..].iter().zip(targets) {
            assert_eq!(
                state.position.to_array().map(f32::to_bits),
                before.position.to_array().map(f32::to_bits)
            );
            assert_eq!(state.flags & !CONNECTED_EDGES, before.flags);
            assert_eq!(state.z_order, before.z_order);
        }
    }
}

#[test]
fn fractional_closure_rejects_a_nearby_different_translation() {
    let d = PuzzleDefinition {
        generator_version: GENERATOR_VERSION,
        seed: 42,
        grid_size: UVec2::new(3, 1),
        image_size: UVec2::new(4096, 20),
        snap_distance: 5.0,
    };
    let mut s = PieceDataStore::default();
    s.initialize(
        [96.37, 100.37, 100.38]
            .into_iter()
            .enumerate()
            .map(|(id, x)| d.correct_position(PieceId(id as u32)) + Vec2::new(x, 0.0))
            .collect(),
    );
    let before = s.states[2];
    release(&mut s, &d, &[0], Vec2::ZERO);
    assert_eq!(s.connectivity.component_size(PieceId(0)), 2);
    assert!(!s.connectivity.same_component(PieceId(0), PieceId(2)));
    assert_eq!(s.states[2], before);
}

#[test]
fn closure_validates_every_target_member_against_the_fixed_offset() {
    let d = PuzzleDefinition {
        generator_version: GENERATOR_VERSION,
        seed: 42,
        grid_size: UVec2::splat(3),
        image_size: UVec2::splat(4096),
        snap_distance: 5.0,
    };
    let mut s = PieceDataStore::default();
    s.initialize(
        [
            100.37, 100.37, 5000.0, 5000.0, 104.37, 5000.0, 5000.0, 100.37065, 5000.0,
        ]
        .into_iter()
        .enumerate()
        .map(|(id, x)| d.correct_position(PieceId(id as u32)) + Vec2::new(x, 1000.0))
        .collect(),
    );
    s.connectivity.union(PieceId(0), PieceId(1));
    let final_offset = s.states[7].position - d.correct_position(PieceId(7));
    assert!(matches_translation(
        s.states[0].position,
        d.correct_position(PieceId(0)),
        final_offset
    ));
    assert!(!matches_translation(
        s.states[1].position,
        d.correct_position(PieceId(1)),
        final_offset
    ));
    release(&mut s, &d, &[4], Vec2::ZERO);
    assert!(s.connectivity.same_component(PieceId(4), PieceId(7)));
    assert!(!s.connectivity.same_component(PieceId(4), PieceId(0)));
}

#[test]
fn small_component_snap_on_a_million_piece_puzzle_keeps_scratch_masks_on_stack() {
    for members in [1, 32] {
        let (d, mut s) = fixture(
            UVec2::splat(1000),
            (0..1_000_000).map(|id| {
                Vec2::new(
                    10000.0 + if id > members { id as f32 * 20.0 } else { 0.0 },
                    10000.0,
                )
            }),
        );
        for id in 1..members {
            s.connectivity.union(PieceId(0), PieceId(id));
        }
        let mut scratch = snapping::SnapScratch::new(s.len(), &d);
        assert_eq!(scratch.mask_heap_bytes(), 0);
        s.resolve_component_snap(PieceId(0), &mut scratch);
        assert_eq!(
            s.connectivity.component_size(PieceId(0)),
            members as usize + 1
        );
        assert_eq!(scratch.mask_heap_bytes(), 0);
    }
}

#[test]
fn growing_fractional_cluster_keeps_one_logical_offset_and_scans_boundaries_once() {
    let d = PuzzleDefinition {
        generator_version: GENERATOR_VERSION,
        seed: 42,
        grid_size: UVec2::splat(100),
        image_size: UVec2::splat(4096),
        snap_distance: 5.0,
    };
    let mut s = PieceDataStore::default();
    s.initialize(
        (0..10_000)
            .map(|id| {
                d.correct_position(PieceId(id))
                    + Vec2::new(
                        if (id % 100 + id / 100) % 2 == 0 {
                            100.37
                        } else {
                            104.37
                        },
                        1000.37,
                    )
            })
            .collect(),
    );
    let mut scratch = snapping::SnapScratch::new(s.len(), &d);
    for id in 0..10_000 {
        let root = s.connectivity.find_root(PieceId(id));
        if !scratch.resolved.contains(&root) {
            s.resolve_component_snap(root, &mut scratch);
        }
    }
    assert_eq!(s.connectivity.component_size(PieceId(0)), 10_000);
    assert_eq!(scratch.boundary_members, 10_000);
    let offset = s.states[0].position - d.correct_position(PieceId(0));
    assert!(s.states.iter().enumerate().all(|(id, state)| {
        matches_translation(
            state.position,
            d.correct_position(PieceId(id as u32)),
            offset,
        )
    }));
}

#[test]
fn small_grab_plans_in_a_million_piece_puzzle_have_no_membership_heap() {
    for members in [1, 8, 32] {
        let mut s = PieceDataStore::default();
        s.initialize(vec![Vec2::ZERO; 1_000_000]);
        let base = 700_000;
        for member in 1..members {
            s.connectivity.union(PieceId(base), PieceId(base + member));
        }
        s.held_by.ensure_len(s.len()); // Persistent first-hold allocation is separate.
        let id = PieceId(base + members / 2);
        let plan = s.grab_roots([s.connectivity.minimum_member(id)], None);
        assert!(plan.members.is_none());
        assert_eq!(plan.ids.len(), members as usize);
        assert!(plan.ids.capacity() <= 32);
        let requested = mask(s.len(), &[id.0, base + members - 1]);
        let mut scratch = PieceScratchSet::new(s.len());
        let group_plan = s.grab_roots(s.grab_component_roots(&requested, &mut scratch), None);
        assert_eq!(scratch.heap_bytes(), 0);
        assert!(group_plan.members.is_none());
        assert_eq!(group_plan.ids.len(), members as usize);
        assert!(group_plan.ids.capacity() <= 32);
        let states = s.states.as_ptr();
        let owners = s.held_by.owners.as_ptr();
        let occupancy = s.held_by.occupied.words().as_ptr();
        let dirty = s.dirty_pieces.words().as_ptr();
        assert_eq!(
            s.apply_command(
                LOCAL_PLAYER,
                &PieceCommand::Grab(id),
                None,
                puzzella_core::LOCAL_PLAYER
            )
            .grabbed,
            members as usize
        );
        assert_eq!(s.dirty_pieces.count(), members as usize);
        assert_eq!(s.held_by.len(), members as usize);
        assert_eq!(s.states.as_ptr(), states);
        assert_eq!(s.held_by.owners.as_ptr(), owners);
        assert_eq!(s.held_by.occupied.words().as_ptr(), occupancy);
        assert_eq!(s.dirty_pieces.words().as_ptr(), dirty);
        assert!(s.drag.members.is_empty());
    }
}

#[test]
fn scalar_and_partial_group_grab_preserve_local_selection_and_duplicate_counts() {
    for scalar in [false, true] {
        let (_, mut s) = fixture(UVec2::new(4, 1), [Vec2::splat(100.0); 4]);
        // The stable minimum differs from the DSU root.
        s.connectivity.union(PieceId(1), PieceId(2));
        s.connectivity.union(PieceId(0), PieceId(2));
        assert_ne!(s.connectivity.find_root(PieceId(0)), PieceId(0));
        s.selected_pieces.fill();
        let selected = s.selected_pieces.words().clone();
        let command = if scalar {
            PieceCommand::Grab(PieceId(2))
        } else {
            PieceCommand::GrabGroup {
                members: mask(4, &[1, 2]),
            }
        };
        assert_eq!(
            s.apply_command(LOCAL_PLAYER, &command, None, puzzella_core::LOCAL_PLAYER)
                .grabbed,
            3
        );
        assert!(Arc::ptr_eq(&selected, s.selected_pieces.words()));
        for id in 0..3 {
            assert_eq!(s.held_by.get(&PieceId(id)), Some(&LOCAL_PLAYER));
            assert_ne!(s.states[id as usize].flags & HELD, 0);
        }
        s.dirty_pieces.clear();
        let states = s.states.to_vec();
        let next_z = s.next_z_order;
        for player in [LOCAL_PLAYER, PlayerId(1)] {
            assert_eq!(
                s.apply_command(player, &command, None, puzzella_core::LOCAL_PLAYER)
                    .grabbed,
                0
            );
            assert_eq!(s.held_by.len(), 3);
            assert_eq!(s.held_by.counts.get(&LOCAL_PLAYER), Some(&3));
            assert_eq!(&*s.states, states.as_slice());
            assert_eq!(s.next_z_order, next_z);
            assert!(s.dirty_pieces.is_empty());
        }
    }
}

#[test]
fn group_grab_rejects_invalid_components_atomically_and_accepts_siblings() {
    for invalid in [
        "remote_owner",
        "local_owner",
        "remote_held",
        "placed",
        "disabled",
        "stale_held",
    ] {
        for scalar in [false, true] {
            let (_, mut s) = fixture(UVec2::new(4, 1), [Vec2::splat(100.0); 4]);
            s.connectivity.union(PieceId(0), PieceId(1));
            s.connectivity.union(PieceId(2), PieceId(3));
            match invalid {
                "remote_owner" => s.held_by.insert(PieceId(1), PlayerId(1)),
                "local_owner" => s.held_by.insert(PieceId(1), LOCAL_PLAYER),
                "remote_held" => {
                    s.held_by.insert(PieceId(1), PlayerId(1));
                    s.states[1].flags |= HELD;
                }
                "placed" => s.states[1].flags |= PLACED,
                "disabled" => s.states[1].flags &= !ENABLED,
                _ => s.states[1].flags |= HELD,
            }
            let before = s.states[..2].to_vec();
            let command = if scalar {
                PieceCommand::Grab(PieceId(0))
            } else {
                PieceCommand::GrabGroup {
                    members: mask(4, &[0, 3]),
                }
            };
            assert_eq!(
                s.apply_command(LOCAL_PLAYER, &command, None, puzzella_core::LOCAL_PLAYER)
                    .grabbed,
                if scalar { 0 } else { 2 },
                "{invalid}"
            );
            assert_eq!(&s.states[..2], before.as_slice());
            assert!(s.held_by.get(&PieceId(0)).is_none());
            assert_eq!(s.dirty_pieces.count(), if scalar { 0 } else { 2 });
            if !scalar {
                assert_eq!(s.held_by.get(&PieceId(2)), Some(&LOCAL_PLAYER));
                assert_eq!(s.held_by.get(&PieceId(3)), Some(&LOCAL_PLAYER));
            }
        }
    }
}

#[test]
fn group_grab_preserves_cross_component_z_ties_and_max_z_compaction() {
    for compact in [false, true] {
        for full in [false, true] {
            let (_, mut s) = fixture(UVec2::new(7, 1), [Vec2::splat(100.0); 7]);
            s.connectivity.union(PieceId(1), PieceId(2));
            s.connectivity.union(PieceId(0), PieceId(2));
            s.connectivity.union(PieceId(3), PieceId(4));
            for (state, z) in s.states.iter_mut().zip([10, 30, 30, 20, 30, 15, 50]) {
                state.z_order = z;
            }
            s.next_z_order = if compact { MAX_Z - 1 } else { 100 };
            let command = PieceCommand::GrabGroup {
                members: mask(
                    7,
                    if full {
                        &[0, 1, 2, 3, 4, 5]
                    } else {
                        &[1, 2, 4, 5]
                    },
                ),
            };
            assert_eq!(
                s.apply_command(LOCAL_PLAYER, &command, None, puzzella_core::LOCAL_PLAYER)
                    .grabbed,
                6
            );
            let mut order: Vec<_> = (0..6).collect();
            order.sort_unstable_by_key(|&id| (s.states[id].z_order, id));
            assert_eq!(order, [0, 5, 3, 1, 2, 4]);
            assert!(s.states[6].z_order < s.states[0].z_order);
            assert!(s.next_z_order <= MAX_Z);
            assert_eq!(s.dirty_pieces.count(), if compact { 7 } else { 6 });
        }
    }
}

#[test]
fn local_group_grab_syncs_partial_drag_to_complete_accepted_components() {
    for full in [false, true] {
        let (_, mut s) = fixture(UVec2::new(5, 1), [Vec2::splat(100.0); 5]);
        s.connectivity.union(PieceId(0), PieceId(1));
        s.connectivity.union(PieceId(2), PieceId(3));
        s.held_by.insert(PieceId(1), PlayerId(1));
        let requested = mask(5, if full { &[0, 1, 2, 3, 4] } else { &[0, 3, 4] });
        let frozen = requested.words().clone();
        s.drag = DragTransform {
            members: requested.words().clone(),
            delta: Vec2::ONE,
        };
        assert_eq!(
            s.apply_command(
                LOCAL_PLAYER,
                &PieceCommand::GrabGroup {
                    members: requested.clone()
                },
                None,
                puzzella_core::LOCAL_PLAYER
            )
            .grabbed,
            3
        );
        assert_eq!(
            s.drag.members.as_ref(),
            mask(5, &[2, 3, 4]).words().as_ref()
        );
        assert_eq!(s.drag.delta, Vec2::ONE);
        assert!(!Arc::ptr_eq(&frozen, &s.drag.members));
        assert!(Arc::ptr_eq(&frozen, requested.words()));
        assert_eq!(s.held_by.get(&PieceId(1)), Some(&PlayerId(1)));
        assert_eq!(s.held_by.counts.get(&LOCAL_PLAYER), Some(&3));
    }
}

#[test]
fn all_valid_group_grab_keeps_drag_membership_arc_shared() {
    let (_, mut s) = fixture(UVec2::new(64, 1), [Vec2::splat(100.0); 64]);
    for id in 1..64 {
        s.connectivity.union(PieceId(0), PieceId(id));
    }
    let mut requested = PieceBitSet::new(64);
    requested.fill();
    s.drag.members = requested.words().clone();
    assert_eq!(
        s.apply_command(
            LOCAL_PLAYER,
            &PieceCommand::GrabGroup {
                members: requested.clone()
            },
            None,
            puzzella_core::LOCAL_PLAYER
        )
        .grabbed,
        64
    );
    assert!(Arc::ptr_eq(&s.drag.members, requested.words()));
    assert_eq!(s.held_by.len(), 64);
}

#[test]
fn remote_scalar_and_group_grab_remove_complete_local_selection_and_drag() {
    for scalar in [false, true] {
        let (_, mut s) = fixture(UVec2::new(128, 1), [Vec2::splat(100.0); 128]);
        s.connectivity.union(PieceId(0), PieceId(1));
        s.connectivity.union(PieceId(1), PieceId(2));
        s.selected_pieces.fill();
        let before = s.selected_pieces.clone();
        s.drag = DragTransform {
            members: before.words().clone(),
            delta: Vec2::ONE,
        };
        let command = if scalar {
            PieceCommand::Grab(PieceId(1))
        } else {
            PieceCommand::GrabGroup {
                members: mask(128, &[1, 2]),
            }
        };
        assert_eq!(
            s.apply_command(PlayerId(1), &command, None, puzzella_core::LOCAL_PLAYER)
                .grabbed,
            3
        );
        for id in 0..3 {
            assert!(!s.selected_pieces.contains(&PieceId(id)));
            assert_eq!(s.drag.members[0] & (1 << id), 0);
        }
        assert_eq!(before.count(), 128);
        assert_eq!(s.selected_pieces.count(), 125);
        assert_eq!(s.dirty_pieces.count(), 3);
        assert_eq!(s.drag.delta, Vec2::ONE);
        assert!(s.highlights_dirty);
    }
}

#[test]
fn scalar_grab_of_large_component_updates_every_member() {
    let (_, mut s) = fixture(UVec2::new(128, 1), [Vec2::splat(100.0); 128]);
    for id in 1..100 {
        s.connectivity.union(PieceId(0), PieceId(id));
    }
    assert_eq!(
        s.apply_command(
            LOCAL_PLAYER,
            &PieceCommand::Grab(PieceId(75)),
            None,
            puzzella_core::LOCAL_PLAYER
        )
        .grabbed,
        100
    );
    assert_eq!(s.held_by.len(), 100);
    assert_eq!(s.dirty_pieces.count(), 100);
    assert!(s.drag.members.is_empty());
    assert_eq!(
        s.apply_command(
            LOCAL_PLAYER,
            &PieceCommand::Release(PieceId(15)),
            None,
            puzzella_core::LOCAL_PLAYER
        )
        .released,
        100
    );
    assert!(s.held_by.is_empty());
}

#[test]
fn million_piece_singleton_grab_uploads_exactly_sixteen_bytes() {
    let mut store = PieceDataStore::default();
    store.initialize(vec![Vec2::ZERO; 1_000_000]);
    let mut app = App::new();
    app.insert_resource(store)
        .init_resource::<PieceUpload>()
        .add_systems(Update, prepare_piece_upload);
    app.update();
    app.update();
    let id = PieceId(777_777);
    assert_eq!(
        app.world_mut()
            .resource_mut::<PieceDataStore>()
            .apply_command(
                LOCAL_PLAYER,
                &PieceCommand::Grab(id),
                None,
                puzzella_core::LOCAL_PLAYER
            )
            .grabbed,
        1
    );
    app.update();
    let upload = app.world().resource::<PieceUpload>();
    assert_eq!(upload.ranges.len(), 1);
    assert_eq!(upload.ranges[0].start, id.0);
    assert_eq!(
        upload.ranges[0].states.len() * std::mem::size_of::<GpuPieceState>(),
        16
    );
}

#[test]
fn group_grab_deduplicates_many_partial_roots_and_full_masks_need_no_scratch_heap() {
    let (_, mut s) = fixture(UVec2::new(600, 1), [Vec2::splat(100.0); 600]);
    for id in (0..600).step_by(2) {
        s.connectivity.union(PieceId(id), PieceId(id + 1));
    }
    let mut full = PieceBitSet::new(600);
    full.fill();
    let mut scratch = PieceScratchSet::new(600);
    assert_eq!(s.grab_component_roots(&full, &mut scratch).count(), 300);
    assert_eq!(scratch.heap_bytes(), 0);
    let mut partial = PieceBitSet::new(600);
    partial.extend((1..600).step_by(2).map(PieceId));
    let mut scratch = PieceScratchSet::new(600);
    let roots: Vec<_> = s.grab_component_roots(&partial, &mut scratch).collect();
    assert_eq!(roots, (0..600).step_by(2).map(PieceId).collect::<Vec<_>>());
    assert!(scratch.heap_bytes() > 0);
    assert_eq!(
        s.apply_command(
            LOCAL_PLAYER,
            &PieceCommand::GrabGroup { members: partial },
            None,
            puzzella_core::LOCAL_PLAYER
        )
        .grabbed,
        600
    );
    assert_eq!(s.held_by.len(), 600);
    assert_eq!(s.dirty_pieces.count(), 600);
}
