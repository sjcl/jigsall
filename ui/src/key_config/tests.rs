use super::*;
use bevy::input::keyboard::Key;

fn event(key_code: KeyCode, state: ButtonState) -> KeyboardInput {
    KeyboardInput {
        key_code,
        logical_key: Key::Unidentified(bevy::input::keyboard::NativeKey::Unidentified),
        state,
        text: None,
        repeat: false,
        window: Entity::PLACEHOLDER,
    }
}

fn editor(secondary: bool) -> KeyConfigEditor {
    KeyConfigEditor {
        draft: Some(KeyBindings::default()),
        capture: Some(KeyCapture {
            action: KeyAction::RotateLeft,
            secondary,
            waiting_for_release: false,
            held: vec![],
            chord: vec![],
            too_many: false,
        }),
        error: None,
    }
}

fn feed(editor: &mut KeyConfigEditor, events: &[KeyboardInput]) -> bool {
    editor.capture_input(&CaptureInput {
        keys: &ButtonInput::default(),
        events,
        focused: true,
    })
}

#[test]
fn captures_fast_single_taps_and_two_key_chords_after_release() {
    let mut editor = editor(true);
    feed(
        &mut editor,
        &[event(KeyCode::ShiftRight, ButtonState::Pressed)],
    );
    feed(
        &mut editor,
        &[
            event(KeyCode::KeyR, ButtonState::Pressed),
            event(KeyCode::KeyR, ButtonState::Released),
        ],
    );
    assert!(editor.capture.is_some());
    assert!(editor
        .draft
        .as_ref()
        .unwrap()
        .binding(KeyAction::RotateLeft)
        .secondary
        .is_none());
    feed(
        &mut editor,
        &[event(KeyCode::ShiftRight, ButtonState::Released)],
    );
    assert_eq!(
        editor
            .draft
            .as_ref()
            .unwrap()
            .binding(KeyAction::RotateLeft)
            .secondary
            .unwrap()
            .label(),
        "Shift + R"
    );
    assert_eq!(
        editor
            .draft
            .as_ref()
            .unwrap()
            .binding(KeyAction::RotateLeft)
            .primary
            .unwrap()
            .label(),
        "Q"
    );
    editor.capture = Some(KeyCapture {
        action: KeyAction::RotateRight,
        secondary: false,
        waiting_for_release: false,
        held: vec![],
        chord: vec![],
        too_many: false,
    });
    feed(
        &mut editor,
        &[
            event(KeyCode::KeyT, ButtonState::Pressed),
            event(KeyCode::KeyT, ButtonState::Released),
        ],
    );
    assert_eq!(
        editor
            .draft
            .as_ref()
            .unwrap()
            .binding(KeyAction::RotateRight)
            .primary
            .unwrap()
            .label(),
        "T"
    );
}

#[test]
fn rejects_three_keys_and_cancels_on_escape_or_focus_loss() {
    let mut editor = editor(false);
    feed(
        &mut editor,
        &[
            event(KeyCode::KeyA, ButtonState::Pressed),
            event(KeyCode::KeyB, ButtonState::Pressed),
            event(KeyCode::KeyC, ButtonState::Pressed),
            event(KeyCode::KeyA, ButtonState::Released),
            event(KeyCode::KeyB, ButtonState::Released),
            event(KeyCode::KeyC, ButtonState::Released),
        ],
    );
    assert_eq!(editor.error, Some("keys-too-many"));
    assert_eq!(editor.draft, Some(KeyBindings::default()));
    let mut editor = self::editor(false);
    assert!(feed(
        &mut editor,
        &[event(KeyCode::Escape, ButtonState::Pressed)]
    ));
    assert!(editor.capture.is_none());
    assert_eq!(editor.draft, Some(KeyBindings::default()));
    let mut editor = self::editor(false);
    editor.capture_input(&CaptureInput {
        keys: &ButtonInput::default(),
        events: &[],
        focused: false,
    });
    assert!(editor.capture.is_none());
    assert_eq!(editor.draft, Some(KeyBindings::default()));
}

#[test]
fn waits_for_keys_held_before_capture_and_does_not_combine_sequential_taps() {
    let mut editor = editor(false);
    editor.capture.as_mut().unwrap().waiting_for_release = true;
    let mut keys = ButtonInput::default();
    keys.press(KeyCode::ShiftLeft);
    editor.capture_input(&CaptureInput {
        keys: &keys,
        events: &[event(KeyCode::ShiftLeft, ButtonState::Pressed)],
        focused: true,
    });
    feed(
        &mut editor,
        &[event(KeyCode::ShiftLeft, ButtonState::Released)],
    );
    assert!(editor.capture.as_ref().unwrap().chord.is_empty());
    feed(
        &mut editor,
        &[
            event(KeyCode::KeyA, ButtonState::Pressed),
            event(KeyCode::KeyA, ButtonState::Released),
            event(KeyCode::KeyB, ButtonState::Pressed),
            event(KeyCode::KeyB, ButtonState::Released),
        ],
    );
    assert_eq!(
        editor
            .draft
            .as_ref()
            .unwrap()
            .binding(KeyAction::RotateLeft)
            .primary
            .unwrap()
            .label(),
        "A"
    );
}
