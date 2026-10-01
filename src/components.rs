use crate::gameplay::PieceId;
use bevy::prelude::*;

pub use crate::gameplay::PuzzlePiece;

#[derive(Component)]
pub struct PieceShape {
    pub vertices: Vec<[f32; 2]>, // メッシュの頂点（2D）
    pub indices: Vec<u32>,       // 三角形インデックス
    pub shape_hash: String,      // 形状のハッシュ（ストロークメッシュキャッシュのキー）
}

#[derive(Component)]
pub struct MainCamera;

#[derive(Component)]
pub struct GridReference;

/// 選択されたピースをマークするコンポーネント
#[derive(Component)]
pub struct SelectedPiece;

/// 選択範囲の可視化用コンポーネント
#[derive(Component)]
pub struct SelectionBox;

/// 選択されたピースの枠線表示用コンポーネント
#[derive(Component)]
pub struct PieceOutline;

/// 選択範囲プレビュー中のピースをマークするコンポーネント
#[derive(Component)]
pub struct SelectionPreview;

/// ピース移動完了イベント
#[derive(Message)]
pub struct PieceMoveCompleted {
    pub id: PieceId,
}

/// ピース配置イベント
#[derive(Message)]
pub struct PiecePlacedEvent {
    pub id: PieceId,
}

// ==========================================
// Mesh Batching System Components
// ==========================================

/// 結合メッシュエンティティをマークするコンポーネント
#[derive(Component)]
pub struct BatchedMeshEntity {
    pub piece_count: usize,
    pub last_updated: std::time::Instant,
}

/// バッチ再構築要求イベント
#[derive(Message)]
pub struct BatchRebuildRequest {
    pub reason: BatchRebuildReason,
    pub affected_pieces: Vec<PieceId>,
}

/// バッチ再構築の理由
#[derive(Debug, Clone)]
pub enum BatchRebuildReason {
    /// 手動でバッチ再構築が要求された
    ManualRebuild,
}

/// アクティブなピース（選択中・ドラッグ中）用の一時エンティティマーカー
#[derive(Component)]
pub struct TemporaryPieceEntity {
    pub piece_id: PieceId,
}
