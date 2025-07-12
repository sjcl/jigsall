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
        piece_width, piece_height,
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

/// 同心円レイヤー方式でピース位置を生成（グリッド回避）
fn generate_spiral_positions(
    num_pieces: usize,
    piece_width: f32,
    piece_height: f32,
    puzzle_area: &(Vec2, Vec2),
    display_width: f32,
    display_height: f32
) -> Vec<Vec2> {
    let _span = info_span!("generate_spiral_positions").entered();
    let mut positions = Vec::new();
    let (puzzle_min, puzzle_max) = puzzle_area;
    
    // パズルの中心を配置の中心とする（画像の中心 = 原点）
    let center = Vec2::new(0.0, 0.0);
    
    // 除外エリアのサイズを計算
    let exclusion_width = puzzle_max.x - puzzle_min.x;
    let exclusion_height = puzzle_max.y - puzzle_min.y;
    
    // ピースサイズとマージン設定
    let piece_size = piece_width.max(piece_height);
    let margin_factor = if piece_size > 500.0 { 0.3 } else { 0.5 };
    let margin_x = piece_width * margin_factor;
    let margin_y = piece_height * margin_factor;
    let effective_piece_width = piece_width + margin_x;
    let effective_piece_height = piece_height + margin_y;
    
    // 初期半径を除外エリアの外側から開始
    let exclusion_half_width = exclusion_width / 2.0;
    let exclusion_half_height = exclusion_height / 2.0;
    let exclusion_diagonal = (exclusion_half_width * exclusion_half_width + exclusion_half_height * exclusion_half_height).sqrt();
    let initial_radius = exclusion_diagonal + piece_size;
    
    // レイヤー間隔とピース間隔
    let layer_spacing = piece_size * 1.1; // レイヤー間の距離
    let piece_spacing = piece_size * 0.9; // 同じレイヤー内のピース間距離
    
    println!("🎯 Generating concentric circle layers from center ({:.1}, {:.1})", center.x, center.y);
    println!("   📊 Exclusion area: ({:.1}, {:.1}) to ({:.1}, {:.1}), size: {:.1}x{:.1}", 
        puzzle_min.x, puzzle_min.y, puzzle_max.x, puzzle_max.y, exclusion_width, exclusion_height);
    println!("   🎯 Initial radius: {:.1}, Layer spacing: {:.1}, Piece spacing: {:.1}", 
        initial_radius, layer_spacing, piece_spacing);
    
    let mut current_layer = 0;
    let mut radius = initial_radius;
    let max_screen_distance = (display_width.max(display_height)) * 2.0;
    
    // 各レイヤーを順次生成
    while positions.len() < num_pieces && radius <= max_screen_distance {
        // 残り必要なピース数を計算
        let remaining_pieces = num_pieces - positions.len();
        
        let layer_positions = generate_circle_layer(
            center,
            radius,
            piece_spacing,
            puzzle_area,
            effective_piece_width,
            effective_piece_height,
            &positions,
            max_screen_distance,
            current_layer,
            remaining_pieces  // 残り必要数を渡す
        );
        
        let placed_count = layer_positions.len();
        positions.extend(layer_positions);
        
        println!("   ⭕ Layer {}: radius {:.1}, placed {} pieces, total: {}/{}", 
            current_layer, radius, placed_count, positions.len(), num_pieces);
        
        // 目標数に達したら完了
        if positions.len() >= num_pieces {
            println!("✅ Target piece count reached, stopping at layer {}", current_layer);
            break;
        }
        
        // 次のレイヤーに移動
        current_layer += 1;
        radius += layer_spacing;
        
        // 安全チェック：レイヤー数が多すぎる場合は停止
        if current_layer > 50 {
            println!("⚠️ Maximum layer count reached, stopping");
            break;
        }
    }
    
    if positions.len() < num_pieces {
        println!("⚠️ Could only place {} of {} pieces in {} layers", 
            positions.len(), num_pieces, current_layer);
        println!("   📐 May need fallback placement for remaining {} pieces", 
            num_pieces - positions.len());
    } else {
        println!("✅ Successfully placed {} pieces in {} concentric layers", 
            positions.len(), current_layer);
    }
    
    positions
}

/// 指定半径の円周上にピースを配置する
fn generate_circle_layer(
    center: Vec2,
    radius: f32,
    piece_spacing: f32,
    puzzle_area: &(Vec2, Vec2),
    effective_piece_width: f32,
    effective_piece_height: f32,
    existing_positions: &[Vec2],
    max_screen_distance: f32,
    layer_index: usize,
    max_pieces_needed: usize,  // このレイヤーで必要な最大ピース数
) -> Vec<Vec2> {
    let mut layer_positions = Vec::new();
    
    // 円周の長さを計算
    let circumference = 2.0 * std::f32::consts::PI * radius;
    
    // この円周に配置できるピースの概算数
    let estimated_pieces = (circumference / piece_spacing) as usize;
    
    if estimated_pieces == 0 {
        return layer_positions;
    }
    
    // 実際に配置を試行する数（必要数と推定数の小さい方）
    let target_pieces = estimated_pieces.min(max_pieces_needed);
    
    // 実際の角度ステップを計算
    let angle_step = (2.0 * std::f32::consts::PI) / estimated_pieces as f32;
    
    // レイヤーごとに開始角度を少しずらす（均等分散のため）
    let start_angle = (layer_index as f32 * 0.1) % (2.0 * std::f32::consts::PI);
    
    println!("     🎯 Layer {} target: {} pieces (estimated: {}, needed: {})", 
        layer_index, target_pieces, estimated_pieces, max_pieces_needed);
    
    // 円周上の各位置にピースを配置試行
    for i in 0..estimated_pieces {
        // 必要数に達したら停止
        if layer_positions.len() >= target_pieces {
            println!("     ✅ Layer {} completed: reached target {} pieces", layer_index, target_pieces);
            break;
        }
        
        let angle = start_angle + (i as f32 * angle_step);
        let x = center.x + radius * angle.cos();
        let y = center.y + radius * angle.sin();
        let position = Vec2::new(x, y);
        
        // 各種チェック
        let distance_from_center = position.distance(Vec2::ZERO);
        if distance_from_center <= max_screen_distance {
            if !is_in_grid_area(position, puzzle_area, effective_piece_width, effective_piece_height) {
                if !is_overlapping_with_existing(position, existing_positions, effective_piece_width, effective_piece_height) {
                    if !is_overlapping_with_existing(position, &layer_positions, effective_piece_width, effective_piece_height) {
                        layer_positions.push(position);
                        
                        // 最初のいくつかのピースの詳細ログ
                        if layer_positions.len() <= 5 {
                            println!("     📍 Layer {} piece #{} at ({:.1}, {:.1}), angle: {:.1}°", 
                                layer_index, layer_positions.len(), x, y, angle.to_degrees());
                        }
                    }
                }
            }
        }
    }
    
    // レイヤー完了ログ
    if layer_positions.len() < target_pieces {
        println!("     ⚠️ Layer {} partial: placed {} of {} target pieces", 
            layer_index, layer_positions.len(), target_pieces);
    }
    
    layer_positions
}

/// 指定位置がグリッド領域内かどうかをチェック
fn is_in_grid_area(
    position: Vec2,
    puzzle_area: &(Vec2, Vec2),
    piece_width: f32,
    piece_height: f32
) -> bool {
    let (puzzle_min, puzzle_max) = puzzle_area;
    
    // ピースサイズに応じて適応的な余白を設定
    let piece_size = piece_width.max(piece_height);
    let margin_factor = if piece_size > 300.0 {
        0.8  // 大きなピース：80%マージン
    } else if piece_size > 150.0 {
        0.9  // 中サイズピース：90%マージン  
    } else {
        1.0  // 小さなピース：元の100%マージン
    };
    let margin = piece_size * margin_factor;
    
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
    // 最小距離を少し緩和（90%）
    let min_distance = piece_width.max(piece_height) * 0.9;
    
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
    
    // ピースサイズに応じてスペーシングを設定（元の1.2を基準に調整）
    let spacing_factor = if piece_size > 300.0 {
        1.1   // 大きなピース：10%スペース
    } else {
        1.2   // 標準：元の値
    };
    let spacing = piece_size * spacing_factor;
    
    println!("🔄 Fallback: attempting grid-based placement...");
    
    // 四方向に均等に配置するための準備
    let grid_margin = piece_size * 0.6;
    let pieces_per_side = (count + 3) / 4; // 各方向の最大ピース数
    
    // 各方向のエリア定義
    let areas = [
        // 上側エリア
        ("top", puzzle_min.x, puzzle_max.x, puzzle_max.y + grid_margin, spacing, true),
        // 下側エリア  
        ("bottom", puzzle_min.x, puzzle_max.x, puzzle_min.y - grid_margin, -spacing, true),
        // 右側エリア
        ("right", puzzle_max.x + grid_margin, spacing, puzzle_min.y, puzzle_max.y, false),
        // 左側エリア
        ("left", puzzle_min.x - grid_margin, -spacing, puzzle_min.y, puzzle_max.y, false),
    ];
    
    for (side_name, start_pos, step_or_end, fixed_coord, range_end, is_horizontal) in areas {
        if positions.len() >= count { break; }
        
        let pieces_this_side = (pieces_per_side).min(count - positions.len());
        println!("   📍 Placing {} pieces on {} side", pieces_this_side, side_name);
        
        if is_horizontal {
            // 水平配置（上下）
            let x_range = step_or_end - start_pos;
            let cols = (x_range / spacing).max(1.0) as usize;
            let rows = (pieces_this_side + cols - 1) / cols; // 切り上げ除算
            
            for row in 0..rows {
                for col in 0..cols {
                    if positions.len() >= count || (row * cols + col) >= pieces_this_side { break; }
                    
                    let x = start_pos + (col as f32 * spacing) + (spacing / 2.0);
                    let y = fixed_coord + (row as f32 * range_end);
                    let pos = Vec2::new(x, y);
                    
                    if pos.x.abs() <= display_width * 0.9 && pos.y.abs() <= display_height * 0.9 {
                        if !is_overlapping_with_all(pos, existing_positions, &positions, piece_size * 0.8) {
                            positions.push(pos);
                        }
                    }
                }
            }
        } else {
            // 垂直配置（左右）
            let y_range = range_end - fixed_coord;
            let rows = (y_range / spacing).max(1.0) as usize;
            let cols = (pieces_this_side + rows - 1) / rows; // 切り上げ除算
            
            for col in 0..cols {
                for row in 0..rows {
                    if positions.len() >= count || (col * rows + row) >= pieces_this_side { break; }
                    
                    let x = start_pos + (col as f32 * step_or_end);
                    let y = fixed_coord + (row as f32 * spacing) + (spacing / 2.0);
                    let pos = Vec2::new(x, y);
                    
                    if pos.x.abs() <= display_width * 0.9 && pos.y.abs() <= display_height * 0.9 {
                        if !is_overlapping_with_all(pos, existing_positions, &positions, piece_size * 0.8) {
                            positions.push(pos);
                        }
                    }
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
    piece_width: f32,
    piece_height: f32,
    display_width: f32,
    display_height: f32,
) -> (Vec2, Vec2) {
    // 画像サイズに応じた適応的マージン計算
    let piece_size = piece_width.max(piece_height);
    
    // 画像サイズとピース数の複合指標でマージンを決定
    let total_pieces_estimate = (display_width * display_height) / (piece_width * piece_height);
    
    // 基本マージン係数：ピース数が多いほど小さく
    let base_margin_factor = if total_pieces_estimate > 2000.0 {
        0.15  // 大量ピース：小さなマージン（4K画像の問題を解決するため調整）
    } else if total_pieces_estimate > 500.0 {
        0.3   // 多めピース：やや小さなマージン
    } else if total_pieces_estimate > 100.0 {
        0.5   // 標準ピース数：元の値
    } else {
        0.6   // 少ないピース：大きなマージン
    };
    
    // ピースサイズに応じてマージンを微調整
    let size_adjusted_margin = if piece_size > 400.0 {
        base_margin_factor * 0.8  // 大ピース：80%
    } else if piece_size > 200.0 {
        base_margin_factor * 0.9  // 中ピース：90%
    } else {
        base_margin_factor        // 小ピース：100%
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
    println!("   📏 Image: {:.0}x{:.0}, Piece: {:.0}x{:.0}, Est. pieces: {:.0}", 
        display_width, display_height, piece_width, piece_height, total_pieces_estimate);
    println!("   🎚️ Base margin factor: {:.3}, Size adjusted: {:.3}, Final margin: {:.1}", 
        base_margin_factor, size_adjusted_margin, margin);
    
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
    puzzle_config: Res<PuzzleConfig>,
    file_registry: Res<crate::asset_reader::ExternalFileRegistry>,
) {
    let _span = info_span!("update_puzzle_image_size").entered();
    if let Some(mut puzzle_image) = puzzle_image {
        // 外部ファイルかどうかチェック
        let is_external = file_registry.is_external_image_path(&puzzle_config.image_path);
        
        // デバッグ用に状態を出力（頻度制限）
        static mut DEBUG_COUNTER: usize = 0;
        unsafe {
            DEBUG_COUNTER += 1;
            if DEBUG_COUNTER % 300 == 0 { // 5秒に1回程度
                if is_external {
                    println!("🖼️ External image check: handle {:?}, current size: {:.0}x{:.0}", 
                        puzzle_image.handle.id(), puzzle_image.size.x, puzzle_image.size.y);
                } else {
                    let load_state = asset_server.load_state(&puzzle_image.handle);
                    println!("🖼️ Asset image loading: handle {:?}, state: {:?}, current size: {:.0}x{:.0}", 
                        puzzle_image.handle.id(), load_state, puzzle_image.size.x, puzzle_image.size.y);
                }
            }
        }
        
        if is_external {
            // 外部ファイルの場合：AssetServerの状態チェックをスキップして直接Imageをチェック
            if let Some(image) = images.get(&puzzle_image.handle) {
                let actual_size = image.size();
                let new_size = Vec2::new(actual_size.x as f32, actual_size.y as f32);
                
                // サイズが変更された場合のみ更新
                if puzzle_image.size != new_size {
                    println!("✅ External image size updated from {}x{} to {}x{}", 
                        puzzle_image.size.x, puzzle_image.size.y, new_size.x, new_size.y);
                    puzzle_image.size = new_size;
                }
            } else {
                unsafe {
                    if DEBUG_COUNTER % 120 == 0 {
                        println!("⚠️ External image not found in Assets<Image> - may still be loading");
                    }
                }
            }
        } else {
            // 通常のアセットファイルの場合：従来のAssetServer状態チェック
            let load_state = asset_server.load_state(&puzzle_image.handle);
            
            match load_state {
                bevy::asset::LoadState::Loaded => {
                    if let Some(image) = images.get(&puzzle_image.handle) {
                        // Bevy 0.16では image.size() メソッドを使用
                        let actual_size = image.size();
                        let new_size = Vec2::new(actual_size.x as f32, actual_size.y as f32);
                        
                        // サイズが変更された場合のみ更新
                        if puzzle_image.size != new_size {
                            println!("Updating asset image size from {}x{} to {}x{}", 
                                puzzle_image.size.x, puzzle_image.size.y, new_size.x, new_size.y);
                            puzzle_image.size = new_size;
                        }
                    } else {
                        println!("Asset image is loaded but not found in Assets<Image>");
                    }
                }
                bevy::asset::LoadState::Loading => {
                    // 頻度を制限してログ出力
                    unsafe {
                        if DEBUG_COUNTER % 120 == 0 {
                            println!("Asset image is still loading...");
                        }
                    }
                }
                bevy::asset::LoadState::Failed(_) => {
                    println!("Failed to load asset image!");
                }
                bevy::asset::LoadState::NotLoaded => {
                    // NotLoadedの場合は、再度読み込みを試行
                    unsafe {
                        if DEBUG_COUNTER % 120 == 0 {
                            println!("Asset image not loaded - this may indicate the image was not properly loaded by AssetServer");
                        }
                    }
                }
            }
        }
    }
}