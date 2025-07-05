use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Resource, Default)]
pub struct GameState {
    pub current_screen: GameScreen,
    pub is_host: bool,
    pub players: Vec<PlayerInfo>,
    pub puzzle_completed: bool,
    pub puzzle_progress: f32,
}

#[derive(Default, PartialEq, Clone)]
pub enum GameScreen {
    #[default]
    Menu,
    HostSetup,
    JoinGame,
    InGame,
    GameComplete,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlayerInfo {
    pub id: Uuid,
    pub name: String,
    pub score: u32,
}

#[derive(Resource)]
pub struct PuzzleConfig {
    pub grid_size: (usize, usize),
    pub piece_size: f32,
    pub snap_distance: f32,
    pub image_path: String,
    pub target_piece_count: usize,
    pub use_target_mode: bool, // true: ターゲットピース数モード, false: 手動グリッドサイズモード
}

impl Default for PuzzleConfig {
    fn default() -> Self {
        Self {
            grid_size: (4, 4),
            piece_size: 100.0,
            snap_distance: 50.0, // Reduced to prevent immediate snapping
            image_path: String::new(), // 空の文字列から開始
            target_piece_count: 16, // デフォルト16ピース
            use_target_mode: true, // デフォルトはターゲットピース数モード
        }
    }
}

#[derive(Resource)]
pub struct PuzzleImage {
    pub handle: Handle<Image>,
    pub size: Vec2,
}

#[derive(Resource, Default)]
pub struct NetworkInfo {
    pub server_address: String,
    pub port: u16,
    pub player_id: Option<Uuid>,
}

#[derive(Resource)]
pub struct InputState {
    pub mouse_position: Vec2,
    pub is_mouse_pressed: bool,
    pub selected_piece: Option<Entity>,
    pub next_z_order: f32,
    pub is_camera_dragging: bool,
    pub camera_drag_start_pos: Vec2,
    pub last_mouse_position: Vec2,
    pub last_cursor_position: Option<Vec2>,
}

impl Default for InputState {
    fn default() -> Self {
        Self {
            mouse_position: Vec2::ZERO,
            is_mouse_pressed: false,
            selected_piece: None,
            next_z_order: 1.0, // 1.0から開始
            is_camera_dragging: false,
            camera_drag_start_pos: Vec2::ZERO,
            last_mouse_position: Vec2::ZERO,
            last_cursor_position: None,
        }
    }
}