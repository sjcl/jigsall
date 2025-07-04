use bevy::prelude::*;
use bevy::sprite::ColorMaterial;
use crate::components::*;
use crate::resources::*;
use crate::jigsaw_shapes::{JigsawShapeGenerator, clone_mesh_from_shape};
use uuid::Uuid;
use rand::Rng;

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
    
    // 配置済みピース位置を記録
    let mut placed_positions: Vec<Vec2> = Vec::new();
    
    for y in 0..grid_height {
        for x in 0..grid_width {
            let piece_id = Uuid::new_v4();
            
            // 正しい位置を画面中央に配置（Y軸は上が正）
            let correct_x = (x as f32 - (grid_width as f32 - 1.0) / 2.0) * piece_width;
            let correct_y = ((grid_height as f32 - 1.0) / 2.0 - y as f32) * piece_height;
            let correct_position = Vec2::new(correct_x, correct_y);
            
            let mut rng = rand::thread_rng();
            
            // 表示サイズに基づいたグリッド外への配置
            let grid_half_width = display_width / 2.0;
            let grid_half_height = display_height / 2.0;
            
            // ピースサイズに基づいてマージンを計算（ピースサイズの半分 + 固定値）
            let margin = piece_width.max(piece_height) / 2.0 + 50.0;
            
            // ピース数に応じて配置範囲を動的に計算
            let total_pieces = (grid_width * grid_height) as f32;
            let density_factor = (total_pieces / 16.0).sqrt().max(1.0); // 4x4を基準とした密度係数
            
            let base_extension = 200.0; // 基本の拡張距離
            let extension_x = base_extension * density_factor;
            let extension_y = base_extension * density_factor * 0.75; // Y方向は少し小さく
            
            let max_x = grid_half_width + margin + extension_x;
            let max_y = grid_half_height + margin + extension_y;
            
            let mut area = rng.gen_range(0..8); // 8方向に拡張
            let mut random_x;
            let mut random_y;
            
            // 重ならない位置を見つける（最大50回試行）
            let mut attempts = 0;
            let piece_spacing = piece_width.max(piece_height) * 1.2; // ピース間の最小距離
            
            loop {
                match area {
                    0 => {
                        // 左側
                        random_x = rng.gen_range(-max_x..-grid_half_width - margin);
                        random_y = rng.gen_range(-grid_half_height - margin..grid_half_height + margin);
                    },
                    1 => {
                        // 右側
                        random_x = rng.gen_range(grid_half_width + margin..max_x);
                        random_y = rng.gen_range(-grid_half_height - margin..grid_half_height + margin);
                    },
                    2 => {
                        // 上側
                        random_x = rng.gen_range(-grid_half_width - margin..grid_half_width + margin);
                        random_y = rng.gen_range(grid_half_height + margin..max_y);
                    },
                    3 => {
                        // 下側
                        random_x = rng.gen_range(-grid_half_width - margin..grid_half_width + margin);
                        random_y = rng.gen_range(-max_y..-grid_half_height - margin);
                    },
                    4 => {
                        // 左上（斜め）
                        random_x = rng.gen_range(-max_x..-grid_half_width - margin);
                        random_y = rng.gen_range(grid_half_height + margin..max_y);
                    },
                    5 => {
                        // 右上（斜め）
                        random_x = rng.gen_range(grid_half_width + margin..max_x);
                        random_y = rng.gen_range(grid_half_height + margin..max_y);
                    },
                    6 => {
                        // 左下（斜め）
                        random_x = rng.gen_range(-max_x..-grid_half_width - margin);
                        random_y = rng.gen_range(-max_y..-grid_half_height - margin);
                    },
                    _ => {
                        // 右下（斜め）
                        random_x = rng.gen_range(grid_half_width + margin..max_x);
                        random_y = rng.gen_range(-max_y..-grid_half_height - margin);
                    }
                }
                
                let candidate_position = Vec2::new(random_x, random_y);
                
                // 他のピースとの重なりをチェック
                let mut overlaps = false;
                for placed_pos in &placed_positions {
                    if candidate_position.distance(*placed_pos) < piece_spacing {
                        overlaps = true;
                        break;
                    }
                }
                
                // 重ならない位置が見つかったか、試行回数上限に達した場合は終了
                if !overlaps || attempts >= 50 {
                    break;
                }
                
                attempts += 1;
                
                // 試行回数が多くなったら別のエリアに変更
                if attempts % 10 == 0 {
                    area = rng.gen_range(0..8);
                }
            }
            
            let start_position = Vec2::new(random_x, random_y);
            placed_positions.push(start_position);
            
            println!("Image piece ({},{}) placed at ({:.1}, {:.1}), grid size: {}x{}", 
                x, y, random_x, random_y, grid_width, grid_height);
            
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
                current_position: start_position,
                correct_position,
                texture_coords,
                is_placed: false,
                grid_x: x,
                grid_y: y,
                bounds: shape.bounds,
            };
            
            // 各ピースに一意のZ値を設定（重なり順制御）
            let z_offset = (y * grid_width + x) as f32 * 0.001;
            
            // メッシュをクローンしてアセットに追加
            let mesh = clone_mesh_from_shape(shape);
            let mesh_handle = meshes.add(mesh);
            
            // ColorMaterialを作成（画像のテクスチャを使用）
            let material = ColorMaterial {
                texture: Some(puzzle_image.handle.clone()),
                ..default()
            };
            let material_handle = materials.add(material);
            
            println!("Spawning 2D jigsaw piece at ({:.1}, {:.1}, {:.3})", start_position.x, start_position.y, z_offset);
            
            // 2D メッシュコンポーネントを使用してピースを生成
            commands.spawn((
                Mesh2d(mesh_handle),
                MeshMaterial2d(material_handle),
                Transform::from_translation(start_position.extend(z_offset)),
                piece,
                Draggable {
                    is_dragging: false,
                    drag_offset: Vec2::ZERO,
                },
            ));
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
) {
    if let Some(mut puzzle_image) = puzzle_image {
        if let Some(image) = images.get(&puzzle_image.handle) {
            let actual_size = image.texture_descriptor.size;
            let new_size = Vec2::new(actual_size.width as f32, actual_size.height as f32);
            
            // サイズが変更された場合のみ更新
            if puzzle_image.size != new_size {
                println!("Updating image size from {}x{} to {}x{}", 
                    puzzle_image.size.x, puzzle_image.size.y, new_size.x, new_size.y);
                puzzle_image.size = new_size;
            }
        }
    }
}