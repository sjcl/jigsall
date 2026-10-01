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
