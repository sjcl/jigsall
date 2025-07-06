use bevy::prelude::*;
use bevy::sprite::ColorMaterial;
use bevy::picking::Pickable;
use bevy::render::mesh::{Indices, VertexAttributeValues};
use crate::components::*;
use crate::resources::*;
use crate::jigsaw_shapes::{JigsawShapeGenerator, clone_mesh_from_shape};
use uuid::Uuid;
use rand::Rng;

/// 整列配置用のグリッド位置を生成
fn generate_placement_grid(
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
    
    // 配置エリアのマージン
    let margin = piece_width.max(piece_height) + 100.0;
    
    // 4つの配置エリアを定義（左、右、上、下）
    let extension_x = piece_width * 8.0; // X方向の拡張幅
    let extension_y = piece_height * 6.0; // Y方向の拡張幅
    
    let areas = [
        // 左エリア
        (-grid_half_width - margin - extension_x, -grid_half_width - margin, 
         -grid_half_height, grid_half_height),
        // 右エリア  
        (grid_half_width + margin, grid_half_width + margin + extension_x,
         -grid_half_height, grid_half_height),
        // 上エリア
        (-grid_half_width, grid_half_width,
         grid_half_height + margin, grid_half_height + margin + extension_y),
        // 下エリア
        (-grid_half_width, grid_half_width,
         -grid_half_height - margin - extension_y, -grid_half_height - margin),
    ];
    
    println!("📐 Grid bounds: ({:.1}, {:.1}) to ({:.1}, {:.1})", 
        -grid_half_width, -grid_half_height, grid_half_width, grid_half_height);
    println!("📐 Extension: X={:.1}, Y={:.1}, Margin={:.1}", extension_x, extension_y, margin);
    
    // 各エリアに配置するピース数を計算
    let pieces_per_area = total_pieces / 4;
    let remaining_pieces = total_pieces % 4;
    
    println!("📐 Generating placement grid for {} pieces", total_pieces);
    println!("    Pieces per area: {}, Remaining: {}", pieces_per_area, remaining_pieces);
    
    for (area_idx, &(min_x, max_x, min_y, max_y)) in areas.iter().enumerate() {
        let area_pieces = pieces_per_area + if area_idx < remaining_pieces { 1 } else { 0 };
        
        if area_pieces == 0 { continue; }
        
        // エリア内でのグリッド配置を計算
        let area_width = max_x - min_x;
        let area_height = max_y - min_y;
        
        let cols = ((area_width / piece_width).floor() as usize).max(1);
        let rows = (area_pieces as f32 / cols as f32).ceil() as usize;
        
        let spacing_x = area_width / cols as f32;
        let spacing_y = area_height / rows as f32;
        
        println!("    Area {}: {}x{} grid, spacing: ({:.1}, {:.1}), bounds: ({:.1},{:.1}) to ({:.1},{:.1})", 
            area_idx, cols, rows, spacing_x, spacing_y, min_x, min_y, max_x, max_y);
        
        for i in 0..area_pieces {
            let col = i % cols;
            let row = i / cols;
            
            let x = min_x + (col as f32 + 0.5) * spacing_x;
            let y = min_y + (row as f32 + 0.5) * spacing_y;
            
            positions.push(Vec2::new(x, y));
        }
    }
    
    println!("📐 Generated {} placement positions", positions.len());
    positions
}

/// メッシュから精密当たり判定用の形状データを抽出
fn extract_shape_data(mesh: &Mesh) -> PieceShape {
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
    
    PieceShape { vertices, indices }
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
    
    println!("Original image size: {}x{}, Display size: {}x{}, Piece size: {}x{}", 
        puzzle_image.size.x, puzzle_image.size.y, display_width, display_height, piece_width, piece_height);
    
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
    
    let total_pieces = grid_width * grid_height;
    
    for y in 0..grid_height {
        for x in 0..grid_width {
            let piece_id = Uuid::new_v4();
            
            // 正しい位置を画面中央に配置（Y軸は上が正）
            let correct_x = (x as f32 - (grid_width as f32 - 1.0) / 2.0) * piece_width;
            let correct_y = ((grid_height as f32 - 1.0) / 2.0 - y as f32) * piece_height;
            let correct_position = Vec2::new(correct_x, correct_y);
            
            println!("📍 Piece({},{}) correct position: ({:.1}, {:.1})", 
                x, y, correct_position.x, correct_position.y);
            
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
            
            // ピースの実際のスポーン位置をログ出力
            println!("🎯 Piece({},{}) spawned at: ({:.1}, {:.1}) [grid position {}]", 
                x, y, start_position.x, start_position.y, position_index - 1);
            
            // Debug: Check initial vs correct position distance for potential immediate snapping
            let distance_to_correct = start_position.distance(correct_position);
            
            // Debug: Only log first few pieces to verify placement
            if x <= 1 && y <= 1 {
                println!("Piece ({},{}) initial: ({:.1}, {:.1}), correct: ({:.1}, {:.1}), distance: {:.1} (snap_distance: {})", 
                    x, y, start_position.x, start_position.y, correct_position.x, correct_position.y, distance_to_correct, 50.0);
                if distance_to_correct < 50.0 {
                    println!("⚠️  WARNING: Piece will immediately snap! Distance {:.1} < snap_distance {}", distance_to_correct, 50.0);
                }
            }
            
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
            
            let piece = PuzzlePiece {
                id: piece_id,
                original_position: start_position,
                current_position: start_position, // Initial position should be the random start position, not correct position
                correct_position,
                texture_coords,
                is_placed: false, // Ensure pieces start as not placed
                grid_x: x,
                grid_y: y,
                bounds: shape.bounds,
            };
            
            // 各ピースに一意のZ値を設定（重なり順制御）
            let z_offset = (y * grid_width + x) as f32 * 0.001;
            
            // メッシュをクローンしてアセットに追加
            let mesh = clone_mesh_from_shape(shape);
            
            // メッシュから形状データを抽出してPieceShapeコンポーネント用に準備
            let piece_shape = extract_shape_data(&mesh);
            
            // デバッグ情報を先に取得
            let vertices_count = piece_shape.vertices.len();
            
            let mesh_handle = meshes.add(mesh);
            
            // ColorMaterialを作成（各ピースに個別のマテリアル）
            let material = ColorMaterial {
                texture: Some(puzzle_image.handle.clone()),
                ..default()
            };
            let material_handle = materials.add(material);
            
            // 100ピース超の場合の追加デバッグ
            if total_pieces > 100 && x == 0 && y == 0 {
                println!("🎨 Material debugging:");
                println!("    Image handle: {:?}", puzzle_image.handle);
                println!("    Material handle: {:?}", material_handle);
            }
            
            // println!("Spawning 2D jigsaw piece at ({:.1}, {:.1}, {:.3})", start_position.x, start_position.y, z_offset);
            
            // 2D メッシュコンポーネントを使用してピースを生成
            // Picking systemを使用する場合
            let entity = commands.spawn((
                Mesh2d(mesh_handle),
                MeshMaterial2d(material_handle),
                Transform::from_translation(start_position.extend(z_offset)),
                piece,
                piece_shape, // 精密当たり判定用の形状データ
                PickablePiece {
                    drag_offset: Vec2::ZERO,
                },
                // Pickable::default(), // 一時的に無効化してテスト
                // 旧システムとの互換性のため残す
                Draggable {
                    is_dragging: false,
                    drag_offset: Vec2::ZERO,
                },
            )).id();
            
            // 描画用Transform座標を確認（100ピース超の場合により詳細に）
            if total_pieces > 100 && x <= 3 && y <= 3 {
                println!("🎨 Entity {:?} Piece({},{}) Transform set to: ({:.1}, {:.1}, {:.3})", 
                    entity, x, y, start_position.x, start_position.y, z_offset);
                println!("    Mesh vertices: {}, Grid position: ({}, {})", 
                    vertices_count, x, y);
                println!("    Texture coords: ({:.3}, {:.3}) to ({:.3}, {:.3})",
                    texture_coords.x, texture_coords.y,
                    texture_coords.z, texture_coords.w);
            } else if total_pieces <= 100 && x <= 2 && y <= 2 {
                println!("🎨 Entity {:?} Piece({},{}) Transform set to: ({:.1}, {:.1}, {:.3})", 
                    entity, x, y, start_position.x, start_position.y, z_offset);
            }
            // 一時的に無効化してテスト
            // .observe(crate::systems::on_piece_drag_start)
            // .observe(crate::systems::on_piece_drag)
            // .observe(crate::systems::on_piece_drag_end)
            // .observe(crate::systems::on_piece_click)
            // .observe(crate::systems::on_piece_over)
            ;
        }
    }
}

pub fn load_puzzle_image(
    asset_server: &AssetServer,
    puzzle_config: &PuzzleConfig,
) -> Handle<Image> {
    asset_server.load(&puzzle_config.image_path)
}

pub fn setup_puzzle_from_image(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    puzzle_config: Res<PuzzleConfig>,
    mut images: ResMut<Assets<Image>>,
) {
    // 画像パスが設定されている場合のみ画像を読み込み
    if !puzzle_config.image_path.is_empty() {
        let image_handle = load_puzzle_image(&asset_server, &puzzle_config);
        
        // デフォルトサイズを設定（実際の画像サイズは後で更新）
        commands.insert_resource(PuzzleImage {
            handle: image_handle.clone(),
            size: Vec2::new(800.0, 600.0),
        });
    }
}

pub fn update_puzzle_image_size(
    mut puzzle_image: Option<ResMut<PuzzleImage>>,
    images: Res<Assets<Image>>,
    asset_server: Res<AssetServer>,
) {
    if let Some(mut puzzle_image) = puzzle_image {
        // Asset loading状態をチェック
        let load_state = asset_server.load_state(&puzzle_image.handle);
        
        // デバッグ用に毎回状態を出力（頻度を制限）
        static mut DEBUG_COUNTER: usize = 0;
        unsafe {
            DEBUG_COUNTER += 1;
            if DEBUG_COUNTER % 600 == 0 { // 600フレームに1回（10秒に1回程度）
                println!("Image handle: {:?}, Load state: {:?}", puzzle_image.handle, load_state);
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