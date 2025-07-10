use bevy::prelude::*;
use bevy::input::mouse::MouseWheel;
use crate::components::*;
use crate::resources::*;

pub fn update_input_state(
    mut input_state: ResMut<InputState>,
    windows: Query<&Window>,
    camera_q: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    mouse_input: Res<ButtonInput<MouseButton>>,
    mut perf_monitor: ResMut<PerformanceMonitor>,
) {
    let start_time = perf_monitor.start_system_timing("update_input_state");
    
    let Ok(window) = windows.single() else { 
        perf_monitor.end_system_timing("update_input_state", start_time);
        return; 
    };
    let Ok((camera, camera_transform)) = camera_q.single() else { 
        perf_monitor.end_system_timing("update_input_state", start_time);
        return; 
    };
    
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
    
    perf_monitor.end_system_timing("update_input_state", start_time);
}

/// ゲーム開始時にカメラズームを自動調整（解像度適応型・OnEnterで1回のみ実行）
pub fn auto_adjust_camera_zoom(
    mut camera_query: Query<&mut Transform, With<MainCamera>>,
    puzzle_image: Option<Res<PuzzleImage>>,
    windows: Query<&Window>,
) {
    if let Some(puzzle_image) = puzzle_image.as_ref() {
        if let Ok(window) = windows.single() {
            for mut transform in camera_query.iter_mut() {
                let window_width = window.width();
                let window_height = window.height();
                let image_width = puzzle_image.size.x;
                let image_height = puzzle_image.size.y;
                
                // 解像度に応じて適応的マージンを計算
                let resolution_factor = (image_width * image_height) / (1920.0 * 1080.0);
                let base_margin = 1.2; // 基本マージン
                let raw_margin: f32 = if resolution_factor > 4.0 {
                    // 4K以上の超高解像度
                    base_margin + 0.3
                } else if resolution_factor > 2.0 {
                    // 2K-4K解像度
                    base_margin + 0.2
                } else if resolution_factor > 1.0 {
                    // Full HD以上
                    base_margin + 0.1
                } else if resolution_factor > 0.5 {
                    // HD
                    base_margin
                } else {
                    // SD以下
                    base_margin - 0.1
                };
                let adaptive_margin = raw_margin.clamp(1.05, 1.8);
                
                // 画像とウィンドウのアスペクト比を考慮した最適スケール計算
                let image_aspect = image_width / image_height;
                let window_aspect = window_width / window_height;
                
                let scale_x = image_width / window_width;
                let scale_y = image_height / window_height;
                
                // アスペクト比差を考慮してスケール調整
                let optimal_scale = if (image_aspect - window_aspect).abs() < 0.1 {
                    // アスペクト比が近い場合：均等スケール
                    scale_x.max(scale_y)
                } else if image_aspect > window_aspect {
                    // 画像が横長：幅基準でスケール
                    scale_x * 1.1 // 横長画像には少し余裕を持たせる
                } else {
                    // 画像が縦長：高さ基準でスケール
                    scale_y * 1.1 // 縦長画像には少し余裕を持たせる
                };
                
                // 最終的なズーム値を計算（解像度適応マージン適用）
                let final_scale = (optimal_scale * adaptive_margin).clamp(0.1, 15.0);
                
                transform.scale = Vec3::splat(final_scale);
                
                // カメラを画像の中心に配置
                transform.translation.x = 0.0;
                transform.translation.y = 0.0;
                
                println!("🎥 Adaptive camera zoom: {:.2}x for {}x{} image (aspect: {:.2})", 
                    final_scale, image_width, image_height, image_aspect);
                println!("   Resolution factor: {:.2}, Adaptive margin: {:.2}", 
                    resolution_factor, adaptive_margin);
                println!("   Window: {}x{} (aspect: {:.2}), Scale factors: x={:.2}, y={:.2}", 
                    window_width, window_height, window_aspect, scale_x, scale_y);
            }
        }
    }
}

pub fn handle_camera_zoom(
    mut scroll_evr: EventReader<MouseWheel>,
    mut camera_query: Query<&mut Transform, With<MainCamera>>,
    puzzle_image: Option<Res<PuzzleImage>>,
    mut perf_monitor: ResMut<PerformanceMonitor>,
) {
    let start_time = perf_monitor.start_system_timing("handle_camera_zoom");
    
    for ev in scroll_evr.read() {
        for mut transform in camera_query.iter_mut() {
            let zoom_factor = if ev.y > 0.0 { 0.9 } else { 1.1 };
            
            // 解像度に応じた適応的ズーム制限
            let (min_zoom, max_zoom) = if let Some(puzzle_image) = puzzle_image.as_ref() {
                let resolution_factor = (puzzle_image.size.x * puzzle_image.size.y) / (1920.0 * 1080.0);
                
                if resolution_factor > 4.0 {
                    // 4K以上の高解像度画像
                    (0.05, 15.0)
                } else if resolution_factor > 2.0 {
                    // 2K-4K画像
                    (0.1, 12.0)
                } else if resolution_factor > 1.0 {
                    // Full HD以上
                    (0.2, 10.0)
                } else {
                    // HD以下
                    (0.3, 8.0)
                }
            } else {
                // デフォルト値
                (0.1, 10.0)
            };
            
            let current_scale = transform.scale.x;
            let new_scale = (current_scale * zoom_factor).clamp(min_zoom, max_zoom);
            
            transform.scale = Vec3::splat(new_scale);
            
            // デバッグ出力（頻度制限）
            static mut ZOOM_LOG_COUNT: usize = 0;
            unsafe {
                ZOOM_LOG_COUNT += 1;
                if ZOOM_LOG_COUNT % 5 == 0 {
                    println!("🔍 Camera zoom: {:.2} (limits: {:.2}-{:.1})", new_scale, min_zoom, max_zoom);
                }
            }
        }
    }
    
    perf_monitor.end_system_timing("handle_camera_zoom", start_time);
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