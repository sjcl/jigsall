use super::*;
use crate::systems::{game_logic::apply_piece_commands, piece_interaction::handle_piece_input};
use std::sync::Arc;

fn app() -> App {
    let d = PuzzleDefinition {
        generator_version: GENERATOR_VERSION,
        seed: 42,
        grid_size: UVec2::new(2, 1),
        image_size: UVec2::new(40, 30),
        snap_distance: 5.,
    };
    let mut store = PieceDataStore::default();
    store.initialize(
        (0..2)
            .map(|id| d.correct_position(PieceId(id)) + Vec2::splat(1000.))
            .collect(),
    );
    store.connectivity.union(PieceId(0), PieceId(1));
    let mut members = PieceBitSet::new(2);
    members.fill();
    store.selected_pieces = members.clone();
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
    app.add_plugins(MinimalPlugins)
        .insert_resource(d)
        .insert_resource(store)
        .init_resource::<LocalPlayerId>()
        .insert_resource(PieceInteraction {
            gesture: Gesture::Dragging {
                members,
                anchor: Vec2::splat(1000.),
            },
        })
        .init_resource::<PuzzleSelection>()
        .init_resource::<GameUiPointerCapture>()
        .init_resource::<InputState>()
        .init_resource::<PerformanceMonitor>()
        .init_resource::<ButtonInput<MouseButton>>()
        .init_resource::<ButtonInput<KeyCode>>()
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
    frame(&mut app, Vec2::splat(1000.), true, None);
    frame(&mut app, Vec2::splat(1000.), true, None);
    app
}

fn frame(app: &mut App, point: Vec2, pressed: bool, key: Option<KeyCode>) {
    let mut input = app.world_mut().resource_mut::<InputState>();
    input.mouse_position = Some(point);
    input.cursor_screen_position = Some(point);
    input.window_focused = true;
    let mut mouse = app.world_mut().resource_mut::<ButtonInput<MouseButton>>();
    mouse.clear();
    if pressed {
        mouse.press(MouseButton::Left);
    } else {
        mouse.release(MouseButton::Left);
    }
    let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
    *keys = ButtonInput::default();
    if let Some(key) = key {
        keys.press(key);
    }
    app.update();
}

#[test]
fn accepted_drag_rotation_rebases_anchor_in_same_frame_and_pointer_moves_remain_constant_cost() {
    let mut app = app();
    let mask = app
        .world()
        .resource::<PieceDataStore>()
        .drag
        .members
        .clone();
    frame(&mut app, Vec2::new(1020., 1030.), true, Some(KeyCode::KeyQ));
    let states = app.world().resource::<PieceDataStore>().states.to_vec();
    assert_eq!(states[0].position, Vec2::new(1020., 1020.));
    assert_eq!(states[1].position, Vec2::new(1020., 1040.));
    let upload = app.world().resource::<pieces::PieceUpload>();
    assert_eq!(upload.drag.delta, Vec2::ZERO);
    assert_eq!(
        upload.ranges.iter().map(|r| r.states.len()).sum::<usize>(),
        2
    );
    let revision = upload.revision;
    frame(&mut app, Vec2::new(1020., 1030.), true, None);
    assert_eq!(
        app.world().resource::<PieceDataStore>().drag.delta,
        Vec2::ZERO
    );
    frame(&mut app, Vec2::new(1030., 1030.), true, None);
    let store = app.world().resource::<PieceDataStore>();
    assert_eq!(store.drag.delta, Vec2::new(10., 0.));
    assert_eq!(store.states.to_vec(), states);
    assert!(Arc::ptr_eq(&mask, &store.drag.members));
    assert!(store.dirty_pieces.is_empty());
    assert_eq!(
        app.world().resource::<pieces::PieceUpload>().revision,
        revision
    );
    // Mouse release wins over Q/E; it cannot issue a second rotation from Idle.
    frame(
        &mut app,
        Vec2::new(1030., 1030.),
        false,
        Some(KeyCode::KeyE),
    );
    let store = app.world().resource::<PieceDataStore>();
    for (id, state) in store.states.iter().enumerate() {
        assert_eq!(decode_rotation(state.flags), 1);
        assert_eq!(state.position, states[id].position + Vec2::new(10., 0.));
    }
    assert!(store.held_by.is_empty());
    assert!(!app.world().resource::<PieceInteraction>().is_dragging());
}

#[test]
fn q_e_rotate_dragged_singletons_about_their_displayed_centers() {
    let mut app = app();
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .connectivity = PieceConnectivity::new(2);
    let before = app.world().resource::<PieceDataStore>().states.to_vec();
    let next_z = app.world().resource::<PieceDataStore>().next_z_order;
    for (key, rotation) in [(KeyCode::KeyQ, 1), (KeyCode::KeyE, 0)] {
        frame(&mut app, Vec2::new(1020., 1030.), true, Some(key));
        let store = app.world().resource::<PieceDataStore>();
        for (state, original) in store.states.iter().zip(&before) {
            assert_eq!(state.position, original.position + Vec2::new(20., 30.));
            assert_eq!(decode_rotation(state.flags), rotation);
        }
        assert_eq!(store.drag.delta, Vec2::ZERO);
        assert_eq!(store.next_z_order, next_z);
        assert_eq!(store.held_by.len(), 2);
    }
}

#[test]
fn rejected_drag_rotation_keeps_the_old_anchor_and_delta() {
    let mut app = app();
    app.world_mut().resource_mut::<PieceDataStore>().states[1].flags &= !pieces::HELD;
    let before = app.world().resource::<PieceDataStore>().states.to_vec();
    frame(&mut app, Vec2::new(1020., 1030.), true, Some(KeyCode::KeyQ));
    assert_eq!(
        app.world().resource::<PieceDataStore>().states.to_vec(),
        before
    );
    assert_eq!(
        app.world().resource::<PieceDataStore>().drag.delta,
        Vec2::new(20., 30.)
    );
    app.world_mut().resource_mut::<PieceDataStore>().states[1].flags |= pieces::HELD;
    frame(&mut app, Vec2::new(1030., 1030.), true, None);
    assert_eq!(
        app.world().resource::<PieceDataStore>().drag.delta,
        Vec2::new(30., 30.)
    );
}

#[test]
fn pending_point_and_box_selection_ignore_rotation() {
    let mut interaction = PieceInteraction::default();
    let mut store = PieceDataStore::default();
    store.initialize(vec![Vec2::ZERO]);
    store.selected_pieces.fill();
    let mut selection = PuzzleSelection::default();
    let frame = || PointerFrame {
        position: Some(Vec2::ZERO),
        screen_position: Some(Vec2::ZERO),
        pressed: true,
        just_pressed: true,
        ctrl: false,
        over_ui: false,
        focused: true,
    };
    interaction.update(
        frame(),
        &mut store,
        &mut selection,
        puzzella_core::LOCAL_PLAYER,
    );
    assert!(interaction.rotation_command(&store, 1).is_none());
    let request = selection.latest.unwrap();
    selection.completed = Some(SelectionResult {
        request_id: request.request_id,
        mode: request.mode,
        payload: SelectionPayload::Point(None),
        error: None,
    });
    interaction.update(
        frame(),
        &mut store,
        &mut selection,
        puzzella_core::LOCAL_PLAYER,
    );
    assert!(matches!(interaction.gesture, Gesture::BoxSelecting { .. }));
    assert!(interaction.rotation_command(&store, -1).is_none());
}
