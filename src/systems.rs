use bevy::prelude::*;
use bevy::input::mouse::MouseWheel;
use bevy::sprite::ColorMaterial;
use crate::components::*;
use crate::resources::*;
use crate::puzzle::*;
use crate::jigsaw_shapes::JigsawShapeGenerator;
use rand::Rng;
use uuid::Uuid;

pub fn update_input_state(
    mut input_state: ResMut<InputState>,
    windows: Query<&Window>,
    camera_q: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    mouse_input: Res<ButtonInput<MouseButton>>,
) {
    let Ok(window) = windows.get_single() else { return; };
    let Ok((camera, camera_transform)) = camera_q.get_single() else { return; };
    
    // 前のマウス位置を保存
    input_state.last_mouse_position = input_state.mouse_position;
    
    if let Some(cursor_pos) = window.cursor_position() {
        if let Ok(world_pos) = camera.viewport_to_world_2d(camera_transform, cursor_pos) {
            input_state.mouse_position = world_pos;
            // デバッグ: マウス座標変換を確認（頻繁すぎるので制限）
            if mouse_input.just_pressed(MouseButton::Left) {
                println!("Cursor: ({:.1}, {:.1}) -> World: ({:.1}, {:.1})", 
                    cursor_pos.x, cursor_pos.y, world_pos.x, world_pos.y);
            }
        }
    }
    
    input_state.is_mouse_pressed = mouse_input.pressed(MouseButton::Left);
}

pub fn handle_piece_dragging(
    _commands: Commands,
    mut piece_query: Query<(Entity, &mut Transform, &mut Draggable, &PuzzlePiece)>,
    mut input_state: ResMut<InputState>,
    mut game_state: ResMut<GameState>,
    mouse_input: Res<ButtonInput<MouseButton>>,
    puzzle_config: Res<PuzzleConfig>,
    puzzle_image: Option<Res<PuzzleImage>>,
    camera_query: Query<&Transform, (With<MainCamera>, Without<PuzzlePiece>)>,
) {
    let mouse_just_pressed = mouse_input.just_pressed(MouseButton::Left);
    let mouse_pressed = mouse_input.pressed(MouseButton::Left);
    let mouse_just_released = mouse_input.just_released(MouseButton::Left);
    
    // カメラのスケールを取得
    let camera_scale = if let Ok(camera_transform) = camera_query.get_single() {
        camera_transform.scale.x
    } else {
        1.0
    };
    
    // 現在ドラッグ中のピースがあるかチェック
    let mut current_dragging_piece: Option<Entity> = None;
    for (entity, _transform, draggable, _piece) in piece_query.iter() {
        if draggable.is_dragging {
            current_dragging_piece = Some(entity);
            break;
        }
    }
    
    // ピースサイズを計算（オリジナル画像サイズを使用）
    let (piece_width, piece_height) = if let Some(puzzle_image) = puzzle_image.as_ref() {
        let (grid_width, grid_height) = puzzle_config.grid_size;
        // オリジナル画像サイズを使用
        let display_width = puzzle_image.size.x;
        let display_height = puzzle_image.size.y;
        let piece_size = (display_width / grid_width as f32, display_height / grid_height as f32);
        
        // デバッグ: ピースサイズ情報を出力（一度だけ）
        if mouse_just_pressed {
            println!("Debug: Grid {}x{}, Original {}x{}, Piece size {}x{}, Camera scale {:.2}", 
                grid_width, grid_height, display_width, display_height, 
                piece_size.0, piece_size.1, camera_scale);
        }
        
        piece_size
    } else {
        (80.0, 80.0) // デフォルト値
    };
    
    // マウスがクリックされた瞬間かつ、他にドラッグ中のピースがない場合、かつカメラがドラッグ中でない場合のみ新しい選択を行う
    if mouse_just_pressed && current_dragging_piece.is_none() && !input_state.is_camera_dragging {
        let mut closest_piece: Option<(Entity, f32, f32)> = None;
        
        // デバッグ: 最初の数個のピースの距離を出力
        let mut piece_count = 0;
        
        for (entity, transform, draggable, piece) in piece_query.iter() {
            if draggable.is_dragging {
                continue;
            }
            
            let piece_pos = transform.translation.truncate();
            let distance_to_mouse = piece_pos.distance(input_state.mouse_position);
            
            // 最初の3個のピースの位置情報をデバッグ出力
            if piece_count < 3 {
                println!("Piece {} at ({:.1}, {:.1}), distance to mouse: {:.1}, bounds: {:.1}x{:.1}", 
                    piece_count, piece_pos.x, piece_pos.y, distance_to_mouse, piece_width, piece_height);
                piece_count += 1;
            }
            
            // ジグソー形状での当たり判定（ワールド座標系で計算）
            // マウス位置をピース座標系に変換
            let relative_mouse_pos = input_state.mouse_position - piece_pos;
            
            // 実際のジグソー形状のバウンディングボックスを使用
            let bounds = piece.bounds;
            let margin_factor = 1.1; // 10%マージンを追加
            
            // マージンを加えて拡大したバウンディングボックス
            let margin_x = bounds.width() * (margin_factor - 1.0) / 2.0;
            let margin_y = bounds.height() * (margin_factor - 1.0) / 2.0;
            let expanded_bounds = Rect::new(
                bounds.min.x - margin_x,
                bounds.min.y - margin_y,
                bounds.width() + margin_x * 2.0,
                bounds.height() + margin_y * 2.0,
            );
            
            let mouse_in_bounds = expanded_bounds.contains(relative_mouse_pos);
            
            // デバッグ: 距離が近い（1500以下）のピースの詳細を出力
            if distance_to_mouse < 1500.0 {
                println!("  Piece {} (close): relative ({:.1}, {:.1}), bounds {}x{}, in_bounds: {}", 
                    piece_count-1, relative_mouse_pos.x, relative_mouse_pos.y, 
                    bounds.width(), bounds.height(), mouse_in_bounds);
            }
            
            if mouse_in_bounds {
                let distance = piece_pos.distance(input_state.mouse_position);
                println!("Hit piece at ({:.1}, {:.1}), mouse at ({:.1}, {:.1}), relative ({:.1}, {:.1}), bounds {:.1}x{:.1}", 
                    piece_pos.x, piece_pos.y, 
                    input_state.mouse_position.x, input_state.mouse_position.y,
                    relative_mouse_pos.x, relative_mouse_pos.y,
                    bounds.width(), bounds.height());
                
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
            
            for (entity, mut transform, mut draggable, piece) in piece_query.iter_mut() {
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
    for (entity, mut transform, mut draggable, piece) in piece_query.iter_mut() {
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
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
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
                    create_puzzle_pieces(&mut commands, &asset_server, &puzzle_config, &puzzle_image, &mut meshes, &mut materials);
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
            Sprite {
                color: Color::srgb(1.0, 1.0, 1.0).with_alpha(0.3), // 半透明
                image: texture_handle,
                ..default()
            },
            Transform::from_translation(Vec3::new(0.0, 0.0, -10.0)), // 背景に配置
            // 参照画像としてマーク
            GridReference,
        ));
    }
}

pub fn auto_adjust_camera_zoom(
    mut camera_query: Query<&mut Transform, With<MainCamera>>,
    puzzle_image: Option<Res<PuzzleImage>>,
    windows: Query<&Window>,
) {
    if let Some(puzzle_image) = puzzle_image.as_ref() {
        if let Ok(window) = windows.get_single() {
            for mut transform in camera_query.iter_mut() {
                // 現在のスケールが1.0（初期状態）の場合のみ自動調整
                if (transform.scale.x - 1.0).abs() < 0.01 {
                    let window_width = window.width();
                    let window_height = window.height();
                    
                    // 画像がウィンドウに収まるように初期ズームを計算
                    let scale_x = window_width / puzzle_image.size.x * 0.8; // 80%のマージン
                    let scale_y = window_height / puzzle_image.size.y * 0.8;
                    let initial_scale = scale_x.min(scale_y).clamp(0.1, 5.0);
                    
                    transform.scale = Vec3::splat(initial_scale);
                    println!("Auto-adjusted camera zoom to {:.2} for image {}x{}", 
                        initial_scale, puzzle_image.size.x, puzzle_image.size.y);
                }
            }
        }
    }
}

pub fn handle_camera_zoom(
    mut scroll_evr: EventReader<MouseWheel>,
    mut camera_query: Query<&mut Transform, With<MainCamera>>,
) {
    for ev in scroll_evr.read() {
        for mut transform in camera_query.iter_mut() {
            let zoom_factor = if ev.y > 0.0 { 0.9 } else { 1.1 };
            
            // ズーム制限 (0.1倍から5.0倍まで - 大きな画像に対応)
            let current_scale = transform.scale.x;
            let new_scale = (current_scale * zoom_factor).clamp(0.1, 5.0);
            
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
    
    let Ok(window) = windows.get_single() else { return; };
    
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