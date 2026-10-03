//! Local puzzle texture budget. This never changes saved puzzle coordinates.
use crate::settings_file::{SettingsFile, SettingsSection};
use bevy::prelude::*;
use puzzella_core::MAX_PUZZLE_IMAGE_DIMENSION;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::resources::PuzzleImageLimits;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextureBudget {
    Auto { percent: u32 },
    Manual { mib: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageSettings {
    /// Budget for the single shared RGBA8 puzzle texture, without mipmaps.
    pub texture_budget: TextureBudget,
}

impl Default for ImageSettings {
    fn default() -> Self {
        Self {
            texture_budget: TextureBudget::Auto { percent: 20 },
        }
    }
}

impl ImageSettings {
    pub fn validate(&self) -> Result<(), &'static str> {
        match self.texture_budget {
            TextureBudget::Auto { percent } if !(1..=100).contains(&percent) => {
                return Err("Automatic GPU memory percentage must be between 1 and 100");
            }
            TextureBudget::Manual { mib: 0 } => {
                return Err("Puzzle texture budget must be positive")
            }
            _ => {}
        }
        Ok(())
    }

    /// Resolve against this client's GPU, including portable manual settings
    /// saved on another GPU. Unsupported capacity queries use a 512 MiB fallback.
    pub fn budget_mib(&self, limits: &PuzzleImageLimits) -> u64 {
        match self.texture_budget {
            TextureBudget::Auto { percent } => limits
                .gpu_memory_bytes
                .map(|bytes| {
                    // Convert to MiB before multiplying to avoid overflow.
                    (bytes / (1024 * 1024)).saturating_mul(u64::from(percent.clamp(1, 100))) / 100
                })
                .unwrap_or(512)
                .clamp(1, limits.max_budget_mib()),
            TextureBudget::Manual { mib } => mib.clamp(1, limits.max_budget_mib()),
        }
    }

    pub fn max_texture_dimension(&self, limits: &PuzzleImageLimits) -> u32 {
        let bytes = self.budget_mib(limits).saturating_mul(1024 * 1024);
        (bytes / 4)
            .isqrt()
            .min(u64::from(MAX_PUZZLE_IMAGE_DIMENSION)) as u32
    }
}

#[derive(Debug)]
pub enum ImageSettingsError {
    Read(String),
    Save(String),
}

#[derive(Resource)]
pub struct ImageSettingsState {
    pub current: ImageSettings,
    pub error: Option<ImageSettingsError>,
    file: SettingsFile,
}

impl Default for ImageSettingsState {
    fn default() -> Self {
        Self::from_file(SettingsFile::default())
    }
}

impl ImageSettingsState {
    pub fn load(path: Option<PathBuf>) -> Self {
        Self::from_file(SettingsFile::new(path))
    }

    fn from_file(file: SettingsFile) -> Self {
        let (mut current, mut error) = file.load::<ImageSettings>(SettingsSection::Image);
        if let Err(reason) = current.validate() {
            current = default();
            error = Some(reason.into());
        }
        Self {
            current,
            error: error.map(ImageSettingsError::Read),
            file,
        }
    }

    /// Persist immediately. Only subsequent image selections / save loads use it.
    pub fn set_budget(&mut self, texture_budget: TextureBudget) {
        let settings = ImageSettings { texture_budget };
        if let Err(reason) = settings.validate() {
            self.error = Some(ImageSettingsError::Save(reason.into()));
            return;
        }
        self.current = settings;
        self.error = self
            .file
            .save(SettingsSection::Image, &settings)
            .err()
            .map(ImageSettingsError::Save);
    }

    pub fn is_save_pending(&self) -> bool {
        self.file.is_save_pending()
    }

    pub fn poll_save(&mut self) {
        if let Some(result) = self.file.poll_save() {
            self.error = result.err().map(ImageSettingsError::Save);
        }
    }
}

#[cfg(test)]
mod tests;
