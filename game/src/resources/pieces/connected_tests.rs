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
        image_size: grid * 20,
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
    );
    store.apply_command(
        LOCAL_PLAYER,
        &PieceCommand::ReleaseGroup { members, delta },
        Some(d),
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
        std::iter::repeat_n(Vec2::splat(10_000.0), 10_000),
    );
    let mut scratch = snapping::SnapScratch::new(s.len(), &d);
    s.resolve_component_snap(PieceId(0), &mut scratch);
    assert_eq!(scratch.boundary_members, 10_000);
    assert_eq!(s.connectivity.component_size(PieceId(0)), 10_000);
    assert_offset(&s, &d, 0, Vec2::splat(10_000.0));
    assert_render_edges(&s, &d);
}

#[test]
fn multiple_disconnected_components_commit_delta_once_and_stay_independent() {
    let (d, mut s) = fixture(
        UVec2::new(4, 1),
        [100.0, 100.0, 300.0, 300.0].map(Vec2::splat),
    );
    s.connectivity.union(PieceId(0), PieceId(1));
    s.connectivity.union(PieceId(2), PieceId(3));
    let result = release(&mut s, &d, &[0, 3], Vec2::new(17.0, 19.0));
    assert_eq!(result.released, 4);
    assert_eq!(s.connectivity.component_size(PieceId(0)), 2);
    assert_offset(&s, &d, 0, Vec2::new(117.0, 119.0));
    assert_offset(&s, &d, 3, Vec2::new(317.0, 319.0));
}

#[test]
fn simultaneous_releases_see_other_components_after_their_delta_commit() {
    let (d, mut s) = fixture(UVec2::new(2, 1), [Vec2::splat(100.0), Vec2::splat(103.0)]);
    let result = release(&mut s, &d, &[0, 1], Vec2::new(50.0, 60.0));
    assert_eq!(result.released, 2);
    assert_offset(&s, &d, 0, Vec2::new(153.0, 163.0));
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
            Some(&d)
        )
        .grabbed,
        4
    );
    assert_eq!(
        s.apply_command(PlayerId(1), &PieceCommand::Grab(PieceId(1)), Some(&d))
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
    );
    assert!(s.dirty_pieces.is_empty());
    assert_eq!(
        s.apply_command(
            LOCAL_PLAYER,
            &PieceCommand::ReleaseGroup {
                members,
                delta: Vec2::ONE
            },
            Some(&d)
        )
        .released,
        4
    );
    assert_offset(&s, &d, 0, Vec2::splat(201.0));
    assert!(s.held_by.is_empty());
    assert_eq!(
        s.apply_command(LOCAL_PLAYER, &PieceCommand::Grab(PieceId(1)), Some(&d))
            .grabbed,
        4
    );
    assert_eq!(
        s.apply_command(LOCAL_PLAYER, &PieceCommand::Release(PieceId(3)), Some(&d))
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
        s.apply_command(LOCAL_PLAYER, &PieceCommand::Grab(PieceId(0)), Some(&d))
            .grabbed,
        0
    );
    s.held_by.insert(PieceId(0), LOCAL_PLAYER);
    assert_eq!(
        s.apply_command(LOCAL_PLAYER, &PieceCommand::Release(PieceId(0)), Some(&d))
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
    s.apply_command(PlayerId(1), &PieceCommand::Grab(PieceId(2)), Some(&d));
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
    s.set_state(PieceId(2), state);
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
    s.apply_command(LOCAL_PLAYER, &PieceCommand::Grab(PieceId(0)), Some(&d));
    s.dirty_pieces.clear();
    s.apply_command(LOCAL_PLAYER, &PieceCommand::Release(PieceId(0)), Some(&d));
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
    s.apply_command(LOCAL_PLAYER, &PieceCommand::Grab(PieceId(0)), Some(&d));
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
fn overflowing_release_ignores_translation_for_whole_component() {
    let (d, mut s) = fixture(UVec2::new(2, 1), [Vec2::splat(f32::MAX); 2]);
    s.connectivity.union(PieceId(0), PieceId(1));
    let result = release(&mut s, &d, &[0], Vec2::splat(f32::MAX));
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
    s.apply_command(LOCAL_PLAYER, &PieceCommand::Grab(PieceId(0)), None);
    s.apply_command(
        LOCAL_PLAYER,
        &PieceCommand::Move {
            id: PieceId(0),
            position: Vec2::splat(-f32::MAX),
        },
        None,
    );
    assert_eq!(s.states[0].position, Vec2::splat(-f32::MAX));
    assert_eq!(
        s.apply_command(LOCAL_PLAYER, &PieceCommand::Release(PieceId(0)), None)
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
    );
    let result = s.apply_command(
        LOCAL_PLAYER,
        &PieceCommand::ReleaseGroup {
            members: all,
            delta: Vec2::ZERO,
        },
        Some(&d),
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
        [100.0, 104.0, 500.0, 600.0, 604.0, 700.0].map(|x| Vec2::new(x, 100.0)),
    );
    release(&mut s, &d, &[0, 3], Vec2::ZERO);
    assert_offset(&s, &d, 0, Vec2::new(104.0, 100.0));
    assert_offset(&s, &d, 3, Vec2::new(604.0, 100.0));
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
