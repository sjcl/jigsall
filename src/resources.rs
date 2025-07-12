use bevy::prelude::*;
use bevy::sprite::ColorMaterial;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crossbeam::channel;
use std::collections::{HashMap, HashSet};
use crate::jigsaw_shapes::JigsawShapeGenerator;
use crate::components::{PuzzlePiece, PieceShape};
use instant::Instant;

/// ピースの一意識別子（将来的にバッチング対応）
pub type PieceId = Uuid;

/// ピースの当たり判定データ（CPU側で管理）
#[derive(Debug, Clone)]
pub struct PieceCollisionData {
    pub piece_id: PieceId,
    pub position: Vec2,
    pub bounding_box: Rect,
    pub vertices: Vec<Vec2>,    // 精密判定用のポリゴン頂点（ローカル座標）
    pub indices: Vec<u32>,     // トライアングル頂点インデックス
}

/// QuadTree ノード（空間分割用）
#[derive(Debug, Clone)]
pub struct QuadTreeNode {
    pub bounds: Rect,
    pub pieces: Vec<PieceId>,
    pub children: Option<Box<[QuadTreeNode; 4]>>,
    pub max_pieces: usize,
    pub max_depth: usize,
    pub current_depth: usize,
}

impl QuadTreeNode {
    pub fn new(bounds: Rect, max_pieces: usize, max_depth: usize, current_depth: usize) -> Self {
        Self {
            bounds,
            pieces: Vec::new(),
            children: None,
            max_pieces,
            max_depth,
            current_depth,
        }
    }

    pub fn insert(&mut self, piece_id: PieceId, piece_bounds: Rect) {
        if self.bounds.intersect(piece_bounds).is_empty() {
            return;
        }

        if self.children.is_none() {
            self.pieces.push(piece_id);

            if self.pieces.len() > self.max_pieces && self.current_depth < self.max_depth {
                self.subdivide();
            }
        } else if let Some(ref mut children) = self.children {
            for child in children.iter_mut() {
                child.insert(piece_id, piece_bounds);
            }
        }
    }

    fn subdivide(&mut self) {
        let half_width = self.bounds.width() / 2.0;
        let half_height = self.bounds.height() / 2.0;
        let center_x = self.bounds.min.x + half_width;
        let center_y = self.bounds.min.y + half_height;

        let children = [
            QuadTreeNode::new(
                Rect::new(self.bounds.min.x, self.bounds.min.y, center_x, center_y),
                self.max_pieces,
                self.max_depth,
                self.current_depth + 1,
            ),
            QuadTreeNode::new(
                Rect::new(center_x, self.bounds.min.y, self.bounds.max.x, center_y),
                self.max_pieces,
                self.max_depth,
                self.current_depth + 1,
            ),
            QuadTreeNode::new(
                Rect::new(self.bounds.min.x, center_y, center_x, self.bounds.max.y),
                self.max_pieces,
                self.max_depth,
                self.current_depth + 1,
            ),
            QuadTreeNode::new(
                Rect::new(center_x, center_y, self.bounds.max.x, self.bounds.max.y),
                self.max_pieces,
                self.max_depth,
                self.current_depth + 1,
            ),
        ];

        self.children = Some(Box::new(children));
        
        // 注意: subdivide では piece_bounds が取得できないため、
        // 実際のピース再配置は PieceCollisionSystem::rebuild_quad_tree で行う
        // ここでは子ノードの準備のみ
        self.pieces.clear();
    }

    pub fn query(&self, query_bounds: Rect, results: &mut Vec<PieceId>) {
        if self.bounds.intersect(query_bounds).is_empty() {
            return;
        }

        for &piece_id in &self.pieces {
            results.push(piece_id);
        }

        if let Some(ref children) = self.children {
            for child in children.iter() {
                child.query(query_bounds, results);
            }
        }
    }
}

/// IDベースの当たり判定システム
#[derive(Resource, Default)]
pub struct PieceCollisionSystem {
    pub pieces: HashMap<PieceId, PieceCollisionData>,
    pub quad_tree: Option<QuadTreeNode>,
    pub need_rebuild: bool,
    pub world_bounds: Rect,
}

impl PieceCollisionSystem {
    pub fn new() -> Self {
        Self {
            pieces: HashMap::new(),
            quad_tree: None,
            need_rebuild: true,
            world_bounds: Rect::new(-2000.0, -2000.0, 2000.0, 2000.0), // 初期の大きな範囲
        }
    }

    pub fn add_piece(&mut self, collision_data: PieceCollisionData) {
        self.pieces.insert(collision_data.piece_id, collision_data);
        self.need_rebuild = true;
    }

    pub fn remove_piece(&mut self, piece_id: PieceId) {
        self.pieces.remove(&piece_id);
        self.need_rebuild = true;
    }

    pub fn update_piece_position(&mut self, piece_id: PieceId, new_position: Vec2) {
        if let Some(piece_data) = self.pieces.get_mut(&piece_id) {
            // 位置の変化量を計算
            let offset = new_position - piece_data.position;
            piece_data.position = new_position;
            
            // バウンディングボックスを更新
            piece_data.bounding_box = Rect::new(
                piece_data.bounding_box.min.x + offset.x,
                piece_data.bounding_box.min.y + offset.y,
                piece_data.bounding_box.max.x + offset.x,
                piece_data.bounding_box.max.y + offset.y,
            );
            self.need_rebuild = true;
        }
    }

    pub fn rebuild_quad_tree(&mut self) {
        if !self.need_rebuild {
            return;
        }

        self.quad_tree = Some(QuadTreeNode::new(self.world_bounds, 8, 5, 0));

        if let Some(ref mut quad_tree) = self.quad_tree {
            for (piece_id, piece_data) in &self.pieces {
                quad_tree.insert(*piece_id, piece_data.bounding_box);
            }
        }

        self.need_rebuild = false;
    }

    pub fn query_pieces_in_rect(&mut self, query_rect: Rect) -> Vec<PieceId> {
        self.rebuild_quad_tree();

        let mut results = Vec::new();
        if let Some(ref quad_tree) = self.quad_tree {
            quad_tree.query(query_rect, &mut results);
        }
        results
    }

    pub fn ray_cast(&mut self, ray_origin: Vec2, ray_direction: Vec2) -> Option<PieceId> {
        // 簡単なレイキャスト実装（バウンディングボックスのみ）
        let ray_end = ray_origin + ray_direction * 10000.0; // 十分大きな距離
        let ray_rect = Rect::from_corners(ray_origin, ray_end);

        let candidate_pieces = self.query_pieces_in_rect(ray_rect);

        for piece_id in candidate_pieces {
            if let Some(piece_data) = self.pieces.get(&piece_id) {
                if piece_data.bounding_box.contains(ray_origin) {
                    return Some(piece_id);
                }
            }
        }

        None
    }

    pub fn get_piece_data(&self, piece_id: PieceId) -> Option<&PieceCollisionData> {
        self.pieces.get(&piece_id)
    }

    /// マウス位置でのピース検索（ドラッグ用API）
    pub fn find_piece_at_position(&mut self, position: Vec2) -> Option<PieceId> {
        // 小さな範囲でクエリ
        let query_size = 1.0;
        let query_rect = Rect::new(
            position.x - query_size,
            position.y - query_size,
            position.x + query_size,
            position.y + query_size,
        );

        let candidate_pieces = self.query_pieces_in_rect(query_rect);

        // バウンディングボックスでフィルタリング
        for piece_id in candidate_pieces {
            if let Some(piece_data) = self.pieces.get(&piece_id) {
                if piece_data.bounding_box.contains(position) {
                    return Some(piece_id);
                }
            }
        }

        None
    }

    /// 矩形範囲内のピース検索（範囲選択用API）
    pub fn find_pieces_in_rect(&mut self, rect: Rect) -> Vec<PieceId> {
        let candidate_pieces = self.query_pieces_in_rect(rect);
        let mut result = Vec::new();

        for piece_id in candidate_pieces {
            if let Some(piece_data) = self.pieces.get(&piece_id) {
                // バウンディングボックスが範囲と重なるかチェック
                if !piece_data.bounding_box.intersect(rect).is_empty() {
                    result.push(piece_id);
                }
            }
        }

        result
    }

    /// 精密なポリゴン当たり判定API
    pub fn precise_point_in_piece(&self, piece_id: PieceId, world_position: Vec2) -> bool {
        if let Some(piece_data) = self.pieces.get(&piece_id) {
            // まずバウンディングボックスでチェック
            if !piece_data.bounding_box.contains(world_position) {
                return false;
            }

            // ローカル座標に変換
            let local_position = world_position - piece_data.position;

            // 簡単なポリゴン内判定（レイキャスト法）
            self.point_in_polygon(local_position, &piece_data.vertices)
        } else {
            false
        }
    }

    /// ポリゴン内判定（レイキャスト法）
    fn point_in_polygon(&self, point: Vec2, vertices: &[Vec2]) -> bool {
        if vertices.len() < 3 {
            return false;
        }

        let mut intersections = 0;
        let ray_y = point.y;

        for i in 0..vertices.len() {
            let j = (i + 1) % vertices.len();
            let v1 = vertices[i];
            let v2 = vertices[j];

            // 水平レイが線分と交差するかチェック
            if ((v1.y > ray_y) != (v2.y > ray_y)) &&
               (point.x < (v2.x - v1.x) * (ray_y - v1.y) / (v2.y - v1.y) + v1.x) {
                intersections += 1;
            }
        }

        intersections % 2 == 1
    }

    /// デバッグ用：統計情報取得
    pub fn get_debug_stats(&self) -> (usize, bool, bool) {
        (
            self.pieces.len(),
            self.quad_tree.is_some(),
            self.need_rebuild
        )
    }

    /// パフォーマンス統計取得
    pub fn get_performance_stats(&self) -> String {
        let quad_tree_nodes = if let Some(ref qt) = self.quad_tree {
            self.count_quad_tree_nodes(qt)
        } else {
            0
        };

        format!(
            "Collision System Stats:\n  Pieces: {}\n  QuadTree nodes: {}\n  Needs rebuild: {}",
            self.pieces.len(),
            quad_tree_nodes,
            self.need_rebuild
        )
    }

    fn count_quad_tree_nodes(&self, node: &QuadTreeNode) -> usize {
        let mut count = 1;
        if let Some(ref children) = node.children {
            for child in children.iter() {
                count += self.count_quad_tree_nodes(child);
            }
        }
        count
    }
}

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
    pub need_refresh: bool,  // キャッシュ更新が必要か
}

/// ID管理とEntity関連付けシステム（将来的なバッチング対応）
#[derive(Resource, Default)]
pub struct PieceIdManager {
    /// ID → Entity の関連付け
    id_to_entity: HashMap<PieceId, Entity>,
    /// Entity → ID の関連付け（逆引き用）
    entity_to_id: HashMap<Entity, PieceId>,
    /// 次に使用可能なID（Uuidの代替案として、デバッグ用）
    next_id_counter: u32,
}

impl PieceIdManager {
    /// 新しいピースIDを生成してEntityと関連付け
    pub fn register_piece(&mut self, entity: Entity, existing_id: Option<PieceId>) -> PieceId {
        let piece_id = existing_id.unwrap_or_else(|| Uuid::new_v4());
        
        // 既存の関連付けを削除
        if let Some(old_id) = self.entity_to_id.remove(&entity) {
            self.id_to_entity.remove(&old_id);
        }
        
        // 新しい関連付けを登録
        self.id_to_entity.insert(piece_id, entity);
        self.entity_to_id.insert(entity, piece_id);
        
        piece_id
    }
    
    /// EntityからピースIDを取得
    pub fn get_piece_id(&self, entity: Entity) -> Option<PieceId> {
        self.entity_to_id.get(&entity).copied()
    }
    
    /// ピースIDからEntityを取得
    pub fn get_entity(&self, piece_id: PieceId) -> Option<Entity> {
        self.id_to_entity.get(&piece_id).copied()
    }
    
    /// Entity削除時のクリーンアップ
    pub fn unregister_entity(&mut self, entity: Entity) -> Option<PieceId> {
        if let Some(piece_id) = self.entity_to_id.remove(&entity) {
            self.id_to_entity.remove(&piece_id);
            Some(piece_id)
        } else {
            None
        }
    }
    
    /// 将来的なバッチング対応：複数ピースを1つのEntityに関連付け
    pub fn register_batch(&mut self, entity: Entity, piece_ids: Vec<PieceId>) {
        for piece_id in piece_ids {
            self.id_to_entity.insert(piece_id, entity);
            // 注意：entity_to_id は1対1のため、バッチング時は別のマップが必要
        }
    }
    
    /// 統計情報取得（デバッグ用）
    pub fn get_stats(&self) -> (usize, usize, u32) {
        (
            self.id_to_entity.len(),
            self.entity_to_id.len(),
            self.next_id_counter
        )
    }
    
    /// 全てのピースIDを取得
    pub fn get_all_piece_ids(&self) -> Vec<PieceId> {
        self.id_to_entity.keys().copied().collect()
    }
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
    // Complete is removed as it's unused
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

    // reset_statistics method removed - never used
}

// ImageLoadingStatus removed - unused with new thread-based implementation

// ImageLoadingTask removed - unused with new thread-based implementation

// ImageLoadingTasks removed - unused with new thread-based implementation


/// 画像読み込みチャネル（crossbeam-channel）
#[derive(Resource)]
pub struct ImageLoadChannels {
    /// 画像読み込み結果を受信するチャネル
    pub rx_results: crossbeam::channel::Receiver<crate::asset_reader::ImageLoadResult>,
}

/// 画像読み込み送信チャネル（スレッド間通信用）
#[derive(Resource)]
pub struct ImageLoadSender {
    pub tx_results: crossbeam::channel::Sender<crate::asset_reader::ImageLoadResult>,
}

// バックグラウンドスレッド版の完了