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
    wait_for_save(|| {
        autosave.poll_save();
        autosave.is_save_pending()
    });
    assert!(autosave.error.is_none());
    let saved: Value = read_json(&path).unwrap().unwrap();
    assert_eq!(
        saved["autosave"],
        json!({"interval_minutes": null, "max_saves_per_game": 1})
    );
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
        wait_for_save(|| {
            keys.poll_save();
            keys.is_save_pending()
        });
        assert!(matches!(keys.error, Some(KeyBindingsError::Save(_))));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}

#[test]
fn saves_and_polling_return_while_the_writer_is_blocked() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let (mut file, resume) = SettingsFile::paused(path.clone());
    file.save(SettingsSection::Preferences, &json!({"language": "ja"}))
        .unwrap();
    assert!(file.is_save_pending());
    assert!(file.poll_save().is_none());
    assert!(
        !path.exists(),
        "the caller must not perform filesystem writes"
    );
    resume.send(()).unwrap();
    wait_for_save(|| {
        assert!(file.poll_save().is_none_or(|result| result.is_ok()));
        file.is_save_pending()
    });
    assert_eq!(
        read_json::<Value>(&path).unwrap().unwrap()["preferences"]["language"],
        "ja"
    );
}

#[test]
fn section_resources_share_a_writer_and_keep_fifo_order_and_unknown_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    std::fs::write(&path, br#"{"future":{"value":42}}"#).unwrap();
    let mut display = SettingsFile::new(Some(path.clone()));
    let mut preferences = SettingsFile::new(Some(path.clone()));
    let mut autosave = SettingsFile::new(Some(path.clone()));
    // Enqueue without waiting between sections, as separate UI resources do.
    for fps in [60, 144, 240] {
        display
            .save(SettingsSection::Display, &json!({"max_fps": fps}))
            .unwrap();
        preferences
            .save(SettingsSection::Preferences, &json!({"language": "ja"}))
            .unwrap();
        autosave
            .save(SettingsSection::Autosave, &json!({"interval_minutes": fps}))
            .unwrap();
    }
    assert!(Arc::ptr_eq(
        display.writer.as_ref().unwrap(),
        preferences.writer.as_ref().unwrap()
    ));
    assert!(Arc::ptr_eq(
        display.writer.as_ref().unwrap(),
        autosave.writer.as_ref().unwrap()
    ));
    wait_for_save(|| {
        for file in [&mut display, &mut preferences, &mut autosave] {
            assert!(file.poll_save().is_none_or(|result| result.is_ok()));
        }
        display.is_save_pending() || preferences.is_save_pending() || autosave.is_save_pending()
    });
    assert_eq!(
        read_json::<Value>(&path).unwrap().unwrap(),
        json!({
            "display": {"max_fps": 240},
            "preferences": {"language": "ja"},
            "autosave": {"interval_minutes": 240},
            "future": {"value": 42}
        })
    );
}

#[test]
fn a_new_save_discards_an_older_failure_reply() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    std::fs::write(&path, b"{broken").unwrap();
    let (mut file, resume) = SettingsFile::paused(path.clone());
    file.save(SettingsSection::Preferences, &json!({"language": "ja"}))
        .unwrap();
    let (entered, ready) = channel::bounded(1);
    let (continue_write, gate) = channel::bounded(1);
    file.writer
        .as_ref()
        .unwrap()
        .requests
        .as_ref()
        .unwrap()
        .send(WriteRequest::Pause {
            entered,
            resume: gate,
        })
        .unwrap();
    resume.send(()).unwrap();
    ready
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    // The old error is ready, but a new request supersedes it before polling.
    std::fs::write(&path, b"{}").unwrap();
    file.save(SettingsSection::Preferences, &json!({"language": "en-US"}))
        .unwrap();
    assert!(file.poll_save().is_none());
    continue_write.send(()).unwrap();
    wait_for_save(|| {
        assert!(file.poll_save().is_none_or(|result| result.is_ok()));
        file.is_save_pending()
    });
    assert_eq!(
        read_json::<Value>(&path).unwrap().unwrap()["preferences"]["language"],
        "en-US"
    );
}

#[test]
fn dropping_the_last_owner_drains_all_accepted_saves() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let (mut file, resume) = SettingsFile::paused(path.clone());
    let writer = Arc::downgrade(file.writer.as_ref().unwrap());
    file.save(SettingsSection::Preferences, &json!({"language": "ja"}))
        .unwrap();
    file.save(SettingsSection::Autosave, &json!({"interval_minutes": 12}))
        .unwrap();
    file.save(SettingsSection::Preferences, &json!({"language": "en-US"}))
        .unwrap();
    resume.send(()).unwrap();
    // Teardown, without polling any reply, still waits for the final atomic replace.
    drop(file);
    assert!(writer.upgrade().is_none());
    assert_eq!(
        read_json::<Value>(&path).unwrap().unwrap(),
        json!({
            "preferences": {"language": "en-US"},
            "autosave": {"interval_minutes": 12}
        })
    );
}
