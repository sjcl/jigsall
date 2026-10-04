use bevy::prelude::*;

#[derive(Clone, Copy, PartialEq, Default)]
pub enum PieceMode {
    #[default]
    SquarePieces, // 正方形に近いピースを優先
    TargetCount, // 指定ピース数を優先
    ManualGrid,  // 手動でグリッドサイズを指定
}

#[derive(Resource)]
pub struct PuzzleConfig {
    pub grid_size: (usize, usize),
    pub rotation_enabled: bool,
    pub image_path: String,
    pub target_piece_count: usize,
    pub seed: u64,
    pub piece_mode: PieceMode,
}

impl Default for PuzzleConfig {
    fn default() -> Self {
        Self {
            grid_size: (4, 4),
            image_path: String::new(), // 空の文字列から開始
            target_piece_count: 100,
            seed: 42,
            rotation_enabled: false,
            piece_mode: PieceMode::SquarePieces,
        }
    }
}
