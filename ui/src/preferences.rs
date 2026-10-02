//! UI preferences use the same per-user directory and atomic writes as display settings.
use crate::localization::{LanguagePreference, Localization};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::{io::Write, path::PathBuf};

#[derive(Default, Serialize, Deserialize)]
struct Preferences {
    #[serde(default)]
    language: LanguagePreference,
}

#[derive(Resource)]
pub(crate) struct UiPreferences {
    pub language: LanguagePreference,
    pub error: Option<PreferenceError>,
    path: Option<PathBuf>,
}

pub(crate) enum PreferenceError {
    Read(String),
    Save(String),
}

impl Default for UiPreferences {
    fn default() -> Self {
        Self::load(
            directories::BaseDirs::new()
                .map(|dirs| dirs.data_local_dir().join("puzzella/ui-settings.json")),
        )
    }
}

impl UiPreferences {
    pub fn load(path: Option<PathBuf>) -> Self {
        let mut state = Self {
            language: LanguagePreference::Auto,
            error: None,
            path,
        };
        if let Some(path) = &state.path {
            match std::fs::read(path) {
                Ok(bytes) => match serde_json::from_slice::<Preferences>(&bytes) {
                    Ok(settings) => state.language = settings.language,
                    Err(error) => state.error = Some(PreferenceError::Read(error.to_string())),
                },
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => state.error = Some(PreferenceError::Read(error.to_string())),
            }
        }
        state
    }

    /// Change only after an explicit UI selection; a failed save keeps the choice usable.
    pub fn set_language(&mut self, language: LanguagePreference, i18n: &mut Localization) {
        self.language = language;
        i18n.set_preference(language);
        self.error = self.save().err().map(PreferenceError::Save);
    }

    fn save(&self) -> Result<(), String> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let write = || -> Result<(), Box<dyn std::error::Error>> {
            let parent = path.parent().ok_or("missing preferences directory")?;
            std::fs::create_dir_all(parent)?;
            let mut file = tempfile::NamedTempFile::new_in(parent)?;
            file.write_all(&serde_json::to_vec_pretty(&Preferences {
                language: self.language,
            })?)?;
            file.as_file().sync_all()?;
            file.persist(path)?;
            Ok(())
        };
        write().map_err(|error| error.to_string())
    }
}

pub(crate) fn initialize(preferences: Res<UiPreferences>, mut i18n: ResMut<Localization>) {
    i18n.set_preference(preferences.language);
}
