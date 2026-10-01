use puzzella_game::resources::{PieceMode, PuzzleConfig, PuzzleImage};
use puzzella_puzzle::grid::{calculate_aspect_ratio_grid, generate_columns_rows_numbers};

/// PuzzleConfigの現在のモードに基づいてグリッドサイズを計算
/// 画像が読み込まれていない場合はNoneを返す
pub fn calculate_grid_from_config(
    config: &PuzzleConfig,
    puzzle_image: Option<&PuzzleImage>,
) -> Option<(usize, usize, String)> {
    // 画像サイズを取得（適切でない場合はNoneを返す）
    let (image_width, image_height) = {
        let puzzle_image = puzzle_image?;
        // 画像サイズが適切に読み込まれているかチェック
        if puzzle_image.size.x > 10.0 && puzzle_image.size.y > 10.0 {
            (puzzle_image.size.x, puzzle_image.size.y)
        } else {
            // まだ読み込み中または無効なサイズの場合はNoneを返す
            return None;
        }
    };

    match config.piece_mode {
        PieceMode::TargetCount => {
            // 既存のロジック：目標ピース数から最適なグリッドを計算
            let (optimal_cols, optimal_rows) =
                generate_columns_rows_numbers(image_width, image_height, config.target_piece_count);
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
