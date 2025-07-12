use bevy::prelude::*;
use bevy::sprite::ColorMaterial;
use bevy::tasks::Task;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crossbeam::channel;
use std::collections::{HashMap, HashSet};
use crate::jigsaw_shapes::JigsawShapeGenerator;
use crate::components::{PuzzlePiece, PieceShape};
use instant::Instant;

/// ストロークメッシュのキャッシュリソース
#[derive(Resource, Default)]
pub struct StrokeMeshCache {
    pub stroke_meshes: HashMap<String, Handle<Mesh>>, // shape_hash -> stroke mesh handle
}

/// ハイライト表示用の共有マテリアルリソース
#[derive(Resource)]
pub struct HighlightMaterials {
    pub preview_material: Handle<ColorMaterial>,  // プレビュー用（薄い青色）
    pub selected_material: Handle<ColorMaterial>, // 選択用（黄色）
}

/// ハイライト状態変更検出リソース
#[derive(Resource, Default)]
pub struct HighlightState {
    pub last_selected_pieces: HashSet<Entity>,    // 前フレームの選択ピース
    pub last_preview_pieces: HashSet<Entity>,     // 前フレームのプレビューピース
    pub selection_changed: bool,                  // 選択状態が変わったか
    pub preview_changed: bool,                    // プレビュー状態が変わったか
    pub frame_count: u64,                         // フレーム数（デバッグ用）
}

/// パフォーマンス最適化用のピース検索キャッシュ
#[derive(Resource, Default)]
pub struct PieceSelectionCache {
    pub all_pieces: Vec<Entity>,  // 全ピースのキャッシュリスト
    pub piece_positions: HashMap<Entity, Vec2>,  // ピース位置のキャッシュ
    pub piece_bounds: HashMap<Entity, (Vec2, Vec2)>,  // ピース境界ボックスのキャッシュ
    pub last_update_frame: u64,  // 最終更新フレーム
    pub need_refresh: bool,  // キャッシュ更新が必要か
}

#[derive(Resource, Default)]
pub struct GameData {
    pub current_screen: GameScreen,  // 一時的に残す
    pub is_host: bool,
    pub players: Vec<PlayerInfo>,
    pub puzzle_completed: bool,
    pub puzzle_progress: f32,
    pub needs_reset: bool, // パズルをリセットする必要があるかのフラグ
}

/// メインアプリケーションの状態
#[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AppState {
    #[default]
    Loading,      // 起動時の初期化
    Menu,         // メインメニュー
    GameSetup,    // ゲーム設定・画像読み込み
    InGame,       // ゲーム中
    GameComplete, // ゲーム完了
}

/// ゲーム内のサブ状態（InGame時のみ有効）
#[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum GameSubState {
    #[default]
    Initializing, // パズル生成中
    Playing,      // プレイ中
    Paused,       // ポーズ中（ESCメニュー）
    Complete,     // 完了（結果表示）
}

/// 従来のGameScreen（後で削除予定）
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
    pub last_mouse_position: Vec2,
    pub last_cursor_position: Option<Vec2>,
    
    // 新しいマルチ選択関連フィールド
    pub selection_mode: SelectionMode,
    pub selection_start: Option<Vec2>,
    pub selection_current: Option<Vec2>,
    pub selected_pieces: Vec<Entity>,
    pub multi_drag_offset: HashMap<Entity, Vec2>,
    
    // パフォーマンス最適化用のキャッシュ
    pub selected_pieces_set: HashSet<Entity>,  // 高速な選択状態チェック用
    pub last_selection_rect: Option<(Vec2, Vec2)>,  // 前回の選択範囲
    pub cached_drag_entity: Option<Entity>,  // ドラッグ中のエンティティキャッシュ
    
    // エッジスクロール用
    pub cursor_screen_position: Option<Vec2>,  // スクリーン座標でのカーソル位置
    pub is_dragging_piece: bool,  // ピースをドラッグ中かどうか
}

impl Default for InputState {
    fn default() -> Self {
        Self {
            mouse_position: Vec2::ZERO,
            is_mouse_pressed: false,
            selected_piece: None,
            next_z_order: 1.0, // 1.0から開始
            is_camera_dragging: false,
            last_mouse_position: Vec2::ZERO,
            last_cursor_position: None,
            
            // 新しいマルチ選択関連フィールドの初期化
            selection_mode: SelectionMode::Single,
            selection_start: None,
            selection_current: None,
            selected_pieces: Vec::new(),
            multi_drag_offset: HashMap::new(),
            
            // パフォーマンス最適化用のキャッシュの初期化
            selected_pieces_set: HashSet::new(),
            last_selection_rect: None,
            cached_drag_entity: None,
            
            // エッジスクロール用の初期化
            cursor_screen_position: None,
            is_dragging_piece: false,
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
    pub placement_positions: Vec<Vec2>,
    pub pending_pieces: Vec<PieceData>, // 非同期で作成されたピースデータの待機列
    pub pieces_spawned_this_frame: usize, // 今フレームでスポーンしたピース数
    
    // 新しい標準スレッド用フィールド（crossbeam channelを使用）
    pub bg_thread_receiver: Option<channel::Receiver<ShapeGenerationResult>>,
    pub piece_thread_receiver: Option<channel::Receiver<PieceCreationResult>>,
    pub progress_receiver: Option<channel::Receiver<()>>,
    
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
            placement_positions: Vec::new(),
            pending_pieces: Vec::new(),
            pieces_spawned_this_frame: 0,
            bg_thread_receiver: None,
            piece_thread_receiver: None,
            progress_receiver: None,
        }
    }
}

/// パフォーマンス計測のデバッグレベル
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PerformanceDebugLevel {
    Off,
    Low,     // 基本的な統計のみ
    Medium,  // 個別システムの時間
    High,    // 詳細な内部計測
}

impl Default for PerformanceDebugLevel {
    fn default() -> Self {
        PerformanceDebugLevel::Off
    }
}

/// 個別システムの計測データ
#[derive(Debug, Clone)]
pub struct SystemTiming {
    pub name: String,
    pub last_duration: std::time::Duration,
    pub total_duration: std::time::Duration,
    pub call_count: u64,
    pub min_duration: std::time::Duration,
    pub max_duration: std::time::Duration,
}

impl SystemTiming {
    pub fn new(name: String) -> Self {
        Self {
            name,
            last_duration: std::time::Duration::ZERO,
            total_duration: std::time::Duration::ZERO,
            call_count: 0,
            min_duration: std::time::Duration::MAX,
            max_duration: std::time::Duration::ZERO,
        }
    }

    pub fn record_timing(&mut self, duration: std::time::Duration) {
        self.last_duration = duration;
        self.total_duration += duration;
        self.call_count += 1;
        self.min_duration = self.min_duration.min(duration);
        self.max_duration = self.max_duration.max(duration);
    }

    pub fn average_duration(&self) -> std::time::Duration {
        if self.call_count > 0 {
            self.total_duration / self.call_count as u32
        } else {
            std::time::Duration::ZERO
        }
    }

    pub fn reset(&mut self) {
        self.total_duration = std::time::Duration::ZERO;
        self.call_count = 0;
        self.min_duration = std::time::Duration::MAX;
        self.max_duration = std::time::Duration::ZERO;
    }
}

/// パフォーマンス監視リソース
#[derive(Resource)]
pub struct PerformanceMonitor {
    pub debug_level: PerformanceDebugLevel,
    pub enabled: bool,
    pub frame_start: Option<Instant>,
    pub frame_times: Vec<std::time::Duration>,
    pub system_timings: HashMap<String, SystemTiming>,
    pub last_report_time: Instant,
    pub report_interval: std::time::Duration,
    pub frame_count: u64,
    pub max_stored_frames: usize,
}

impl Default for PerformanceMonitor {
    fn default() -> Self {
        Self {
            debug_level: PerformanceDebugLevel::Off,
            enabled: false,
            frame_start: None,
            frame_times: Vec::new(),
            system_timings: HashMap::new(),
            last_report_time: Instant::now(),
            report_interval: std::time::Duration::from_secs(5), // 5秒間隔でレポート
            frame_count: 0,
            max_stored_frames: 300, // 5秒分のフレーム（60FPS想定）
        }
    }
}

impl PerformanceMonitor {
    pub fn new(debug_level: PerformanceDebugLevel) -> Self {
        Self {
            debug_level,
            enabled: debug_level != PerformanceDebugLevel::Off,
            ..Default::default()
        }
    }

    pub fn start_frame(&mut self) {
        if self.enabled {
            self.frame_start = Some(Instant::now());
        }
    }

    pub fn end_frame(&mut self) {
        if self.enabled {
            if let Some(start) = self.frame_start {
                let frame_duration = start.elapsed();
                self.frame_times.push(frame_duration);
                
                // 古いフレームデータを削除
                if self.frame_times.len() > self.max_stored_frames {
                    self.frame_times.remove(0);
                }
                
                self.frame_count += 1;
                self.frame_start = None;
            }
        }
    }

    pub fn start_system_timing(&mut self, _system_name: &str) -> Option<Instant> {
        if self.enabled {
            Some(Instant::now())
        } else {
            None
        }
    }

    pub fn end_system_timing(&mut self, system_name: &str, start_time: Option<Instant>) {
        if self.enabled {
            if let Some(start) = start_time {
                let duration = start.elapsed();
                let timing = self.system_timings
                    .entry(system_name.to_string())
                    .or_insert_with(|| SystemTiming::new(system_name.to_string()));
                timing.record_timing(duration);
            }
        }
    }

    pub fn should_report(&self) -> bool {
        self.enabled && self.last_report_time.elapsed() >= self.report_interval
    }

    pub fn reset_report_timer(&mut self) {
        self.last_report_time = Instant::now();
    }

    pub fn get_fps(&self) -> f32 {
        if self.frame_times.is_empty() {
            return 0.0;
        }
        
        let total_time: std::time::Duration = self.frame_times.iter().sum();
        let avg_frame_time_nanos = total_time.as_nanos() / self.frame_times.len() as u128;
        
        if avg_frame_time_nanos > 0 {
            1_000_000_000.0 / avg_frame_time_nanos as f32
        } else {
            0.0
        }
    }

    pub fn get_frame_time_ms(&self) -> f32 {
        if self.frame_times.is_empty() {
            return 0.0;
        }
        
        let total_time: std::time::Duration = self.frame_times.iter().sum();
        let avg_frame_time_nanos = total_time.as_nanos() / self.frame_times.len() as u128;
        avg_frame_time_nanos as f32 / 1_000_000.0
    }

    pub fn toggle_debug_level(&mut self) {
        self.debug_level = match self.debug_level {
            PerformanceDebugLevel::Off => PerformanceDebugLevel::Low,
            PerformanceDebugLevel::Low => PerformanceDebugLevel::Medium,
            PerformanceDebugLevel::Medium => PerformanceDebugLevel::High,
            PerformanceDebugLevel::High => PerformanceDebugLevel::Off,
        };
        self.enabled = self.debug_level != PerformanceDebugLevel::Off;
        
        if self.enabled {
            println!("🔍 Performance monitoring enabled: {:?}", self.debug_level);
        } else {
            println!("🔍 Performance monitoring disabled");
        }
    }

    pub fn reset_statistics(&mut self) {
        self.frame_times.clear();
        for timing in self.system_timings.values_mut() {
            timing.reset();
        }
        self.frame_count = 0;
    }
}

/// 画像読み込みタスクの状態
#[derive(Debug, Clone, PartialEq)]
pub enum ImageLoadingStatus {
    /// 読み込み中
    Loading { 
        /// 読み込み開始時刻
        started_at: Instant,
        /// ファイルサイズ（バイト）
        file_size: Option<u64>,
    },
    /// 読み込み完了
    Completed,
    /// エラーで失敗
    Failed(String),
}

/// 非同期画像読み込みタスク
pub struct ImageLoadingTask {
    /// 仮想キー（external_file_1.jpg など）
    pub virtual_key: String,
    /// 実際のファイルパス
    pub file_path: String,
    /// 非同期タスク
    pub task: Task<Result<Image, String>>,
    /// 現在の状態
    pub status: ImageLoadingStatus,
}

/// 画像読み込みタスクのコレクション
#[derive(Resource, Default)]
pub struct ImageLoadingTasks {
    /// アクティブなタスクのリスト
    pub tasks: Vec<ImageLoadingTask>,
}


/// 画像読み込みチャネル（crossbeam-channel）
#[derive(Resource)]
pub struct ImageLoadChannels {
    /// 画像読み込み結果を受信するチャネル
    pub rx_results: crossbeam::channel::Receiver<crate::asset_reader::ImageLoadResult>,
    /// 画像読み込み結果を送信するチャネル（スレッド用）
    pub tx_results: crossbeam::channel::Sender<crate::asset_reader::ImageLoadResult>,
}

// バックグラウンドスレッド版の完了