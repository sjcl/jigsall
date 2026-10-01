use bevy::prelude::*;
use crossbeam::channel;
use puzzella_puzzle::{PieceCreationResult, PieceData, ShapeGenerationResult};
use std::collections::VecDeque;

#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum GenerationPhase {
    #[default]
    NotStarted,
    PreparingShapes,  // ジグソー形状を生成中（非同期）
    CreatingPieces,   // ピースデータを作成中（非同期）
    SpawningEntities, // メインスレッドでデータストアに保存中（エンティティは作成しない）
    Completed,
    Failed,
}

#[derive(Resource)]
pub struct PieceGenerationProgress {
    // バックグラウンドスレッド版 - フィールドテスト
    pub is_generating: bool,
    pub total_pieces: usize,
    pub generation_phase: GenerationPhase,
    pub grid_size: (usize, usize),
    pub shapes_generated: usize,
    pub pieces_created: usize,
    pub pending_pieces: VecDeque<PieceData>, // 非同期で作成されたピースデータの待機列
    pub pieces_spawned_this_frame: usize,    // 今フレームでスポーンしたピース数

    // 新しい標準スレッド用フィールド（crossbeam channelを使用）
    pub bg_thread_receiver: Option<channel::Receiver<Result<ShapeGenerationResult, String>>>,
    pub piece_thread_receiver: Option<channel::Receiver<Result<PieceCreationResult, String>>>,
    pub error: Option<String>,
}

impl Default for PieceGenerationProgress {
    fn default() -> Self {
        Self {
            is_generating: false,
            total_pieces: 0,
            generation_phase: GenerationPhase::NotStarted,
            grid_size: (0, 0),
            shapes_generated: 0,
            pieces_created: 0,
            pending_pieces: VecDeque::new(),
            pieces_spawned_this_frame: 0,
            bg_thread_receiver: None,
            piece_thread_receiver: None,
            error: None,
        }
    }
}
