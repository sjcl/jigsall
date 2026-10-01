use bevy::prelude::*;

#[derive(Clone, Copy, PartialEq, Default)]
pub enum PieceMode {
    #[default]
    TargetCount, // 目標ピース数から計算
    ManualGrid,   // 手動でグリッドサイズを指定
    SquarePieces, // 正方形ピースサイズから計算
}

#[derive(Resource)]
pub struct PuzzleConfig {
    pub grid_size: (usize, usize),
    pub snap_distance: f32,
    pub image_path: String,
    pub target_piece_count: usize,
    pub seed: u64,
    pub piece_mode: PieceMode,  // 新しいモード選択
    pub target_piece_size: f32, // 正方形ピースの目標サイズ（ピクセル）
}

impl Default for PuzzleConfig {
    fn default() -> Self {
        Self {
            grid_size: (4, 4),
            snap_distance: 50.0,       // Reduced to prevent immediate snapping
            image_path: String::new(), // 空の文字列から開始
            target_piece_count: 16,    // デフォルト16ピース
            seed: 42,
            piece_mode: PieceMode::SquarePieces, // アスペクト比モードをデフォルトに
            target_piece_size: 4.0,              // 4x4グリッド相当（16ピース）
        }
    }
}
