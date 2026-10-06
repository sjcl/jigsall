//! Display settings, independent of the menu. Resolutions are physical pixels.
use crate::{
    render::visuals::PieceVisualQuality,
    settings_file::{SettingsFile, SettingsSection},
};
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum GameBackground {
    #[default]
    Light,
    Dark,
}

impl GameBackground {
    pub fn color(self) -> Color {
        match self {
            Self::Light => Color::srgb_u8(243, 234, 215),
            Self::Dark => Color::srgb_u8(43, 44, 47),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplaySettings {
    pub resolution: UVec2,
    pub mode: ScreenMode,
    /// None removes the software frame limit. Presentation may still be driver limited.
    pub max_fps: Option<u32>,
    #[serde(default)]
    pub game_background: GameBackground,
    #[serde(default)]
    pub piece_visual_quality: PieceVisualQuality,
}

impl Default for DisplaySettings {
    fn default() -> Self {
        Self {
            resolution: UVec2::new(1280, 720),
            mode: ScreenMode::Windowed,
            max_fps: Some(60),
            game_background: GameBackground::default(),
            piece_visual_quality: PieceVisualQuality::default(),
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
    file: SettingsFile,
    preview: Option<DisplayPreview>,
    saving_preview: Option<DisplayPreview>,
    show_save_notice: bool,
}

impl Default for DisplaySettingsState {
    fn default() -> Self {
        Self::from_file(SettingsFile::default())
    }
}

impl DisplaySettingsState {
    /// Passing None supports probes without reading or writing the user's settings.
    pub fn load(path: Option<PathBuf>) -> Self {
        Self::from_file(SettingsFile::new(path))
    }

    fn from_file(file: SettingsFile) -> Self {
        let (current, error) = file.load::<DisplaySettings>(SettingsSection::Display);
        let mut state = Self {
            current,
            error: error.map(DisplaySettingsError::ReadFailed),
            notice: None,
            file,
            preview: None,
            saving_preview: None,
            show_save_notice: false,
        };
        if let Err(error) = state.current.validate() {
            state.current = default();
            state.error = Some(DisplaySettingsError::InvalidSaved(error));
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
            && !self.file.is_save_pending()
            && capabilities.validate_settings(settings).is_ok()
            && (self.display_changed(settings)
                || settings.max_fps != self.current.max_fps
                || settings.game_background != self.current.game_background
                || settings.piece_visual_quality != self.current.piece_visual_quality
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

    fn save(&mut self) -> Result<(), DisplaySettingsError> {
        self.file
            .save(SettingsSection::Display, &self.current)
            .map_err(DisplaySettingsError::SaveFailed)
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
                // Keep is an explicit confirmation: a slow write must not let
                // the preview timeout revert a choice already queued for saving.
                self.saving_preview = self.preview.take();
                self.error = None;
                self.notice = None;
                self.show_save_notice = true;
                self.poll_save();
            }
            Err(error) => self.error = Some(error),
        }
    }

    pub fn is_save_pending(&self) -> bool {
        self.file.is_save_pending()
    }

    pub fn poll_save(&mut self) {
        let Some(result) = self.file.poll_save() else {
            return;
        };
        match result {
            Ok(()) => {
                self.saving_preview = None;
                self.error = None;
                if self.show_save_notice {
                    self.notice = Some(DisplaySettingsNotice::Saved);
                }
            }
            Err(error) => {
                self.error = Some(DisplaySettingsError::SaveFailed(error));
                // If the dialog remains open, retain the option to revert a
                // confirmed display change whose persistence failed.
                self.preview = self.saving_preview.take().map(|mut preview| {
                    preview.deadline = Instant::now() + Duration::from_secs(CONFIRM_SECONDS);
                    preview
                });
            }
        }
        self.show_save_notice = false;
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
            .init_resource::<PieceVisualQuality>()
            .init_resource::<DisplayCapabilities>()
            .init_resource::<FramePacer>()
            .add_message::<DisplaySettingsAction>()
            .add_systems(First, limit_frame_rate.before(TimeSystems))
            .add_systems(Update, |mut state: ResMut<DisplaySettingsState>| {
                state.poll_save()
            })
            .add_systems(
                PostUpdate,
                (
                    refresh_capabilities,
                    process_actions,
                    apply_window_settings,
                    apply_game_background,
                    apply_piece_visual_quality,
                )
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
                state.saving_preview = None;
                state.show_save_notice = false;
                state.notice = None;
            }
            DisplaySettingsAction::Revert => state.revert(),
            DisplaySettingsAction::Keep => {
                if state.preview.is_some() {
                    state.commit();
                }
            }
            DisplaySettingsAction::Apply(settings) => {
                if state.preview.is_some() || state.file.is_save_pending() {
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

// Settings own this synchronization, so standalone renderer fixtures can still
// replace the quality resource without installing display settings.
fn apply_piece_visual_quality(
    state: Res<DisplaySettingsState>,
    mut quality: ResMut<PieceVisualQuality>,
) {
    if *quality != state.current.piece_visual_quality {
        *quality = state.current.piece_visual_quality;
    }
}

fn apply_game_background(
    state: Res<DisplaySettingsState>,
    mut cameras: Query<&mut Camera, With<crate::components::MainCamera>>,
) {
    let color = state.current.game_background.color();
    for mut camera in &mut cameras {
        if !matches!(camera.clear_color, bevy::camera::ClearColorConfig::Custom(current) if current == color)
        {
            camera.clear_color = bevy::camera::ClearColorConfig::Custom(color);
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
