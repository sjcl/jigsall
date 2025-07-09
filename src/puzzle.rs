use bevy::prelude::*;
use bevy::sprite::ColorMaterial;
use bevy::render::mesh::{Indices, VertexAttributeValues};
use crate::components::*;
use crate::resources::*;
use crate::jigsaw_shapes::{JigsawShapeGenerator, clone_mesh_from_shape};
use uuid::Uuid;

/// 整列配置用のグリッド位置を生成（同心円状にグリッドを囲む配置）
pub fn generate_placement_grid(
    grid_width: usize, 
    grid_height: usize, 
    piece_width: f32, 
    piece_height: f32,
    display_width: f32,
    display_height: f32
) -> Vec<Vec2> {
    let total_pieces = grid_width * grid_height;
    let mut positions = Vec::with_capacity(total_pieces);
    
    // グリッド境界を計算
    let grid_half_width = display_width / 2.0;
    let grid_half_height = display_height / 2.0;
    
    // ピース間のマージン（ピースサイズに応じた割合で設定）
    let margin_ratio = 0.6; // ピースサイズの60%をマージンとして使用
    let piece_margin_x = piece_width * margin_ratio;
    let piece_margin_y = piece_height * margin_ratio;
    let effective_piece_width = piece_width + piece_margin_x;
    let effective_piece_height = piece_height + piece_margin_y;
    
    // グリッドからの最小距離（グリッドの境界から十分離す）
    let min_distance = piece_width.max(piece_height) + 80.0;
    
    // 同心円状のレイヤーを作成
    let mut layer = 1;
    let mut pieces_placed = 0;
    
    while pieces_placed < total_pieces {
        // 現在のレイヤーの配置位置を計算
        let layer_positions = generate_layer_positions(
            grid_half_width, 
            grid_half_height, 
            effective_piece_width, 
            effective_piece_height, 
            layer, 
            min_distance
        );
        
        // このレイヤーに配置できるピース数を計算
        let remaining_pieces = total_pieces - pieces_placed;
        let pieces_to_place = remaining_pieces.min(layer_positions.len());
        
        // 距離順にソート（グリッド中心に近い順）
        let mut sorted_positions = layer_positions;
        sorted_positions.sort_by(|a, b| {
            let dist_a = a.length_squared();
            let dist_b = b.length_squared();
            dist_a.partial_cmp(&dist_b).unwrap_or(std::cmp::Ordering::Equal)
        });
        
        // 必要な数だけ配置
        for i in 0..pieces_to_place {
            positions.push(sorted_positions[i]);
        }
        
        pieces_placed += pieces_to_place;
        layer += 1;
        
        // 無限ループ防止（より大きな値に設定）
        if layer > 1000 {
            println!("Error: Excessive layers needed ({}), something went wrong", layer);
            break;
        }
        
        // 進捗ログ（大量ピースの場合）
        if total_pieces > 1000 && layer % 10 == 0 {
            println!("📍 Placement progress: layer {}, placed {}/{} pieces", 
                layer, pieces_placed, total_pieces);
        }
    }
    
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