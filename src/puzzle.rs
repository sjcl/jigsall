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
            let random_x = rng.gen_range(-400.0..400.0);
            let random_y = rng.gen_range(-300.0..300.0);
            let start_position = Vec2::new(random_x, random_y);
            
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
    let image_handle = load_puzzle_image(&asset_server, &puzzle_config);
    
    // デフォルトサイズを設定（実際の画像サイズは後で更新）
    commands.insert_resource(PuzzleImage {
        handle: image_handle.clone(),
        size: Vec2::new(800.0, 600.0),
    });
}

pub fn update_puzzle_image_size(
    mut puzzle_image: ResMut<PuzzleImage>,
    images: Res<Assets<Image>>,
) {
    if let Some(image) = images.get(&puzzle_image.handle) {
        let actual_size = image.texture_descriptor.size;
        puzzle_image.size = Vec2::new(actual_size.width as f32, actual_size.height as f32);
    }
}