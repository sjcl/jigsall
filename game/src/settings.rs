//! Display settings, independent of the menu. Resolutions are physical pixels.
use bevy::{
    prelude::*,
    time::TimeSystems,
    window::{
        Monitor, MonitorSelection, OnMonitor, PresentMode, PrimaryMonitor, PrimaryWindow,
        VideoMode, VideoModeSelection, WindowMode,
    },
};
use bevy_egui::EguiPostUpdateSet;
use serde::{Deserialize, Serialize};
use std::{
    io::Write,
    path::PathBuf,
    time::{Duration, Instant},
};

const CONFIRM_SECONDS: u64 = 15;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayValidationError {
    InvalidResolution,
    InvalidFps,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DisplaySettingsError {
    Invalid(DisplayValidationError),
    InvalidSaved(DisplayValidationError),
    ReadFailed(String),
    SaveFailed(String),
    DirectoryUnavailable,
    UnsupportedFullscreen,
    NoDisplay,
    FullscreenUnavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplaySettingsNotice {
    Restored,
    Saved,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScreenMode {
    #[default]
    Windowed,
    Borderless,
    Fullscreen,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplaySettings {
    pub resolution: UVec2,
    pub mode: ScreenMode,
    /// None removes the software frame limit. Presentation may still be driver limited.
    pub max_fps: Option<u32>,
}

impl Default for DisplaySettings {
    fn default() -> Self {
        Self {
            resolution: UVec2::new(1280, 720),
            mode: ScreenMode::Windowed,
            max_fps: Some(60),
        }
    }
}

impl DisplaySettings {
    pub fn validate(&self) -> Result<(), DisplayValidationError> {
        if self.resolution.x < 640
            || self.resolution.y < 360
            || self.resolution.x > 16384
            || self.resolution.y > 16384
        {
            return Err(DisplayValidationError::InvalidResolution);
        }
        if self.max_fps.is_some_and(|fps| !(10..=1000).contains(&fps)) {
            return Err(DisplayValidationError::InvalidFps);
        }
        Ok(())
    }

    fn frame_interval(&self) -> Option<Duration> {
        self.max_fps
            .filter(|fps| *fps > 0)
            .map(|fps| Duration::from_secs_f64(1.0 / f64::from(fps)))
    }
}

#[derive(Resource, Default)]
pub struct DisplayCapabilities {
    pub monitor: Option<Entity>,
    pub desktop_resolution: Option<UVec2>,
    pub video_modes: Vec<VideoMode>,
}

impl DisplayCapabilities {
    fn validate_settings(&self, settings: &DisplaySettings) -> Result<(), DisplaySettingsError> {
        settings.validate().map_err(DisplaySettingsError::Invalid)?;
        if settings.mode != ScreenMode::Windowed && self.monitor.is_none() {
            return Err(DisplaySettingsError::NoDisplay);
        }
        if settings.mode == ScreenMode::Fullscreen
            && self.fullscreen_mode(settings.resolution).is_none()
        {
            return Err(DisplaySettingsError::UnsupportedFullscreen);
        }
        Ok(())
    }

    pub fn fullscreen_mode(&self, size: UVec2) -> Option<VideoMode> {
        self.video_modes
            .iter()
            .filter(|mode| mode.physical_size == size)
            .max_by_key(|mode| (mode.refresh_rate_millihertz, mode.bit_depth))
            .copied()
    }

    pub fn resolutions(&self, mode: ScreenMode, selected: UVec2) -> Vec<UVec2> {
        let mut sizes = if mode == ScreenMode::Fullscreen {
            self.video_modes
                .iter()
                .map(|mode| mode.physical_size)
                .collect::<Vec<_>>()
        } else {
            [
                (800, 600),
                (1024, 768),
                (1280, 720),
                (1280, 800),
                (1366, 768),
                (1600, 900),
                (1920, 1080),
                (1920, 1200),
                (2560, 1440),
                (3840, 2160),
            ]
            .into_iter()
            .map(|(w, h)| UVec2::new(w, h))
            .filter(|size| {
                self.desktop_resolution
                    .is_none_or(|desktop| size.x <= desktop.x && size.y <= desktop.y)
            })
            .collect()
        };
        if mode != ScreenMode::Fullscreen {
            sizes.push(selected);
            if let Some(desktop) = self.desktop_resolution {
                sizes.push(desktop);
            }
        }
        sizes.retain(|size| size.x >= 640 && size.y >= 360);
        sizes.sort_unstable_by_key(|size| (size.x, size.y));
        sizes.dedup();
        sizes
    }
}

struct DisplayPreview {
    previous: DisplaySettings,
    deadline: Instant,
}

#[derive(Resource)]
pub struct DisplaySettingsState {
    pub current: DisplaySettings,
    pub error: Option<DisplaySettingsError>,
    pub notice: Option<DisplaySettingsNotice>,
    path: Option<PathBuf>,
    preview: Option<DisplayPreview>,
}

impl Default for DisplaySettingsState {
    fn default() -> Self {
        let path = directories::BaseDirs::new()
            .map(|dirs| dirs.data_local_dir().join("puzzella/settings.json"));
        Self::load(path)
    }
}

impl DisplaySettingsState {
    /// Passing None supports probes without reading or writing the user's settings.
    pub fn load(path: Option<PathBuf>) -> Self {
        let mut state = Self {
            current: default(),
            error: None,
            notice: None,
            path,
            preview: None,
        };
        if let Some(path) = &state.path {
            match std::fs::read(path) {
                Ok(bytes) => match serde_json::from_slice::<DisplaySettings>(&bytes) {
                    Ok(settings) => match settings.validate() {
                        Ok(()) => state.current = settings,
                        Err(error) => state.error = Some(DisplaySettingsError::InvalidSaved(error)),
                    },
                    Err(error) => {
                        state.error = Some(DisplaySettingsError::ReadFailed(error.to_string()))
                    }
                },
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    state.error = Some(DisplaySettingsError::ReadFailed(error.to_string()))
                }
            }
        }
        state
    }

    pub fn confirmation_seconds(&self) -> Option<u64> {
        self.preview.as_ref().map(|preview| {
            preview
                .deadline
                .saturating_duration_since(Instant::now())
                .as_secs()
                + 1
        })
    }

    /// Whether applying this draft can change the display or retry a failed save.
    pub fn can_apply(
        &self,
        settings: &DisplaySettings,
        capabilities: &DisplayCapabilities,
    ) -> bool {
        self.preview.is_none()
            && capabilities.validate_settings(settings).is_ok()
            && (self.display_changed(settings)
                || settings.max_fps != self.current.max_fps
                || matches!(
                    self.error,
                    Some(
                        DisplaySettingsError::SaveFailed(_)
                            | DisplaySettingsError::DirectoryUnavailable
                    )
                ))
    }

    fn display_changed(&self, settings: &DisplaySettings) -> bool {
        settings.mode != self.current.mode
            || (settings.mode != ScreenMode::Borderless
                && settings.resolution != self.current.resolution)
    }

    fn save(&self) -> Result<(), DisplaySettingsError> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let parent = path
            .parent()
            .ok_or(DisplaySettingsError::DirectoryUnavailable)?;
        let write = || -> Result<(), Box<dyn std::error::Error>> {
            std::fs::create_dir_all(parent)?;
            let mut file = tempfile::NamedTempFile::new_in(parent)?;
            file.write_all(&serde_json::to_vec_pretty(&self.current)?)?;
            file.as_file().sync_all()?;
            file.persist(path)?;
            Ok(())
        };
        write().map_err(|error| DisplaySettingsError::SaveFailed(error.to_string()))
    }

    fn revert(&mut self) {
        if let Some(preview) = self.preview.take() {
            self.current = preview.previous;
            self.error = None;
            self.notice = Some(DisplaySettingsNotice::Restored);
        }
    }

    fn commit(&mut self) {
        match self.save() {
            Ok(()) => {
                self.preview = None;
                self.error = None;
                self.notice = Some(DisplaySettingsNotice::Saved);
            }
            Err(error) => self.error = Some(error),
        }
    }
}

#[derive(Message)]
pub enum DisplaySettingsAction {
    Apply(DisplaySettings),
    Keep,
    Revert,
    /// Discard an unconfirmed preview and clear the dialog's completion notice.
    Dismiss,
}

pub struct DisplaySettingsPlugin;
impl Plugin for DisplaySettingsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DisplaySettingsState>()
            .init_resource::<DisplayCapabilities>()
            .init_resource::<FramePacer>()
            .add_message::<DisplaySettingsAction>()
            .add_systems(First, limit_frame_rate.before(TimeSystems))
            .add_systems(
                PostUpdate,
                (refresh_capabilities, process_actions, apply_window_settings)
                    .chain()
                    .after(EguiPostUpdateSet::EndPass),
            );
    }
}

fn refresh_capabilities(
    windows: Query<Option<&OnMonitor>, With<PrimaryWindow>>,
    monitors: Query<(Entity, &Monitor, Has<PrimaryMonitor>)>,
    mut capabilities: ResMut<DisplayCapabilities>,
) {
    let linked = windows.single().ok().flatten().map(|monitor| monitor.0);
    let monitor = linked
        .and_then(|entity| monitors.get(entity).ok())
        .or_else(|| monitors.iter().find(|(_, _, primary)| *primary))
        .or_else(|| monitors.iter().next());
    let Some((entity, monitor, _)) = monitor else {
        if capabilities.monitor.is_some() {
            *capabilities = default();
        }
        return;
    };
    if capabilities.monitor != Some(entity)
        || capabilities.desktop_resolution != Some(monitor.physical_size())
        || capabilities.video_modes != monitor.video_modes
    {
        *capabilities = DisplayCapabilities {
            monitor: Some(entity),
            desktop_resolution: Some(monitor.physical_size()),
            video_modes: monitor.video_modes.clone(),
        };
    }
}

fn process_actions(
    mut actions: MessageReader<DisplaySettingsAction>,
    capabilities: Res<DisplayCapabilities>,
    mut state: ResMut<DisplaySettingsState>,
) {
    if state
        .preview
        .as_ref()
        .is_some_and(|preview| Instant::now() >= preview.deadline)
    {
        state.revert();
    }
    for action in actions.read() {
        match action {
            DisplaySettingsAction::Dismiss => {
                state.revert();
                state.notice = None;
            }
            DisplaySettingsAction::Revert => state.revert(),
            DisplaySettingsAction::Keep => {
                if state.preview.is_some() {
                    state.commit();
                }
            }
            DisplaySettingsAction::Apply(settings) => {
                if state.preview.is_some() {
                    continue;
                }
                if let Err(error) = capabilities.validate_settings(settings) {
                    state.error = Some(error);
                    continue;
                }
                if !state.can_apply(settings, &capabilities) {
                    continue;
                }
                let display_changed = state.display_changed(settings);
                if display_changed {
                    state.preview = Some(DisplayPreview {
                        previous: state.current.clone(),
                        deadline: Instant::now() + Duration::from_secs(CONFIRM_SECONDS),
                    });
                }
                state.current = settings.clone();
                state.error = None;
                state.notice = None;
                if !display_changed {
                    state.commit();
                }
            }
        }
    }
}

fn apply_window_settings(
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    capabilities: Res<DisplayCapabilities>,
    mut state: ResMut<DisplaySettingsState>,
    mut applied: Local<Option<DisplaySettings>>,
) {
    if applied.as_ref() == Some(&state.current) {
        return;
    }
    let Ok(mut window) = windows.single_mut() else {
        return;
    };
    let mode = match state.current.mode {
        ScreenMode::Windowed => WindowMode::Windowed,
        ScreenMode::Borderless => {
            let Some(monitor) = capabilities.monitor else {
                return;
            };
            WindowMode::BorderlessFullscreen(MonitorSelection::Entity(monitor))
        }
        ScreenMode::Fullscreen => {
            let Some(monitor) = capabilities.monitor else {
                return;
            };
            if let Some(video_mode) = capabilities.fullscreen_mode(state.current.resolution) {
                WindowMode::Fullscreen(
                    MonitorSelection::Entity(monitor),
                    VideoModeSelection::Specific(video_mode),
                )
            } else {
                state.current.mode = ScreenMode::Windowed;
                state.error = Some(DisplaySettingsError::FullscreenUnavailable);
                WindowMode::Windowed
            }
        }
    };
    let display_changed = applied.as_ref().is_none_or(|previous| {
        previous.mode != state.current.mode || previous.resolution != state.current.resolution
    });
    if display_changed {
        window.mode = mode;
        if state.current.mode == ScreenMode::Windowed {
            window.set_maximized(false);
            window
                .resolution
                .set_physical_resolution(state.current.resolution.x, state.current.resolution.y);
        }
    }
    // AutoNoVsync supports caps above refresh rate and safely falls back if unavailable.
    window.present_mode = PresentMode::AutoNoVsync;
    *applied = Some(state.current.clone());
}

#[derive(Resource, Default)]
struct FramePacer {
    interval: Option<Duration>,
    last_start: Option<Instant>,
}

impl FramePacer {
    fn remaining(&mut self, now: Instant, interval: Option<Duration>) -> Duration {
        if self.interval != interval {
            self.interval = interval;
            self.last_start = None;
        }
        match (self.last_start, interval) {
            (Some(last), Some(interval)) => (last + interval).saturating_duration_since(now),
            _ => Duration::ZERO,
        }
    }
}

fn limit_frame_rate(
    state: Res<DisplaySettingsState>,
    mut pacer: ResMut<FramePacer>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    if windows.is_empty() {
        return;
    }
    let interval = state.current.frame_interval();
    loop {
        let remaining = pacer.remaining(Instant::now(), interval);
        if remaining.is_zero() {
            break;
        }
        std::thread::sleep(remaining);
    }
    pacer.last_start = Some(Instant::now());
}

#[cfg(test)]
mod tests;
