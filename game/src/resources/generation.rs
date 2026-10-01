use bevy::prelude::*;
use crossbeam::channel;
use puzzella_puzzle::{PieceCreationResult, PieceData};
use std::collections::VecDeque;

#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum GenerationPhase {
    #[default]
    NotStarted,
    GeneratingPieces, // One worker: placement + parallel geometry.
    SpawningEntities, // メインスレッドでデータストアに保存中（エンティティは作成しない）
    Completed,
    Failed,
}

#[derive(Resource)]
pub struct PieceGenerationProgress {
    pub is_generating: bool,
    pub total_pieces: usize,
    pub generation_phase: GenerationPhase,
    pub grid_size: (usize, usize),
    pub pieces_created: usize,
    pub pending_pieces: VecDeque<PieceData>, // 非同期で作成されたピースデータの待機列
    pub pieces_spawned_this_frame: usize,    // 今フレームでスポーンしたピース数

    pub receiver: Option<channel::Receiver<Result<PieceCreationResult, String>>>,
    pub error: Option<String>,
}

impl Default for PieceGenerationProgress {
    fn default() -> Self {
        Self {
            is_generating: false,
            total_pieces: 0,
            generation_phase: GenerationPhase::NotStarted,
            grid_size: (0, 0),
            pieces_created: 0,
            pending_pieces: VecDeque::new(),
            pieces_spawned_this_frame: 0,
            receiver: None,
            error: None,
        }
    }
}
