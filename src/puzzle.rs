use bevy::prelude::*;
use bevy::render::mesh::{Indices, VertexAttributeValues};
use crate::components::*;
use crate::resources::*;
use rand::prelude::*;

/// パズルグリッドの周囲を囲む配置でピース位置を生成（マージン付き）
pub fn generate_placement_grid(
    grid_width: usize, 
    grid_height: usize, 
    piece_width: f32, 
    piece_height: f32,
    display_width: f32,
    display_height: f32
) -> Vec<Vec2> {
    let total_pieces = grid_width * grid_height;
    let mut rng = thread_rng();
    
    println!("🎯 Generating {} positions surrounding puzzle grid...", total_pieces);
    
    // パズルグリッド領域を計算
    let puzzle_area = calculate_puzzle_grid_area(
        grid_width, grid_height, piece_width, piece_height,
        display_width, display_height
    );
    
    // 渦巻き状配置でピース位置を生成
    let mut surrounding_positions = generate_spiral_positions(
        total_pieces,
        piece_width,
        piece_height,
        &puzzle_area,
        display_width,
        display_height
    );
    
    // 必要に応じてフォールバック配置を追加
    if surrounding_positions.len() < total_pieces {
        let missing_pieces = total_pieces - surrounding_positions.len();
        println!("🔄 Attempting fallback placement for {} remaining pieces...", missing_pieces);
        
        let fallback_positions = generate_fallback_positions(
            missing_pieces,
            piece_width,
            piece_height,
            &puzzle_area,
            &surrounding_positions,
            display_width,
            display_height
        );
        
        surrounding_positions.extend(fallback_positions);
        println!("🆘 Fallback added {} positions, total: {}", 
            surrounding_positions.len() - (total_pieces - missing_pieces), 
            surrounding_positions.len());
    }
    
    // ランダムシャッフルで配置をランダム化
    let mut positions = surrounding_positions;
    positions.shuffle(&mut rng);
    
    println!("✅ Generated {} positions surrounding puzzle", positions.len());
    
    positions
}

/// 中心からの渦巻き状配置でピース位置を生成（グリッド回避）
fn generate_spiral_positions(
    num_pieces: usize,
    piece_width: f32,
    piece_height: f32,
    puzzle_area: &(Vec2, Vec2),
    display_width: f32,
    display_height: f32
) -> Vec<Vec2> {
    let mut positions = Vec::new();
    let (puzzle_min, puzzle_max) = puzzle_area;
    
    // パズルの中心を渦巻きの中心とする
    let center = Vec2::new(
        (puzzle_min.x + puzzle_max.x) / 2.0,
        (puzzle_min.y + puzzle_max.y) / 2.0
    );
    
    // 除外エリアのサイズを計算
    let exclusion_width = puzzle_max.x - puzzle_min.x;
    let exclusion_height = puzzle_max.y - puzzle_min.y;
    let exclusion_radius = (exclusion_width.max(exclusion_height)) / 2.0;
    
    // マージンを除外エリアサイズに応じて調整
    let piece_size = piece_width.max(piece_height);
    let margin_factor = if piece_size > 500.0 { 0.3 } else { 0.5 };
    let margin_x = piece_width * margin_factor;
    let margin_y = piece_height * margin_factor;
    let effective_piece_width = piece_width + margin_x;
    let effective_piece_height = piece_height + margin_y;
    
    // 渦巻きパラメータを除外エリアサイズに基づいて動的計算
    let mut radius = exclusion_radius + piece_size * 0.5; // 除外エリアの外から開始
    let mut angle: f32 = 0.0;
    
    // 角度ステップと半径成長率を除外エリアに適応
    let angle_step = if exclusion_radius > 1000.0 { 0.08 } else { 0.12 };
    let radius_growth = if exclusion_radius > 1000.0 { 
        exclusion_radius * 0.0008  // 大きな除外エリア：急速に外向き拡散
    } else { 
        piece_size * 0.02  // 小さな除外エリア：標準成長率
    };
    
    println!("🌀 Generating spiral placement from center ({:.1}, {:.1})", center.x, center.y);
    println!("   📊 Exclusion radius: {:.1}, Initial spiral radius: {:.1}", exclusion_radius, radius);
    println!("   ⚙️ Angle step: {:.3}, Radius growth: {:.4}", angle_step, radius_growth);
    
    // 最大試行回数を除外エリアサイズに応じて調整
    let base_attempts = num_pieces * 150;
    let size_multiplier = if exclusion_radius > 1500.0 { 3.0 } else if exclusion_radius > 1000.0 { 2.0 } else { 1.0 };
    let max_attempts = (base_attempts as f32 * size_multiplier) as usize;
    let mut attempts = 0;
    
    while positions.len() < num_pieces && attempts < max_attempts {
        // 渦巻き座標を計算
        let x = center.x + radius * angle.cos();
        let y = center.y + radius * angle.sin();
        let position = Vec2::new(x, y);
        
        // 画面境界チェック（表示領域の2倍まで許可）
        let max_screen_distance = (display_width.max(display_height)) * 1.5;
        let distance_from_center = position.distance(Vec2::ZERO);
        
        if distance_from_center <= max_screen_distance {
            // グリッド領域内でないかチェック
            if !is_in_grid_area(position, puzzle_area, effective_piece_width, effective_piece_height) {
                // 他のピースと重複していないかチェック
                if !is_overlapping_with_existing(position, &positions, effective_piece_width, effective_piece_height) {
                    positions.push(position);
                    
                    // 進捗ログ（10個ごと）
                    if positions.len() % 10 == 0 || positions.len() <= 5 {
                        println!("   📍 Placed piece #{} at ({:.1}, {:.1}), radius: {:.1}", 
                            positions.len(), x, y, radius);
                    }
                }
            }
        }
        
        // 渦巻きパラメータを更新
        angle += angle_step;
        radius += radius_growth;
        attempts += 1;
        
        // 定期的な進捗ログ（大きな除外エリアの場合）
        if exclusion_radius > 1000.0 && attempts % 1000 == 0 {
            println!("   🔄 Spiral attempt #{}, placed: {}/{}, radius: {:.1}", 
                attempts, positions.len(), num_pieces, radius);
        }
    }
    
    if positions.len() < num_pieces {
        println!("⚠️ Could only place {} of {} pieces in spiral (tried {} attempts)", 
            positions.len(), num_pieces, attempts);
        println!("   📐 May need fallback placement for remaining {} pieces", 
            num_pieces - positions.len());
    } else {
        println!("✅ Successfully placed {} pieces in spiral pattern ({} attempts)", 
            positions.len(), attempts);
    }
    
    positions
}

/// 指定位置がグリッド領域内かどうかをチェック
fn is_in_grid_area(
    position: Vec2,
    puzzle_area: &(Vec2, Vec2),
    piece_width: f32,
    piece_height: f32
) -> bool {
    let (puzzle_min, puzzle_max) = puzzle_area;
    
    // ピースサイズを考慮した余白を設定（より大きく）
    let margin = piece_width.max(piece_height) * 1.0;  // 50% -> 100%
    let expanded_min = Vec2::new(puzzle_min.x - margin, puzzle_min.y - margin);
    let expanded_max = Vec2::new(puzzle_max.x + margin, puzzle_max.y + margin);
    
    position.x >= expanded_min.x && position.x <= expanded_max.x &&
    position.y >= expanded_min.y && position.y <= expanded_max.y
}

/// 指定位置が既存のピースと重複しているかチェック
fn is_overlapping_with_existing(
    position: Vec2,
    existing_positions: &[Vec2],
    piece_width: f32,
    piece_height: f32
) -> bool {
    let min_distance = piece_width.max(piece_height);
    
    for existing_pos in existing_positions {
        let distance = position.distance(*existing_pos);
        if distance < min_distance {
            return true;
        }
    }
    
    false
}

/// フォールバック用の配置生成（グリッド状 + ランダム配置）
fn generate_fallback_positions(
    count: usize,
    piece_width: f32,
    piece_height: f32,
    puzzle_area: &(Vec2, Vec2),
    existing_positions: &[Vec2],
    display_width: f32,
    display_height: f32,
) -> Vec<Vec2> {
    let mut positions = Vec::new();
    let mut rng = thread_rng();
    
    let (puzzle_min, puzzle_max) = puzzle_area;
    let piece_size = piece_width.max(piece_height);
    let spacing = piece_size * 1.2; // ピース間のスペース
    
    println!("🔄 Fallback: attempting grid-based placement...");
    
    // 除外エリアの外側にグリッド状配置を試行
    let grid_margin = piece_size * 0.5;
    let _left_x = puzzle_min.x - grid_margin - piece_size;
    let _right_x = puzzle_max.x + grid_margin + piece_size;
    let top_y = puzzle_max.y + grid_margin + piece_size;
    let bottom_y = puzzle_min.y - grid_margin - piece_size;
    
    // 上下のエリアにグリッド配置
    let cols = ((puzzle_max.x - puzzle_min.x) / spacing).max(1.0) as usize;
    let needed_rows_top = (count / 2 / cols).max(1);
    let needed_rows_bottom = (count - (needed_rows_top * cols)) / cols;
    
    // 上側エリア
    for row in 0..needed_rows_top {
        for col in 0..cols {
            if positions.len() >= count { break; }
            
            let x = puzzle_min.x + (col as f32 * spacing) + (spacing / 2.0);
            let y = top_y + (row as f32 * spacing);
            let pos = Vec2::new(x, y);
            
            // 画面境界内かチェック
            if pos.x.abs() <= display_width * 0.8 && pos.y.abs() <= display_height * 0.8 {
                if !is_overlapping_with_all(pos, existing_positions, &positions, piece_size) {
                    positions.push(pos);
                }
            }
        }
    }
    
    // 下側エリア
    for row in 0..needed_rows_bottom {
        for col in 0..cols {
            if positions.len() >= count { break; }
            
            let x = puzzle_min.x + (col as f32 * spacing) + (spacing / 2.0);
            let y = bottom_y - (row as f32 * spacing);
            let pos = Vec2::new(x, y);
            
            // 画面境界内かチェック
            if pos.x.abs() <= display_width * 0.8 && pos.y.abs() <= display_height * 0.8 {
                if !is_overlapping_with_all(pos, existing_positions, &positions, piece_size) {
                    positions.push(pos);
                }
            }
        }
    }
    
    // 残りピースをランダム配置で補完
    if positions.len() < count {
        let remaining = count - positions.len();
        println!("🎲 Fallback: attempting random placement for {} pieces...", remaining);
        
        let max_attempts = remaining * 200;
        let mut attempts = 0;
        
        while positions.len() < count && attempts < max_attempts {
            // より広いエリアからランダム選択
            let area_scale = 1.5;
            let x = rng.gen_range(-display_width * area_scale..display_width * area_scale);
            let y = rng.gen_range(-display_height * area_scale..display_height * area_scale);
            let candidate = Vec2::new(x, y);
            
            // 除外エリア外で、既存ピースと重複しなければ追加
            if !is_in_grid_area(candidate, puzzle_area, piece_size, piece_size) {
                if !is_overlapping_with_all(candidate, existing_positions, &positions, piece_size) {
                    positions.push(candidate);
                }
            }
            
            attempts += 1;
        }
    }
    
    println!("✅ Fallback generated {} positions", positions.len());
    positions
}

/// 位置が既存の全ピースと重複していないかチェック
fn is_overlapping_with_all(
    position: Vec2,
    existing_positions: &[Vec2],
    new_positions: &[Vec2],
    min_distance: f32,
) -> bool {
    // 既存のピースとのチェック
    for existing_pos in existing_positions {
        if position.distance(*existing_pos) < min_distance {
            return true;
        }
    }
    
    // 新しく配置されたピースとのチェック
    for new_pos in new_positions {
        if position.distance(*new_pos) < min_distance {
            return true;
        }
    }
    
    false
}















/// パズルグリッド領域を計算（除外エリア）
fn calculate_puzzle_grid_area(
    _grid_width: usize,
    _grid_height: usize,
    piece_width: f32,
    piece_height: f32,
    display_width: f32,
    display_height: f32,
) -> (Vec2, Vec2) {
    // 画像サイズに応じた適応的マージン計算
    let image_area = display_width * display_height;
    let piece_size = piece_width.max(piece_height);
    
    // 画像が大きいほど、またピースが大きいほどマージンを小さくする
    let base_margin_factor = if image_area > 8_000_000.0 {
        // 4K以上の大画像：非常に小さなマージン
        0.15
    } else if image_area > 2_000_000.0 {
        // Full HD以上：小さなマージン
        0.25
    } else if image_area > 500_000.0 {
        // HD以上：標準マージン
        0.4
    } else {
        // 小画像：大きなマージン
        0.6
    };
    
    // ピースサイズに応じてマージンを調整（大きなピースほど小さなマージン）
    let size_adjusted_margin = if piece_size > 500.0 {
        base_margin_factor * 0.5  // 大ピース：マージンを半分に
    } else if piece_size > 200.0 {
        base_margin_factor * 0.75 // 中ピース：マージンを3/4に
    } else {
        base_margin_factor        // 小ピース：標準マージン
    };
    
    let margin = piece_size * size_adjusted_margin;
    
    let min_x = -display_width / 2.0 - margin;
    let max_x = display_width / 2.0 + margin;
    let min_y = -display_height / 2.0 - margin;
    let max_y = display_height / 2.0 + margin;
    
    let exclusion_min = Vec2::new(min_x, min_y);
    let exclusion_max = Vec2::new(max_x, max_y);
    
    println!("🚫 Exclusion area: ({:.1}, {:.1}) to ({:.1}, {:.1})", 
        min_x, min_y, max_x, max_y);
    println!("   📏 Image: {:.0}x{:.0}, Piece: {:.0}x{:.0}, Margin factor: {:.2}", 
        display_width, display_height, piece_width, piece_height, size_adjusted_margin);
    
    (exclusion_min, exclusion_max)
}










/// メッシュから精密当たり判定用の形状データを抽出（JigsawPieceShapeから）
pub fn extract_shape_data_from_jigsaw_shape(jigsaw_shape: &crate::jigsaw_shapes::JigsawPieceShape) -> PieceShape {
    let vertices = match jigsaw_shape.mesh.attribute(Mesh::ATTRIBUTE_POSITION) {
        Some(VertexAttributeValues::Float32x3(positions)) => {
            positions.iter().map(|pos| [pos[0], pos[1]]).collect()
        }
        _ => {
            println!("Warning: Could not extract vertices from jigsaw mesh, using fallback");
            vec![[0.0, 0.0], [100.0, 0.0], [0.0, 100.0], [100.0, 100.0]]
        }
    };
    
    let indices = match jigsaw_shape.mesh.indices() {
        Some(Indices::U32(idx)) => idx.clone(),
        Some(Indices::U16(idx)) => idx.iter().map(|&i| i as u32).collect(),
        None => {
            println!("Warning: Could not extract indices from jigsaw mesh, using fallback");
            vec![0, 1, 2, 1, 2, 3]
        }
    };
    
    PieceShape { 
        vertices, 
        indices,
        shape_hash: jigsaw_shape.shape_hash.clone(), // JigsawPieceShapeから形状ハッシュを取得
    }
}



pub fn update_puzzle_image_size(
    puzzle_image: Option<ResMut<PuzzleImage>>,
    images: Res<Assets<Image>>,
    asset_server: Res<AssetServer>,
) {
    if let Some(mut puzzle_image) = puzzle_image {
        // Asset loading状態をチェック
        let load_state = asset_server.load_state(&puzzle_image.handle);
        
        // デバッグ用に状態を出力（頻度制限）
        static mut DEBUG_COUNTER: usize = 0;
        unsafe {
            DEBUG_COUNTER += 1;
            if DEBUG_COUNTER % 300 == 0 { // 5秒に1回程度
                println!("🖼️ Image loading: handle {:?}, state: {:?}, current size: {:.0}x{:.0}", 
                    puzzle_image.handle.id(), load_state, puzzle_image.size.x, puzzle_image.size.y);
            }
        }
        
        match load_state {
            bevy::asset::LoadState::Loaded => {
                if let Some(image) = images.get(&puzzle_image.handle) {
                    // Bevy 0.16では image.size() メソッドを使用
                    let actual_size = image.size();
                    let new_size = Vec2::new(actual_size.x as f32, actual_size.y as f32);
                    
                    // サイズが変更された場合のみ更新
                    if puzzle_image.size != new_size {
                        println!("Updating image size from {}x{} to {}x{}", 
                            puzzle_image.size.x, puzzle_image.size.y, new_size.x, new_size.y);
                        puzzle_image.size = new_size;
                    }
                } else {
                    println!("Image is loaded but not found in Assets<Image>");
                }
            }
            bevy::asset::LoadState::Loading => {
                // 頻度を制限してログ出力
                unsafe {
                    if DEBUG_COUNTER % 120 == 0 {
                        println!("Image is still loading...");
                    }
                }
            }
            bevy::asset::LoadState::Failed(_) => {
                println!("Failed to load image!");
            }
            bevy::asset::LoadState::NotLoaded => {
                // NotLoadedの場合は、再度読み込みを試行
                unsafe {
                    if DEBUG_COUNTER % 120 == 0 {
                        println!("Image not loaded - this may indicate the image was not properly loaded by AssetServer");
                    }
                }
            }
        }
    }
}