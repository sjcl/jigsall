use crate::resources::{PieceMode, PuzzleConfig, PuzzleImage};

/// 縦横比保持スケールから最適なグリッドサイズを計算
pub fn calculate_aspect_ratio_grid(
    image_width: f32,
    image_height: f32,
    scale_factor: f32,
) -> (usize, usize, f32, f32) {
    // 画像の縦横比を計算
    let aspect_ratio = image_width / image_height;

    // スケールファクターから基本グリッドサイズを計算
    // scale_factor = 1 なら 1x1、scale_factor = 2 なら 2x2、など
    let base_grid = scale_factor.round() as usize;

    // 縦横比を保ったグリッドサイズを計算
    let grid_width: usize;
    let grid_height: usize;

    if aspect_ratio >= 1.0 {
        // 横長または正方形の画像
        grid_width = (base_grid as f32 * aspect_ratio.sqrt()).round() as usize;
        grid_height = (base_grid as f32 / aspect_ratio.sqrt()).round() as usize;
    } else {
        // 縦長の画像
        grid_width = (base_grid as f32 * aspect_ratio.sqrt()).round() as usize;
        grid_height = (base_grid as f32 / aspect_ratio.sqrt()).round() as usize;
    }

    // 最小グリッドサイズを確保
    let grid_width = grid_width.max(1);
    let grid_height = grid_height.max(1);

    // 実際のピースサイズを再計算
    let actual_piece_width = image_width / grid_width as f32;
    let actual_piece_height = image_height / grid_height as f32;

    (
        grid_width,
        grid_height,
        actual_piece_width,
        actual_piece_height,
    )
}

/// PuzzleConfigの現在のモードに基づいてグリッドサイズを計算
/// 画像が読み込まれていない場合はNoneを返す
pub fn calculate_grid_from_config(
    config: &PuzzleConfig,
    puzzle_image: Option<&PuzzleImage>,
) -> Option<(usize, usize, String)> {
    // 画像サイズを取得（適切でない場合はNoneを返す）
    let (image_width, image_height) = if let Some(puzzle_image) = puzzle_image {
        // 画像サイズが適切に読み込まれているかチェック
        if puzzle_image.size.x > 10.0 && puzzle_image.size.y > 10.0 {
            (puzzle_image.size.x, puzzle_image.size.y)
        } else {
            // まだ読み込み中または無効なサイズの場合はNoneを返す
            return None;
        }
    } else {
        return None;
    };

    match config.piece_mode {
        PieceMode::TargetCount => {
            // 既存のロジック：目標ピース数から最適なグリッドを計算
            let (optimal_cols, optimal_rows) = puzzle_paths::generate_columns_rows_numbers(
                image_width,
                image_height,
                config.target_piece_count,
            );
            let actual_pieces = optimal_cols * optimal_rows;
            let info = if actual_pieces != config.target_piece_count {
                format!(
                    "{}x{} = {} pieces (target: {})",
                    optimal_cols, optimal_rows, actual_pieces, config.target_piece_count
                )
            } else {
                format!(
                    "{}x{} = {} pieces",
                    optimal_cols, optimal_rows, actual_pieces
                )
            };
            Some((optimal_cols, optimal_rows, info))
        }

        PieceMode::ManualGrid => {
            // 既存のロジック：手動で指定されたグリッドサイズを使用
            let total_pieces = config.grid_size.0 * config.grid_size.1;
            let info = format!(
                "{}x{} = {} pieces (manual)",
                config.grid_size.0, config.grid_size.1, total_pieces
            );
            Some((config.grid_size.0, config.grid_size.1, info))
        }

        PieceMode::SquarePieces => {
            // 新ロジック：縦横比保持スケールから計算
            let (grid_width, grid_height, actual_width, actual_height) =
                calculate_aspect_ratio_grid(image_width, image_height, config.target_piece_size);

            let total_pieces = grid_width * grid_height;
            let info = format!(
                "{}x{} = {} pieces ({:.0}x{:.0}px each)",
                grid_width, grid_height, total_pieces, actual_width, actual_height
            );
            Some((grid_width, grid_height, info))
        }
    }
}
