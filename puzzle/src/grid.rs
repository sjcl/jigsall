//! Grid sizing shared by puzzle setup adapters.
/// Choose exact divisor pairs whose piece dimensions are closest to square.
/// Preserve the setup UI's former divisor ordering and tie-breaking behavior.
pub fn generate_columns_rows_numbers(width: f32, height: f32, count: usize) -> (usize, usize) {
    if count == 0 {
        return (1, 1);
    }
    let mut pairs = Vec::new();
    let mut divisor = 1;
    while divisor <= count / divisor {
        if count.is_multiple_of(divisor) {
            pairs.push((divisor, count / divisor));
        }
        divisor += 1;
    }
    let mirrored: Vec<_> = pairs
        .iter()
        .rev()
        .filter(|(a, b)| a != b)
        .map(|&(a, b)| (b, a))
        .collect();
    pairs.extend(mirrored);
    let mut best = pairs[0];
    let mut difference = f32::MAX;
    for (columns, rows) in pairs {
        let candidate = (width / columns as f32 - height / rows as f32).abs();
        if candidate < 1.0 {
            return (columns, rows);
        }
        if candidate > difference {
            break;
        }
        best = (columns, rows);
        difference = candidate;
    }
    best
}

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
