//! Per-user autosave preferences and the host's active-play timer.
use super::runtime::{PersistenceService, PersistenceState};
use super::SaveTitle;
use crate::resources::{GameSubState, LocalPlayerId, SessionHostId};
use crate::settings_file::{SettingsFile, SettingsSection};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::{num::NonZeroU32, path::PathBuf, time::Duration};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutosaveSettings {
    /// None disables autosaving. Zero is rejected when reading settings.
    pub interval_minutes: Option<NonZeroU32>,
    /// Number of automatic checkpoints retained for each persistent game ID.
    #[serde(default = "default_max_saves_per_game")]
    pub max_saves_per_game: NonZeroU32,
}

fn default_max_saves_per_game() -> NonZeroU32 {
    NonZeroU32::MIN
}

impl Default for AutosaveSettings {
    fn default() -> Self {
        Self {
            interval_minutes: NonZeroU32::new(5),
            max_saves_per_game: default_max_saves_per_game(),
        }
    }
}

#[derive(Debug)]
pub enum AutosaveSettingsError {
    Read(String),
    Save(String),
}

#[derive(Resource)]
pub struct AutosaveSettingsState {
    pub current: AutosaveSettings,
    pub error: Option<AutosaveSettingsError>,
    file: SettingsFile,
}

impl Default for AutosaveSettingsState {
    fn default() -> Self {
        Self::from_file(SettingsFile::default())
    }
}

impl AutosaveSettingsState {
    pub fn load(path: Option<PathBuf>) -> Self {
        Self::from_file(SettingsFile::new(path))
    }

    fn from_file(file: SettingsFile) -> Self {
        let (current, error) = file.load(SettingsSection::Autosave);
        Self {
            current,
            error: error.map(AutosaveSettingsError::Read),
            file,
        }
    }

    /// Apply immediately, like language preferences; a failed write keeps the choice usable.
    pub fn set_interval(&mut self, interval_minutes: Option<NonZeroU32>) {
        self.current.interval_minutes = interval_minutes;
        self.error = self.save().err().map(AutosaveSettingsError::Save);
    }

    pub fn set_max_saves_per_game(&mut self, max_saves_per_game: NonZeroU32) {
        self.current.max_saves_per_game = max_saves_per_game;
        self.error = self.save().err().map(AutosaveSettingsError::Save);
    }

    fn save(&mut self) -> Result<(), String> {
        self.file.save(SettingsSection::Autosave, &self.current)
    }

    pub fn is_save_pending(&self) -> bool {
        self.file.is_save_pending()
    }

    pub fn poll_save(&mut self) {
        if let Some(result) = self.file.poll_save() {
            self.error = result.err().map(AutosaveSettingsError::Save);
        }
    }
}

#[derive(Resource, Default)]
pub(crate) struct AutosaveTimer {
    elapsed: Duration,
    interval: Option<NonZeroU32>,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn tick_autosave(
    time: Res<Time<Real>>,
    settings: Res<AutosaveSettingsState>,
    local: Res<LocalPlayerId>,
    host: Res<SessionHostId>,
    playing: Option<Res<State<GameSubState>>>,
    mut timer: ResMut<AutosaveTimer>,
    service: Res<PersistenceService>,
    mut state: ResMut<PersistenceState>,
) {
    let interval = settings.current.interval_minutes;
    if timer.interval != interval || local.0 != host.0 {
        timer.elapsed = Duration::ZERO;
        timer.interval = interval;
        return;
    }
    let Some(minutes) = interval else {
        return;
    };
    if playing.is_none_or(|state| *state.get() != GameSubState::Playing) {
        return;
    }
    let duration = Duration::from_secs(u64::from(minutes.get()) * 60);
    timer.elapsed = timer.elapsed.saturating_add(time.delta()).min(duration);
    if timer.elapsed < duration || state.busy || state.title_dialog_open {
        return;
    }
    let title = state
        .current_save
        .as_ref()
        .or(state.current_autosave.as_ref())
        .map(|metadata| metadata.title.clone())
        .unwrap_or_else(|| SaveTitle::new("Puzzle").expect("valid default title"));
    service.request_autosave(&mut state, title, settings.current.max_saves_per_game);
    timer.elapsed = Duration::ZERO;
}

#[cfg(test)]
mod tests;
