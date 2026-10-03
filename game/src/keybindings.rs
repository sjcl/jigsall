//! Local keyboard controls, shared by gameplay and the settings UI.
use bevy::input::{keyboard::KeyboardInput, ButtonState};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::{io::Write, path::PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyAction {
    RotateLeft,
    RotateRight,
    MultiSelect,
    ShowPlayers,
    Performance,
}

impl KeyAction {
    pub const ALL: [Self; 5] = [
        Self::RotateLeft,
        Self::RotateRight,
        Self::MultiSelect,
        Self::ShowPlayers,
        Self::Performance,
    ];

    pub fn label_key(self) -> &'static str {
        match self {
            Self::RotateLeft => "keys-rotate-left",
            Self::RotateRight => "keys-rotate-right",
            Self::MultiSelect => "keys-multi-select",
            Self::ShowPlayers => "keys-show-players",
            Self::Performance => "keys-performance",
        }
    }
}

/// Left and right modifier keys share an assignment.
pub fn normalize_key(key: KeyCode) -> KeyCode {
    match key {
        KeyCode::ShiftRight => KeyCode::ShiftLeft,
        KeyCode::ControlRight => KeyCode::ControlLeft,
        KeyCode::AltRight => KeyCode::AltLeft,
        KeyCode::SuperRight => KeyCode::SuperLeft,
        _ => key,
    }
}

fn equivalent_keys(key: KeyCode) -> [KeyCode; 2] {
    let key = normalize_key(key);
    [
        key,
        match key {
            KeyCode::ShiftLeft => KeyCode::ShiftRight,
            KeyCode::ControlLeft => KeyCode::ControlRight,
            KeyCode::AltLeft => KeyCode::AltRight,
            KeyCode::SuperLeft => KeyCode::SuperRight,
            _ => key,
        },
    ]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyChord {
    first: KeyCode,
    second: Option<KeyCode>,
}

impl KeyChord {
    pub fn new(first: KeyCode, second: Option<KeyCode>) -> Result<Self, &'static str> {
        let mut keys = vec![normalize_key(first)];
        if let Some(second) = second {
            keys.push(normalize_key(second));
        }
        if keys
            .iter()
            .any(|key| matches!(key, KeyCode::Escape | KeyCode::Unidentified(_)))
        {
            return Err("keys-invalid");
        }
        keys.sort_unstable();
        keys.dedup();
        if second.is_some() && keys.len() != 2 {
            return Err("keys-invalid");
        }
        Ok(Self {
            first: keys[0],
            second: keys.get(1).copied(),
        })
    }

    pub fn keys(self) -> impl Iterator<Item = KeyCode> {
        [Some(self.first), self.second].into_iter().flatten()
    }

    fn pressed(self, input: &ButtonInput<KeyCode>) -> bool {
        self.keys()
            .all(|key| input.any_pressed(equivalent_keys(key)))
    }

    fn just_pressed(self, input: &ButtonInput<KeyCode>) -> bool {
        (self.second.is_none() || self.pressed(input))
            && self.keys().any(|key| {
                let alternatives = equivalent_keys(key);
                // Pressing the other side of an already held modifier is not a new gesture.
                input.any_just_pressed(alternatives)
                    && alternatives
                        .into_iter()
                        .all(|key| !input.pressed(key) || input.just_pressed(key))
            })
    }

    pub fn label(self) -> String {
        let mut keys: Vec<_> = self.keys().collect();
        keys.sort_by_key(|key| (!is_modifier(*key), *key));
        keys.into_iter()
            .map(key_label)
            .collect::<Vec<_>>()
            .join(" + ")
    }
}

fn is_modifier(key: KeyCode) -> bool {
    matches!(
        key,
        KeyCode::ShiftLeft | KeyCode::ControlLeft | KeyCode::AltLeft | KeyCode::SuperLeft
    )
}

fn key_label(key: KeyCode) -> String {
    let label = match key {
        KeyCode::ShiftLeft => "Shift",
        KeyCode::ControlLeft => "Ctrl",
        KeyCode::AltLeft => "Alt",
        KeyCode::SuperLeft => "Super",
        KeyCode::ArrowUp => "↑",
        KeyCode::ArrowDown => "↓",
        KeyCode::ArrowLeft => "←",
        KeyCode::ArrowRight => "→",
        KeyCode::Backquote => "`",
        KeyCode::Backslash => "\\",
        KeyCode::BracketLeft => "[",
        KeyCode::BracketRight => "]",
        KeyCode::Comma => ",",
        KeyCode::Period => ".",
        KeyCode::Slash => "/",
        KeyCode::Semicolon => ";",
        KeyCode::Quote => "'",
        KeyCode::Minus => "-",
        KeyCode::Equal => "=",
        _ => {
            let name = format!("{key:?}");
            return name
                .strip_prefix("Key")
                .or_else(|| name.strip_prefix("Digit"))
                .unwrap_or(&name)
                .to_owned();
        }
    };
    label.to_owned()
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionBinding {
    pub primary: Option<KeyChord>,
    pub secondary: Option<KeyChord>,
}

impl ActionBinding {
    pub fn chords(&self) -> impl Iterator<Item = KeyChord> + '_ {
        [self.primary, self.secondary].into_iter().flatten()
    }

    pub fn label(&self) -> String {
        self.chords()
            .map(KeyChord::label)
            .collect::<Vec<_>>()
            .join(" / ")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct KeyBindings {
    rotate_left: ActionBinding,
    rotate_right: ActionBinding,
    multi_select: ActionBinding,
    show_players: ActionBinding,
    performance: ActionBinding,
}

impl Default for KeyBindings {
    fn default() -> Self {
        let binding = |key| ActionBinding {
            primary: Some(KeyChord::new(key, None).unwrap()),
            secondary: None,
        };
        Self {
            rotate_left: binding(KeyCode::KeyQ),
            rotate_right: binding(KeyCode::KeyE),
            multi_select: binding(KeyCode::ControlLeft),
            show_players: binding(KeyCode::Tab),
            performance: binding(KeyCode::F3),
        }
    }
}

impl KeyBindings {
    pub fn binding(&self, action: KeyAction) -> &ActionBinding {
        match action {
            KeyAction::RotateLeft => &self.rotate_left,
            KeyAction::RotateRight => &self.rotate_right,
            KeyAction::MultiSelect => &self.multi_select,
            KeyAction::ShowPlayers => &self.show_players,
            KeyAction::Performance => &self.performance,
        }
    }

    pub fn binding_mut(&mut self, action: KeyAction) -> &mut ActionBinding {
        match action {
            KeyAction::RotateLeft => &mut self.rotate_left,
            KeyAction::RotateRight => &mut self.rotate_right,
            KeyAction::MultiSelect => &mut self.multi_select,
            KeyAction::ShowPlayers => &mut self.show_players,
            KeyAction::Performance => &mut self.performance,
        }
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        let mut seen = Vec::new();
        for action in KeyAction::ALL {
            for chord in self.binding(action).chords() {
                let canonical = KeyChord::new(chord.first, chord.second)?;
                if seen.contains(&canonical) {
                    return Err("keys-conflict");
                }
                seen.push(canonical);
            }
        }
        Ok(())
    }

    /// A held two-key chord takes priority over its one-key subsets.
    fn unshadowed(&self, chord: KeyChord, input: &ButtonInput<KeyCode>) -> bool {
        chord.second.is_some()
            || !KeyAction::ALL.into_iter().any(|action| {
                self.binding(action).chords().any(|other| {
                    other.second.is_some()
                        && other.keys().any(|key| key == chord.first)
                        && other.pressed(input)
                })
            })
    }

    pub fn pressed(&self, action: KeyAction, input: &ButtonInput<KeyCode>) -> bool {
        self.binding(action)
            .chords()
            .any(|chord| chord.pressed(input) && self.unshadowed(chord, input))
    }

    pub fn just_pressed(&self, action: KeyAction, input: &ButtonInput<KeyCode>) -> bool {
        self.binding(action)
            .chords()
            .any(|chord| chord.just_pressed(input) && self.unshadowed(chord, input))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum KeyBindingsError {
    Read(String),
    Save(String),
    Invalid(&'static str),
}

#[derive(Resource)]
pub struct KeyBindingsState {
    pub current: KeyBindings,
    pub error: Option<KeyBindingsError>,
    path: Option<PathBuf>,
}

impl Default for KeyBindingsState {
    fn default() -> Self {
        Self::load(
            directories::BaseDirs::new()
                .map(|dirs| dirs.data_local_dir().join("puzzella/keybindings.json")),
        )
    }
}

impl KeyBindingsState {
    pub fn just_pressed(
        &self,
        action: KeyAction,
        keys: &ButtonInput<KeyCode>,
        presses: Option<&KeyPresses>,
    ) -> bool {
        presses.map_or_else(
            || self.current.just_pressed(action, keys),
            |presses| presses.triggered[action as usize],
        )
    }

    pub fn load(path: Option<PathBuf>) -> Self {
        let mut state = Self {
            current: default(),
            error: None,
            path,
        };
        if let Some(path) = &state.path {
            match std::fs::read(path) {
                Ok(bytes) => match serde_json::from_slice::<KeyBindings>(&bytes) {
                    Ok(mut bindings) => match bindings.validate() {
                        Ok(()) => {
                            for action in KeyAction::ALL {
                                let binding = bindings.binding_mut(action);
                                for chord in [&mut binding.primary, &mut binding.secondary]
                                    .into_iter()
                                    .flatten()
                                {
                                    *chord = KeyChord::new(chord.first, chord.second).unwrap();
                                }
                            }
                            state.current = bindings;
                        }
                        Err(key) => state.error = Some(KeyBindingsError::Invalid(key)),
                    },
                    Err(error) => state.error = Some(KeyBindingsError::Read(error.to_string())),
                },
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => state.error = Some(KeyBindingsError::Read(error.to_string())),
            }
        }
        state
    }

    /// Keep the previous controls active if validation or the atomic save fails.
    pub fn apply(&mut self, bindings: KeyBindings) {
        if let Err(key) = bindings.validate() {
            self.error = Some(KeyBindingsError::Invalid(key));
            return;
        }
        let save = || -> Result<(), Box<dyn std::error::Error>> {
            if let Some(path) = &self.path {
                let parent = path.parent().ok_or("missing settings directory")?;
                std::fs::create_dir_all(parent)?;
                let mut file = tempfile::NamedTempFile::new_in(parent)?;
                file.write_all(&serde_json::to_vec_pretty(&bindings)?)?;
                file.as_file().sync_all()?;
                file.persist(path)?;
            }
            Ok(())
        };
        match save() {
            Ok(()) => {
                self.current = bindings;
                self.error = None;
            }
            Err(error) => self.error = Some(KeyBindingsError::Save(error.to_string())),
        }
    }
}

/// Retain the order of keyboard events so short chords still work at low frame rates.
#[derive(Resource, Default)]
pub struct KeyPresses {
    held: ButtonInput<KeyCode>,
    triggered: [bool; KeyAction::ALL.len()],
}

pub fn sample_key_presses(
    mut presses: ResMut<KeyPresses>,
    bindings: Res<KeyBindingsState>,
    keys: Res<ButtonInput<KeyCode>>,
    mut events: MessageReader<KeyboardInput>,
) {
    let chords: [Option<KeyChord>; 10] = std::array::from_fn(|index| {
        let binding = bindings.current.binding(KeyAction::ALL[index / 2]);
        if index.is_multiple_of(2) {
            binding.primary
        } else {
            binding.secondary
        }
    });
    let mut candidates = [false; 10];
    let mut saw_event = false;
    for event in events.read() {
        saw_event = true;
        presses.held.clear();
        match event.state {
            ButtonState::Pressed => {
                if event.repeat {
                    continue;
                }
                presses.held.press(event.key_code);
                for (index, chord) in chords.iter().enumerate() {
                    if chord.is_some_and(|chord| chord.just_pressed(&presses.held)) {
                        candidates[index] = true;
                    }
                }
            }
            ButtonState::Released => presses.held.release(event.key_code),
        }
    }
    // If a chord completed in this frame, suppress the constituent single-key presses.
    let triggered_chords = candidates;
    for (index, chord) in chords.iter().enumerate() {
        if let Some(chord) = chord.filter(|chord| chord.second.is_none()) {
            if chords.iter().enumerate().any(|(other_index, other)| {
                other.is_some_and(|other| {
                    other.second.is_some()
                        && other.keys().any(|key| key == chord.first)
                        && (triggered_chords[other_index] || other.pressed(keys.as_ref()))
                })
            }) {
                candidates[index] = false;
            }
        }
    }
    for (index, action) in KeyAction::ALL.into_iter().enumerate() {
        presses.triggered[index] = if saw_event {
            candidates[index * 2] || candidates[index * 2 + 1]
        } else {
            bindings.current.just_pressed(action, &keys)
        };
    }
    // Also resynchronize after focus loss, which may clear keys without individual releases.
    presses.held.reset_all();
    for key in keys.get_pressed() {
        presses.held.press(*key);
    }
    presses.held.clear();
}

#[cfg(test)]
mod tests;
