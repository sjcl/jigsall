use super::*;
use std::time::{Duration, Instant};

#[test]
fn preferences_roundtrip_validate_and_preserve_other_sections() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    std::fs::write(&path, r#"{"preferences":{"language":"ja"}}"#).unwrap();
    let mut state = PlayerSettingsState::load(Some(path.clone()));
    assert!(state.commit("　日本語 🧩　"));
    wait(&mut state);
    assert!(state.error.is_none());
    assert_eq!(
        PlayerSettingsState::load(Some(path.clone())).current,
        state.current
    );
    let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["preferences"]["language"], "ja");
    assert_eq!(saved["player"]["display_name"], "日本語 🧩");
    for input in ["name\n", "\u{202e}spoof", &"x".repeat(33)] {
        assert!(!state.commit(input));
        assert!(matches!(state.error, Some(PlayerSettingsError::Invalid(_))));
        assert_eq!(
            PlayerSettingsState::load(Some(path.clone())).current,
            state.current
        );
        assert!(!state.is_save_pending());
    }
    assert!(state.commit("　"));
    wait(&mut state);
    assert!(PlayerSettingsState::load(Some(path.clone()))
        .current
        .display_name
        .is_none());
    std::fs::write(&path, r#"{"player":{"display_name":"bad\nname"}}"#).unwrap();
    let invalid = PlayerSettingsState::load(Some(path));
    assert!(invalid.current.display_name.is_none());
    assert!(matches!(invalid.error, Some(PlayerSettingsError::Read(_))));
}
fn wait(state: &mut PlayerSettingsState) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while state.is_save_pending() {
        assert!(Instant::now() < deadline);
        state.poll_save();
        std::thread::yield_now();
    }
}
