use super::*;

#[test]
fn primary_secondary_and_chords_require_all_keys_and_trigger_once() {
    let mut bindings = KeyBindings::default();
    bindings.binding_mut(KeyAction::RotateLeft).primary =
        Some(KeyChord::new(KeyCode::ShiftLeft, Some(KeyCode::KeyR)).unwrap());
    bindings.binding_mut(KeyAction::RotateLeft).secondary =
        Some(KeyChord::new(KeyCode::KeyA, Some(KeyCode::KeyB)).unwrap());
    let mut keys = ButtonInput::default();
    keys.press(KeyCode::KeyQ);
    assert!(!bindings.just_pressed(KeyAction::RotateLeft, &keys));
    keys.press(KeyCode::ShiftRight);
    assert!(!bindings.just_pressed(KeyAction::RotateLeft, &keys));
    keys.clear();
    keys.press(KeyCode::KeyR);
    assert!(bindings.just_pressed(KeyAction::RotateLeft, &keys));
    keys.clear();
    assert!(bindings.pressed(KeyAction::RotateLeft, &keys));
    assert!(!bindings.just_pressed(KeyAction::RotateLeft, &keys));
    keys.release(KeyCode::ShiftRight);
    assert!(!bindings.pressed(KeyAction::RotateLeft, &keys));
    keys.press(KeyCode::KeyB);
    assert!(!bindings.just_pressed(KeyAction::RotateLeft, &keys));
    keys.clear();
    keys.press(KeyCode::KeyA);
    assert!(
        bindings.just_pressed(KeyAction::RotateLeft, &keys),
        "Either key can complete a chord"
    );
}

#[test]
fn chord_suppresses_the_other_actions_single_key_without_losing_default_ctrl_rotation() {
    let mut bindings = KeyBindings::default();
    bindings.binding_mut(KeyAction::RotateRight).primary =
        Some(KeyChord::new(KeyCode::ShiftLeft, Some(KeyCode::KeyQ)).unwrap());
    let mut keys = ButtonInput::default();
    keys.press(KeyCode::ShiftLeft);
    keys.press(KeyCode::KeyQ);
    assert!(bindings.just_pressed(KeyAction::RotateRight, &keys));
    assert!(!bindings.just_pressed(KeyAction::RotateLeft, &keys));
    keys.reset_all();
    keys.press(KeyCode::ControlRight);
    keys.press(KeyCode::KeyQ);
    assert!(bindings.just_pressed(KeyAction::RotateLeft, &keys));
    assert!(bindings.pressed(KeyAction::MultiSelect, &keys));
}

#[test]
fn modifiers_match_both_sides_and_pressing_other_side_does_not_retrigger() {
    for (left, right) in [
        (KeyCode::ShiftLeft, KeyCode::ShiftRight),
        (KeyCode::ControlLeft, KeyCode::ControlRight),
        (KeyCode::AltLeft, KeyCode::AltRight),
        (KeyCode::SuperLeft, KeyCode::SuperRight),
    ] {
        let chord = KeyChord::new(right, None).unwrap();
        assert_eq!(chord, KeyChord::new(left, None).unwrap());
        let mut keys = ButtonInput::default();
        keys.press(right);
        assert!(chord.just_pressed(&keys));
        keys.clear();
        keys.press(left);
        assert!(chord.pressed(&keys));
        assert!(!chord.just_pressed(&keys));
    }
}

#[test]
fn reserved_keys_duplicate_keys_and_duplicate_bindings_are_rejected() {
    assert!(KeyChord::new(KeyCode::Escape, None).is_err());
    assert!(KeyChord::new(KeyCode::KeyQ, Some(KeyCode::Escape)).is_err());
    assert!(KeyChord::new(KeyCode::ShiftLeft, Some(KeyCode::ShiftRight)).is_err());
    let mut bindings = KeyBindings::default();
    bindings.binding_mut(KeyAction::ShowPlayers).secondary =
        bindings.binding(KeyAction::RotateLeft).primary;
    assert_eq!(bindings.validate(), Err("keys-conflict"));
}

#[test]
fn saves_both_slots_and_unassigned_keys_and_loads_older_files_with_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("controls.json");
    let mut state = KeyBindingsState::load(Some(path.clone()));
    let mut bindings = state.current.clone();
    bindings.binding_mut(KeyAction::RotateLeft).secondary =
        Some(KeyChord::new(KeyCode::ShiftRight, Some(KeyCode::KeyR)).unwrap());
    bindings.binding_mut(KeyAction::Performance).primary = None;
    state.apply(bindings.clone());
    assert!(state.error.is_none());
    assert_eq!(KeyBindingsState::load(Some(path.clone())).current, bindings);
    std::fs::write(&path, "{}").unwrap();
    assert_eq!(
        KeyBindingsState::load(Some(path.clone())).current,
        KeyBindings::default()
    );
    std::fs::write(
        &path,
        r#"{"rotate_left":{"primary":{"first":"Escape","second":null},"secondary":null}}"#,
    )
    .unwrap();
    let state = KeyBindingsState::load(Some(path));
    assert_eq!(state.current, KeyBindings::default());
    assert_eq!(state.error, Some(KeyBindingsError::Invalid("keys-invalid")));
}

#[test]
fn failed_save_and_invalid_apply_preserve_previous_controls() {
    let dir = tempfile::tempdir().unwrap();
    let parent = dir.path().join("file");
    std::fs::write(&parent, "not a directory").unwrap();
    let mut state = KeyBindingsState::load(Some(parent.join("controls.json")));
    let mut bindings = state.current.clone();
    bindings.binding_mut(KeyAction::Performance).primary = None;
    state.apply(bindings);
    assert!(matches!(state.error, Some(KeyBindingsError::Save(_))));
    assert_eq!(state.current, KeyBindings::default());
    let mut bindings = state.current.clone();
    bindings.binding_mut(KeyAction::Performance).primary =
        bindings.binding(KeyAction::RotateLeft).primary;
    state.apply(bindings);
    assert_eq!(
        state.error,
        Some(KeyBindingsError::Invalid("keys-conflict"))
    );
    assert_eq!(state.current, KeyBindings::default());
}

#[test]
fn labels_are_readable_and_chord_order_is_canonical() {
    let chord = KeyChord::new(KeyCode::KeyR, Some(KeyCode::ShiftRight)).unwrap();
    assert_eq!(chord.label(), "Shift + R");
    assert_eq!(
        chord,
        KeyChord::new(KeyCode::ShiftLeft, Some(KeyCode::KeyR)).unwrap()
    );
    assert_eq!(
        KeyBindings::default()
            .binding(KeyAction::RotateLeft)
            .label(),
        "Q"
    );
}

#[test]
fn single_key_tap_released_in_the_same_frame_is_not_lost() {
    let bindings = KeyBindings::default();
    let mut keys = ButtonInput::default();
    keys.press(KeyCode::KeyQ);
    keys.release(KeyCode::KeyQ);
    assert!(bindings.just_pressed(KeyAction::RotateLeft, &keys));
    assert!(!bindings.pressed(KeyAction::RotateLeft, &keys));
}

fn event(key_code: KeyCode, state: ButtonState) -> KeyboardInput {
    KeyboardInput {
        key_code,
        state,
        logical_key: bevy::input::keyboard::Key::Unidentified(
            bevy::input::keyboard::NativeKey::Unidentified,
        ),
        text: None,
        repeat: false,
        window: Entity::PLACEHOLDER,
    }
}

fn input_app() -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, bevy::input::InputPlugin))
        .insert_resource(KeyBindingsState::load(None))
        .init_resource::<KeyPresses>()
        .add_systems(
            PreUpdate,
            sample_key_presses.after(bevy::input::InputSystems),
        );
    app
}

fn feed(app: &mut App, events: &[(KeyCode, ButtonState)]) {
    for (key, state) in events {
        app.world_mut().write_message(event(*key, *state));
    }
    app.update();
}

#[test]
fn ordered_events_keep_fast_chords_and_reject_sequential_keys_in_the_same_frame() {
    use ButtonState::{Pressed, Released};
    let mut app = input_app();
    app.world_mut()
        .resource_mut::<KeyBindingsState>()
        .current
        .binding_mut(KeyAction::RotateLeft)
        .primary = Some(KeyChord::new(KeyCode::KeyA, Some(KeyCode::KeyB)).unwrap());
    feed(
        &mut app,
        &[
            (KeyCode::KeyA, Pressed),
            (KeyCode::KeyA, Released),
            (KeyCode::KeyB, Pressed),
            (KeyCode::KeyB, Released),
        ],
    );
    assert!(!app.world().resource::<KeyPresses>().triggered[KeyAction::RotateLeft as usize]);
    feed(
        &mut app,
        &[
            (KeyCode::KeyB, Pressed),
            (KeyCode::KeyA, Pressed),
            (KeyCode::KeyB, Released),
            (KeyCode::KeyA, Released),
        ],
    );
    assert!(app.world().resource::<KeyPresses>().triggered[KeyAction::RotateLeft as usize]);
    app.update();
    assert!(!app.world().resource::<KeyPresses>().triggered[KeyAction::RotateLeft as usize]);
}

#[test]
fn fast_modifier_chord_shadows_the_single_key_and_holding_does_not_repeat() {
    use ButtonState::{Pressed, Released};
    let mut app = input_app();
    app.world_mut()
        .resource_mut::<KeyBindingsState>()
        .current
        .binding_mut(KeyAction::RotateRight)
        .primary = Some(KeyChord::new(KeyCode::ShiftLeft, Some(KeyCode::KeyQ)).unwrap());
    feed(
        &mut app,
        &[
            (KeyCode::KeyQ, Pressed),
            (KeyCode::ShiftRight, Pressed),
            (KeyCode::KeyQ, Released),
            (KeyCode::ShiftRight, Released),
        ],
    );
    let presses = app.world().resource::<KeyPresses>();
    assert!(presses.triggered[KeyAction::RotateRight as usize]);
    assert!(!presses.triggered[KeyAction::RotateLeft as usize]);
    feed(&mut app, &[(KeyCode::ShiftRight, Pressed)]);
    assert!(!app.world().resource::<KeyPresses>().triggered[KeyAction::RotateRight as usize]);
    feed(
        &mut app,
        &[(KeyCode::KeyQ, Pressed), (KeyCode::KeyQ, Released)],
    );
    assert!(app.world().resource::<KeyPresses>().triggered[KeyAction::RotateRight as usize]);
    assert!(!app.world().resource::<KeyPresses>().triggered[KeyAction::RotateLeft as usize]);
    app.update();
    assert!(!app.world().resource::<KeyPresses>().triggered[KeyAction::RotateRight as usize]);
    // InputPlugin resets held keys on focus loss even without individual release events.
    app.world_mut()
        .write_message(bevy::input::keyboard::KeyboardFocusLost);
    app.update();
    feed(&mut app, &[(KeyCode::KeyQ, Pressed)]);
    assert!(app.world().resource::<KeyPresses>().triggered[KeyAction::RotateLeft as usize]);
    assert!(!app.world().resource::<KeyPresses>().triggered[KeyAction::RotateRight as usize]);
}
