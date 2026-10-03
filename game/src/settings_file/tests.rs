use super::*;
use crate::{
    keybindings::{KeyBindings, KeyBindingsError, KeyBindingsState},
    persistence::autosave::{AutosaveSettings, AutosaveSettingsError, AutosaveSettingsState},
    settings::{DisplaySettings, DisplaySettingsState},
};
use serde_json::json;

#[test]
fn old_files_and_flat_display_settings_are_not_loaded_or_migrated() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let display = DisplaySettings {
        max_fps: Some(144),
        ..Default::default()
    };
    let bytes = serde_json::to_vec(&display).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    for (name, value) in [
        ("autosave-settings.json", json!({"interval_minutes": 12})),
        (
            "keybindings.json",
            json!({"performance": {"primary": null}}),
        ),
        ("ui-settings.json", json!({"language": "ja"})),
    ] {
        std::fs::write(dir.path().join(name), serde_json::to_vec(&value).unwrap()).unwrap();
    }
    let display = DisplaySettingsState::load(Some(path.clone()));
    assert_eq!(display.current, DisplaySettings::default());
    assert!(display.error.is_none());
    let keys = KeyBindingsState::load(Some(path.clone()));
    assert_eq!(keys.current, KeyBindings::default());
    assert!(keys.error.is_none());
    let autosave = AutosaveSettingsState::load(Some(path.clone()));
    assert_eq!(autosave.current, AutosaveSettings::default());
    assert!(autosave.error.is_none());
    let file = SettingsFile::new(Some(path.clone()));
    let (preferences, error) = file.load::<Value>(SettingsSection::Preferences);
    assert_eq!(preferences, Value::default());
    assert!(error.is_none());
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

#[test]
fn invalid_sections_use_defaults_without_blocking_other_sections_or_losing_data() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let display = DisplaySettings {
        max_fps: None,
        ..Default::default()
    };
    let document = json!({
        "display": display,
        "autosave": {"interval_minutes": 0},
        "keybindings": {"rotate_left": {"primary": {"first": "Escape", "second": null}}},
        "future": {"value": 42}
    });
    std::fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
    assert_eq!(
        DisplaySettingsState::load(Some(path.clone())).current,
        display
    );
    let mut autosave = AutosaveSettingsState::load(Some(path.clone()));
    assert_eq!(autosave.current, AutosaveSettings::default());
    assert!(matches!(
        autosave.error,
        Some(AutosaveSettingsError::Read(_))
    ));
    let keys = KeyBindingsState::load(Some(path.clone()));
    assert_eq!(keys.current, KeyBindings::default());
    assert!(matches!(keys.error, Some(KeyBindingsError::Invalid(_))));
    autosave.set_interval(None);
    assert!(autosave.error.is_none());
    let saved: Value = read_json(&path).unwrap().unwrap();
    assert_eq!(saved["autosave"], json!({"interval_minutes": null}));
    assert_eq!(saved["display"], document["display"]);
    assert_eq!(saved["keybindings"], document["keybindings"]);
    assert_eq!(saved["future"], document["future"]);
}

#[test]
fn corrupt_document_is_not_overwritten_when_saving_one_section() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let mut keys = KeyBindingsState::load(Some(path.clone()));
    for bytes in [b"{broken".as_slice(), b"[]", b"null"] {
        std::fs::write(&path, bytes).unwrap();
        keys.apply(KeyBindings::default());
        assert!(matches!(keys.error, Some(KeyBindingsError::Save(_))));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}
