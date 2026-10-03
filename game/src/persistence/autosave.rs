//! Per-user autosave preferences and the host's active-play timer.
use super::runtime::{PersistenceService, PersistenceState};
use super::SaveTitle;
use crate::resources::{GameSubState, LocalPlayerId, SessionHostId};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::{io::Write, num::NonZeroU32, path::PathBuf, time::Duration};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutosaveSettings {
    /// None disables autosaving. Zero is rejected when reading settings.
    pub interval_minutes: Option<NonZeroU32>,
}

impl Default for AutosaveSettings {
    fn default() -> Self {
        Self {
            interval_minutes: NonZeroU32::new(5),
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
    path: Option<PathBuf>,
}

impl Default for AutosaveSettingsState {
    fn default() -> Self {
        Self::load(directories::BaseDirs::new().map(|dirs| {
            dirs.data_local_dir()
                .join("puzzella/autosave-settings.json")
        }))
    }
}

impl AutosaveSettingsState {
    pub fn load(path: Option<PathBuf>) -> Self {
        let mut state = Self {
            current: default(),
            error: None,
            path,
        };
        if let Some(path) = &state.path {
            match std::fs::read(path) {
                Ok(bytes) => match serde_json::from_slice(&bytes) {
                    Ok(settings) => state.current = settings,
                    Err(error) => {
                        state.error = Some(AutosaveSettingsError::Read(error.to_string()))
                    }
                },
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => state.error = Some(AutosaveSettingsError::Read(error.to_string())),
            }
        }
        state
    }

    /// Apply immediately, like language preferences; a failed write keeps the choice usable.
    pub fn set_interval(&mut self, interval_minutes: Option<NonZeroU32>) {
        self.current.interval_minutes = interval_minutes;
        self.error = self.save().err().map(AutosaveSettingsError::Save);
    }

    fn save(&self) -> Result<(), String> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let write = || -> Result<(), Box<dyn std::error::Error>> {
            let parent = path.parent().ok_or("missing autosave settings directory")?;
            std::fs::create_dir_all(parent)?;
            let mut file = tempfile::NamedTempFile::new_in(parent)?;
            file.write_all(&serde_json::to_vec_pretty(&self.current)?)?;
            file.as_file().sync_all()?;
            file.persist(path)?;
            Ok(())
        };
        write().map_err(|error| error.to_string())
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
    service.request_autosave(&mut state, title);
    timer.elapsed = Duration::ZERO;
}

#[cfg(test)]
mod tests;
