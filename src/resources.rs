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
    pub needs_reset: bool, // パズルをリセットする必要があるかのフラグ
}

#[derive(Default, PartialEq, Clone)]
pub enum GameScreen {
    #[default]
    Menu,
    HostSetup,
    JoinGame,
    InGame,
    InGameMenu,  // ESCキーで表示されるゲーム内メニュー
    GameComplete,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlayerInfo {
    pub id: Uuid,
    pub name: String,
    pub score: u32,
}

#[derive(Clone, Copy, PartialEq)]
pub enum PieceMode {
    TargetCount,  // 目標ピース数から計算
    ManualGrid,   // 手動でグリッドサイズを指定
    SquarePieces, // 正方形ピースサイズから計算
}

impl Default for PieceMode {
    fn default() -> Self {
        PieceMode::TargetCount
    }
}

#[derive(Resource)]
pub struct PuzzleConfig {
    pub grid_size: (usize, usize),
    pub piece_size: f32,
    pub snap_distance: f32,
    pub image_path: String,
    pub target_piece_count: usize,
    pub use_target_mode: bool, // 下位互換性のため残す
    pub piece_mode: PieceMode, // 新しいモード選択
    pub target_piece_size: f32, // 正方形ピースの目標サイズ（ピクセル）
}

impl Default for PuzzleConfig {
    fn default() -> Self {
        Self {
            grid_size: (4, 4),
            piece_size: 100.0,
            snap_distance: 50.0, // Reduced to prevent immediate snapping
            image_path: String::new(), // 空の文字列から開始
            target_piece_count: 16, // デフォルト16ピース
            use_target_mode: true, // デフォルトはターゲットピース数モード（下位互換性）
            piece_mode: PieceMode::TargetCount, // 新しいデフォルトモード
            target_piece_size: 4.0, // 4x4グリッド相当（16ピース）
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