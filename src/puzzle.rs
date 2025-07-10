use bevy::prelude::*;
use bevy::sprite::ColorMaterial;
use bevy::render::mesh::{Indices, VertexAttributeValues};
use crate::components::*;
use crate::resources::*;
use crate::jigsaw_shapes::{JigsawShapeGenerator, clone_mesh_from_shape};
use uuid::Uuid;
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

/// 左右レイヤー数のみを計算（上下の拡張は考慮しない）
fn calculate_left_right_layers_needed(
    remaining_pieces: usize,
    piece_width: f32,
    piece_height: f32,
    puzzle_area: &(Vec2, Vec2)
) -> usize {
    if remaining_pieces == 0 { return 0; }
    
    let (puzzle_min, puzzle_max) = puzzle_area;
    let puzzle_width = puzzle_max.x - puzzle_min.x;
    let puzzle_height = puzzle_max.y - puzzle_min.y;
    
    // 基本レイヤー（レイヤー1）でのピース数を計算
    let basic_top_cols = (puzzle_width / piece_width).ceil() as usize;
    let basic_side_rows = (puzzle_height / piece_height).ceil() as usize;
    let basic_layer_pieces = basic_top_cols * 2 + basic_side_rows * 2; // 上下 + 左右
    
    // 基本レイヤーで配置できる分を除く
    let pieces_for_additional_layers = if remaining_pieces > basic_layer_pieces {
        remaining_pieces - basic_layer_pieces
    } else {
        return 0; // 基本レイヤーで十分
    };
    
    // 追加レイヤーは左右のみなので、左右ピース数で計算
    let left_right_pieces_per_layer = basic_side_rows * 2; // 左辺 + 右辺
    
    let additional_layers = if left_right_pieces_per_layer > 0 {
        (pieces_for_additional_layers as f32 / left_right_pieces_per_layer as f32).ceil() as usize
    } else {
        0
    };
    
    println!("📊 Basic layer: {} pieces, additional: {} pieces, {} left/right layers needed", 
        basic_layer_pieces, pieces_for_additional_layers, additional_layers);
    
    additional_layers
}

/// 総レイヤー数を事前計算
fn calculate_total_layers_needed(
    remaining_pieces: usize,
    piece_width: f32,
    piece_height: f32,
    puzzle_area: &(Vec2, Vec2),
    base_margin: f32
) -> usize {
    if remaining_pieces == 0 { return 0; }
    
    let (puzzle_min, puzzle_max) = puzzle_area;
    let puzzle_width = puzzle_max.x - puzzle_min.x;
    let puzzle_height = puzzle_max.y - puzzle_min.y;
    
    // 1レイヤーあたりのピース数を概算
    let top_cols = (puzzle_width / piece_width).ceil() as usize;
    let side_rows = (puzzle_height / piece_height).ceil() as usize;
    let pieces_per_layer = top_cols * 2 + side_rows * 2; // 上下 + 左右
    
    let layers_needed = (remaining_pieces as f32 / pieces_per_layer as f32).ceil() as usize;
    println!("📊 Estimated {} pieces per layer, need {} layers for {} pieces", 
        pieces_per_layer, layers_needed, remaining_pieces);
    
    layers_needed
}

/// 動的横幅拡張付きフレームレイヤーを生成
fn generate_frame_layer_with_dynamic_width(
    num_pieces: usize,
    piece_width: f32,
    piece_height: f32,
    puzzle_area: &(Vec2, Vec2),
    total_margin: f32,  // パズルからの総距離
    max_layer_margin: f32   // 現在の最大レイヤーマージン
) -> Vec<Vec2> {
    let mut positions = Vec::new();
    let (puzzle_min, puzzle_max) = puzzle_area;
    
    let puzzle_width = puzzle_max.x - puzzle_min.x;
    let puzzle_height = puzzle_max.y - puzzle_min.y;
    
    // 横幅を現在の最大レイヤーまで拡張（動的更新）
    let extended_width = puzzle_width + max_layer_margin * 2.0;
    
    println!("🔧 Layer frame: puzzle {}x{}, margin {:.1}, extended_width {:.1}", 
        puzzle_width, puzzle_height, total_margin, extended_width);
    
    // 上辺（実際の最外層まで拡張）
    let top_y = puzzle_max.y + total_margin;
    let top_cols = (extended_width / piece_width).ceil() as usize;
    let top_start_x = puzzle_min.x - max_layer_margin;
    
    for col in 0..top_cols {
        if positions.len() >= num_pieces { break; }
        let x = top_start_x + (col as f32 * piece_width);
        positions.push(Vec2::new(x, top_y));
    }
    
    // 下辺（拡張された横幅）
    let bottom_y = puzzle_min.y - total_margin;
    let bottom_cols = top_cols; // 上辺と同じ列数
    let bottom_start_x = top_start_x; // 上辺と同じ開始位置
    
    for col in 0..bottom_cols {
        if positions.len() >= num_pieces { break; }
        let x = bottom_start_x + (col as f32 * piece_width);
        positions.push(Vec2::new(x, bottom_y));
    }
    
    // 左辺（上下辺を除く）
    let left_x = puzzle_min.x - total_margin;
    let side_rows = (puzzle_height / piece_height).ceil() as usize;
    
    for row in 0..side_rows {
        if positions.len() >= num_pieces { break; }
        let y = puzzle_min.y + (row as f32 * piece_height);
        positions.push(Vec2::new(left_x, y));
    }
    
    // 右辺（上下辺を除く）
    let right_x = puzzle_max.x + total_margin;
    for row in 0..side_rows {
        if positions.len() >= num_pieces { break; }
        let y = puzzle_min.y + (row as f32 * piece_height);
        positions.push(Vec2::new(right_x, y));
    }
    
    println!("🔧 Generated {} extended frame layer positions", positions.len());
    positions
}

/// 横幅拡張により過去のレイヤーの拡張エリアに遡及配置
fn backfill_expanded_areas(
    num_pieces: usize,
    piece_width: f32,
    piece_height: f32,
    puzzle_area: &(Vec2, Vec2),
    base_margin: f32,
    start_layer: usize,
    end_layer: usize,
    current_max_margin: f32
) -> Vec<Vec2> {
    let mut positions = Vec::new();
    
    if num_pieces == 0 {
        return positions;
    }
    
    let (puzzle_min, puzzle_max) = puzzle_area;
    let puzzle_width = puzzle_max.x - puzzle_min.x;
    
    println!("🔄 Backfilling layers {}-{} with expanded width (max margin: {:.1})", 
        start_layer, end_layer, current_max_margin);
    
    // 過去のレイヤーを遡って拡張エリアに配置
    for layer in start_layer..=end_layer {
        if positions.len() >= num_pieces { break; }
        
        let layer_margin = base_margin * layer as f32;
        let layer_max_margin = base_margin * layer as f32; // そのレイヤー時点での最大マージン
        
        // そのレイヤー時点の横幅 vs 現在の横幅の差分を計算
        let old_extended_width = puzzle_width + layer_max_margin * 2.0;
        let new_extended_width = puzzle_width + current_max_margin * 2.0;
        
        if new_extended_width > old_extended_width {
            // 拡張された部分に配置
            let width_diff = new_extended_width - old_extended_width;
            let extra_cols = (width_diff / piece_width).floor() as usize;
            
            if extra_cols > 0 {
                let top_y = puzzle_max.y + layer_margin;
                let bottom_y = puzzle_min.y - layer_margin;
                
                // 左側の拡張部分
                let left_start_x = puzzle_min.x - current_max_margin;
                for col in 0..(extra_cols / 2) {
                    if positions.len() >= num_pieces { break; }
                    let x = left_start_x + (col as f32 * piece_width);
                    positions.push(Vec2::new(x, top_y));
                    if positions.len() < num_pieces {
                        positions.push(Vec2::new(x, bottom_y));
                    }
                }
                
                // 右側の拡張部分
                let right_start_x = puzzle_max.x + layer_max_margin;
                for col in 0..(extra_cols / 2) {
                    if positions.len() >= num_pieces { break; }
                    let x = right_start_x + (col as f32 * piece_width);
                    positions.push(Vec2::new(x, top_y));
                    if positions.len() < num_pieces {
                        positions.push(Vec2::new(x, bottom_y));
                    }
                }
            }
        }
    }
    
    println!("🔄 Backfilled {} positions in expanded areas", positions.len());
    positions
}


/// 最適なパッキング矩形を計算
fn calculate_optimal_packing_rectangle(
    num_pieces: usize,
    piece_width: f32,
    piece_height: f32,
    display_width: f32,
    display_height: f32
) -> (usize, usize, f32, f32) {
    // 理想的な正方形に近い配置を目指す
    let ideal_side = (num_pieces as f32).sqrt();
    
    // 幅優先と高さ優先の2つの候補を計算
    let cols_wide = ideal_side.ceil() as usize;
    let rows_wide = (num_pieces as f32 / cols_wide as f32).ceil() as usize;
    
    let rows_tall = ideal_side.ceil() as usize;
    let cols_tall = (num_pieces as f32 / rows_tall as f32).ceil() as usize;
    
    // 両方の候補の実際の寸法を計算
    let wide_rect = (cols_wide, rows_wide, cols_wide as f32 * piece_width, rows_wide as f32 * piece_height);
    let tall_rect = (cols_tall, rows_tall, cols_tall as f32 * piece_width, rows_tall as f32 * piece_height);
    
    // ディスプレイサイズに収まりやすい方を選択
    let wide_fits = wide_rect.2 < display_width * 0.8 && wide_rect.3 < display_height * 0.8;
    let tall_fits = tall_rect.2 < display_width * 0.8 && tall_rect.3 < display_height * 0.8;
    
    match (wide_fits, tall_fits) {
        (true, true) => {
            // 両方収まる場合は、よりコンパクトな方を選択
            if wide_rect.2 * wide_rect.3 < tall_rect.2 * tall_rect.3 {
                wide_rect
            } else {
                tall_rect
            }
        },
        (true, false) => wide_rect,
        (false, true) => tall_rect,
        (false, false) => {
            // どちらも収まらない場合は、アスペクト比が良い方を選択
            wide_rect
        }
    }
}

/// パズルエリアとの干渉を避ける最適配置位置を計算
fn find_best_placement_position(
    rect_width: f32,
    rect_height: f32,
    display_width: f32,
    display_height: f32,
    exclusion_area: &(Vec2, Vec2)
) -> Vec2 {
    let (exclusion_min, exclusion_max) = exclusion_area;
    
    // パズルの右側に配置を試す
    let right_x = exclusion_max.x + rect_width * 0.1;
    let right_y = -rect_height / 2.0;
    
    // ディスプレイ範囲内に収まるかチェック
    if right_x + rect_width < display_width / 2.0 {
        return Vec2::new(right_x, right_y);
    }
    
    // パズルの左側に配置を試す
    let left_x = exclusion_min.x - rect_width - rect_width * 0.1;
    let left_y = -rect_height / 2.0;
    
    if left_x > -display_width / 2.0 {
        return Vec2::new(left_x, left_y);
    }
    
    // パズルの下側に配置
    let bottom_x = -rect_width / 2.0;
    let bottom_y = exclusion_min.y - rect_height - rect_height * 0.1;
    
    Vec2::new(bottom_x, bottom_y)
}

/// 隣接エリアで完全充填を行う
fn generate_adjacent_perfect_fill(
    shortage: usize,
    piece_width: f32,
    piece_height: f32,
    origin: Vec2,
    rect_width: f32,
    rect_height: f32,
    exclusion_area: &(Vec2, Vec2)
) -> Vec<Vec2> {
    let mut positions = Vec::new();
    
    // 右隣接エリアに配置
    let adjacent_x = origin.x + rect_width;
    let cols_needed = (shortage as f32 / (rect_height / piece_height)).ceil() as usize;
    
    for row in 0..(rect_height / piece_height) as usize {
        for col in 0..cols_needed {
            if positions.len() >= shortage {
                break;
            }
            
            let x = adjacent_x + (col as f32 * piece_width);
            let y = origin.y + (row as f32 * piece_height);
            let position = Vec2::new(x, y);
            
            if !is_position_in_exclusion_area(&position, exclusion_area) {
                positions.push(position);
            }
        }
        if positions.len() >= shortage {
            break;
        }
    }
    
    positions
}

/// パズル周囲にコンパクトな配置エリアを生成（大幅拡張版）
fn generate_compact_placement_areas(
    piece_width: f32,
    piece_height: f32,
    puzzle_width: f32,
    puzzle_height: f32,
) -> Vec<(String, Vec<Vec2>)> {
    let mut areas = Vec::new();
    let min_distance = piece_width.max(piece_height) * 0.5;
    
    // エリアサイズを動的計算（十分に大きくする）
    let expansion_factor = 2.5; // パズルサイズの2.5倍のエリア
    let extended_width = puzzle_width * expansion_factor;
    let extended_height = puzzle_height * expansion_factor;
    
    // 各エリアで必要な行数・列数を計算
    let side_cols = ((extended_width / 2.0) / piece_width).floor() as usize; // 片側の幅
    let side_rows = (extended_height / piece_height).floor() as usize;
    let horizontal_cols = (extended_width / piece_width).floor() as usize;
    let horizontal_rows = ((extended_height / 2.0) / piece_height).floor() as usize; // 片側の高さ
    
    println!("🏗️ Area calculations: side={}x{}, horizontal={}x{}", 
        side_cols, side_rows, horizontal_cols, horizontal_rows);
    
    // 右側エリア（縦列のコンパクトグリッド）
    let right_area = generate_side_compact_area(
        puzzle_width / 2.0 + min_distance,
        0.0,
        piece_width,
        piece_height,
        "right",
        side_cols.max(8), // 最低8列は確保
        side_rows.max(30), // 最低30行は確保
    );
    areas.push(("Right".to_string(), right_area));
    
    // 左側エリア（縦列のコンパクトグリッド）
    let left_area = generate_side_compact_area(
        -puzzle_width / 2.0 - min_distance,
        0.0,
        piece_width,
        piece_height,
        "left",
        side_cols.max(8),
        side_rows.max(30),
    );
    areas.push(("Left".to_string(), left_area));
    
    // 上側エリア（横列のコンパクトグリッド）
    let top_area = generate_horizontal_compact_area(
        0.0,
        puzzle_height / 2.0 + min_distance,
        piece_width,
        piece_height,
        "top",
        horizontal_cols.max(20), // 最低20列は確保
        horizontal_rows.max(8),  // 最低8行は確保
    );
    areas.push(("Top".to_string(), top_area));
    
    // 下側エリア（横列のコンパクトグリッド）
    let bottom_area = generate_horizontal_compact_area(
        0.0,
        -puzzle_height / 2.0 - min_distance,
        piece_width,
        piece_height,
        "bottom",
        horizontal_cols.max(20),
        horizontal_rows.max(8),
    );
    areas.push(("Bottom".to_string(), bottom_area));
    
    // 4つのコーナーエリアも追加（さらなる拡張）
    let corner_size = 10; // 10x10のコーナーエリア
    
    // 右上コーナー
    let top_right_area = generate_corner_compact_area(
        puzzle_width / 2.0 + min_distance,
        puzzle_height / 2.0 + min_distance,
        piece_width,
        piece_height,
        corner_size,
        corner_size,
    );
    areas.push(("TopRight".to_string(), top_right_area));
    
    // 右下コーナー
    let bottom_right_area = generate_corner_compact_area(
        puzzle_width / 2.0 + min_distance,
        -puzzle_height / 2.0 - min_distance,
        piece_width,
        piece_height,
        corner_size,
        corner_size,
    );
    areas.push(("BottomRight".to_string(), bottom_right_area));
    
    // 左上コーナー
    let top_left_area = generate_corner_compact_area(
        -puzzle_width / 2.0 - min_distance,
        puzzle_height / 2.0 + min_distance,
        piece_width,
        piece_height,
        corner_size,
        corner_size,
    );
    areas.push(("TopLeft".to_string(), top_left_area));
    
    // 左下コーナー
    let bottom_left_area = generate_corner_compact_area(
        -puzzle_width / 2.0 - min_distance,
        -puzzle_height / 2.0 - min_distance,
        piece_width,
        piece_height,
        corner_size,
        corner_size,
    );
    areas.push(("BottomLeft".to_string(), bottom_left_area));
    
    areas
}

/// 縦側（左右）のコンパクトエリアを生成
fn generate_side_compact_area(
    center_x: f32,
    center_y: f32,
    piece_width: f32,
    piece_height: f32,
    side: &str,
    max_cols: usize,
    max_rows: usize,
) -> Vec<Vec2> {
    let mut positions = Vec::new();
    
    let start_x = if side == "left" { 
        center_x - (max_cols as f32 * piece_width) 
    } else { 
        center_x 
    };
    let start_y = center_y - (max_rows as f32 * piece_height) / 2.0;
    
    for row in 0..max_rows {
        for col in 0..max_cols {
            let x = start_x + col as f32 * piece_width;
            let y = start_y + row as f32 * piece_height;
            positions.push(Vec2::new(x, y));
        }
    }
    
    positions
}

/// 横側（上下）のコンパクトエリアを生成
fn generate_horizontal_compact_area(
    center_x: f32,
    center_y: f32,
    piece_width: f32,
    piece_height: f32,
    side: &str,
    max_cols: usize,
    max_rows: usize,
) -> Vec<Vec2> {
    let mut positions = Vec::new();
    
    let start_x = center_x - (max_cols as f32 * piece_width) / 2.0;
    let start_y = if side == "bottom" { 
        center_y - (max_rows as f32 * piece_height) 
    } else { 
        center_y 
    };
    
    for row in 0..max_rows {
        for col in 0..max_cols {
            let x = start_x + col as f32 * piece_width;
            let y = start_y + row as f32 * piece_height;
            positions.push(Vec2::new(x, y));
        }
    }
    
    positions
}

/// コーナーエリアのコンパクトグリッドを生成
fn generate_corner_compact_area(
    corner_x: f32,
    corner_y: f32,
    piece_width: f32,
    piece_height: f32,
    cols: usize,
    rows: usize,
) -> Vec<Vec2> {
    let mut positions = Vec::new();
    
    // コーナー位置から外側に向かって展開
    let start_x = if corner_x < 0.0 {
        corner_x - (cols as f32 * piece_width)
    } else {
        corner_x
    };
    
    let start_y = if corner_y < 0.0 {
        corner_y - (rows as f32 * piece_height)
    } else {
        corner_y
    };
    
    for row in 0..rows {
        for col in 0..cols {
            let x = start_x + col as f32 * piece_width;
            let y = start_y + row as f32 * piece_height;
            positions.push(Vec2::new(x, y));
        }
    }
    
    positions
}

/// コンパクトな追加位置を生成
fn generate_compact_additional_positions(
    count: usize,
    piece_width: f32,
    piece_height: f32,
    puzzle_width: f32,
    puzzle_height: f32,
    exclusion_area: &(Vec2, Vec2),
    existing_positions: &[Vec2],
) -> Vec<Vec2> {
    let mut positions = Vec::with_capacity(count);
    let mut rng = thread_rng();
    
    // 既存位置の近くにコンパクトに追加
    let max_attempts = count * 50;
    let mut attempts = 0;
    
    while positions.len() < count && attempts < max_attempts {
        attempts += 1;
        
        if !existing_positions.is_empty() {
            // 既存位置の近くを優先
            let base_pos = existing_positions.choose(&mut rng).unwrap();
            let offset_x = rng.gen_range(-piece_width * 2.0..piece_width * 2.0);
            let offset_y = rng.gen_range(-piece_height * 2.0..piece_height * 2.0);
            let candidate = Vec2::new(base_pos.x + offset_x, base_pos.y + offset_y);
            
            if !is_position_in_exclusion_area(&candidate, exclusion_area) {
                // 既存位置との重複チェック
                let min_distance = piece_width.min(piece_height) * 0.8;
                let mut valid = true;
                
                for existing in existing_positions.iter().chain(positions.iter()) {
                    if candidate.distance(*existing) < min_distance {
                        valid = false;
                        break;
                    }
                }
                
                if valid {
                    positions.push(candidate);
                }
            }
        } else {
            // フォールバック: パズル周辺のランダム位置
            let area_expansion = 1.5;
            let x = rng.gen_range(-puzzle_width * area_expansion..puzzle_width * area_expansion);
            let y = rng.gen_range(-puzzle_height * area_expansion..puzzle_height * area_expansion);
            let candidate = Vec2::new(x, y);
            
            if !is_position_in_exclusion_area(&candidate, exclusion_area) {
                positions.push(candidate);
            }
        }
    }
    
    println!("🔄 Generated {} compact additional positions after {} attempts", positions.len(), attempts);
    
    positions
}

/// パズル周辺の拡張エリアに詰められたグリッドを生成
fn generate_extended_area_grid(
    piece_width: f32,
    piece_height: f32,
    puzzle_width: f32,
    puzzle_height: f32,
) -> Vec<Vec2> {
    let mut positions = Vec::new();
    
    // 拡張エリアのサイズを計算（パズルの3倍のエリア）
    let extended_width = puzzle_width * 3.0;
    let extended_height = puzzle_height * 3.0;
    
    // グリッドの行数・列数を計算
    let cols = (extended_width / piece_width).floor() as usize;
    let rows = (extended_height / piece_height).floor() as usize;
    
    // グリッドの開始位置を計算（中央寄せ）
    let start_x = -(cols as f32 * piece_width) / 2.0;
    let start_y = -(rows as f32 * piece_height) / 2.0;
    
    // 詰められたグリッド状に位置を生成
    for row in 0..rows {
        for col in 0..cols {
            let x = start_x + col as f32 * piece_width + piece_width / 2.0;
            let y = start_y + row as f32 * piece_height + piece_height / 2.0;
            positions.push(Vec2::new(x, y));
        }
    }
    
    println!("🗂️ Generated {}x{} = {} extended grid positions", cols, rows, positions.len());
    
    positions
}

/// パズルグリッド領域を計算（除外エリア）
fn calculate_puzzle_grid_area(
    grid_width: usize,
    grid_height: usize,
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

/// 位置が除外エリア内にあるかチェック
fn is_position_in_exclusion_area(pos: &Vec2, exclusion_area: &(Vec2, Vec2)) -> bool {
    let (min, max) = exclusion_area;
    pos.x >= min.x && pos.x <= max.x && pos.y >= min.y && pos.y <= max.y
}

/// 不足分の位置をフォールバック生成

/// 詰められたグリッド配置を生成（複数のレイアウトパターン対応）
fn generate_compact_grid_layout(
    total_pieces: usize,
    piece_width: f32,
    piece_height: f32,
    puzzle_width: f32,
    puzzle_height: f32,
    min_distance_from_puzzle: f32,
) -> Vec<Vec2> {
    let mut rng = thread_rng();
    
    // 複数の配置パターンからランダムに選択
    let layout_patterns = vec![
        "compact_grid",      // 詰められたグリッド
        "surrounding_frame", // パズル周囲のフレーム状配置
        "side_columns",      // 左右の縦列配置
        "mixed_areas",       // 複数エリアに分散配置
    ];
    
    let selected_pattern = layout_patterns.choose(&mut rng).unwrap();
    
    println!("🎨 Selected layout pattern: {}", selected_pattern);
    
    match *selected_pattern {
        "compact_grid" => generate_compact_grid_pattern(
            total_pieces, piece_width, piece_height, 
            puzzle_width, puzzle_height, min_distance_from_puzzle
        ),
        "surrounding_frame" => generate_surrounding_frame_pattern(
            total_pieces, piece_width, piece_height,
            puzzle_width, puzzle_height, min_distance_from_puzzle
        ),
        "side_columns" => generate_side_columns_pattern(
            total_pieces, piece_width, piece_height,
            puzzle_width, puzzle_height, min_distance_from_puzzle
        ),
        "mixed_areas" => generate_mixed_areas_pattern(
            total_pieces, piece_width, piece_height,
            puzzle_width, puzzle_height, min_distance_from_puzzle
        ),
        _ => generate_compact_grid_pattern(
            total_pieces, piece_width, piece_height,
            puzzle_width, puzzle_height, min_distance_from_puzzle
        ),
    }
}

/// パターン1: グリッド周囲の詰められた配置
fn generate_compact_grid_pattern(
    total_pieces: usize,
    piece_width: f32,
    piece_height: f32,
    puzzle_width: f32,
    puzzle_height: f32,
    min_distance: f32,
) -> Vec<Vec2> {
    let mut positions = Vec::with_capacity(total_pieces);
    
    // グリッド周囲のエリアを4つに分けて配置
    let pieces_per_side = total_pieces / 4;
    let remaining_pieces = total_pieces % 4;
    
    // 右側エリア
    let right_pieces = pieces_per_side + if remaining_pieces > 0 { 1 } else { 0 };
    let right_cols = (right_pieces as f32).sqrt().ceil() as usize;
    let right_rows = (right_pieces as f32 / right_cols as f32).ceil() as usize;
    
    let right_start_x = puzzle_width / 2.0 + min_distance;
    let right_start_y = -(right_rows as f32 * piece_height) / 2.0;
    
    for i in 0..right_pieces {
        let col = i % right_cols;
        let row = i / right_cols;
        let x = right_start_x + col as f32 * piece_width;
        let y = right_start_y + row as f32 * piece_height;
        positions.push(Vec2::new(x, y));
    }
    
    // 左側エリア
    let left_pieces = pieces_per_side + if remaining_pieces > 1 { 1 } else { 0 };
    let left_cols = (left_pieces as f32).sqrt().ceil() as usize;
    let left_rows = (left_pieces as f32 / left_cols as f32).ceil() as usize;
    
    let left_end_x = -puzzle_width / 2.0 - min_distance;
    let left_start_y = -(left_rows as f32 * piece_height) / 2.0;
    
    for i in 0..left_pieces {
        let col = i % left_cols;
        let row = i / left_cols;
        let x = left_end_x - col as f32 * piece_width;
        let y = left_start_y + row as f32 * piece_height;
        positions.push(Vec2::new(x, y));
    }
    
    // 上側エリア
    let top_pieces = pieces_per_side + if remaining_pieces > 2 { 1 } else { 0 };
    let top_cols = (top_pieces as f32).sqrt().ceil() as usize;
    let top_rows = (top_pieces as f32 / top_cols as f32).ceil() as usize;
    
    let top_start_x = -(top_cols as f32 * piece_width) / 2.0;
    let top_start_y = puzzle_height / 2.0 + min_distance;
    
    for i in 0..top_pieces {
        let col = i % top_cols;
        let row = i / top_cols;
        let x = top_start_x + col as f32 * piece_width;
        let y = top_start_y + row as f32 * piece_height;
        positions.push(Vec2::new(x, y));
    }
    
    // 下側エリア
    let bottom_pieces = pieces_per_side;
    let bottom_cols = (bottom_pieces as f32).sqrt().ceil() as usize;
    let bottom_rows = (bottom_pieces as f32 / bottom_cols as f32).ceil() as usize;
    
    let bottom_start_x = -(bottom_cols as f32 * piece_width) / 2.0;
    let bottom_end_y = -puzzle_height / 2.0 - min_distance;
    
    for i in 0..bottom_pieces {
        let col = i % bottom_cols;
        let row = i / bottom_cols;
        let x = bottom_start_x + col as f32 * piece_width;
        let y = bottom_end_y - row as f32 * piece_height;
        positions.push(Vec2::new(x, y));
    }
    
    println!("📐 Compact grid around puzzle: {} right, {} left, {} top, {} bottom", 
        right_pieces, left_pieces, top_pieces, bottom_pieces);
    
    positions
}

/// パターン2: パズル周囲のフレーム状配置
fn generate_surrounding_frame_pattern(
    total_pieces: usize,
    piece_width: f32,
    piece_height: f32,
    puzzle_width: f32,
    puzzle_height: f32,
    min_distance: f32,
) -> Vec<Vec2> {
    let mut positions = Vec::with_capacity(total_pieces);
    
    // パズル周囲のフレーム配置
    let frame_margin = min_distance;
    let frame_left = -puzzle_width / 2.0 - frame_margin;
    let frame_right = puzzle_width / 2.0 + frame_margin;
    let frame_top = puzzle_height / 2.0 + frame_margin;
    let frame_bottom = -puzzle_height / 2.0 - frame_margin;
    
    // 上下の横列
    let cols_per_row = ((puzzle_width + frame_margin * 2.0) / piece_width).floor() as usize;
    let rows_needed = (total_pieces as f32 / (cols_per_row * 2 + 2) as f32).ceil() as usize;
    
    let mut piece_count = 0;
    
    for row in 0..rows_needed {
        if piece_count >= total_pieces { break; }
        
        // 上の行
        let y_top = frame_top + row as f32 * piece_height;
        for col in 0..cols_per_row {
            if piece_count >= total_pieces { break; }
            let x = frame_left + col as f32 * piece_width;
            positions.push(Vec2::new(x, y_top));
            piece_count += 1;
        }
        
        // 下の行
        if piece_count >= total_pieces { break; }
        let y_bottom = frame_bottom - row as f32 * piece_height;
        for col in 0..cols_per_row {
            if piece_count >= total_pieces { break; }
            let x = frame_left + col as f32 * piece_width;
            positions.push(Vec2::new(x, y_bottom));
            piece_count += 1;
        }
    }
    
    println!("🖼️ Frame layout: {} pieces in surrounding frame", positions.len());
    positions
}

/// パターン3: 左右の縦列配置
fn generate_side_columns_pattern(
    total_pieces: usize,
    piece_width: f32,
    piece_height: f32,
    puzzle_width: f32,
    puzzle_height: f32,
    min_distance: f32,
) -> Vec<Vec2> {
    let mut positions = Vec::with_capacity(total_pieces);
    
    let pieces_per_side = total_pieces / 2;
    let remaining_pieces = total_pieces % 2;
    
    // 左側の縦列
    let left_x = -puzzle_width / 2.0 - min_distance - piece_width;
    let left_start_y = -(pieces_per_side as f32 * piece_height) / 2.0;
    
    for i in 0..pieces_per_side + remaining_pieces {
        let y = left_start_y + i as f32 * piece_height;
        positions.push(Vec2::new(left_x, y));
    }
    
    // 右側の縦列
    let right_x = puzzle_width / 2.0 + min_distance + piece_width;
    let right_start_y = -(pieces_per_side as f32 * piece_height) / 2.0;
    
    for i in 0..pieces_per_side {
        let y = right_start_y + i as f32 * piece_height;
        positions.push(Vec2::new(right_x, y));
    }
    
    println!("📏 Side columns: {} left, {} right", pieces_per_side + remaining_pieces, pieces_per_side);
    positions
}

/// パターン4: 複数エリアに分散配置
fn generate_mixed_areas_pattern(
    total_pieces: usize,
    piece_width: f32,
    piece_height: f32,
    puzzle_width: f32,
    puzzle_height: f32,
    min_distance: f32,
) -> Vec<Vec2> {
    let mut positions = Vec::with_capacity(total_pieces);
    
    // 4つのエリアに分散
    let pieces_per_area = total_pieces / 4;
    let remaining_pieces = total_pieces % 4;
    
    let areas = [
        // 右上
        (puzzle_width / 2.0 + min_distance, puzzle_height / 2.0 + min_distance),
        // 右下  
        (puzzle_width / 2.0 + min_distance, -puzzle_height / 2.0 - min_distance),
        // 左上
        (-puzzle_width / 2.0 - min_distance, puzzle_height / 2.0 + min_distance),
        // 左下
        (-puzzle_width / 2.0 - min_distance, -puzzle_height / 2.0 - min_distance),
    ];
    
    for (area_idx, (start_x, start_y)) in areas.iter().enumerate() {
        let pieces_in_this_area = pieces_per_area + if area_idx < remaining_pieces { 1 } else { 0 };
        
        // 各エリア内で小さなグリッド配置
        let cols = (pieces_in_this_area as f32).sqrt().ceil() as usize;
        let rows = (pieces_in_this_area as f32 / cols as f32).ceil() as usize;
        
        for i in 0..pieces_in_this_area {
            let col = i % cols;
            let row = i / cols;
            
            let x = start_x + col as f32 * piece_width * if start_x < &0.0 { -1.0 } else { 1.0 };
            let y = start_y + row as f32 * piece_height * if start_y < &0.0 { -1.0 } else { 1.0 };
            
            positions.push(Vec2::new(x, y));
        }
    }
    
    println!("🎯 Mixed areas: 4 compact areas with {}-{} pieces each", 
        pieces_per_area, pieces_per_area + 1);
    positions
}

/// 指定されたレイヤーの配置位置を生成
fn generate_layer_positions(
    grid_half_width: f32,
    grid_half_height: f32,
    piece_width: f32,
    piece_height: f32,
    layer: usize,
    min_distance: f32
) -> Vec<Vec2> {
    let mut positions = Vec::new();
    
    // レイヤーの距離を計算
    let layer_distance = min_distance + (layer - 1) as f32 * piece_height.max(piece_width);
    
    // 各方向の範囲を計算
    let left_x = -grid_half_width - layer_distance;
    let right_x = grid_half_width + layer_distance;
    let top_y = grid_half_height + layer_distance;
    let bottom_y = -grid_half_height - layer_distance;
    
    // 上辺（左から右へ）
    let top_cols = ((right_x - left_x) / piece_width).floor() as usize;
    for i in 0..top_cols {
        let x = left_x + (i as f32 + 0.5) * piece_width;
        positions.push(Vec2::new(x, top_y));
    }
    
    // 右辺（上から下へ、角は除く）
    let right_rows = ((top_y - bottom_y) / piece_height).floor() as usize;
    for i in 1..right_rows {
        let y = top_y - (i as f32 + 0.5) * piece_height;
        positions.push(Vec2::new(right_x, y));
    }
    
    // 下辺（右から左へ、角は除く）
    for i in 1..top_cols {
        let x = right_x - (i as f32 + 0.5) * piece_width;
        positions.push(Vec2::new(x, bottom_y));
    }
    
    // 左辺（下から上へ、角は除く）
    for i in 1..right_rows {
        let y = bottom_y + (i as f32 + 0.5) * piece_height;
        positions.push(Vec2::new(left_x, y));
    }
    
    positions
}

/// SVGパスから形状ハッシュを計算する（実際の形状データに基づく）
fn calculate_shape_hash_from_svg(svg_path: &str) -> String {
    // SVGパスの長さと複雑さに基づいて簡単なハッシュを計算
    // より正確にするには、実際の辺の形状を解析する必要がある
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    
    let mut hasher = DefaultHasher::new();
    svg_path.hash(&mut hasher);
    let hash_value = hasher.finish();
    
    // SVGパスから形状の特徴を抽出してより意味のあるハッシュを作成
    let _normalized_path = svg_path.replace(&[' ', '\n', '\t'][..], "").to_lowercase();
    format!("svg_{:x}", hash_value)
}

/// ピースの形状ハッシュを計算する（代替案：グリッド位置ベース、後でSVGベースに変更）
fn calculate_piece_shape_hash(grid_x: usize, grid_y: usize, _grid_width: usize, _grid_height: usize) -> String {
    // 一時的な実装：グリッド位置をそのまま使用
    // 後でSVGパスから実際の形状を解析するように変更する予定
    format!("temp_{}_{}", grid_x, grid_y)
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

/// メッシュから精密当たり判定用の形状データを抽出（レガシー関数、後方互換性のため残す）
pub fn extract_shape_data(mesh: &Mesh, grid_x: usize, grid_y: usize, grid_width: usize, grid_height: usize) -> PieceShape {
    let vertices = match mesh.attribute(Mesh::ATTRIBUTE_POSITION) {
        Some(VertexAttributeValues::Float32x3(positions)) => {
            positions.iter().map(|pos| [pos[0], pos[1]]).collect()
        }
        _ => {
            println!("Warning: Could not extract vertices from mesh, using fallback");
            vec![[0.0, 0.0], [100.0, 0.0], [0.0, 100.0], [100.0, 100.0]]
        }
    };
    
    let indices = match mesh.indices() {
        Some(Indices::U32(idx)) => idx.clone(),
        Some(Indices::U16(idx)) => idx.iter().map(|&i| i as u32).collect(),
        None => {
            println!("Warning: Could not extract indices from mesh, using fallback");
            vec![0, 1, 2, 1, 2, 3]
        }
    };
    
    // 形状ハッシュを計算（レガシー関数）
    let shape_hash = calculate_piece_shape_hash(grid_x, grid_y, grid_width, grid_height);
    
    PieceShape { 
        vertices, 
        indices,
        shape_hash, // 形状ハッシュを保存
    }
}

pub fn create_puzzle_pieces(
    commands: &mut Commands,
    _asset_server: &AssetServer,
    puzzle_config: &PuzzleConfig,
    puzzle_image: &PuzzleImage,
    meshes: &mut ResMut<Assets<Mesh>>,
    materials: &mut ResMut<Assets<ColorMaterial>>,
) {
    let (grid_width, grid_height) = puzzle_config.grid_size;
    
    // オリジナル画像サイズを使用（正規化なし）
    let display_width = puzzle_image.size.x;
    let display_height = puzzle_image.size.y;
    
    let piece_width = display_width / grid_width as f32;
    let piece_height = display_height / grid_height as f32;
    
    println!("🧩 Creating puzzle pieces: image_size({}x{}), grid({}x{}), piece_size({}x{})", 
        puzzle_image.size.x, puzzle_image.size.y, grid_width, grid_height, piece_width, piece_height);
    
    // ジグソー形状ジェネレータを初期化
    let mut shape_generator = JigsawShapeGenerator::new(
        (piece_width, piece_height),
        (grid_width, grid_height),
    );
    
    // 全ての形状を事前生成
    if let Err(e) = shape_generator.generate_all_shapes() {
        println!("Failed to generate jigsaw shapes: {}", e);
        return;
    }
    
    // 整列配置用のグリッド位置を事前生成
    let mut placement_positions = generate_placement_grid(grid_width, grid_height, piece_width, piece_height, display_width, display_height);
    
    // 順番をランダムにシャッフル
    use rand::seq::SliceRandom;
    placement_positions.shuffle(&mut rand::thread_rng());
    
    let mut position_index = 0;
    
    let _total_pieces = grid_width * grid_height;
    
    for y in 0..grid_height {
        for x in 0..grid_width {
            let piece_id = Uuid::new_v4();
            
            // 正しい位置を画面中央に配置（Y軸は上が正）
            let correct_x = (x as f32 - (grid_width as f32 - 1.0) / 2.0) * piece_width;
            let correct_y = ((grid_height as f32 - 1.0) / 2.0 - y as f32) * piece_height;
            let correct_position = Vec2::new(correct_x, correct_y);
            
            // 事前生成された配置位置を使用
            let start_position = if position_index < placement_positions.len() {
                let pos = placement_positions[position_index];
                position_index += 1;
                pos
            } else {
                // フォールバック（通常は発生しない）
                println!("⚠️ Fallback position for piece ({}, {})", x, y);
                Vec2::new(0.0, 0.0)
            };
            
            
            // ジグソー形状を取得
            let shape = if let Some(shape_data) = shape_generator.get_shape(x, y) {
                shape_data
            } else {
                println!("Failed to get shape for piece ({}, {})", x, y);
                continue;
            };
            
            let texture_coords = Vec4::new(
                x as f32 / grid_width as f32,
                y as f32 / grid_height as f32,
                (x + 1) as f32 / grid_width as f32,
                (y + 1) as f32 / grid_height as f32,
            );
            
            // Create proper collision bounds based on piece size, not tight mesh bounds
            // Add margin to cover jigsaw tabs and blanks properly
            let margin_ratio = 1.3; // 30% larger to cover jigsaw tabs and blanks
            let half_width = (piece_width * margin_ratio) / 2.0;
            let half_height = (piece_height * margin_ratio) / 2.0;
            let collision_bounds = Rect::new(
                -half_width,
                -half_height,
                half_width,
                half_height
            );
            
            let piece = PuzzlePiece {
                id: piece_id,
                original_position: start_position,
                current_position: start_position, // Initial position should be the random start position, not correct position
                correct_position,
                texture_coords,
                is_placed: false, // Ensure pieces start as not placed
                grid_x: x,
                grid_y: y,
                bounds: collision_bounds, // Use full piece size for collision detection
            };
            
            // 各ピースに一意のZ値を設定（重なり順制御）
            let z_offset = (y * grid_width + x) as f32 * 0.001;
            
            // メッシュをクローンしてアセットに追加
            let mesh = clone_mesh_from_shape(shape);
            
            // JigsawPieceShapeから形状データを抽出してPieceShapeコンポーネント用に準備
            let piece_shape = extract_shape_data_from_jigsaw_shape(shape);
            
            // デバッグ情報を先に取得
            let _vertices_count = piece_shape.vertices.len();
            
            let mesh_handle = meshes.add(mesh);
            
            // ColorMaterialを作成（各ピースに個別のマテリアル）
            let material = ColorMaterial {
                texture: Some(puzzle_image.handle.clone()),
                ..default()
            };
            let material_handle = materials.add(material);
            
            
            // println!("Spawning 2D jigsaw piece at ({:.1}, {:.1}, {:.3})", start_position.x, start_position.y, z_offset);
            
            // 2D メッシュコンポーネントを使用してピースを生成
            // Picking systemを使用する場合
            let _entity = commands.spawn((
                Mesh2d(mesh_handle),
                MeshMaterial2d(material_handle),
                Transform::from_translation(start_position.extend(z_offset)),
                piece,
                piece_shape, // 精密当たり判定用の形状データ
                PickablePiece {
                    drag_offset: Vec2::ZERO,
                },
                // 旧システムとの互換性のため残す
                Draggable {
                    is_dragging: false,
                    drag_offset: Vec2::ZERO,
                },
            )).id();
        }
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