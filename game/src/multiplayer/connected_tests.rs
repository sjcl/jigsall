use super::*;
use crate::{
    interaction::{PieceInteraction, PointerFrame},
    selection::{PuzzleSelection, SelectionMode, SelectionPayload, SelectionResult},
};
use bevy::prelude::*;
use puzzella_core::{
    session::{ImageHash, SessionDefinition, SessionId},
    PieceCommand, GENERATOR_VERSION, LOCAL_PLAYER,
};

const SESSION: SessionDefinition = SessionDefinition {
    id: SessionId(200),
    image_hash: ImageHash([9; 32]),
};
fn fixture(placed: bool) -> (PuzzleDefinition, PieceDataStore) {
    let definition = PuzzleDefinition {
        generator_version: GENERATOR_VERSION,
        seed: 42,
        grid_size: UVec2::new(4, 2),
        image_size: UVec2::new(80, 40),
        snap_distance: 5.0,
    };
    let mut store = PieceDataStore::default();
    store.initialize(
        (0..8)
            .map(|id| {
                definition.correct_position(PieceId(id))
                    + if placed {
                        Vec2::ZERO
                    } else {
                        Vec2::splat(100.0)
                    }
            })
            .collect(),
    );
    if placed {
        for id in 0..8 {
            let mut state = store.state(PieceId(id)).unwrap();
            state.placed = true;
            store.set_state(PieceId(id), state);
        }
    }
    // Two already-connected components merge across their correct boundary.
    for (a, b) in [(0, 1), (1, 5), (2, 3), (3, 7), (1, 2)] {
        store.connectivity.union(PieceId(a), PieceId(b));
    }
    (definition, store)
}
fn expected(definition: &PuzzleDefinition) -> SnapshotExpectation<'_> {
    SnapshotExpectation {
        session: SESSION.id,
        image_hash: SESSION.image_hash,
        cursor: AuthorityCursor::new(3, 50),
        definition,
    }
}

#[test]
fn connected_snapshot_round_trip_preserves_positions_placement_and_edges() {
    for placed in [false, true] {
        let (d, mut s) = fixture(placed);
        s.selected_pieces.fill();
        if !placed {
            s.apply_command(PlayerId(1), &PieceCommand::Grab(PieceId(5)), Some(&d));
        }
        let snapshot = GameSnapshot::capture(&s, &d, SESSION, expected(&d).cursor).unwrap();
        assert_eq!(snapshot.schema_version, 3);
        assert_eq!(std::mem::size_of::<SnapshotPieceState>(), 16);
        assert_ne!(snapshot.pieces[1].flags & SNAPSHOT_CONNECTED_RIGHT, 0);
        assert_ne!(snapshot.pieces[1].flags & SNAPSHOT_CONNECTED_DOWN, 0);
        let mut restored = PieceDataStore::default();
        snapshot.install(&mut restored, expected(&d)).unwrap();
        assert!(restored.held_by.is_empty());
        assert!(restored.selected_pieces.is_empty());
        assert!(restored.drag.members.is_empty());
        assert_eq!(restored.placed_count, if placed { 8 } else { 0 });
        for a in 0..8 {
            assert_eq!(restored.states[a].position, s.states[a].position);
            assert_eq!(restored.states[a].z_order, s.states[a].z_order);
            for b in 0..8 {
                assert_eq!(
                    restored
                        .connectivity
                        .same_component(PieceId(a as u32), PieceId(b)),
                    s.connectivity.same_component(PieceId(a as u32), PieceId(b))
                );
            }
        }
    }
}

#[test]
fn invalid_edges_offsets_partial_placement_and_old_schema_are_atomic() {
    let (d, s) = fixture(false);
    let snapshot = GameSnapshot::capture(&s, &d, SESSION, expected(&d).cursor).unwrap();
    let mut cases = Vec::new();
    for (id, flag) in [(3, SNAPSHOT_CONNECTED_RIGHT), (4, SNAPSHOT_CONNECTED_DOWN)] {
        let mut invalid = snapshot.clone();
        invalid.pieces[id].flags |= flag;
        cases.push((
            invalid,
            SnapshotError::InvalidBorderConnection(PieceId(id as u32)),
        ));
    }
    let mut invalid = snapshot.clone();
    invalid.pieces[5].position.x += 1.0;
    cases.push((invalid, SnapshotError::InconsistentComponent(PieceId(5))));
    let mut invalid = snapshot.clone();
    invalid.pieces[0].flags |= SNAPSHOT_PLACED;
    cases.push((invalid, SnapshotError::InvalidPlacedPosition(PieceId(0))));
    let (d, placed) = fixture(true);
    let mut invalid = GameSnapshot::capture(&placed, &d, SESSION, expected(&d).cursor).unwrap();
    invalid.pieces[1].flags &= !SNAPSHOT_PLACED;
    cases.push((invalid, SnapshotError::InconsistentComponent(PieceId(1))));
    for version in [1, 2, 4] {
        let mut invalid = snapshot.clone();
        invalid.schema_version = version;
        cases.push((invalid, SnapshotError::UnsupportedSchema(version)));
    }
    for (invalid, error) in cases {
        let (_, mut current) = fixture(false);
        let states = current.states.clone();
        let connectivity = current.connectivity.clone();
        let epoch = current.epoch;
        assert_eq!(invalid.install(&mut current, expected(&d)), Err(error));
        assert_eq!(current.states, states);
        assert_eq!(current.connectivity, connectivity);
        assert_eq!(current.epoch, epoch);
    }
}

#[test]
fn migration_then_gpu_point_selection_moves_the_complete_restored_component() {
    let (d, source) = fixture(false);
    let cursor = expected(&d).cursor;
    let snapshot = GameSnapshot::capture(&source, &d, SESSION, cursor).unwrap();
    let mut authority = AuthoritySession::new(SESSION, PlayerId(1), cursor);
    authority.begin_graceful(PlayerId(2)).unwrap();
    authority
        .acknowledge_snapshot(SESSION.id, PlayerId(2), cursor)
        .unwrap();
    authority.host_changed(PlayerId(2)).unwrap();
    let mut restored = PieceDataStore::default();
    install_migration_snapshot(&mut authority, &mut restored, &d, &snapshot).unwrap();
    assert!(authority.is_active());
    let mut interaction = PieceInteraction::default();
    let mut selection = PuzzleSelection::default();
    let frame = |position, pressed, just_pressed| PointerFrame {
        position: Some(position),
        screen_position: Some(position),
        pressed,
        just_pressed,
        ctrl: false,
        over_ui: false,
        focused: true,
    };
    interaction.update(frame(Vec2::ZERO, true, true), &mut restored, &mut selection);
    let request = selection.latest.unwrap();
    selection.completed = Some(SelectionResult {
        request_id: request.request_id,
        mode: SelectionMode::Point,
        payload: SelectionPayload::Point(Some(PieceId(5))),
        error: None,
    });
    let commands = interaction.update(
        frame(Vec2::ZERO, true, false),
        &mut restored,
        &mut selection,
    );
    assert_eq!(commands.len(), 1);
    assert_eq!(
        restored
            .apply_command(LOCAL_PLAYER, &commands[0], Some(&d))
            .grabbed,
        6
    );
    let commands = interaction.update(
        frame(Vec2::splat(20.0), false, false),
        &mut restored,
        &mut selection,
    );
    assert_eq!(
        restored
            .apply_command(LOCAL_PLAYER, &commands[0], Some(&d))
            .released,
        6
    );
    // The two remaining loose neighbors are now outside the snap threshold.
    assert_eq!(restored.connectivity.component_size(PieceId(5)), 6);
    for id in [0, 1, 2, 3, 5, 7] {
        assert_eq!(
            restored.states[id].position,
            d.correct_position(PieceId(id as u32)) + Vec2::splat(120.0)
        );
    }
}

#[test]
fn million_connected_fractional_positions_round_trip_and_stay_atomic() {
    let d = PuzzleDefinition {
        generator_version: GENERATOR_VERSION,
        seed: 42,
        grid_size: UVec2::splat(1000),
        image_size: UVec2::splat(4096),
        snap_distance: 5.0,
    };
    let mut source = PieceDataStore::default();
    source.initialize(
        (0..1_000_000)
            .map(|id| d.correct_position(PieceId(id)) + Vec2::new(10_000.37, 20_000.93))
            .collect(),
    );
    for id in 1..1_000_000 {
        source.connectivity.union(PieceId(id - 1), PieceId(id));
    }
    let snapshot = GameSnapshot::capture(&source, &d, SESSION, expected(&d).cursor).unwrap();
    let mut restored = PieceDataStore::default();
    snapshot.install(&mut restored, expected(&d)).unwrap();
    assert_eq!(source.states, restored.states);
    assert_eq!(
        restored.connectivity.component_size(PieceId(777_777)),
        1_000_000
    );
    assert_eq!(
        restored
            .apply_command(
                LOCAL_PLAYER,
                &PieceCommand::Grab(PieceId(777_777)),
                Some(&d)
            )
            .grabbed,
        1_000_000
    );
    assert_eq!(
        restored
            .apply_command(
                LOCAL_PLAYER,
                &PieceCommand::Release(PieceId(777_777)),
                Some(&d)
            )
            .released,
        1_000_000
    );
    assert!(restored.held_by.is_empty());
    let recaptured = GameSnapshot::capture(&restored, &d, SESSION, expected(&d).cursor).unwrap();
    assert_eq!(recaptured.pieces.len(), 1_000_000);
}

#[test]
fn snap_ties_are_unchanged_when_snapshot_reconstructs_different_dsu_roots() {
    assert_snapshot_root_independence(UVec2::new(40, 80), Vec2::ZERO);
    // Different y coordinates round the same fractional offset differently.
    assert_snapshot_root_independence(UVec2::new(123, 4096), Vec2::new(0.37, 0.93));
}

fn assert_snapshot_root_independence(image_size: UVec2, fractional: Vec2) {
    let d = PuzzleDefinition {
        generator_version: GENERATOR_VERSION,
        seed: 42,
        grid_size: UVec2::new(2, 4),
        image_size,
        snap_distance: 5.0,
    };
    let mut source = PieceDataStore::default();
    source.initialize(
        [98.0, 102.0, 98.0, 106.0, 98.0, 500.0, 98.0, 700.0]
            .into_iter()
            .enumerate()
            .map(|(id, x)| {
                d.correct_position(PieceId(id as u32)) + Vec2::new(x, 100.0) + fractional
            })
            .collect(),
    );
    for id in [4, 2, 0] {
        source.connectivity.union(PieceId(6), PieceId(id));
    }
    assert_eq!(source.connectivity.find_root(PieceId(0)), PieceId(4));
    let snapshot = GameSnapshot::capture(&source, &d, SESSION, expected(&d).cursor).unwrap();
    let mut restored = PieceDataStore::default();
    snapshot.install(&mut restored, expected(&d)).unwrap();
    assert_eq!(restored.connectivity.find_root(PieceId(0)), PieceId(0));
    for s in [&mut source, &mut restored] {
        s.apply_command(LOCAL_PLAYER, &PieceCommand::Grab(PieceId(1)), Some(&d));
        s.apply_command(LOCAL_PLAYER, &PieceCommand::Release(PieceId(1)), Some(&d));
        assert!(s.connectivity.same_component(PieceId(1), PieceId(0)));
        assert!(!s.connectivity.same_component(PieceId(1), PieceId(3)));
        assert_eq!(
            s.states[1].position,
            d.correct_position(PieceId(1))
                + (s.states[0].position - d.correct_position(PieceId(0)))
        );
    }
    assert_eq!(source.states, restored.states);
}

#[test]
fn fixed_offset_snap_closure_and_board_priority_survive_snapshot_restore() {
    for (offsets, expected_size, expected_offset, placed) in [
        (
            [0.0, 4.0, 8.0, 12.0].map(|x| Vec2::new(x, 100.0)),
            2,
            Vec2::new(4.0, 100.0),
            0,
        ),
        (
            [0.0, 4.0, 4.0, 4.0].map(|x| Vec2::new(x, 100.0)),
            4,
            Vec2::new(4.0, 100.0),
            0,
        ),
        (
            [3.0, 4.0, 100.0, 200.0].map(|x| Vec2::new(x, 0.0)),
            1,
            Vec2::ZERO,
            1,
        ),
    ] {
        let d = PuzzleDefinition {
            generator_version: GENERATOR_VERSION,
            seed: 42,
            grid_size: UVec2::new(4, 1),
            image_size: UVec2::new(80, 20),
            snap_distance: 5.0,
        };
        let mut source = PieceDataStore::default();
        source.initialize(
            offsets
                .into_iter()
                .enumerate()
                .map(|(id, offset)| d.correct_position(PieceId(id as u32)) + offset)
                .collect(),
        );
        let snapshot = GameSnapshot::capture(&source, &d, SESSION, expected(&d).cursor).unwrap();
        let mut restored = PieceDataStore::default();
        snapshot.install(&mut restored, expected(&d)).unwrap();
        for store in [&mut source, &mut restored] {
            store.apply_command(LOCAL_PLAYER, &PieceCommand::Grab(PieceId(0)), Some(&d));
            let result =
                store.apply_command(LOCAL_PLAYER, &PieceCommand::Release(PieceId(0)), Some(&d));
            assert_eq!(result.placed, placed);
            assert_eq!(store.connectivity.component_size(PieceId(0)), expected_size);
            assert_eq!(
                store.states[0].position,
                d.correct_position(PieceId(0)) + expected_offset
            );
        }
        assert_eq!(source.states, restored.states);
        let capture = |store: &PieceDataStore| {
            GameSnapshot::capture(store, &d, SESSION, expected(&d).cursor).unwrap()
        };
        assert_eq!(capture(&source).pieces, capture(&restored).pieces);
    }
}

#[test]
fn fractional_closure_is_identical_after_restore_and_preserves_target_positions() {
    let d = PuzzleDefinition {
        generator_version: GENERATOR_VERSION,
        seed: 42,
        grid_size: UVec2::new(3, 1),
        image_size: UVec2::new(4096, 20),
        snap_distance: 5.0,
    };
    let mut source = PieceDataStore::default();
    source.initialize(
        [96.37, 100.37, 100.37]
            .into_iter()
            .enumerate()
            .map(|(id, x)| d.correct_position(PieceId(id as u32)) + Vec2::new(x, 0.0))
            .collect(),
    );
    let snapshot = GameSnapshot::capture(&source, &d, SESSION, expected(&d).cursor).unwrap();
    let mut restored = PieceDataStore::default();
    snapshot.install(&mut restored, expected(&d)).unwrap();
    for store in [&mut source, &mut restored] {
        let targets = store.states[1..].to_vec();
        store.apply_command(LOCAL_PLAYER, &PieceCommand::Grab(PieceId(0)), Some(&d));
        store.apply_command(LOCAL_PLAYER, &PieceCommand::Release(PieceId(0)), Some(&d));
        assert_eq!(store.connectivity.component_size(PieceId(0)), 3);
        assert_eq!(store.states[1..], targets);
        GameSnapshot::capture(store, &d, SESSION, expected(&d).cursor).unwrap();
    }
    assert_eq!(source.states, restored.states);
}
