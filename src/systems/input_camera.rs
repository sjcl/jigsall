use bevy::prelude::*;
use bevy::input::mouse::MouseWheel;
use crate::components::*;
use crate::resources::*;

/// カメラのズーム・パンを考慮してスクリーン座標をワールド座標に変換
pub(super) fn screen_to_world_pos(screen_pos: Vec2, camera_transform: &Transform) -> Vec2 {
    // Bevyでは、カメラの逆変換を使用してスクリーン座標をワールド座標に変換
    // スケールが小さい = ズームイン、大きい = ズームアウト
    let scale = camera_transform.scale.x;
    let camera_translation = camera_transform.translation.truncate();
    
    // 正しい逆変換: (screen_pos - screen_center) / scale + camera_position
    // ただし、Bevyの座標系を考慮
    screen_pos / scale + camera_translation
}

/// ワールド座標をカメラのズーム・パンを考慮してスクリーン座標に変換
pub(super) fn world_to_screen_pos(world_pos: Vec2, camera_transform: &Transform) -> Vec2 {
    let scale = camera_transform.scale.x;
    let camera_translation = camera_transform.translation.truncate();
    
    (world_pos - camera_translation) * scale
}

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