use super::*;
use crate::systems::{game_logic::apply_piece_commands, piece_interaction::handle_piece_input};
use std::sync::Arc;

fn app(connected: bool) -> App {
    let definition = PuzzleDefinition {
        generator_version: GENERATOR_VERSION,
        seed: 42,
        grid_size: UVec2::new(3, 1),
        image_size: UVec2::new(120, 30),
        snap_distance: 5.,
    };
    let mut store = PieceDataStore::default();
    store.initialize(
        (0..3)
            .map(|id| definition.correct_position(PieceId(id)) + Vec2::splat(100.))
            .collect(),
    );
    if connected {
        store.connectivity.union(PieceId(0), PieceId(1));
    }
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .insert_resource(definition)
        .insert_resource(store)
        .insert_resource(LocalPlayerId(PlayerId(42)))
        .init_resource::<PieceInteraction>()
        .init_resource::<PuzzleSelection>()
        .init_resource::<InputState>()
        .init_resource::<GameUiPointerCapture>()
        .init_resource::<PerformanceMonitor>()
        .init_resource::<ButtonInput<MouseButton>>()
        .init_resource::<ButtonInput<KeyCode>>()
        .insert_resource(crate::keybindings::KeyBindingsState::load(None))
        .init_resource::<bevy_egui::EguiUserTextures>()
        .init_resource::<pieces::PieceUpload>()
        .add_message::<ClientCommand>()
        .add_systems(
            Update,
            (
                handle_piece_input,
                apply_piece_commands,
                pieces::prepare_piece_upload,
            )
                .chain(),
        );
    set_pointer(&mut app, Some(Vec2::new(10., 20.)));
    frame(&mut app, None);
    frame(&mut app, None);
    app
}

fn set_pointer(app: &mut App, point: Option<Vec2>) {
    let mut input = app.world_mut().resource_mut::<InputState>();
    input.window_focused = true;
    input.mouse_position = point;
    input.cursor_screen_position = point;
}

fn frame(app: &mut App, key: Option<KeyCode>) {
    let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
    *keys = ButtonInput::default();
    if let Some(key) = key {
        keys.press(key);
    }
    app.update();
}

fn complete(app: &mut App, hit: Option<PieceId>) {
    let mut selection = app.world_mut().resource_mut::<PuzzleSelection>();
    let request = selection.latest.unwrap();
    selection.completed = Some(SelectionResult {
        request_id: request.request_id,
        mode: request.mode,
        payload: SelectionPayload::Point(hit),
        error: None,
    });
}

#[test]
fn remapped_chord_and_secondary_rotate_through_the_real_input_adapter() {
    use crate::keybindings::{KeyAction, KeyChord};
    let mut app = app(false);
    {
        let mut bindings = app
            .world_mut()
            .resource_mut::<crate::keybindings::KeyBindingsState>();
        let binding = bindings.current.binding_mut(KeyAction::RotateLeft);
        binding.primary = Some(KeyChord::new(KeyCode::ShiftLeft, Some(KeyCode::KeyR)).unwrap());
        binding.secondary = Some(KeyChord::new(KeyCode::KeyT, None).unwrap());
    }
    frame(&mut app, Some(KeyCode::KeyQ));
    assert!(app.world().resource::<PuzzleSelection>().latest.is_none());
    frame(&mut app, Some(KeyCode::KeyR));
    assert!(app.world().resource::<PuzzleSelection>().latest.is_none());
    {
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        *keys = default();
        keys.press(KeyCode::ShiftRight);
        keys.press(KeyCode::KeyR);
    }
    app.update();
    complete(&mut app, Some(PieceId(1)));
    frame(&mut app, None);
    assert_eq!(
        decode_rotation(app.world().resource::<PieceDataStore>().states[1].flags),
        1
    );
    frame(&mut app, Some(KeyCode::KeyT));
    complete(&mut app, Some(PieceId(1)));
    frame(&mut app, None);
    assert_eq!(
        decode_rotation(app.world().resource::<PieceDataStore>().states[1].flags),
        2
    );
}

#[test]
fn q_e_without_selection_rotates_picked_singleton_or_entire_component_without_selecting() {
    for connected in [false, true] {
        let mut app = app(connected);
        let before = app.world().resource::<PieceDataStore>().states.to_vec();
        let selection = app
            .world()
            .resource::<PieceDataStore>()
            .selected_pieces
            .words()
            .clone();
        let membership = app
            .world()
            .resource::<PieceDataStore>()
            .drag
            .members
            .clone();
        let next_z = app.world().resource::<PieceDataStore>().next_z_order;
        for (key, rotation) in [(KeyCode::KeyQ, 1), (KeyCode::KeyE, 0)] {
            frame(&mut app, Some(key));
            let request = app.world().resource::<PuzzleSelection>().latest.unwrap();
            assert_eq!(request.mode, SelectionMode::Point);
            assert!(request.readback);
            assert_eq!(
                request.region,
                Rect::from_corners(Vec2::new(10., 20.), Vec2::new(10., 20.))
            );
            complete(&mut app, Some(PieceId(1)));
            frame(&mut app, None);
            let store = app.world().resource::<PieceDataStore>();
            for (id, state) in store.states.iter().enumerate() {
                let affected = id == 1 || (connected && id == 0);
                assert_eq!(
                    decode_rotation(state.flags),
                    if affected { rotation } else { 0 }
                );
                assert_eq!(state.z_order, before[id].z_order);
            }
            if connected {
                let pivot = (before[0].position + before[1].position) * 0.5;
                assert_eq!(
                    store.states[0].position,
                    pivot + rotate_quarter(before[0].position - pivot, rotation)
                );
                assert_eq!(
                    store.states[1].position,
                    pivot + rotate_quarter(before[1].position - pivot, rotation)
                );
            } else {
                assert_eq!(
                    store
                        .states
                        .to_vec()
                        .iter()
                        .map(|s| s.position)
                        .collect::<Vec<_>>(),
                    before.iter().map(|s| s.position).collect::<Vec<_>>()
                );
            }
            assert!(store.selected_pieces.is_empty());
            assert!(Arc::ptr_eq(&selection, store.selected_pieces.words()));
            assert!(Arc::ptr_eq(&membership, &store.drag.members));
            assert!(store.held_by.is_empty());
            assert_eq!(store.next_z_order, next_z);
            assert!(app.world().resource::<PuzzleSelection>().latest.is_none());
            assert!(matches!(
                app.world().resource::<PieceInteraction>().gesture,
                Gesture::Idle
            ));
        }
    }
}

#[test]
fn idle_and_pointer_only_frames_do_not_request_picks_or_dirty_uploads() {
    let mut app = app(true);
    let before = app.world().resource::<PieceDataStore>().states.to_vec();
    let selected = app
        .world()
        .resource::<PieceDataStore>()
        .selected_pieces
        .words()
        .clone();
    let revision = app.world().resource::<pieces::PieceUpload>().revision;
    for i in 0..100 {
        set_pointer(&mut app, Some(Vec2::splat(i as f32)));
        frame(&mut app, None);
        assert!(app.world().resource::<PuzzleSelection>().latest.is_none());
        assert_eq!(
            app.world().resource::<pieces::PieceUpload>().revision,
            revision
        );
    }
    let store = app.world().resource::<PieceDataStore>();
    assert_eq!(store.states.to_vec(), before);
    assert!(Arc::ptr_eq(&selected, store.selected_pieces.words()));
    assert!(store.dirty_pieces.is_empty());
}

#[test]
fn existing_selection_takes_priority_over_pointer_and_cancels_pending_hover() {
    let mut app = app(false);
    frame(&mut app, Some(KeyCode::KeyQ));
    complete(&mut app, Some(PieceId(1)));
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .selected_pieces
        .insert(PieceId(2));
    frame(&mut app, Some(KeyCode::KeyE));
    let store = app.world().resource::<PieceDataStore>();
    assert_eq!(decode_rotation(store.states[1].flags), 0);
    assert_eq!(decode_rotation(store.states[2].flags), 3);
    assert_eq!(
        store.selected_pieces.iter().collect::<Vec<_>>(),
        [PieceId(2)]
    );
    assert!(app.world().resource::<PuzzleSelection>().latest.is_none());
}

#[test]
fn repeated_keys_share_one_pick_and_opposite_keys_cancel() {
    for cancel in [false, true] {
        let mut app = app(false);
        frame(&mut app, Some(KeyCode::KeyQ));
        let request_id = app
            .world()
            .resource::<PuzzleSelection>()
            .latest
            .unwrap()
            .request_id;
        frame(
            &mut app,
            Some(if cancel { KeyCode::KeyE } else { KeyCode::KeyQ }),
        );
        if cancel {
            assert!(app.world().resource::<PuzzleSelection>().latest.is_none());
        } else {
            assert_eq!(
                app.world()
                    .resource::<PuzzleSelection>()
                    .latest
                    .unwrap()
                    .request_id,
                request_id
            );
            complete(&mut app, Some(PieceId(1)));
            frame(&mut app, None);
        }
        assert_eq!(
            decode_rotation(app.world().resource::<PieceDataStore>().states[1].flags),
            if cancel { 0 } else { 2 }
        );
    }
}

#[test]
fn a_key_at_a_new_point_supersedes_old_pick_and_stale_result_cannot_rotate() {
    let mut app = app(false);
    frame(&mut app, Some(KeyCode::KeyQ));
    let old = app.world().resource::<PuzzleSelection>().latest.unwrap();
    set_pointer(&mut app, Some(Vec2::new(30., 40.)));
    frame(&mut app, Some(KeyCode::KeyE));
    let latest = app.world().resource::<PuzzleSelection>().latest.unwrap();
    assert!(latest.request_id > old.request_id);
    app.world_mut().resource_mut::<PuzzleSelection>().completed = Some(SelectionResult {
        request_id: old.request_id,
        mode: old.mode,
        payload: SelectionPayload::Point(Some(PieceId(0))),
        error: None,
    });
    frame(&mut app, None);
    assert!(app
        .world()
        .resource::<PieceDataStore>()
        .states
        .iter()
        .all(|s| decode_rotation(s.flags) == 0));
    complete(&mut app, Some(PieceId(2)));
    frame(&mut app, None);
    assert_eq!(
        decode_rotation(app.world().resource::<PieceDataStore>().states[2].flags),
        3
    );
}

#[test]
fn mouse_press_supersedes_hover_and_preserves_the_new_gesture_request() {
    let mut app = app(false);
    frame(&mut app, Some(KeyCode::KeyQ));
    complete(&mut app, Some(PieceId(1)));
    let old_id = app
        .world()
        .resource::<PuzzleSelection>()
        .latest
        .unwrap()
        .request_id;
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Left);
    frame(&mut app, Some(KeyCode::KeyE));
    let request = app.world().resource::<PuzzleSelection>().latest.unwrap();
    assert!(request.request_id > old_id);
    assert!(matches!(
        app.world().resource::<PieceInteraction>().gesture,
        Gesture::PendingPoint { .. }
    ));
    assert!(app
        .world()
        .resource::<PieceDataStore>()
        .states
        .iter()
        .all(|s| decode_rotation(s.flags) == 0));
    assert!(app
        .world()
        .resource::<PieceInteraction>()
        .pending_rotation
        .is_none());
}

#[test]
fn focus_ui_and_missing_cursor_cancel_delayed_rotation() {
    for gate in 0..4 {
        let mut app = app(false);
        frame(&mut app, Some(KeyCode::KeyQ));
        complete(&mut app, Some(PieceId(1)));
        match gate {
            0 => app.world_mut().resource_mut::<InputState>().window_focused = false,
            1 => {
                app.world_mut()
                    .resource_mut::<GameUiPointerCapture>()
                    .over_hud = true
            }
            2 => set_pointer(&mut app, None),
            _ => set_pointer(&mut app, Some(Vec2::splat(f32::NAN))),
        }
        frame(&mut app, None);
        assert!(app
            .world()
            .resource::<PieceDataStore>()
            .states
            .iter()
            .all(|s| decode_rotation(s.flags) == 0));
        assert!(app.world().resource::<PuzzleSelection>().latest.is_none());
        assert!(app
            .world()
            .resource::<PieceInteraction>()
            .pending_rotation
            .is_none());
    }
}

#[test]
fn empty_error_and_invalid_point_results_do_not_rotate_or_select() {
    for case in 0..5 {
        let mut app = app(false);
        frame(&mut app, Some(KeyCode::KeyQ));
        complete(&mut app, if case == 0 { None } else { Some(PieceId(1)) });
        {
            let mut selection = app.world_mut().resource_mut::<PuzzleSelection>();
            let result = selection.completed.as_mut().unwrap();
            match case {
                1 => result.error = Some("readback failed".into()),
                2 => result.payload = SelectionPayload::Point(Some(PieceId(99))),
                3 => result.mode = SelectionMode::Rectangle,
                4 => result.payload = SelectionPayload::Rectangle(PieceBitSet::new(3)),
                _ => {}
            }
        }
        frame(&mut app, None);
        assert!(app
            .world()
            .resource::<PieceDataStore>()
            .states
            .iter()
            .all(|s| decode_rotation(s.flags) == 0));
        assert!(app
            .world()
            .resource::<PieceDataStore>()
            .selected_pieces
            .is_empty());
        assert!(app.world().resource::<PuzzleSelection>().latest.is_none());
    }
}

#[test]
fn late_pick_revalidates_placed_disabled_and_held_members_before_rotation() {
    for case in 0..4 {
        let mut app = app(true);
        frame(&mut app, Some(KeyCode::KeyQ));
        complete(&mut app, Some(PieceId(1)));
        {
            let mut store = app.world_mut().resource_mut::<PieceDataStore>();
            match case {
                0 => store.states[1].flags |= pieces::PLACED,
                1 => store.states[1].flags &= !pieces::ENABLED,
                // An eligible hit cannot bypass a sibling's authority validation.
                2 => {
                    store.states[0].flags |= pieces::HELD;
                    store.held_by.insert(PieceId(0), PlayerId(7));
                }
                _ => {
                    store.states[1].flags |= pieces::HELD;
                    store.held_by.insert(PieceId(1), PlayerId(7));
                }
            }
        }
        let before = app.world().resource::<PieceDataStore>().states.to_vec();
        frame(&mut app, None);
        assert_eq!(
            app.world().resource::<PieceDataStore>().states.to_vec(),
            before
        );
        assert!(app
            .world()
            .resource::<PieceDataStore>()
            .selected_pieces
            .is_empty());
    }
}
