//! Local display preference; connection credentials never enter this section.
use crate::settings_file::{SettingsFile, SettingsSection};
use bevy::prelude::Resource;
use puzzella_core::{DisplayNameError, PlayerDisplayName};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerSettings {
    pub display_name: Option<PlayerDisplayName>,
}
#[derive(Debug, PartialEq, Eq)]
pub enum PlayerSettingsError {
    Invalid(DisplayNameError),
    Read(String),
    Save(String),
}
#[derive(Resource)]
pub struct PlayerSettingsState {
    pub current: PlayerSettings,
    pub error: Option<PlayerSettingsError>,
    file: SettingsFile,
}
impl Default for PlayerSettingsState {
    fn default() -> Self {
        Self::from_file(SettingsFile::default())
    }
}
impl PlayerSettingsState {
    pub fn load(path: Option<PathBuf>) -> Self {
        Self::from_file(SettingsFile::new(path))
    }
    fn from_file(file: SettingsFile) -> Self {
        let (current, error) = file.load(SettingsSection::Player);
        Self {
            current,
            error: error.map(PlayerSettingsError::Read),
            file,
        }
    }
    /// Commit a draft, validating once. Host/Join options can clone the preference.
    /// A running session's profile remains fixed until the next session.
    pub fn commit(&mut self, input: &str) -> bool {
        let display_name = match PlayerDisplayName::optional_from_user_input(input) {
            Ok(name) => name,
            Err(error) => {
                self.error = Some(PlayerSettingsError::Invalid(error));
                return false;
            }
        };
        if display_name == self.current.display_name
            && !matches!(
                self.error,
                Some(PlayerSettingsError::Read(_) | PlayerSettingsError::Save(_))
            )
        {
            self.error = None;
            return true;
        }
        self.current.display_name = display_name;
        self.error = self
            .file
            .save(SettingsSection::Player, &self.current)
            .err()
            .map(PlayerSettingsError::Save);
        true
    }
    pub fn is_save_pending(&self) -> bool {
        self.file.is_save_pending()
    }
    pub fn poll_save(&mut self) {
        if let Some(result) = self.file.poll_save() {
            if !matches!(self.error, Some(PlayerSettingsError::Invalid(_))) {
                self.error = result.err().map(PlayerSettingsError::Save);
            }
        }
    }
}

#[cfg(test)]
mod tests;
