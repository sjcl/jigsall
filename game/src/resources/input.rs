use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions};

/// Sampled pointer coordinates and local camera state.
#[derive(Resource, Default)]
pub struct InputState {
    pub mouse_position: Option<Vec2>,
    pub window_focused: bool,
    pub is_camera_dragging: bool,
    pub camera_drag_start_position: Option<Vec2>,
    pub cursor_screen_position: Option<Vec2>,
}

impl InputState {
    /// End every camera capture through this path, including session resets.
    pub(crate) fn end_camera_drag<'a>(
        &mut self,
        windows: impl IntoIterator<Item = (Mut<'a, Window>, Mut<'a, CursorOptions>)>,
    ) {
        let start_position = self.camera_drag_start_position.take();
        let was_dragging = std::mem::take(&mut self.is_camera_dragging);
        if !was_dragging && start_position.is_none() {
            return;
        }
        for (mut window, mut cursor) in windows {
            cursor.grab_mode = CursorGrabMode::None;
            cursor.visible = true;
            if let Some(position) = start_position {
                // Best effort: the window backend handles unsupported cursor warps.
                window.set_cursor_position(Some(position));
            }
        }
    }
}

/// The HUD is built on a separate background Ui, so its rectangle must be
/// captured explicitly as well as egui's normal window/widget capture.
#[derive(Resource, Default)]
pub struct GameUiPointerCapture {
    pub over_hud: bool,
}

/// A modal session operation can suspend local interaction without suspending
/// puzzle generation, GPU upload or network polling. Owned by the game layer.
#[derive(Resource, Default)]
pub struct LocalGameplayBlocked(pub bool);

pub fn local_gameplay_enabled(
    blocked: Res<LocalGameplayBlocked>,
    next: Res<NextState<super::AppState>>,
) -> bool {
    !blocked.0
        && !matches!(
            *next,
            NextState::Pending(super::AppState::Menu)
                | NextState::PendingIfDifferent(super::AppState::Menu)
        )
}

pub(crate) fn local_gameplay_just_blocked(blocked: Res<LocalGameplayBlocked>) -> bool {
    blocked.0 && blocked.is_changed()
}
