//! UI preferences occupy a section of the shared per-user settings.json.
use crate::localization::{LanguagePreference, Localization};
use bevy::prelude::*;
use puzzella_game::settings_file::{SettingsFile, SettingsSection};
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

    fn save(&self) -> Result<(), String> {
        self.file.save(
            SettingsSection::Preferences,
            &Preferences {
                language: self.language,
            },
        )
    }
}

pub(crate) fn initialize(preferences: Res<UiPreferences>, mut i18n: ResMut<Localization>) {
    i18n.set_preference(preferences.language);
}
