use crate::components::*;
use crate::resources::*;
use bevy::input::mouse::{AccumulatedMouseMotion, MouseScrollUnit, MouseWheel};
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};
use bevy_egui::EguiContexts;
use jigsall_core::PuzzleDefinition;
use jigsall_puzzle::{placement::placement_half_extents, procedural::MAX_TAB_DEPTH};

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

struct CameraZoomSettings {
    initial_scale: f32,
    min_scale: f32,
    max_scale: f32,
}

/// Initial framing and wheel limits share the same placement and viewport bounds.
fn camera_zoom_settings(
    image: Option<&PuzzleImage>,
    definition: Option<&PuzzleDefinition>,
    completed: bool,
    window_size: Vec2,
) -> Option<CameraZoomSettings> {
    if !window_size.is_finite() || window_size.min_element() <= 0.0 {
        return None;
    }
    let Some(image) = image else {
        return Some(CameraZoomSettings {
            initial_scale: 1.0,
            min_scale: 0.1,
            max_scale: 10.0,
        });
    };
    if image.logical_size.min_element() == 0 {
        return None;
    }
    let image_size = image.logical_size.as_vec2();
    let resolution_factor = image_size.element_product() / (1920.0 * 1080.0);
    let (margin, min_scale, max_scale): (f32, f32, f32) = if resolution_factor > 4.0 {
        (1.5, 0.05, 15.0)
    } else if resolution_factor > 2.0 {
        (1.4, 0.1, 12.0)
    } else if resolution_factor > 1.0 {
        (1.3, 0.2, 10.0)
    } else if resolution_factor > 0.5 {
        (1.2, 0.3, 8.0)
    } else {
        (1.1, 0.3, 8.0)
    };

    let mut framing_size = image_size;
    if let Some(definition) =
        definition.filter(|definition| !completed && definition.validate().is_ok())
    {
        let display_size = definition.image_size.as_vec2();
        let piece_size = display_size / definition.grid_size.as_vec2();
        let centers = placement_half_extents(
            definition.piece_count(),
            jigsall_puzzle::placement::placement_piece_size(definition),
            display_size,
        );
        let mut piece_half =
            piece_size * 0.5 + Vec2::splat(piece_size.min_element() * MAX_TAB_DEPTH);
        if definition.rotation_enabled {
            piece_half = Vec2::splat(piece_half.max_element());
        }
        framing_size = framing_size.max((centers + piece_half) * 2.0);
    }
    let aspect_margin =
        if (framing_size.x / framing_size.y - window_size.x / window_size.y).abs() < 0.1 {
            1.0
        } else {
            1.1
        };
    let initial_scale =
        ((framing_size / window_size).max_element() * margin * aspect_margin).max(0.1);
    Some(CameraZoomSettings {
        initial_scale,
        // Small images must also be able to keep their initial framing.
        min_scale: min_scale.min(initial_scale),
        // Allow zooming out beyond the full scatter, even in a small window.
        max_scale: max_scale.max(initial_scale * 2.0),
    })
}

/// Frame the initial scatter, or the image alone after completion, once on entry.
pub fn auto_adjust_camera_zoom(
    mut camera_query: Query<&mut Transform, With<MainCamera>>,
    puzzle_image: Option<Res<PuzzleImage>>,
    definition: Option<Res<PuzzleDefinition>>,
    game: Option<Res<GameData>>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let _span = info_span!("auto_adjust_camera_zoom").entered();
    let (Some(image), Ok(window)) = (puzzle_image.as_deref(), windows.single()) else {
        return;
    };
    let Some(settings) = camera_zoom_settings(
        Some(image),
        definition.as_deref(),
        game.as_ref().is_some_and(|game| game.puzzle_completed),
        Vec2::new(window.width(), window.height()),
    ) else {
        return;
    };
    for mut transform in &mut camera_query {
        transform.scale = Vec3::new(settings.initial_scale, settings.initial_scale, 1.0);
        transform.translation.x = 0.0;
        transform.translation.y = 0.0;
    }
}

#[allow(clippy::too_many_arguments)] // Explicit camera, puzzle and UI system resources.
pub fn handle_camera_zoom(
    mut scroll_evr: MessageReader<MouseWheel>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut contexts: EguiContexts,
    ui_capture: Res<GameUiPointerCapture>,
    mut camera_query: Query<&mut Transform, With<MainCamera>>,
    puzzle_image: Option<Res<PuzzleImage>>,
    definition: Option<Res<PuzzleDefinition>>,
    game: Option<Res<GameData>>,
    mut perf_monitor: ResMut<PerformanceMonitor>,
) {
    let _span = info_span!("handle_camera_zoom").entered();
    let start_time = perf_monitor.start_system_timing("handle_camera_zoom");

    let over_ui = ui_capture.over_hud
        || contexts
            .ctx_mut()
            .is_ok_and(|ctx| ctx.is_pointer_over_egui() || ctx.egui_wants_pointer_input());
    let window = windows.single().ok();
    let focused = window.is_some_and(|window| window.focused);
    let settings = window.and_then(|window| {
        camera_zoom_settings(
            puzzle_image.as_deref(),
            definition.as_deref(),
            game.as_ref().is_some_and(|game| game.puzzle_completed),
            Vec2::new(window.width(), window.height()),
        )
    });
    for ev in scroll_evr.read() {
        if !focused || over_ui || ev.y == 0.0 || !ev.y.is_finite() {
            continue;
        }
        let Some(settings) = settings.as_ref() else {
            continue;
        };
        let scroll_lines = match ev.unit {
            MouseScrollUnit::Line => ev.y,
            MouseScrollUnit::Pixel => ev.y / MouseScrollUnit::SCROLL_UNIT_CONVERSION_FACTOR,
        };
        // Preserve the 10% zoom-in step per line, with reciprocal zoom-out.
        // Exponentiation makes the result depend on distance, not event count.
        let zoom_factor = 0.9_f32.powf(scroll_lines);
        for mut transform in camera_query.iter_mut() {
            let current_scale = transform.scale.x;
            // Resizing can move the limits past an existing view. Approach the
            // new range smoothly instead of snapping on the next wheel event.
            let new_scale = (current_scale * zoom_factor).clamp(
                settings.min_scale.min(current_scale),
                settings.max_scale.max(current_scale),
            );

            // Zoom changes XY only; scaling Z would shrink the visible depth
            // range and clip pieces/outlines after bringing them to the front.
            transform.scale = Vec3::new(new_scale, new_scale, 1.0);
        }
    }

    perf_monitor.end_system_timing("handle_camera_zoom", start_time);
}

pub fn release_camera_drag(
    mut input: ResMut<InputState>,
    mut windows: Query<(&mut Window, &mut CursorOptions), With<PrimaryWindow>>,
) {
    input.end_camera_drag(windows.iter_mut());
}

#[allow(clippy::too_many_arguments)] // Explicit raw motion and window capture resources.
pub fn handle_camera_drag(
    mut input_state: ResMut<InputState>,
    mut camera_query: Query<&mut Transform, With<MainCamera>>,
    mouse_input: Res<ButtonInput<MouseButton>>,
    mouse_motion: Res<AccumulatedMouseMotion>,
    mut windows: Query<(&mut Window, &mut CursorOptions), With<PrimaryWindow>>,
    mut contexts: EguiContexts,
    ui_capture: Res<GameUiPointerCapture>,
    mut perf_monitor: ResMut<PerformanceMonitor>,
) {
    let _span = info_span!("handle_camera_drag").entered();
    let start_time = perf_monitor.start_system_timing("handle_camera_drag");
    let mouse_just_pressed = mouse_input.just_pressed(MouseButton::Right);
    let mouse_pressed = mouse_input.pressed(MouseButton::Right);

    let Ok((window, mut cursor)) = windows.single_mut() else {
        input_state.end_camera_drag(windows.iter_mut());
        perf_monitor.end_system_timing("handle_camera_drag", start_time);
        return;
    };

    let over_ui = ui_capture.over_hud
        || contexts
            .ctx_mut()
            .is_ok_and(|ctx| ctx.is_pointer_over_egui() || ctx.egui_wants_pointer_input());
    if !window.focused || !mouse_pressed {
        input_state.end_camera_drag([(window, cursor)]);
        perf_monitor.end_system_timing("handle_camera_drag", start_time);
        return;
    }
    // Capture starts only with a fresh press on the game canvas.
    if !input_state.is_camera_dragging {
        if mouse_just_pressed && !over_ui {
            if let Some(position) = window.cursor_position() {
                input_state.camera_drag_start_position = Some(position);
                input_state.is_camera_dragging = true;
                // Bevy retains its Locked -> Confined backend fallback.
                cursor.grab_mode = CursorGrabMode::Locked;
                cursor.visible = false;
            }
        }
        // This frame's motion can include movement before the press or grab.
        perf_monitor.end_system_timing("handle_camera_drag", start_time);
        return;
    }

    // Raw motion continues at the window edge and without absolute coordinates.
    // Its device-specific units do not depend on the window's display DPI.
    let movement = mouse_motion.delta;
    if movement.is_finite() {
        for mut transform in &mut camera_query {
            let movement = movement * transform.scale.x;
            transform.translation.x -= movement.x;
            transform.translation.y += movement.y;
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
mod drag_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::camera::{ComputedCameraValues, RenderTargetInfo};
    use bevy::ecs::system::RunSystemOnce;
    use jigsall_core::GENERATOR_VERSION;

    fn zoom_app() -> (App, Entity, Entity) {
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
        (app, window, camera)
    }

    fn scroll(app: &mut App, window: Entity, unit: MouseScrollUnit, deltas: &[f32]) {
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
    }

    fn scroll_scale(unit: MouseScrollUnit, deltas: &[f32]) -> Vec3 {
        let (mut app, window, camera) = zoom_app();
        scroll(&mut app, window, unit, deltas);
        app.world().get::<Transform>(camera).unwrap().scale
    }

    fn framed_app(
        image_size: UVec2,
        grid_size: UVec2,
        window_size: UVec2,
    ) -> (App, Entity, Entity) {
        let (mut app, window, camera) = zoom_app();
        app.insert_resource(PuzzleImage {
            handle: default(),
            logical_size: image_size,
            texture_size: image_size,
            opaque: true,
        })
        .insert_resource(PuzzleDefinition {
            generator_version: GENERATOR_VERSION,
            seed: 42,
            grid_size,
            image_size,
            snap_distance: 5.0,
            rotation_enabled: true,
        })
        .init_resource::<GameData>();
        app.world_mut()
            .get_mut::<Window>(window)
            .unwrap()
            .resolution = bevy::window::WindowResolution::new(window_size.x, window_size.y);
        *app.world_mut().get_mut::<Transform>(camera).unwrap() =
            Transform::from_xyz(45.0, -20.0, 500.0).with_scale(Vec3::splat(3.0));
        app.world_mut()
            .run_system_once(auto_adjust_camera_zoom)
            .unwrap();
        (app, window, camera)
    }

    #[test]
    fn camera_framing_ignores_local_texture_resolution() {
        let logical_size = UVec2::new(6000, 4000);
        let (mut app, _, camera) =
            framed_app(logical_size, UVec2::new(40, 25), UVec2::new(1280, 720));
        let expected = *app.world().get::<Transform>(camera).unwrap();
        for cap in [1024, 256] {
            app.world_mut().resource_mut::<PuzzleImage>().texture_size =
                jigsall_core::fit_image_size(logical_size, cap);
            app.world_mut()
                .run_system_once(auto_adjust_camera_zoom)
                .unwrap();
            assert_eq!(*app.world().get::<Transform>(camera).unwrap(), expected);
        }
    }

    #[test]
    fn initial_camera_frames_generated_scatter_including_tabs() {
        let image_size = UVec2::new(1920, 1080);
        let window_size = UVec2::new(1280, 720);
        for grid_size in [
            UVec2::ONE,
            UVec2::new(4, 4),
            UVec2::new(40, 25),
            UVec2::new(1000, 1),
            UVec2::new(1, 1000),
            UVec2::splat(1000),
        ] {
            let (app, _, camera) = framed_app(image_size, grid_size, window_size);
            let transform = app.world().get::<Transform>(camera).unwrap();
            assert_eq!(transform.translation, Vec3::new(0.0, 0.0, 500.0));
            assert_eq!(transform.scale.y, transform.scale.x);
            assert_eq!(transform.scale.z, 1.0);
            let visible_half = window_size.as_vec2() * transform.scale.x * 0.5;
            assert!((image_size.as_vec2() * 0.5).cmple(visible_half).all());
            let piece_size = image_size.as_vec2() / grid_size.as_vec2();
            let piece_half =
                piece_size * 0.5 + Vec2::splat(piece_size.min_element() * MAX_TAB_DEPTH);
            let states = DensePieceStates::generate(app.world().resource::<PuzzleDefinition>());
            for state in states.iter() {
                let half = if jigsall_core::decode_rotation(state.flags) & 1 == 0 {
                    piece_half
                } else {
                    Vec2::new(piece_half.y, piece_half.x)
                };
                assert!((state.position.abs() + half).cmple(visible_half).all());
            }
        }
    }

    #[test]
    fn first_wheel_after_auto_framing_never_jumps_to_a_resolution_limit() {
        for (image_size, grid_size, window_size) in [
            (UVec2::splat(4096), UVec2::new(4, 4), UVec2::new(320, 200)),
            (
                UVec2::new(8000, 100),
                UVec2::new(40, 1),
                UVec2::new(800, 600),
            ),
            (
                UVec2::new(100, 8000),
                UVec2::new(1, 40),
                UVec2::new(800, 600),
            ),
            (UVec2::new(40, 20), UVec2::new(4, 4), UVec2::new(1280, 720)),
        ] {
            for unit in [MouseScrollUnit::Line, MouseScrollUnit::Pixel] {
                for delta in [1.0, -1.0] {
                    let (mut app, window, camera) = framed_app(image_size, grid_size, window_size);
                    let initial = *app.world().get::<Transform>(camera).unwrap();
                    let lines = match unit {
                        MouseScrollUnit::Line => delta,
                        MouseScrollUnit::Pixel => {
                            delta / MouseScrollUnit::SCROLL_UNIT_CONVERSION_FACTOR
                        }
                    };
                    let factor = 0.9_f32.powf(lines);
                    scroll(&mut app, window, unit, &[delta]);
                    let transform = app.world().get::<Transform>(camera).unwrap();
                    let ratio = transform.scale.x / initial.scale.x;
                    assert!(ratio >= factor.min(1.0) - 0.00001);
                    assert!(ratio <= factor.max(1.0) + 0.00001);
                    // Zoom-out has room past the initial frame; zoom-in may be
                    // at the lower limit for a tiny image.
                    if delta < 0.0 || initial.scale.x > 0.3 {
                        assert!((ratio - factor).abs() < 0.00001);
                    }
                    assert_eq!(transform.translation, initial.translation);
                    assert_eq!(transform.scale.y, transform.scale.x);
                    assert_eq!(transform.scale.z, 1.0);
                }
            }
        }
    }

    #[test]
    fn scatter_zoom_out_remains_bounded_beyond_the_old_upper_limit() {
        let (mut app, window, camera) =
            framed_app(UVec2::splat(4096), UVec2::new(4, 4), UVec2::new(320, 200));
        let initial_scale = app.world().get::<Transform>(camera).unwrap().scale.x;
        assert!(initial_scale > 15.0);
        scroll(&mut app, window, MouseScrollUnit::Line, &[-f32::MAX; 2]);
        assert_eq!(
            app.world().get::<Transform>(camera).unwrap().scale,
            Vec3::new(initial_scale * 2.0, initial_scale * 2.0, 1.0)
        );
    }

    #[test]
    fn wheel_after_window_resize_approaches_new_limits_without_jumping() {
        let (mut app, window, camera) =
            framed_app(UVec2::splat(4096), UVec2::new(4, 4), UVec2::new(320, 200));
        let initial = *app.world().get::<Transform>(camera).unwrap();
        app.world_mut()
            .get_mut::<Window>(window)
            .unwrap()
            .resolution = bevy::window::WindowResolution::new(1920, 1080);
        scroll(&mut app, window, MouseScrollUnit::Line, &[1.0]);
        assert_eq!(
            app.world().get::<Transform>(camera).unwrap().scale,
            Vec3::new(initial.scale.x * 0.9, initial.scale.x * 0.9, 1.0)
        );
    }

    #[test]
    fn completed_camera_frames_image_and_keeps_wheel_zoom_consistent() {
        let (mut app, window, camera) = framed_app(
            UVec2::new(1600, 900),
            UVec2::new(4, 4),
            UVec2::new(1280, 720),
        );
        assert!(app.world().get::<Transform>(camera).unwrap().scale.x > 1.5);
        app.world_mut().resource_mut::<GameData>().puzzle_completed = true;
        app.world_mut()
            .run_system_once(auto_adjust_camera_zoom)
            .unwrap();
        assert_eq!(
            app.world().get::<Transform>(camera).unwrap().scale,
            Vec3::new(1.5, 1.5, 1.0)
        );
        scroll(&mut app, window, MouseScrollUnit::Line, &[-1.0]);
        let scale = app.world().get::<Transform>(camera).unwrap().scale;
        assert!((scale.x - 1.5 / 0.9).abs() < 0.00001);
        assert_eq!(scale.y, scale.x);
        assert_eq!(scale.z, 1.0);
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
