use bevy::prelude::*;
use bevy::tasks::Task;
use bevy::sprite::ColorMaterial;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crossbeam::channel;
use std::sync::Arc;
use std::collections::HashMap;
use crate::jigsaw_shapes::JigsawShapeGenerator;
use crate::components::{PuzzlePiece, PieceShape};

/// ストロークメッシュのキャッシュリソース
#[derive(Resource, Default)]
pub struct StrokeMeshCache {
    pub stroke_meshes: HashMap<String, Handle<Mesh>>, // shape_hash -> stroke mesh handle
}

#[derive(Resource, Default)]
pub struct GameState {
    pub current_screen: GameScreen,
    pub is_host: bool,
    pub players: Vec<PlayerInfo>,
    pub puzzle_completed: bool,
    pub puzzle_progress: f32,
    pub needs_reset: bool, // パズルをリセットする必要があるかのフラグ
}

#[derive(Default, PartialEq, Clone, Debug)]
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
            use_target_mode: false, // アスペクト比モードがデフォルト
            piece_mode: PieceMode::SquarePieces, // アスペクト比モードをデフォルトに
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

#[derive(Default, Clone, Debug)]
pub enum SelectionMode {
    #[default]
    Single,        // 単一ピース選択モード
    BoxSelection,  // 範囲選択モード  
    MultiDrag,     // 複数ピース同時移動モード
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
    
    // 新しいマルチ選択関連フィールド
    pub selection_mode: SelectionMode,
    pub selection_start: Option<Vec2>,
    pub selection_current: Option<Vec2>,
    pub selected_pieces: Vec<Entity>,
    pub multi_drag_offset: HashMap<Entity, Vec2>,
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
            
            // 新しいマルチ選択関連フィールドの初期化
            selection_mode: SelectionMode::Single,
            selection_start: None,
            selection_current: None,
            selected_pieces: Vec::new(),
            multi_drag_offset: HashMap::new(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum GenerationPhase {
    NotStarted,
    PreparingShapes,    // ジグソー形状を生成中（非同期）
    CreatingPieces,     // ピースエンティティを作成中（非同期）
    SpawningEntities,   // メインスレッドでエンティティをスポーン中
    Completed,
}

// 非同期タスクの結果を格納する構造体
pub struct ShapeGenerationResult {
    pub shape_generator: JigsawShapeGenerator,
    pub placement_positions: Vec<Vec2>,
    pub grid_size: (usize, usize),
    pub total_pieces: usize,
}

// メッシュとピースデータを含む構造体
pub struct PieceData {
    pub mesh: Mesh,
    pub stroke_mesh: Option<Mesh>, // ストロークメッシュ
    pub piece_component: PuzzlePiece,
    pub piece_shape: PieceShape,
    pub transform: Transform,
    pub material_handle: Handle<ColorMaterial>,
}

// ピース作成の非同期タスク結果
pub struct PieceCreationResult {
    pub pieces: Vec<PieceData>,
}

// 進捗更新メッセージ
#[derive(Clone)]
pub enum ProgressMessage {
    ShapeProgress(usize), // 生成済み形状数
    PieceProgress(usize), // 作成済みピース数
    ShapeCompleted,       // 形状生成完了
    PieceCompleted,       // ピース作成完了
}

impl Default for GenerationPhase {
    fn default() -> Self {
        GenerationPhase::NotStarted
    }
}

#[derive(Resource)]
pub struct PieceGenerationProgress {
    // バックグラウンドスレッド版 - フィールドテスト
    pub is_generating: bool,
    pub current_piece: usize,
    pub total_pieces: usize,
    pub generation_phase: GenerationPhase,
    pub grid_size: (usize, usize),
    pub shapes_generated: usize,
    pub pieces_created: usize,
    pub shape_generator: Option<JigsawShapeGenerator>,
    pub placement_positions: Vec<Vec2>,
    pub async_task: Option<Task<ShapeGenerationResult>>,
    pub piece_creation_task: Option<Task<PieceCreationResult>>,
    pub pending_pieces: Vec<PieceData>, // 非同期で作成されたピースデータの待機列
    pub pieces_spawned_this_frame: usize, // 今フレームでスポーンしたピース数
    
    // 新しい標準スレッド用フィールド（crossbeam channelを使用）
    pub bg_thread_receiver: Option<channel::Receiver<ShapeGenerationResult>>,
    pub piece_thread_receiver: Option<channel::Receiver<PieceCreationResult>>,
    
    // 進捗更新用チャンネル
    pub progress_receiver: Option<channel::Receiver<ProgressMessage>>,
}

impl Default for PieceGenerationProgress {
    fn default() -> Self {
        Self {
            is_generating: false,
            current_piece: 0,
            total_pieces: 0,
            generation_phase: GenerationPhase::NotStarted,
            grid_size: (0, 0),
            shapes_generated: 0,
            pieces_created: 0,
            shape_generator: None,
            placement_positions: Vec::new(),
            async_task: None,
            piece_creation_task: None,
            pending_pieces: Vec::new(),
            pieces_spawned_this_frame: 0,
            bg_thread_receiver: None,
            piece_thread_receiver: None,
            progress_receiver: None,
        }
    }
}

// バックグラウンドスレッド版の完了