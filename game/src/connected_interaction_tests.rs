use super::*;
use std::sync::Arc;

fn frame(point: Vec2, pressed: bool, just_pressed: bool, ctrl: bool) -> PointerFrame {
    PointerFrame {
        position: Some(point),
        screen_position: Some(point),
        pressed,
        just_pressed,
        ctrl,
        focused: true,
        over_ui: false,
    }
}
fn store() -> PieceDataStore {
    let mut store = PieceDataStore::default();
    store.initialize(
        (0..5)
            .map(|id| Vec2::new(id as f32 * 20.0, 100.0))
            .collect(),
    );
    store.connectivity.union(PieceId(0), PieceId(1));
    store.connectivity.union(PieceId(2), PieceId(3));
    store
}
fn point_result(
    gesture: &mut PieceInteraction,
    store: &mut PieceDataStore,
    selection: &mut PuzzleSelection,
    id: u32,
    ctrl: bool,
    pressed: bool,
) -> Vec<PieceCommand> {
    gesture.update(frame(Vec2::ZERO, true, true, ctrl), store, selection);
    let request = selection.latest.unwrap();
    selection.completed = Some(SelectionResult {
        request_id: request.request_id,
        mode: SelectionMode::Point,
        payload: SelectionPayload::Point(Some(PieceId(id))),
        error: None,
    });
    gesture.update(frame(Vec2::ZERO, pressed, false, ctrl), store, selection)
}

#[test]
fn picking_one_member_drags_entire_component_with_frozen_mask() {
    let mut s = store();
    let mut interaction = PieceInteraction::default();
    let mut selection = PuzzleSelection::default();
    let commands = point_result(&mut interaction, &mut s, &mut selection, 1, false, true);
    assert_eq!(
        s.selected_pieces.iter().collect::<Vec<_>>(),
        [PieceId(0), PieceId(1)]
    );
    assert_eq!(commands.len(), 1);
    assert_eq!(s.apply_command(LOCAL_PLAYER, &commands[0], None).grabbed, 2);
    let frozen = s.drag.members.clone();
    let positions: Vec<_> = s.states.iter().map(|state| state.position).collect();
    s.dirty_pieces.clear();
    for step in 1..100 {
        assert!(interaction
            .update(
                frame(Vec2::splat(step as f32), true, false, false),
                &mut s,
                &mut selection
            )
            .is_empty());
        assert!(Arc::ptr_eq(&frozen, &s.drag.members));
        assert!(s.dirty_pieces.is_empty());
    }
    assert_eq!(
        s.states
            .iter()
            .map(|state| state.position)
            .collect::<Vec<_>>(),
        positions
    );
    let commands = interaction.update(
        frame(Vec2::new(70.0, 30.0), false, false, false),
        &mut s,
        &mut selection,
    );
    assert_eq!(commands.len(), 1);
    assert_eq!(
        s.apply_command(LOCAL_PLAYER, &commands[0], None).released,
        2
    );
    assert_eq!(
        s.states[1].position - s.states[0].position,
        Vec2::new(20.0, 0.0)
    );
    assert_eq!(s.states[2].position, positions[2]);
}

#[test]
fn ctrl_toggles_whole_components_and_preserves_other_selections() {
    let mut s = store();
    let mut interaction = PieceInteraction::default();
    let mut selection = PuzzleSelection::default();
    for (id, expected) in [
        (1, vec![0, 1]),
        (3, vec![0, 1, 2, 3]),
        (0, vec![2, 3]),
        (2, vec![]),
    ] {
        assert!(point_result(&mut interaction, &mut s, &mut selection, id, true, false).is_empty());
        assert_eq!(
            s.selected_pieces.iter().map(|id| id.0).collect::<Vec<_>>(),
            expected
        );
    }
}

#[test]
fn final_rectangle_expands_partial_hits_and_additive_multi_selection() {
    let mut s = store();
    let mut hit = PieceBitSet::new(s.len());
    hit.insert(PieceId(1));
    s.commit_selection(hit, None);
    assert_eq!(s.selected_pieces.count(), 2);
    let original = s.selected_pieces.clone();
    let mut hit = PieceBitSet::new(s.len());
    hit.insert(PieceId(3));
    hit.insert(PieceId(4));
    s.commit_selection(hit, Some(&original));
    assert_eq!(s.selected_pieces.count(), 5);
    let mut interaction = PieceInteraction::default();
    let mut selection = PuzzleSelection::default();
    let commands = point_result(&mut interaction, &mut s, &mut selection, 0, false, true);
    assert_eq!(commands.len(), 1);
    assert_eq!(s.apply_command(LOCAL_PLAYER, &commands[0], None).grabbed, 5);
    assert_eq!(s.connectivity.component_size(PieceId(0)), 2);
    assert_eq!(s.connectivity.component_size(PieceId(2)), 2);
    assert_eq!(s.connectivity.component_size(PieceId(4)), 1);
}

#[test]
fn rollback_expands_original_selection_if_connectivity_changed_during_readback() {
    let mut s = store();
    s.selected_pieces.insert(PieceId(4));
    let mut interaction = PieceInteraction::default();
    let mut selection = PuzzleSelection::default();
    interaction.update(frame(Vec2::ZERO, true, true, false), &mut s, &mut selection);
    s.connectivity.union(PieceId(3), PieceId(4));
    interaction.cancel(&mut s, &mut selection);
    assert_eq!(
        s.selected_pieces.iter().collect::<Vec<_>>(),
        [PieceId(2), PieceId(3), PieceId(4)]
    );
}

#[test]
fn component_selection_keeps_local_holds_and_rejects_remote_holds_on_rollback() {
    let mut s = store();
    s.selected_pieces.fill();
    let original = s.selected_pieces.clone();
    let mut interaction = PieceInteraction::default();
    let mut selection = PuzzleSelection::default();
    interaction.update(frame(Vec2::ZERO, true, true, true), &mut s, &mut selection);
    assert_eq!(
        s.apply_command(LOCAL_PLAYER, &PieceCommand::Grab(PieceId(0)), None)
            .grabbed,
        2
    );
    assert_eq!(
        s.apply_command(PlayerId(1), &PieceCommand::Grab(PieceId(3)), None)
            .grabbed,
        2
    );
    assert_eq!(
        s.selected_pieces.iter().collect::<Vec<_>>(),
        [PieceId(0), PieceId(1), PieceId(4)]
    );
    let mut placed = s.state(PieceId(4)).unwrap();
    placed.placed = true;
    s.set_state(PieceId(4), placed);
    let mut hit = PieceBitSet::new(s.len());
    hit.extend([PieceId(1), PieceId(3), PieceId(4)]);
    s.commit_selection(hit, Some(&original));
    assert_eq!(
        s.selected_pieces.iter().collect::<Vec<_>>(),
        [PieceId(0), PieceId(1)]
    );
    let commands = interaction.cancel(&mut s, &mut selection);
    assert_eq!(
        s.selected_pieces.iter().collect::<Vec<_>>(),
        [PieceId(0), PieceId(1)]
    );
    for command in commands {
        s.apply_command(LOCAL_PLAYER, &command, None);
    }
    assert_eq!(
        s.selected_pieces.iter().collect::<Vec<_>>(),
        [PieceId(0), PieceId(1)]
    );
    assert_eq!(s.held_by.len(), 2);
    assert_eq!(original.count(), 5);
}
