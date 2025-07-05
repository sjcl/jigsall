use bevy::prelude::*;
use bevy::input::mouse::MouseWheel;
use bevy::sprite::ColorMaterial;
use bevy::picking::events::{Pointer, Click, Drag, DragStart, DragEnd};
use crate::components::*;
use crate::resources::*;
use crate::puzzle::*;
use crate::jigsaw_shapes::JigsawShapeGenerator;
use rand::Rng;
use uuid::Uuid;
use std::path::Path;

pub fn update_input_state(
    mut input_state: ResMut<InputState>,
    windows: Query<&Window>,
    camera_q: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    mouse_input: Res<ButtonInput<MouseButton>>,
) {
    let Ok(window) = windows.single() else { return; };
    let Ok((camera, camera_transform)) = camera_q.single() else { return; };
    
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

// Picking system用の新しいドラッグハンドラー
pub fn on_piece_drag_start(
    trigger: Trigger<Pointer<DragStart>>,
    mut piece_query: Query<(&mut Transform, &mut PickablePiece, &PuzzlePiece)>,
    mut input_state: ResMut<InputState>,
    windows: Query<&Window>,
    camera_q: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
) {
    let Ok(window) = windows.single() else { return; };
    let Ok((camera, camera_transform)) = camera_q.single() else { return; };
    
    // マウス位置を取得
    if let Some(cursor_pos) = window.cursor_position() {
        if let Ok(world_pos) = camera.viewport_to_world_2d(camera_transform, cursor_pos) {
            input_state.mouse_position = world_pos;
            
            // ピースが選択された場合
            if let Ok((mut transform, mut pickable, piece)) = piece_query.get_mut(trigger.target()) {
                // ドラッグオフセットを計算
                let piece_pos = transform.translation.truncate();
                pickable.drag_offset = piece_pos - world_pos;
                
                // ピースを最前面に移動
                transform.translation.z = 100.0;
                
                // 選択されたピースを記録
                input_state.selected_piece = Some(trigger.target());
                
                println!("Piece drag started at: ({:.2}, {:.2})", world_pos.x, world_pos.y);
            }
        }
    }
}

pub fn on_piece_drag(
    trigger: Trigger<Pointer<Drag>>,
    mut piece_query: Query<(&mut Transform, &PickablePiece, &PuzzlePiece)>,
    mut input_state: ResMut<InputState>,
    windows: Query<&Window>,
    camera_q: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
) {
    let Ok(window) = windows.single() else { return; };
    let Ok((camera, camera_transform)) = camera_q.single() else { return; };
    
    // マウス位置を更新
    if let Some(cursor_pos) = window.cursor_position() {
        if let Ok(world_pos) = camera.viewport_to_world_2d(camera_transform, cursor_pos) {
            input_state.mouse_position = world_pos;
            
            // ピースを移動
            if let Ok((mut transform, pickable, piece)) = piece_query.get_mut(trigger.target()) {
                let new_pos = world_pos + pickable.drag_offset;
                transform.translation.x = new_pos.x;
                transform.translation.y = new_pos.y;
                transform.translation.z = 100.0;
            }
        }
    }
}

pub fn on_piece_drag_end(
    trigger: Trigger<Pointer<DragEnd>>,
    mut piece_query: Query<(&mut Transform, &PickablePiece, &PuzzlePiece)>,
    mut input_state: ResMut<InputState>,
) {
    if let Ok((mut transform, pickable, piece)) = piece_query.get_mut(trigger.target()) {
        // ドロップ時に新しいZ値を割り当て
        input_state.next_z_order += 1.0;
        transform.translation.z = input_state.next_z_order;
        
        // 選択解除
        input_state.selected_piece = None;
        
        println!("Piece dropped at: ({:.2}, {:.2})", transform.translation.x, transform.translation.y);
    }
}

// 旧システムは互換性のため残す（後で削除予定）
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
    // 旧システムは無効化 - picking systemを使用
    return;
}

pub fn check_piece_placement(
    mut commands: Commands,
    mut piece_query: Query<(Entity, &mut Transform, &mut PuzzlePiece), With<PickablePiece>>,
    puzzle_config: Res<PuzzleConfig>,
    input_state: Res<InputState>,
) {
    for (entity, mut transform, mut piece) in piece_query.iter_mut() {
        // 現在ドラッグ中でなく、かつまだ配置されていないピースのみチェック
        let is_currently_dragged = input_state.selected_piece == Some(entity);
        if !is_currently_dragged && !piece.is_placed {
            let current_pos = transform.translation.truncate();
            let correct_pos = piece.correct_position;
            let distance = current_pos.distance(correct_pos);
            
            if distance < puzzle_config.snap_distance {
                transform.translation = correct_pos.extend(-20.0); // 固定ピースは最も下のZ値
                piece.is_placed = true;
                piece.current_position = correct_pos;
                
                // PickablePieceコンポーネントを削除して移動不可にする
                commands.entity(entity).remove::<PickablePiece>();
                
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
    existing_grid_ref: Query<&GridReference>,
    images: Res<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    if game_state.current_screen == GameScreen::InGame && existing_pieces.is_empty() {
        // 画像パスが設定されている場合
        if !puzzle_config.image_path.is_empty() {
            // PuzzleImageリソースがまだない場合は作成
            if puzzle_image.is_none() {
                // ファイルの存在確認
                if !std::path::Path::new(&puzzle_config.image_path).exists() {
                    println!("Error: Image file does not exist: {}", puzzle_config.image_path);
                    return;
                }
                
                // Bevyアセットシステム用のパス変換
                let asset_path = if Path::new(&puzzle_config.image_path).is_absolute() {
                    // 絶対パスの場合は、assetsフォルダにコピーして相対パスを使用
                    let source_path = Path::new(&puzzle_config.image_path);
                    let file_name = source_path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("puzzle_image.png");
                    
                    let assets_dir = Path::new("assets");
                    let dest_path = assets_dir.join(file_name);
                    
                    // assetsディレクトリが存在しない場合は作成
                    if !assets_dir.exists() {
                        if let Err(e) = std::fs::create_dir_all(assets_dir) {
                            println!("Failed to create assets directory: {}", e);
                            return;
                        }
                    }
                    
                    // ファイルをassetsフォルダにコピー
                    if let Err(e) = std::fs::copy(source_path, &dest_path) {
                        println!("Failed to copy image to assets folder: {}", e);
                        return;
                    }
                    
                    println!("Copied image to: {:?}", dest_path);
                    file_name.to_string()
                } else {
                    puzzle_config.image_path.clone()
                };
                
                println!("Original path: {}", puzzle_config.image_path);
                println!("Asset path: {}", asset_path);
                
                let image_handle = asset_server.load(&asset_path);
                println!("Loading image with handle: {:?}", image_handle);
                
                // 読み込み直後の状態もチェック
                let initial_state = asset_server.load_state(&image_handle);
                println!("Initial load state: {:?}", initial_state);
                
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
        
        // 半透明の元画像をグリッドの正しい位置に表示（まだ存在しない場合のみ）
        if existing_grid_ref.is_empty() {
            spawn_grid_reference(&mut commands, &asset_server, &puzzle_config);
        }
    }
}

fn spawn_grid_reference(
    commands: &mut Commands,
    asset_server: &Res<AssetServer>,
    puzzle_config: &PuzzleConfig,
) {
    // 選択された画像を半透明で表示
    if !puzzle_config.image_path.is_empty() {
        // メインの画像読み込みと同じパス変換処理を適用
        let asset_path = if Path::new(&puzzle_config.image_path).is_absolute() {
            // 絶対パスの場合は、ファイル名のみを使用（既にassetsフォルダにコピー済み）
            Path::new(&puzzle_config.image_path)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("puzzle_image.png")
                .to_string()
        } else {
            puzzle_config.image_path.clone()
        };
        
        println!("Loading grid reference image: {}", asset_path);
        let texture_handle = asset_server.load(&asset_path);
        
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
        if let Ok(window) = windows.single() {
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
    
    let Ok(window) = windows.single() else { return; };
    
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