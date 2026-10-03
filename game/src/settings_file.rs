//! Sections of the per-user settings.json, shared by the game and UI crates.
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{Map, Value};
use std::{io::Write, path::PathBuf};

#[derive(Clone, Copy)]
pub enum SettingsSection {
    Display,
    KeyBindings,
    Autosave,
    Preferences,
}

impl SettingsSection {
    fn key(self) -> &'static str {
        match self {
            Self::Display => "display",
            Self::KeyBindings => "keybindings",
            Self::Autosave => "autosave",
            Self::Preferences => "preferences",
        }
    }
}

pub struct SettingsFile {
    path: Option<PathBuf>,
}

impl Default for SettingsFile {
    fn default() -> Self {
        Self::new(
            directories::BaseDirs::new()
                .map(|dirs| dirs.data_local_dir().join("puzzella/settings.json")),
        )
    }
}

impl SettingsFile {
    /// None disables filesystem access for tests and probes.
    pub fn new(path: Option<PathBuf>) -> Self {
        Self { path }
    }

    /// Missing or invalid sections use their defaults. An invalid section cannot
    /// prevent another section from loading.
    pub fn load<T: DeserializeOwned + Default>(
        &self,
        section: SettingsSection,
    ) -> (T, Option<String>) {
        let read = || -> Result<_, String> {
            self.document()?
                .remove(section.key())
                .map(serde_json::from_value)
                .transpose()
                .map(|current| current.unwrap_or_default())
                .map_err(|error| error.to_string())
        };
        match read() {
            Ok(current) => (current, None),
            Err(error) => (T::default(), Some(error)),
        }
    }

    /// Reread before updating one section: other resources may have saved since
    /// this resource loaded. Unconfirmed display previews never enter this file.
    pub fn save<T: Serialize>(&self, section: SettingsSection, current: &T) -> Result<(), String> {
        if self.path.is_none() {
            return Ok(());
        }
        let mut document = self.document()?;
        document.insert(
            section.key().into(),
            serde_json::to_value(current).map_err(|error| error.to_string())?,
        );
        self.write_document(&document)
    }

    fn document(&self) -> Result<Map<String, Value>, String> {
        Ok(match &self.path {
            Some(path) => read_json(path)?.unwrap_or_default(),
            None => Map::new(),
        })
    }

    fn write_document(&self, document: &Map<String, Value>) -> Result<(), String> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let write = || -> Result<(), Box<dyn std::error::Error>> {
            let parent = path.parent().ok_or("missing settings directory")?;
            std::fs::create_dir_all(parent)?;
            let mut file = tempfile::NamedTempFile::new_in(parent)?;
            file.write_all(&serde_json::to_vec_pretty(document)?)?;
            file.as_file().sync_all()?;
            file.persist(path)?;
            Ok(())
        };
        write().map_err(|error| error.to_string())
    }
}

fn read_json<T: DeserializeOwned>(path: &std::path::Path) -> Result<Option<T>, String> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| error.to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(test)]
mod tests;
