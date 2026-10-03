use crate::components::*;
use crate::resources::*;
use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_egui::EguiContexts;

pub fn update_input_state(
    mut input: ResMut<InputState>,
    windows: Query<&Window, With<PrimaryWindow>>,
    cameras: Query<(&Camera, &Transform), With<MainCamera>>,
) {
    // Clear validity first, so a missing cursor/window or failed conversion
    // never reuses a stale world coordinate.
    input.mouse_position = None;
    input.cursor_screen_position = None;
    input.window_focused = false;
    let Ok(window) = windows.single() else {
        return;
    };
    input.window_focused = window.focused;
    if !window.focused {
        return;
    }
    let Some(cursor) = window.cursor_position() else {
        return;
    };
    input.cursor_screen_position = Some(cursor);
    let Ok((camera, transform)) = cameras.single() else {
        return;
    };
    // MainCamera is a root entity. Its current Transform includes this frame's
    // pan/zoom/edge scrolling; GlobalTransform propagates later in PostUpdate.
    input.mouse_position = camera
        .viewport_to_world_2d(&GlobalTransform::from(*transform), cursor)
        .ok()
        .filter(|point| point.is_finite());
}

/// ゲーム開始時にカメラズームを自動調整（解像度適応型・OnEnterで1回のみ実行）
pub fn auto_adjust_camera_zoom(
    mut camera_query: Query<&mut Transform, With<MainCamera>>,
    puzzle_image: Option<Res<PuzzleImage>>,
    windows: Query<&Window>,
) {
    let _span = info_span!("auto_adjust_camera_zoom").entered();
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

                transform.scale = Vec3::new(final_scale, final_scale, 1.0);

                // カメラを画像の中心に配置
                transform.translation.x = 0.0;
                transform.translation.y = 0.0;
            }
        }
    }
}

pub fn handle_camera_zoom(
    mut scroll_evr: MessageReader<MouseWheel>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut contexts: EguiContexts,
    ui_capture: Res<GameUiPointerCapture>,
    mut camera_query: Query<&mut Transform, With<MainCamera>>,
    puzzle_image: Option<Res<PuzzleImage>>,
    mut perf_monitor: ResMut<PerformanceMonitor>,
) {
    let _span = info_span!("handle_camera_zoom").entered();
    let start_time = perf_monitor.start_system_timing("handle_camera_zoom");

    let over_ui = ui_capture.over_hud
        || contexts
            .ctx_mut()
            .is_ok_and(|ctx| ctx.is_pointer_over_egui() || ctx.egui_wants_pointer_input());
    let focused = windows.single().is_ok_and(|window| window.focused);
    for ev in scroll_evr.read() {
        if !focused || over_ui || ev.y == 0.0 || !ev.y.is_finite() {
            continue;
        }
        let scroll_lines = match ev.unit {
            MouseScrollUnit::Line => ev.y,
            MouseScrollUnit::Pixel => ev.y / MouseScrollUnit::SCROLL_UNIT_CONVERSION_FACTOR,
        };
        // Preserve the 10% zoom-in step per line, with reciprocal zoom-out.
        // Exponentiation makes the result depend on distance, not event count.
        let zoom_factor = 0.9_f32.powf(scroll_lines);
        for mut transform in camera_query.iter_mut() {
            // 解像度に応じた適応的ズーム制限
            let (min_zoom, max_zoom) = if let Some(puzzle_image) = puzzle_image.as_ref() {
                let resolution_factor =
                    (puzzle_image.size.x * puzzle_image.size.y) / (1920.0 * 1080.0);

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

            // Zoom changes XY only; scaling Z would shrink the visible depth
            // range and clip pieces/outlines after bringing them to the front.
            transform.scale = Vec3::new(new_scale, new_scale, 1.0);

            // デバッグ出力（頻度制限）
        }
    }

    perf_monitor.end_system_timing("handle_camera_zoom", start_time);
}

pub fn handle_camera_drag(
    mut input_state: ResMut<InputState>,
    mut camera_query: Query<&mut Transform, With<MainCamera>>,
    mouse_input: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut contexts: EguiContexts,
    ui_capture: Res<GameUiPointerCapture>,
    mut perf_monitor: ResMut<PerformanceMonitor>,
) {
    let _span = info_span!("handle_camera_drag").entered();
    let start_time = perf_monitor.start_system_timing("handle_camera_drag");
    let mouse_just_pressed = mouse_input.just_pressed(MouseButton::Right);
    let mouse_pressed = mouse_input.pressed(MouseButton::Right);

    let Ok(window) = windows.single() else {
        perf_monitor.end_system_timing("handle_camera_drag", start_time);
        return;
    };

    let over_ui = ui_capture.over_hud
        || contexts
            .ctx_mut()
            .is_ok_and(|ctx| ctx.is_pointer_over_egui() || ctx.egui_wants_pointer_input());
    if !window.focused || !mouse_pressed || window.cursor_position().is_none() {
        input_state.is_camera_dragging = false;
        input_state.last_cursor_position = None;
        perf_monitor.end_system_timing("handle_camera_drag", start_time);
        return;
    }
    // Capture starts only with a fresh press on the game canvas.
    if mouse_just_pressed && !over_ui {
        input_state.is_camera_dragging = true;
        input_state.last_cursor_position = window.cursor_position();
    }

    // カメラドラッグ中の処理
    if input_state.is_camera_dragging && mouse_pressed {
        if let (Some(current_cursor), Some(last_cursor)) =
            (window.cursor_position(), input_state.last_cursor_position)
        {
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

    perf_monitor.end_system_timing("handle_camera_drag", start_time);
}

/// ピースドラッグ中の画面端カメラスクロール
pub fn handle_edge_scrolling(
    interaction: Res<crate::interaction::PieceInteraction>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut camera_query: Query<&mut Transform, With<MainCamera>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    time: Res<Time>,
) {
    let _span = info_span!("handle_edge_scrolling").entered();

    // ピースをドラッグ中でない場合はスキップ
    if !interaction.is_dragging() || !mouse.pressed(MouseButton::Left) {
        return;
    }

    let Ok(window) = windows.single() else {
        return;
    };
    if !window.focused {
        return;
    }
    let Some(cursor_pos) = window.cursor_position() else {
        return;
    };
    let Ok(mut camera_transform) = camera_query.single_mut() else {
        return;
    };

    // エッジ検出のマージン（ピクセル）
    const EDGE_MARGIN: f32 = 50.0;
    // 最大スクロール速度（ピクセル/秒）
    const MAX_SCROLL_SPEED: f32 = 500.0;

    let window_width = window.width();
    let window_height = window.height();

    let mut scroll_velocity = Vec2::ZERO;

    // 左端
    if cursor_pos.x < EDGE_MARGIN {
        let factor = 1.0 - (cursor_pos.x / EDGE_MARGIN);
        scroll_velocity.x = -MAX_SCROLL_SPEED * factor;
    }
    // 右端
    else if cursor_pos.x > window_width - EDGE_MARGIN {
        let factor = (cursor_pos.x - (window_width - EDGE_MARGIN)) / EDGE_MARGIN;
        scroll_velocity.x = MAX_SCROLL_SPEED * factor;
    }

    // 上端（Y座標は下が正）
    if cursor_pos.y < EDGE_MARGIN {
        let factor = 1.0 - (cursor_pos.y / EDGE_MARGIN);
        scroll_velocity.y = MAX_SCROLL_SPEED * factor;
    }
    // 下端
    else if cursor_pos.y > window_height - EDGE_MARGIN {
        let factor = (cursor_pos.y - (window_height - EDGE_MARGIN)) / EDGE_MARGIN;
        scroll_velocity.y = -MAX_SCROLL_SPEED * factor;
    }

    // スクロール速度が0でない場合のみカメラを移動
    if scroll_velocity.length_squared() > 0.0 {
        let scale_factor = camera_transform.scale.x;
        let movement = scroll_velocity * scale_factor * time.delta_secs();

        camera_transform.translation.x += movement.x;
        camera_transform.translation.y += movement.y;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::camera::{ComputedCameraValues, RenderTargetInfo};

    fn scroll_scale(unit: MouseScrollUnit, deltas: &[f32]) -> Vec3 {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<GameUiPointerCapture>()
            .init_resource::<PerformanceMonitor>()
            .init_resource::<bevy_egui::EguiUserTextures>()
            .add_message::<MouseWheel>()
            .add_systems(Update, handle_camera_zoom);
        let window = app
            .world_mut()
            .spawn((
                Window {
                    focused: true,
                    ..default()
                },
                PrimaryWindow,
            ))
            .id();
        let camera = app
            .world_mut()
            .spawn((Transform::default(), MainCamera))
            .id();
        for &y in deltas {
            app.world_mut().write_message(MouseWheel {
                unit,
                x: 0.0,
                y,
                window,
                phase: bevy::input::touch::TouchPhase::Moved,
            });
        }
        app.update();
        app.world().get::<Transform>(camera).unwrap().scale
    }

    #[test]
    fn wheel_zoom_respects_scroll_amount_and_unit() {
        for (unit, delta, expected) in [
            (MouseScrollUnit::Line, 1.0, 0.9),
            (MouseScrollUnit::Line, 3.0, 0.729),
            (MouseScrollUnit::Line, 0.5, 0.9486833),
            (MouseScrollUnit::Line, -1.0, 1.111_111),
            (MouseScrollUnit::Pixel, 100.0, 0.9),
            (MouseScrollUnit::Pixel, 1.0, 0.99894696),
            (MouseScrollUnit::Pixel, -1.0, 1.0010542),
        ] {
            let scale = scroll_scale(unit, &[delta]);
            assert!((scale.x - expected).abs() < 0.00001, "{unit:?}: {delta}");
            assert_eq!(scale.y, scale.x);
            assert_eq!(scale.z, 1.0);
        }
    }

    #[test]
    fn wheel_zoom_is_independent_of_event_count() {
        for direction in [1.0, -1.0] {
            let combined = scroll_scale(MouseScrollUnit::Line, &[3.0 * direction]);
            let lines = scroll_scale(MouseScrollUnit::Line, &[direction; 3]);
            let pixels = scroll_scale(MouseScrollUnit::Pixel, &[direction; 300]);
            assert!((combined - lines).length() < 0.00001);
            assert!((combined - pixels).length() < 0.0001);
        }
    }

    #[test]
    fn wheel_zoom_is_reversible() {
        for (unit, delta) in [(MouseScrollUnit::Line, 2.5), (MouseScrollUnit::Pixel, 25.0)] {
            let scale = scroll_scale(unit, &[delta, -delta]);
            assert!((scale - Vec3::ONE).length() < 0.00001);
        }
    }

    #[test]
    fn wheel_zoom_ignores_zero_and_non_finite_input() {
        for unit in [MouseScrollUnit::Line, MouseScrollUnit::Pixel] {
            assert_eq!(
                scroll_scale(unit, &[0.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY]),
                Vec3::ONE,
            );
        }
    }

    #[test]
    fn wheel_zoom_clamps_large_scrolls_without_scaling_depth() {
        for unit in [MouseScrollUnit::Line, MouseScrollUnit::Pixel] {
            assert_eq!(scroll_scale(unit, &[f32::MAX]), Vec3::new(0.1, 0.1, 1.0));
            assert_eq!(scroll_scale(unit, &[-f32::MAX]), Vec3::new(10.0, 10.0, 1.0));
        }
    }

    #[test]
    fn pointer_uses_current_camera_transform_and_clears_invalid_coordinates() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<InputState>()
            .add_systems(Update, update_input_state);
        let mut window = Window {
            resolution: bevy::window::WindowResolution::new(1000, 800),
            focused: true,
            ..default()
        };
        window.set_cursor_position(Some(Vec2::new(700.0, 250.0)));
        let window = app.world_mut().spawn((window, PrimaryWindow)).id();
        let camera = app
            .world_mut()
            .spawn((
                Camera {
                    computed: ComputedCameraValues {
                        clip_from_view: Mat4::orthographic_rh(
                            -500.0, 500.0, -400.0, 400.0, 0.0, 1000.0,
                        ),
                        target_info: Some(RenderTargetInfo {
                            physical_size: UVec2::new(1000, 800),
                            scale_factor: 1.0,
                        }),
                        ..default()
                    },
                    ..default()
                },
                Transform::from_xyz(50.0, -20.0, 0.0).with_scale(Vec3::splat(2.0)),
                // Deliberately stale: propagation has not run this frame.
                GlobalTransform::IDENTITY,
                MainCamera,
            ))
            .id();
        app.update();
        let point = app.world().resource::<InputState>().mouse_position.unwrap();
        assert!((point - Vec2::new(450.0, 280.0)).length() < 0.001);
        app.world_mut()
            .entity_mut(camera)
            .get_mut::<Camera>()
            .unwrap()
            .computed
            .target_info = None;
        app.update();
        assert!(app
            .world()
            .resource::<InputState>()
            .mouse_position
            .is_none());
        app.world_mut()
            .entity_mut(window)
            .get_mut::<Window>()
            .unwrap()
            .focused = false;
        app.update();
        let input = app.world().resource::<InputState>();
        assert!(!input.window_focused);
        assert!(input.cursor_screen_position.is_none());
        assert!(input.mouse_position.is_none());
    }
}
