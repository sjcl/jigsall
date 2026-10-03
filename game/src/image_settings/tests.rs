use super::*;
use std::time::{Duration, Instant};

#[test]
fn texture_caps_respect_square_budget_game_and_device_limits() {
    for budget in [1, 16, 63, 256, 512, 1024, 4096, 32768] {
        let settings = ImageSettings {
            texture_budget: TextureBudget::Manual { mib: budget },
        };
        let gpu = limits(32768, 32768);
        let edge = settings.max_texture_dimension(&gpu);
        let bytes = budget * 1024 * 1024;
        assert!(u64::from(edge).pow(2) * 4 <= bytes);
        assert!(edge == MAX_PUZZLE_IMAGE_DIMENSION || u64::from(edge + 1).pow(2) * 4 > bytes);
        for device in [2048, 8192, 16384, 32768] {
            assert_eq!(
                limits(32768, device)
                    .decode_limits(&settings)
                    .max_texture_dimension,
                edge.min(device)
            );
        }
    }
}

fn limits(mib: u64, device_max_dimension: u32) -> PuzzleImageLimits {
    PuzzleImageLimits {
        device_max_dimension,
        gpu_memory_bytes: Some(mib * 1024 * 1024),
    }
}

#[test]
fn default_auto_budget_and_manual_range_follow_each_clients_gpu() {
    let settings = ImageSettings::default();
    assert_eq!(settings.texture_budget, TextureBudget::Auto { percent: 20 });
    for (capacity, expected) in [(2048, 409), (8192, 1638), (32768, 6553), (98304, 19660)] {
        let gpu = limits(capacity, 32768);
        assert_eq!(gpu.max_budget_mib(), capacity);
        assert_eq!(settings.budget_mib(&gpu), expected);
        let manual = ImageSettings {
            texture_budget: TextureBudget::Manual { mib: 32768 },
        };
        assert_eq!(manual.budget_mib(&gpu), capacity.min(32768));
    }
}

#[test]
fn unsupported_capacity_uses_explicit_bounded_fallback() {
    for (device, expected) in [(2048, 16), (8192, 256), (16384, 512)] {
        let gpu = PuzzleImageLimits {
            device_max_dimension: device,
            gpu_memory_bytes: None,
        };
        assert_eq!(ImageSettings::default().budget_mib(&gpu), expected);
        assert!(
            gpu.decode_limits(&ImageSettings::default())
                .max_texture_dimension
                <= device
        );
    }
    assert_eq!(ImageSettings::default().budget_mib(&limits(1, 8192)), 1);
}

#[test]
fn texture_settings_round_trip_preserves_other_sections() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    std::fs::write(&path, r#"{"preferences":{"language":"ja"}}"#).unwrap();
    let mut state = ImageSettingsState::load(Some(path.clone()));
    for budget in [
        TextureBudget::Manual { mib: 24576 },
        TextureBudget::Auto { percent: 35 },
    ] {
        state.set_budget(budget);
        let deadline = Instant::now() + Duration::from_secs(10);
        while state.is_save_pending() {
            state.poll_save();
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(state.error.is_none());
        assert_eq!(
            ImageSettingsState::load(Some(path.clone()))
                .current
                .texture_budget,
            budget
        );
    }
    let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(saved["preferences"]["language"], "ja");
}

#[test]
fn invalid_saved_budgets_use_default_and_invalid_edits_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    for budget in [
        TextureBudget::Auto { percent: 0 },
        TextureBudget::Auto { percent: 101 },
        TextureBudget::Manual { mib: 0 },
    ] {
        let document = serde_json::json!({"image": ImageSettings { texture_budget: budget }});
        std::fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
        let mut state = ImageSettingsState::load(Some(path.clone()));
        assert_eq!(state.current, ImageSettings::default());
        assert!(matches!(state.error, Some(ImageSettingsError::Read(_))));
        state.set_budget(budget);
        assert_eq!(state.current, ImageSettings::default());
        assert!(matches!(state.error, Some(ImageSettingsError::Save(_))));
        assert!(!state.is_save_pending());
    }
}
