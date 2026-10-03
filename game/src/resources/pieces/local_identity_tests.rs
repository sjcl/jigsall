use super::*;
use puzzella_core::GENERATOR_VERSION;

const LOCAL: PlayerId = PlayerId(42);
const REMOTE: PlayerId = PlayerId(0);

fn store(count: usize) -> PieceDataStore {
    let mut store = PieceDataStore::default();
    store.initialize(vec![Vec2::splat(100.0); count]);
    store
}

fn mask(count: usize, ids: impl IntoIterator<Item = PieceId>) -> PieceBitSet {
    let mut mask = PieceBitSet::new(count);
    mask.extend(ids);
    mask
}

#[test]
fn nonzero_local_grab_preserves_and_syncs_sparse_and_dense_presentation() {
    for count in [2, 8] {
        for player in [LOCAL, REMOTE] {
            let mut store = store(96);
            let members = mask(96, (0..count).map(PieceId));
            store.selected_pieces = members.clone();
            store.drag = DragTransform {
                members: members.words().clone(),
                delta: Vec2::new(7.0, 9.0),
            };
            let applied = store.apply_command(
                player,
                &PieceCommand::GrabGroup {
                    members: members.clone(),
                },
                None,
                LOCAL,
            );
            assert_eq!(applied.grabbed, count as usize);
            for id in members.iter() {
                assert_eq!(store.held_by.get(&id), Some(&player));
                assert_eq!(store.selected_pieces.contains(&id), player == LOCAL);
            }
            if player == LOCAL {
                assert!(Arc::ptr_eq(&store.drag.members, members.words()));
                assert_eq!(store.drag.delta, Vec2::new(7.0, 9.0));
            } else {
                assert!(store.drag.members.iter().all(|&word| word == 0));
                assert_eq!(store.drag.delta, Vec2::new(7.0, 9.0));
            }
        }
    }
}

#[test]
fn nonzero_local_grab_syncs_only_accepted_members_on_both_paths() {
    for count in [2, 8] {
        let mut store = store(96);
        store.apply_command(REMOTE, &PieceCommand::Grab(PieceId(0)), None, LOCAL);
        let members = mask(96, (0..count).map(PieceId));
        store.drag.members = members.words().clone();
        let applied = store.apply_command(LOCAL, &PieceCommand::GrabGroup { members }, None, LOCAL);
        assert_eq!(applied.grabbed, count as usize - 1);
        let expected = mask(96, (1..count).map(PieceId));
        assert_eq!(*store.drag.members, **expected.words());
        assert_eq!(store.held_by.get(&PieceId(0)), Some(&REMOTE));
    }
}

#[test]
fn set_state_uses_nonzero_local_ownership_for_drag_and_selection() {
    let mut store = store(1);
    let members = mask(1, [PieceId(0)]);
    store.selected_pieces = members.clone();
    store.drag.members = members.words().clone();
    let mut state = store.state(PieceId(0)).unwrap();
    state.held_by = Some(LOCAL);
    store.set_state(PieceId(0), state, LOCAL);
    assert!(store.selected_pieces.contains(&PieceId(0)));
    assert!(Arc::ptr_eq(&store.drag.members, members.words()));
    state.held_by = Some(REMOTE);
    store.set_state(PieceId(0), state, LOCAL);
    assert!(store.drag.members.iter().all(|&word| word == 0));
    assert!(store.selected_pieces.is_empty());
    assert_eq!(store.held_by.get(&PieceId(0)), Some(&REMOTE));
}

#[test]
fn selection_rollback_allows_local_42_holds_and_excludes_remote_zero() {
    let mut store = store(3);
    for (id, owner) in [(PieceId(0), LOCAL), (PieceId(1), REMOTE)] {
        store.apply_command(owner, &PieceCommand::Grab(id), None, LOCAL);
    }
    let original = mask(3, [PieceId(0), PieceId(1), PieceId(2)]);
    let expected = mask(3, [PieceId(0), PieceId(2)]);
    store.restore_selection(original.clone(), LOCAL);
    assert_eq!(store.selected_pieces, expected);
    store.commit_selection(PieceBitSet::new(3), Some(&original), LOCAL);
    assert_eq!(store.selected_pieces, expected);
}

#[test]
fn only_current_local_release_cleans_up_local_drag() {
    for player in [LOCAL, REMOTE] {
        let mut store = store(1);
        store.apply_command(player, &PieceCommand::Grab(PieceId(0)), None, LOCAL);
        // Keep a presentation mask independently of authority ownership so the
        // test verifies which release is allowed to clean up this process's cache.
        let members = mask(1, [PieceId(0)]);
        store.drag = DragTransform {
            members: members.words().clone(),
            delta: Vec2::ONE,
        };
        let applied = store.apply_command(player, &PieceCommand::Release(PieceId(0)), None, LOCAL);
        assert_eq!(applied.released, 1);
        assert!(store.held_by.is_empty());
        assert_eq!(
            store.drag.members.iter().all(|&word| word == 0),
            player == LOCAL
        );
        if player == REMOTE {
            assert!(Arc::ptr_eq(&store.drag.members, members.words()));
            assert_eq!(store.drag.delta, Vec2::ONE);
        }
    }
}

#[test]
fn nonzero_local_drag_rotation_rebases_and_remote_zero_cannot_use_local_adapter() {
    let definition = PuzzleDefinition {
        generator_version: GENERATOR_VERSION,
        seed: 42,
        grid_size: UVec2::new(2, 1),
        image_size: UVec2::new(40, 20),
        snap_distance: 5.0,
    };
    for player in [LOCAL, REMOTE] {
        let mut store = store(2);
        store.initialize(
            (0..2)
                .map(|id| definition.correct_position(PieceId(id)) + Vec2::splat(100.0))
                .collect(),
        );
        store.connectivity.union(PieceId(0), PieceId(1));
        let members = mask(2, [PieceId(0), PieceId(1)]);
        store.apply_command(
            player,
            &PieceCommand::GrabGroup {
                members: members.clone(),
            },
            Some(&definition),
            LOCAL,
        );
        store.drag = DragTransform {
            members: members.words().clone(),
            delta: Vec2::new(7.0, 9.0),
        };
        let before = store.states.to_vec();
        let applied = store.apply_command(
            player,
            &PieceCommand::RotateDrag {
                members,
                delta: Vec2::new(7.0, 9.0),
                quarter_turns: 1,
            },
            Some(&definition),
            LOCAL,
        );
        assert_eq!(applied.drag_rebased, player == LOCAL);
        assert_eq!(applied.rotated, if player == LOCAL { 2 } else { 0 });
        if player == LOCAL {
            assert_eq!(store.drag.delta, Vec2::ZERO);
            assert!(store.states.iter().all(|s| decode_rotation(s.flags) == 1));
        } else {
            assert_eq!(store.states.to_vec(), before);
        }
    }
}
