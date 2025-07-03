use bevy::prelude::*;
use crate::components::*;
use crate::resources::*;
use uuid::Uuid;
use rand::Rng;

pub fn create_puzzle_pieces(
    commands: &mut Commands,
    _asset_server: &AssetServer,
    puzzle_config: &PuzzleConfig,
    puzzle_image: &PuzzleImage,
) {
    let (grid_width, grid_height) = puzzle_config.grid_size;
    let piece_width = puzzle_image.size.x / grid_width as f32;
    let piece_height = puzzle_image.size.y / grid_height as f32;
    
    for y in 0..grid_height {
        for x in 0..grid_width {
            let piece_id = Uuid::new_v4();
            
            // 正しい位置を画面中央に配置（Y軸は上が正）
            let correct_x = (x as f32 - (grid_width as f32 - 1.0) / 2.0) * piece_width;
            let correct_y = ((grid_height as f32 - 1.0) / 2.0 - y as f32) * piece_height;
            let correct_position = Vec2::new(correct_x, correct_y);
            
            let mut rng = rand::thread_rng();
            
            // 画像サイズに基づいたグリッド外への配置
            let grid_half_width = puzzle_image.size.x / 2.0;
            let grid_half_height = puzzle_image.size.y / 2.0;
            
            // ピースサイズに基づいてマージンを計算（ピースサイズの2倍 + 固定値）
            let margin = (piece_width.max(piece_height) / 2.0 + 50.0);
            
            // 配置可能な画面範囲を動的に計算
            let screen_margin = 100.0;
            let max_x = grid_half_width + margin + 400.0; // グリッド外 + 余裕
            let max_y = grid_half_height + margin + 300.0; // グリッド外 + 余裕
            
            let area = rng.gen_range(0..4);
            let random_x;
            let random_y;
            
            match area {
                0 => {
                    // 左側（グリッドの左端より左に配置）
                    random_x = rng.gen_range(-max_x..-grid_half_width - margin);
                    random_y = rng.gen_range(-grid_half_height - margin..grid_half_height + margin);
                },
                1 => {
                    // 右側（グリッドの右端より右に配置）
                    random_x = rng.gen_range(grid_half_width + margin..max_x);
                    random_y = rng.gen_range(-grid_half_height - margin..grid_half_height + margin);
                },
                2 => {
                    // 上側（グリッドの上端より上に配置）
                    random_x = rng.gen_range(-grid_half_width - margin..grid_half_width + margin);
                    random_y = rng.gen_range(grid_half_height + margin..max_y);
                },
                _ => {
                    // 下側（グリッドの下端より下に配置）
                    random_x = rng.gen_range(-grid_half_width - margin..grid_half_width + margin);
                    random_y = rng.gen_range(-max_y..-grid_half_height - margin);
                }
            }
            
            let start_position = Vec2::new(random_x, random_y);
            
            println!("Image piece ({},{}) placed at ({:.1}, {:.1}), grid size: {}x{}", 
                x, y, random_x, random_y, grid_width, grid_height);
            
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
            };
            
            // 各ピースに一意のZ値を設定（重なり順制御）
            let z_offset = (y * grid_width + x) as f32 * 0.001;
            
            commands.spawn((
                SpriteBundle {
                    texture: puzzle_image.handle.clone(),
                    transform: Transform::from_translation(start_position.extend(z_offset)),
                    sprite: Sprite {
                        rect: Some(Rect::new(
                            x as f32 * piece_width,
                            y as f32 * piece_height,
                            (x + 1) as f32 * piece_width,
                            (y + 1) as f32 * piece_height,
                        )),
                        custom_size: Some(Vec2::new(piece_width, piece_height)),
                        ..default()
                    },
                    ..default()
                },
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