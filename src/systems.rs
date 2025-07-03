use bevy::prelude::*;
use bevy::input::mouse::MouseWheel;
use crate::components::*;
use crate::resources::*;
use crate::puzzle::*;
use rand::Rng;
use uuid::Uuid;

pub fn update_input_state(
    mut input_state: ResMut<InputState>,
    windows: Query<&Window>,
    camera_q: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    mouse_input: Res<ButtonInput<MouseButton>>,
) {
    let window = windows.single();
    let (camera, camera_transform) = camera_q.single();
    
    // 前のマウス位置を保存
    input_state.last_mouse_position = input_state.mouse_position;
    
    if let Some(cursor_pos) = window.cursor_position() {
        if let Some(world_pos) = camera.viewport_to_world_2d(camera_transform, cursor_pos) {
            input_state.mouse_position = world_pos;
        }
    }
    
    input_state.is_mouse_pressed = mouse_input.pressed(MouseButton::Left);
}

pub fn handle_piece_dragging(
    _commands: Commands,
    mut piece_query: Query<(Entity, &mut Transform, &mut Draggable, &PuzzlePiece, &Sprite)>,
    mut input_state: ResMut<InputState>,
    mut game_state: ResMut<GameState>,
    mouse_input: Res<ButtonInput<MouseButton>>,
) {
    let mouse_just_pressed = mouse_input.just_pressed(MouseButton::Left);
    let mouse_pressed = mouse_input.pressed(MouseButton::Left);
    let mouse_just_released = mouse_input.just_released(MouseButton::Left);
    
    // 現在ドラッグ中のピースがあるかチェック
    let mut current_dragging_piece: Option<Entity> = None;
    for (entity, _transform, draggable, _piece, _sprite) in piece_query.iter() {
        if draggable.is_dragging {
            current_dragging_piece = Some(entity);
            break;
        }
    }
    
    // マウスがクリックされた瞬間かつ、他にドラッグ中のピースがない場合、かつカメラがドラッグ中でない場合のみ新しい選択を行う
    if mouse_just_pressed && current_dragging_piece.is_none() && !input_state.is_camera_dragging {
        let mut closest_piece: Option<(Entity, f32, f32)> = None;
        
        for (entity, transform, draggable, _piece, sprite) in piece_query.iter() {
            if draggable.is_dragging {
                continue;
            }
            
            let piece_pos = transform.translation.truncate();
            
            // ピースサイズを動的に取得（Spriteのcustom_sizeから）
            let (piece_width, piece_height) = if let Some(custom_size) = sprite.custom_size {
                (custom_size.x, custom_size.y)
            } else {
                (80.0, 80.0) // デフォルト値
            };
            
            // 矩形範囲での当たり判定（ピースサイズぴったり）
            let half_width = piece_width / 2.0;
            let half_height = piece_height / 2.0;
            
            let mouse_in_bounds = 
                input_state.mouse_position.x >= piece_pos.x - half_width &&
                input_state.mouse_position.x <= piece_pos.x + half_width &&
                input_state.mouse_position.y >= piece_pos.y - half_height &&
                input_state.mouse_position.y <= piece_pos.y + half_height;
            
            if mouse_in_bounds {
                let distance = piece_pos.distance(input_state.mouse_position);
                // Z値が高い（より前面）ピースを優先、同じZ値なら距離が近いピースを選択
                match closest_piece {
                    None => closest_piece = Some((entity, distance, transform.translation.z)),
                    Some((_, closest_distance, closest_z)) => {
                        if transform.translation.z > closest_z || 
                           (transform.translation.z == closest_z && distance < closest_distance) {
                            closest_piece = Some((entity, distance, transform.translation.z));
                        }
                    }
                }
            }
        }
        
        // 選択されたピースをドラッグ開始
        if let Some((selected_entity, _, _)) = closest_piece {
            input_state.selected_piece = Some(selected_entity);
            
            for (entity, mut transform, mut draggable, _piece, _sprite) in piece_query.iter_mut() {
                if entity == selected_entity {
                    draggable.is_dragging = true;
                    let piece_pos = transform.translation.truncate();
                    draggable.drag_offset = piece_pos - input_state.mouse_position;
                    // ドラッグ開始時に最前面に移動
                    transform.translation.z = 100.0; // 十分に高い値
                    break;
                }
            }
        }
    }
    
    // ドラッグ中の処理
    for (entity, mut transform, mut draggable, _piece, _sprite) in piece_query.iter_mut() {
        if draggable.is_dragging {
            if mouse_pressed {
                let old_pos = transform.translation.truncate();
                let new_pos = input_state.mouse_position + draggable.drag_offset;
                let movement = new_pos - old_pos;
                
                // ピースを移動
                transform.translation.x = new_pos.x;
                transform.translation.y = new_pos.y;
                transform.translation.z = 100.0;
            } else if mouse_just_released {
                draggable.is_dragging = false;
                input_state.selected_piece = None;
                
                // ドロップ時に新しいZ値を割り当て（最前面に配置）
                input_state.next_z_order += 1.0;
                transform.translation.z = input_state.next_z_order;
                
                // ドロップ後の結合チェックのためのフラグ
                println!("Piece dropped at: ({:.2}, {:.2})", transform.translation.x, transform.translation.y);
            }
        }
    }
}

pub fn check_piece_placement(
    mut commands: Commands,
    mut piece_query: Query<(Entity, &mut Transform, &mut PuzzlePiece, &Draggable)>,
    puzzle_config: Res<PuzzleConfig>,
) {
    for (entity, mut transform, mut piece, draggable) in piece_query.iter_mut() {
        if !draggable.is_dragging && !piece.is_placed {
            let current_pos = transform.translation.truncate();
            let correct_pos = piece.correct_position;
            let distance = current_pos.distance(correct_pos);
            
            if distance < puzzle_config.snap_distance {
                transform.translation = correct_pos.extend(-20.0); // 固定ピースは最も下のZ値
                piece.is_placed = true;
                piece.current_position = correct_pos;
                
                // Draggableコンポーネントを削除して移動不可にする
                commands.entity(entity).remove::<Draggable>();
                
                println!("Piece placed and fixed at grid({},{})", piece.grid_x, piece.grid_y);
            }
        }
    }
}

pub fn update_game_state(
    mut game_state: ResMut<GameState>,
    piece_query: Query<&PuzzlePiece>,
) {
    let total_pieces = piece_query.iter().count();
    let placed_pieces = piece_query.iter().filter(|p| p.is_placed).count();
    
    if total_pieces > 0 {
        game_state.puzzle_progress = placed_pieces as f32 / total_pieces as f32;
        game_state.puzzle_completed = placed_pieces == total_pieces;
        
        if game_state.puzzle_completed && game_state.current_screen == GameScreen::InGame {
            game_state.current_screen = GameScreen::GameComplete;
        }
    }
}

pub fn spawn_puzzle_pieces(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    puzzle_config: Res<PuzzleConfig>,
    puzzle_image: Option<Res<PuzzleImage>>,
    game_state: Res<GameState>,
    existing_pieces: Query<&PuzzlePiece>,
    images: Res<Assets<Image>>,
) {
    if game_state.current_screen == GameScreen::InGame && existing_pieces.is_empty() {
        // 画像パスが設定されている場合
        if !puzzle_config.image_path.is_empty() {
            // PuzzleImageリソースがまだない場合は作成
            if puzzle_image.is_none() {
                let image_handle = asset_server.load(&puzzle_config.image_path);
                
                println!("Loading image: {}", puzzle_config.image_path);
                commands.insert_resource(PuzzleImage {
                    handle: image_handle,
                    size: Vec2::new(1.0, 1.0), // 初期値として1x1を設定（update_puzzle_image_sizeで更新される）
                });
                return; // 次フレームで再実行
            }
            
            if let Some(puzzle_image) = puzzle_image {
                // 画像サイズが正しく更新されている場合のみピースを作成
                if puzzle_image.size.x > 1.0 && puzzle_image.size.y > 1.0 {
                    println!("Creating pieces with image size: {}x{}", puzzle_image.size.x, puzzle_image.size.y);
                    create_puzzle_pieces(&mut commands, &asset_server, &puzzle_config, &puzzle_image);
                } else {
                    println!("Waiting for image size update: {}x{}", puzzle_image.size.x, puzzle_image.size.y);
                    return; // 画像サイズがまだ更新されていない
                }
            }
        } else {
            // 画像が選択されていない場合はエラーメッセージ
            println!("No image selected for puzzle creation!");
            return;
        }
        
        // 半透明の元画像をグリッドの正しい位置に表示
        spawn_grid_reference(&mut commands, &asset_server, &puzzle_config);
    }
}

fn spawn_grid_reference(
    commands: &mut Commands,
    asset_server: &Res<AssetServer>,
    puzzle_config: &PuzzleConfig,
) {
    // 選択された画像を半透明で表示
    if !puzzle_config.image_path.is_empty() {
        let texture_handle = asset_server.load(&puzzle_config.image_path);
        
        commands.spawn((
            SpriteBundle {
                sprite: Sprite {
                    color: Color::srgba(1.0, 1.0, 1.0, 0.3), // 半透明
                    ..default()
                },
                texture: texture_handle,
                transform: Transform::from_translation(Vec3::new(0.0, 0.0, -10.0)), // 背景に配置
                ..default()
            },
            // 参照画像としてマーク
            GridReference,
        ));
    }
}

pub fn handle_camera_zoom(
    mut scroll_evr: EventReader<MouseWheel>,
    mut camera_query: Query<&mut Transform, With<MainCamera>>,
) {
    for ev in scroll_evr.read() {
        for mut transform in camera_query.iter_mut() {
            let zoom_factor = if ev.y > 0.0 { 0.9 } else { 1.1 };
            
            // ズーム制限 (0.5倍から3.0倍まで)
            let current_scale = transform.scale.x;
            let new_scale = (current_scale * zoom_factor).clamp(0.5, 3.0);
            
            transform.scale = Vec3::splat(new_scale);
            
            println!("Camera zoom: {:.2}", new_scale);
        }
    }
}

pub fn handle_camera_drag(
    mut input_state: ResMut<InputState>,
    mut camera_query: Query<&mut Transform, With<MainCamera>>,
    mouse_input: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
) {
    let mouse_just_pressed = mouse_input.just_pressed(MouseButton::Right);
    let mouse_pressed = mouse_input.pressed(MouseButton::Right);
    let mouse_just_released = mouse_input.just_released(MouseButton::Right);
    
    let window = windows.single();
    
    // 右クリックでカメラドラッグ開始
    if mouse_just_pressed {
        input_state.is_camera_dragging = true;
        input_state.last_cursor_position = window.cursor_position();
    }
    
    // カメラドラッグ中の処理
    if input_state.is_camera_dragging && mouse_pressed {
        if let (Some(current_cursor), Some(last_cursor)) = (window.cursor_position(), input_state.last_cursor_position) {
            // スクリーン座標での移動量を計算
            let cursor_movement = current_cursor - last_cursor;
            
            // 移動量が0でない場合のみカメラを移動
            if cursor_movement.length() > 0.5 {
                for mut transform in camera_query.iter_mut() {
                    // カメラスケールを考慮した移動量
                    let scale_factor = transform.scale.x;
                    let movement = cursor_movement * scale_factor;
                    
                    // カメラの移動（マウスの動きと逆方向に移動、Y軸は反転）
                    transform.translation.x -= movement.x;
                    transform.translation.y += movement.y; // スクリーン座標系ではY軸が反転
                }
                
                // カーソル位置を更新
                input_state.last_cursor_position = Some(current_cursor);
            }
        }
    }
    
    // 右クリックリリースでカメラドラッグ終了
    if mouse_just_released {
        input_state.is_camera_dragging = false;
        input_state.last_cursor_position = None;
    }
}