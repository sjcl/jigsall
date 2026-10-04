use bevy::prelude::*;

/// Sampled pointer coordinates and local camera state.
#[derive(Resource, Default)]
pub struct InputState {
    pub mouse_position: Option<Vec2>,
    pub window_focused: bool,
    pub is_camera_dragging: bool,
    pub last_cursor_position: Option<Vec2>,
    pub cursor_screen_position: Option<Vec2>,
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
                | NextState::PendingIfNeq(super::AppState::Menu)
        )
}

pub(crate) fn local_gameplay_just_blocked(blocked: Res<LocalGameplayBlocked>) -> bool {
    blocked.0 && blocked.is_changed()
}
