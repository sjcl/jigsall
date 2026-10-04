//! UI preferences occupy a section of the shared per-user settings.json.
use crate::localization::{LanguagePreference, Localization};
use bevy::prelude::*;
use jigsall_game::settings_file::{SettingsFile, SettingsSection};
use serde::{Deserialize, Serialize};
#[cfg(test)]
use std::path::PathBuf;

#[derive(Default, Serialize, Deserialize)]
struct Preferences {
    #[serde(default)]
    language: LanguagePreference,
}

#[derive(Resource)]
pub(crate) struct UiPreferences {
    pub language: LanguagePreference,
    pub error: Option<PreferenceError>,
    file: SettingsFile,
}

pub(crate) enum PreferenceError {
    Read(String),
    Save(String),
}

impl Default for UiPreferences {
    fn default() -> Self {
        Self::from_file(SettingsFile::default())
    }
}

impl UiPreferences {
    #[cfg(test)]
    pub fn load(path: Option<PathBuf>) -> Self {
        Self::from_file(SettingsFile::new(path))
    }

    fn from_file(file: SettingsFile) -> Self {
        let (preferences, error) = file.load::<Preferences>(SettingsSection::Preferences);
        Self {
            language: preferences.language,
            error: error.map(PreferenceError::Read),
            file,
        }
    }

    /// Change only after an explicit UI selection; a failed save keeps the choice usable.
    pub fn set_language(&mut self, language: LanguagePreference, i18n: &mut Localization) {
        self.language = language;
        i18n.set_preference(language);
        self.error = self.save().err().map(PreferenceError::Save);
    }

    fn save(&mut self) -> Result<(), String> {
        self.file.save(
            SettingsSection::Preferences,
            &Preferences {
                language: self.language,
            },
        )
    }

    pub fn poll_save(&mut self) {
        if let Some(result) = self.file.poll_save() {
            self.error = result.err().map(PreferenceError::Save);
        }
    }

    #[cfg(test)]
    pub fn is_save_pending(&self) -> bool {
        self.file.is_save_pending()
    }
}

pub(crate) fn poll_save(mut preferences: ResMut<UiPreferences>) {
    preferences.poll_save();
}

pub(crate) fn initialize(preferences: Res<UiPreferences>, mut i18n: ResMut<Localization>) {
    i18n.set_preference(preferences.language);
}

#[cfg(test)]
pub(crate) fn wait_for_save(mut poll: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while poll() {
        assert!(
            std::time::Instant::now() < deadline,
            "settings save timed out"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}
