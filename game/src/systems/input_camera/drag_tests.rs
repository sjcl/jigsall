use super::*;
use bevy::input::{
    mouse::{MouseButtonInput, MouseMotion},
    ButtonState, InputPlugin,
};
use bevy_egui::{egui, EguiContext, PrimaryEguiContext};

const START: Vec2 = Vec2::new(200.0, 150.0);

fn drag_app(dpi_scale: f32, camera_scale: f32) -> (App, Entity, Entity) {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, InputPlugin))
        .init_resource::<InputState>()
        .init_resource::<GameUiPointerCapture>()
        .init_resource::<PerformanceMonitor>()
        .init_resource::<bevy_egui::EguiUserTextures>()
        .add_systems(PostUpdate, handle_camera_drag);
    let mut window = Window {
        focused: true,
        resolution: bevy::window::WindowResolution::new(1280, 720)
            .with_scale_factor_override(dpi_scale),
        ..default()
    };
    window.set_cursor_position(Some(START));
    let window = app.world_mut().spawn((window, PrimaryWindow)).id();
    let camera = app
        .world_mut()
        .spawn((
            Transform::from_xyz(50.0, -20.0, 500.0).with_scale(Vec3::new(
                camera_scale,
                camera_scale,
                1.0,
            )),
            MainCamera,
        ))
        .id();
    (app, window, camera)
}

fn frame(app: &mut App, window: Entity, state: Option<ButtonState>, delta: Vec2) {
    if let Some(state) = state {
        app.world_mut().write_message(MouseButtonInput {
            button: MouseButton::Right,
            state,
            window,
        });
    }
    app.world_mut().write_message(MouseMotion { delta });
    app.update();
}

fn assert_released(app: &App, window: Entity) {
    let input = app.world().resource::<InputState>();
    assert!(!input.is_camera_dragging);
    assert!(input.camera_drag_start_position.is_none());
    let cursor = app.world().get::<CursorOptions>(window).unwrap();
    assert_eq!(cursor.grab_mode, CursorGrabMode::None);
    assert!(cursor.visible);
    assert_eq!(
        app.world().get::<Window>(window).unwrap().cursor_position(),
        Some(START)
    );
}

#[test]
fn camera_drag_requires_a_fresh_press_on_the_canvas() {
    for egui_capture in [false, true] {
        let (mut app, window, camera) = drag_app(1.0, 2.0);
        let transform = *app.world().get::<Transform>(camera).unwrap();
        let mut egui_context = EguiContext::default();
        let context = egui_context.get_mut();
        if egui_capture {
            context
                .run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(1280.0, 720.0),
                        )),
                        events: vec![egui::Event::PointerMoved(egui::pos2(200.0, 150.0))],
                        ..default()
                    },
                    |ui| {
                        egui::CentralPanel::default().show(ui, |ui| {
                            ui.label("UI captures the pointer");
                        });
                    },
                )
                .drop_without_applying_deltas();
            assert!(context.is_pointer_over_egui() || context.egui_wants_pointer_input());
            app.world_mut().spawn((egui_context, PrimaryEguiContext));
        } else {
            app.world_mut()
                .resource_mut::<GameUiPointerCapture>()
                .over_hud = true;
        }
        frame(
            &mut app,
            window,
            Some(ButtonState::Pressed),
            Vec2::splat(100.0),
        );
        assert_released(&app, window);
        // Moving off UI while holding RMB must not begin a capture.
        app.world_mut()
            .resource_mut::<GameUiPointerCapture>()
            .over_hud = false;
        if egui_capture {
            let entities: Vec<_> = app
                .world_mut()
                .query_filtered::<Entity, With<EguiContext>>()
                .iter(app.world())
                .collect();
            for entity in entities {
                app.world_mut().despawn(entity);
            }
        }
        frame(&mut app, window, None, Vec2::splat(100.0));
        assert_released(&app, window);
        assert_eq!(*app.world().get::<Transform>(camera).unwrap(), transform);
        frame(&mut app, window, Some(ButtonState::Released), Vec2::ZERO);
        frame(&mut app, window, Some(ButtonState::Pressed), Vec2::ZERO);
        assert!(app.world().resource::<InputState>().is_camera_dragging);
    }
}

#[test]
fn camera_drag_locks_and_discards_motion_from_the_press_frame() {
    let (mut app, window, camera) = drag_app(1.0, 2.0);
    let transform = *app.world().get::<Transform>(camera).unwrap();
    frame(
        &mut app,
        window,
        Some(ButtonState::Pressed),
        Vec2::splat(100.0),
    );
    let input = app.world().resource::<InputState>();
    assert!(input.is_camera_dragging);
    assert_eq!(input.camera_drag_start_position, Some(START));
    let cursor = app.world().get::<CursorOptions>(window).unwrap();
    assert_eq!(cursor.grab_mode, CursorGrabMode::Locked);
    assert!(!cursor.visible);
    assert_eq!(*app.world().get::<Transform>(camera).unwrap(), transform);
    frame(&mut app, window, None, Vec2::ZERO);
    assert_eq!(*app.world().get::<Transform>(camera).unwrap(), transform);
}

#[test]
fn camera_drag_applies_raw_motion_with_dpi_and_zoom_without_a_threshold() {
    for dpi_scale in [1.0, 1.5, 2.0] {
        for camera_scale in [0.25, 1.0, 3.0] {
            let (mut app, window, camera) = drag_app(dpi_scale, camera_scale);
            let initial = *app.world().get::<Transform>(camera).unwrap();
            frame(&mut app, window, Some(ButtonState::Pressed), Vec2::ZERO);
            for delta in [Vec2::new(12.0, -8.0), Vec2::new(0.1, 0.2)] {
                let before = app.world().get::<Transform>(camera).unwrap().translation;
                frame(&mut app, window, None, delta);
                let transform = app.world().get::<Transform>(camera).unwrap();
                let movement = delta / dpi_scale * camera_scale;
                let expected = before + Vec3::new(-movement.x, movement.y, 0.0);
                assert!((transform.translation - expected).length() < 0.00001);
                assert_eq!(transform.scale, initial.scale);
            }
        }
    }
}

#[test]
fn camera_drag_continues_over_ui_at_edges_and_without_absolute_coordinates() {
    let (mut app, window, camera) = drag_app(1.0, 2.0);
    frame(&mut app, window, Some(ButtonState::Pressed), Vec2::ZERO);
    app.world_mut()
        .resource_mut::<GameUiPointerCapture>()
        .over_hud = true;
    // Confined represents a backend's fallback when Locked is unavailable.
    app.world_mut()
        .get_mut::<CursorOptions>(window)
        .unwrap()
        .grab_mode = CursorGrabMode::Confined;
    for position in [Some(Vec2::new(1279.0, 719.0)), None] {
        app.world_mut()
            .get_mut::<Window>(window)
            .unwrap()
            .set_cursor_position(position);
        frame(&mut app, window, None, Vec2::new(10.0, 5.0));
        assert!(app.world().resource::<InputState>().is_camera_dragging);
    }
    assert_eq!(
        app.world().get::<Transform>(camera).unwrap().translation,
        Vec3::new(10.0, 0.0, 500.0)
    );
    assert_eq!(
        app.world().get::<CursorOptions>(window).unwrap().grab_mode,
        CursorGrabMode::Confined
    );
    frame(
        &mut app,
        window,
        Some(ButtonState::Released),
        Vec2::splat(100.0),
    );
    assert_released(&app, window);
    assert_eq!(
        app.world().get::<Transform>(camera).unwrap().translation,
        Vec3::new(10.0, 0.0, 500.0)
    );
}

#[test]
fn camera_drag_restores_the_cursor_on_release_and_focus_loss() {
    for focus_loss in [false, true] {
        let (mut app, window, camera) = drag_app(2.0, 1.0);
        let initial = *app.world().get::<Transform>(camera).unwrap();
        frame(&mut app, window, Some(ButtonState::Pressed), Vec2::ZERO);
        app.world_mut()
            .get_mut::<Window>(window)
            .unwrap()
            .set_cursor_position(Some(Vec2::splat(250.0)));
        if focus_loss {
            app.world_mut().get_mut::<Window>(window).unwrap().focused = false;
        }
        frame(
            &mut app,
            window,
            (!focus_loss).then_some(ButtonState::Released),
            Vec2::splat(100.0),
        );
        assert_released(&app, window);
        assert_eq!(*app.world().get::<Transform>(camera).unwrap(), initial);
        app.world_mut().get_mut::<Window>(window).unwrap().focused = true;
        frame(&mut app, window, None, Vec2::splat(100.0));
        assert_released(&app, window);
    }
}

#[test]
fn camera_drag_does_not_change_piece_states_or_schedule_uploads() {
    let (mut app, window, _) = drag_app(1.0, 2.0);
    let definition = PuzzleDefinition::new(42, UVec2::splat(2), UVec2::splat(128), false);
    let mut store = PieceDataStore::default();
    store.initialize_dense(DensePieceStates::generate(&definition));
    let states = store.states.clone();
    app.insert_resource(store)
        .insert_resource(definition)
        .init_resource::<PieceUpload>()
        .add_systems(Last, crate::resources::pieces::prepare_piece_upload);
    app.update();
    app.update();
    let revision = app.world().resource::<PieceUpload>().revision;
    let root_revision = app.world().resource::<PieceUpload>().root_revision;
    frame(&mut app, window, Some(ButtonState::Pressed), Vec2::ZERO);
    frame(&mut app, window, None, Vec2::new(20.0, 10.0));
    frame(&mut app, window, Some(ButtonState::Released), Vec2::ZERO);
    assert_eq!(app.world().resource::<PieceDataStore>().states, states);
    let upload = app.world().resource::<PieceUpload>();
    assert_eq!(upload.revision, revision);
    assert_eq!(upload.root_revision, root_revision);
    assert!(upload.initial.is_none());
    assert!(upload.initial_roots.is_none());
    assert!(upload.ranges.is_empty());
    assert!(upload.root_ranges.is_empty());
}
