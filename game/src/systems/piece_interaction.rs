use crate::keybindings::{KeyAction, KeyBindingsState, KeyPresses};
use crate::{components::*, resources::*};
use bevy::prelude::*;
use bevy::window::{CursorOptions, PrimaryWindow};
use bevy_egui::EguiContexts;
use jigsall_core::ClientCommand;
use jigsall_core::*;

/// UI adapters only sample input and publish the gesture's gameplay commands.
#[allow(clippy::too_many_arguments)]
pub fn handle_piece_input(
    local_player: Res<LocalPlayerId>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    bindings: Res<KeyBindingsState>,
    presses: Option<Res<KeyPresses>>,
    input: Res<InputState>,
    definition: Option<Res<PuzzleDefinition>>,
    ui_capture: Res<GameUiPointerCapture>,
    mut interaction: ResMut<crate::interaction::PieceInteraction>,
    mut store: ResMut<PieceDataStore>,
    mut selection: ResMut<crate::selection::PuzzleSelection>,
    mut commands: MessageWriter<ClientCommand>,
    mut contexts: EguiContexts,
    mut perf: ResMut<PerformanceMonitor>,
) {
    let start = perf.start_system_timing("handle_piece_input");
    let over_ui = ui_capture.over_hud
        || contexts
            .ctx_mut()
            .is_ok_and(|ctx| ctx.is_pointer_over_egui() || ctx.egui_wants_pointer_input());
    let keyboard_captured = contexts
        .ctx_mut()
        .is_ok_and(|ctx| ctx.egui_wants_keyboard_input());
    let frame = crate::interaction::PointerFrame {
        position: input.mouse_position,
        screen_position: input.cursor_screen_position,
        pressed: mouse.pressed(MouseButton::Left),
        just_pressed: mouse.just_pressed(MouseButton::Left),
        ctrl: !keyboard_captured && bindings.current.pressed(KeyAction::MultiSelect, &keys),
        over_ui,
        focused: input.window_focused,
    };
    interaction.set_play_area(definition.as_deref());
    let pointer_commands = interaction.update(frame, &mut store, &mut selection, local_player.0);
    let released = pointer_commands
        .iter()
        .any(|command| matches!(command, PieceCommand::ReleaseGroup { .. }));
    for command in pointer_commands {
        commands.write(ClientCommand {
            player: local_player.0,
            command,
        });
    }
    if definition.as_ref().is_some_and(|d| d.rotation_enabled)
        && input.window_focused
        && !over_ui
        && !keyboard_captured
        && !released
    {
        let turns =
            i8::from(bindings.just_pressed(KeyAction::RotateLeft, &keys, presses.as_deref()))
                - i8::from(bindings.just_pressed(
                    KeyAction::RotateRight,
                    &keys,
                    presses.as_deref(),
                ));
        if !interaction.is_dragging() || input.mouse_position.is_some_and(|point| point.is_finite())
        {
            if let Some(command) = interaction.update_rotation(
                &store,
                &mut selection,
                input.cursor_screen_position,
                turns,
            ) {
                commands.write(ClientCommand {
                    player: local_player.0,
                    command,
                });
            }
        }
    } else {
        interaction.cancel_rotation_pick(&mut selection);
    }
    perf.end_system_timing("handle_piece_input", start);
}

pub fn release_local_drag(
    local_player: Res<LocalPlayerId>,
    mut selection: ResMut<crate::selection::PuzzleSelection>,
    mut input: ResMut<InputState>,
    mut interaction: ResMut<crate::interaction::PieceInteraction>,
    mut store: ResMut<PieceDataStore>,
    mut commands: MessageWriter<ClientCommand>,
    mut windows: Query<(&mut Window, &mut CursorOptions), With<PrimaryWindow>>,
) {
    for command in interaction.cancel(&mut store, &mut selection, local_player.0) {
        commands.write(ClientCommand {
            player: local_player.0,
            command,
        });
    }
    input.end_camera_drag(windows.iter_mut());
}

pub fn render_selection_box(
    interaction: Res<crate::interaction::PieceInteraction>,
    cameras: Query<(&Camera, &Transform), With<MainCamera>>,
    mut overlay: ResMut<crate::render::SelectionOverlay>,
) {
    overlay.0 = interaction
        .screen_selection_rect()
        .and_then(|rect| {
            let (camera, transform) = cameras.single().ok()?;
            let transform = GlobalTransform::from(*transform);
            let a = camera.viewport_to_world_2d(&transform, rect.min).ok()?;
            let b = camera.viewport_to_world_2d(&transform, rect.max).ok()?;
            Some(Rect {
                min: a.min(b),
                max: a.max(b),
            })
        })
        .filter(|rect| rect.width() > 0.0 && rect.height() > 0.0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::systems::game_logic::apply_piece_commands;
    use std::collections::HashSet;

    pub(super) fn pointer_frame(
        app: &mut App,
        point: Vec2,
        pressed: bool,
        ctrl: bool,
        hits: &[u32],
    ) {
        let mut input = app.world_mut().resource_mut::<InputState>();
        input.mouse_position = Some(point);
        input.window_focused = true;
        input.cursor_screen_position = Some(point);
        let mut mouse = app.world_mut().resource_mut::<ButtonInput<MouseButton>>();
        mouse.clear();
        if pressed {
            mouse.press(MouseButton::Left);
        } else {
            mouse.release(MouseButton::Left);
        }
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        if ctrl {
            keys.press(KeyCode::ControlLeft);
        } else {
            keys.release(KeyCode::ControlLeft);
        }
        app.update();
        // Each scenario supplies the GPU payload explicitly. These tests cover
        // input/authority integration; real raster coverage is tested by render tests.
        for _ in 0..4 {
            app.world_mut()
                .resource_mut::<ButtonInput<MouseButton>>()
                .clear();
            if let Some(request) = app
                .world()
                .resource::<crate::selection::PuzzleSelection>()
                .latest
                .filter(|request| request.readback)
            {
                use crate::selection::*;
                let ids = hits.iter().copied().map(PieceId).collect();
                app.world_mut().resource_mut::<PuzzleSelection>().completed =
                    Some(SelectionResult {
                        request_id: request.request_id,
                        mode: request.mode,
                        payload: SelectionPayload::from_ids(request.mode, 2, ids),
                        error: None,
                    });
            }
            app.update();
        }
    }

    pub(super) fn input_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<crate::selection::PuzzleSelection>()
            .init_resource::<InputState>()
            .init_resource::<GameUiPointerCapture>()
            .init_resource::<crate::interaction::PieceInteraction>()
            .init_resource::<PieceDataStore>()
            .init_resource::<crate::resources::LocalPlayerId>()
            .init_resource::<PerformanceMonitor>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<ButtonInput<KeyCode>>()
            .insert_resource(KeyBindingsState::load(None))
            .init_resource::<bevy_egui::EguiUserTextures>()
            .add_message::<ClientCommand>()
            .add_systems(Update, (handle_piece_input, apply_piece_commands).chain());
        app.world_mut()
            .resource_mut::<PieceDataStore>()
            .initialize(vec![Vec2::new(100.0, 100.0), Vec2::new(300.0, 100.0)]);
        app.world_mut()
            .resource_mut::<PieceDataStore>()
            .next_z_order = 3;
        app
    }

    #[test]
    fn ctrl_box_selection_and_multi_drag_work_without_render_entities() {
        let mut app = input_app();
        for (id, point) in [Vec2::new(100.0, 100.0), Vec2::new(300.0, 100.0)]
            .into_iter()
            .enumerate()
        {
            pointer_frame(&mut app, point, true, true, &[id as u32]);
            pointer_frame(&mut app, point, false, true, &[id as u32]);
        }
        assert_eq!(
            app.world()
                .resource::<PieceDataStore>()
                .selected_pieces
                .len(),
            2
        );
        pointer_frame(&mut app, Vec2::new(100.0, 100.0), true, false, &[0]);
        assert_eq!(app.world().resource::<PieceDataStore>().held_by.len(), 2);
        pointer_frame(&mut app, Vec2::new(120.0, 130.0), true, false, &[]);
        pointer_frame(&mut app, Vec2::new(120.0, 130.0), false, false, &[]);
        let store = app.world().resource::<PieceDataStore>();
        assert_eq!(
            store.state(PieceId(0)).unwrap().position,
            Vec2::new(120.0, 130.0)
        );
        assert_eq!(
            store.state(PieceId(1)).unwrap().position,
            Vec2::new(320.0, 130.0)
        );
        assert!(store.held_by.is_empty());

        pointer_frame(&mut app, Vec2::new(50.0, 50.0), true, false, &[]);
        assert!(app
            .world()
            .resource::<PieceDataStore>()
            .selected_pieces
            .is_empty());
        pointer_frame(&mut app, Vec2::new(400.0, 200.0), true, false, &[]);
        assert!(
            app.world()
                .resource::<crate::selection::PuzzleSelection>()
                .preview_active
        );
        pointer_frame(&mut app, Vec2::new(400.0, 200.0), false, false, &[0, 1]);
        let store = app.world().resource::<PieceDataStore>();
        assert_eq!(store.selected_pieces.len(), 2);
        assert!(
            !app.world()
                .resource::<crate::selection::PuzzleSelection>()
                .preview_active
        );
    }

    #[test]
    fn local_drag_and_release_at_huge_pointer_coordinates_stay_inside_play_area() {
        let mut app = input_app();
        let definition = PuzzleDefinition {
            generator_version: GENERATOR_VERSION,
            seed: 42,
            grid_size: UVec2::new(2, 1),
            image_size: UVec2::new(200, 100),
            snap_distance: 5.0,
            rotation_enabled: true,
        };
        let area =
            jigsall_puzzle::placement::LogicalPlayArea::from_definition(&definition).unwrap();
        app.insert_resource(definition.clone());
        // Select both independent components, preserving their relative positions.
        for (id, point) in [Vec2::new(100.0, 100.0), Vec2::new(300.0, 100.0)]
            .into_iter()
            .enumerate()
        {
            pointer_frame(&mut app, point, true, true, &[id as u32]);
            pointer_frame(&mut app, point, false, true, &[id as u32]);
        }
        pointer_frame(&mut app, Vec2::new(100.0, 100.0), true, false, &[0]);
        pointer_frame(&mut app, Vec2::splat(1e37), true, false, &[]);
        {
            let store = app.world().resource::<PieceDataStore>();
            assert_eq!(store.states[0].position, Vec2::new(100.0, 100.0));
            assert!(area.contains((store.states[1].position + store.drag.delta).as_dvec2()));
        }
        pointer_frame(&mut app, Vec2::splat(1e37), false, false, &[]);
        let store = app.world().resource::<PieceDataStore>();
        assert!(store.held_by.is_empty());
        assert!(store
            .states
            .iter()
            .all(|s| area.contains(s.position.as_dvec2())));
        assert_eq!(
            store.states[1].position - store.states[0].position,
            Vec2::new(200.0, 0.0)
        );
        crate::checkpoint::PuzzleCheckpoint::capture(
            store,
            &definition,
            jigsall_core::session::ImageHash([0; 32]),
        )
        .unwrap();
    }

    #[test]
    fn release_position_is_applied_before_snap() {
        let mut app = input_app();
        app.insert_resource(PuzzleDefinition {
            generator_version: GENERATOR_VERSION,
            seed: 42,
            grid_size: UVec2::new(2, 1),
            image_size: UVec2::new(200, 100),
            snap_distance: 10.0,
            rotation_enabled: true,
        });
        // Grab off-center, then move and release in one frame.
        pointer_frame(&mut app, Vec2::new(107.0, 103.0), true, false, &[0]);
        pointer_frame(&mut app, Vec2::new(-41.0, 5.0), false, false, &[]);
        let store = app.world().resource::<PieceDataStore>();
        assert_eq!(
            store.state(PieceId(0)).unwrap().position,
            Vec2::new(-50.0, 0.0)
        );
        assert!(store.state(PieceId(0)).unwrap().placed);
        assert!(store.selected_pieces.is_empty());
        assert!(store.held_by.is_empty());
    }

    #[test]
    fn focus_loss_and_pause_release_ownership_and_restore_picking() {
        use bevy::ecs::system::RunSystemOnce;
        for pause in [false, true] {
            let mut app = input_app();
            pointer_frame(&mut app, Vec2::new(100.0, 100.0), true, false, &[0]);
            pointer_frame(&mut app, Vec2::new(120.0, 130.0), true, false, &[]);
            assert_eq!(
                app.world().resource::<PieceDataStore>().states[0].position,
                Vec2::new(100.0, 100.0)
            );
            app.world_mut()
                .resource_mut::<ButtonInput<MouseButton>>()
                .clear();
            if pause {
                app.world_mut().run_system_once(release_local_drag).unwrap();
            } else {
                app.world_mut().resource_mut::<InputState>().window_focused = false;
            }
            app.update();
            assert!(app.world().resource::<PieceDataStore>().held_by.is_empty());
            assert!(!app
                .world()
                .resource::<crate::interaction::PieceInteraction>()
                .is_dragging());
            let store = app.world().resource::<PieceDataStore>();
            assert_eq!(
                store.state(PieceId(0)).unwrap().position,
                Vec2::new(120.0, 130.0)
            );
            assert!(store.is_selectable(PieceId(0)));
            pointer_frame(&mut app, Vec2::new(120.0, 130.0), false, false, &[]);
            pointer_frame(&mut app, Vec2::new(120.0, 130.0), true, false, &[0]);
            assert_eq!(
                app.world()
                    .resource::<PieceDataStore>()
                    .state(PieceId(0))
                    .unwrap()
                    .held_by,
                Some(LOCAL_PLAYER)
            );
            pointer_frame(&mut app, Vec2::new(120.0, 130.0), false, false, &[]);
        }
    }

    #[test]
    fn reverse_box_selection_uses_the_release_frame_and_ctrl_is_additive() {
        let mut app = input_app();
        pointer_frame(&mut app, Vec2::new(100.0, 100.0), true, true, &[0]);
        pointer_frame(&mut app, Vec2::new(100.0, 100.0), false, true, &[0]);
        pointer_frame(&mut app, Vec2::new(400.0, 200.0), true, true, &[]);
        // Modifier is frozen at press, even when released mid-gesture.
        pointer_frame(&mut app, Vec2::new(250.0, 50.0), false, false, &[1]);
        assert_eq!(
            app.world()
                .resource::<PieceDataStore>()
                .selected_pieces
                .len(),
            2
        );
        // Ctrl toggles membership without grabbing or moving.
        pointer_frame(&mut app, Vec2::new(100.0, 100.0), true, true, &[0]);
        pointer_frame(&mut app, Vec2::new(150.0, 150.0), false, true, &[]);
        let store = app.world().resource::<PieceDataStore>();
        assert_eq!(
            store.selected_pieces.iter().collect::<HashSet<_>>(),
            HashSet::from([PieceId(1)])
        );
        assert_eq!(
            store.state(PieceId(0)).unwrap().position,
            Vec2::new(100.0, 100.0)
        );
    }

    #[test]
    fn ui_press_never_starts_a_gesture_but_canvas_drag_can_finish_over_ui() {
        let mut app = input_app();
        app.world_mut()
            .resource_mut::<GameUiPointerCapture>()
            .over_hud = true;
        pointer_frame(&mut app, Vec2::new(100.0, 100.0), true, false, &[0]);
        app.world_mut()
            .resource_mut::<GameUiPointerCapture>()
            .over_hud = false;
        pointer_frame(&mut app, Vec2::new(105.0, 105.0), true, false, &[0]);
        pointer_frame(&mut app, Vec2::new(105.0, 105.0), false, false, &[0]);
        assert!(app
            .world()
            .resource::<PieceDataStore>()
            .selected_pieces
            .is_empty());
        pointer_frame(&mut app, Vec2::new(105.0, 105.0), true, false, &[0]);
        app.world_mut()
            .resource_mut::<GameUiPointerCapture>()
            .over_hud = true;
        pointer_frame(&mut app, Vec2::new(155.0, 135.0), false, false, &[]);
        let store = app.world().resource::<PieceDataStore>();
        assert_eq!(
            store.state(PieceId(0)).unwrap().position,
            Vec2::new(150.0, 130.0)
        );
        assert!(store.held_by.is_empty());
    }

    #[test]
    fn missing_or_invalid_pointer_never_moves_a_piece_and_release_still_works() {
        let mut app = input_app();
        pointer_frame(&mut app, Vec2::new(100.0, 100.0), true, false, &[0]);
        app.world_mut().resource_mut::<InputState>().mouse_position = None;
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .clear();
        app.update();
        assert_eq!(
            app.world()
                .resource::<PieceDataStore>()
                .state(PieceId(0))
                .unwrap()
                .position,
            Vec2::new(100.0, 100.0)
        );
        pointer_frame(&mut app, Vec2::splat(f32::NAN), false, false, &[]);
        assert!(app.world().resource::<PieceDataStore>().held_by.is_empty());
        let store = app.world().resource::<PieceDataStore>();
        assert_eq!(
            store.state(PieceId(0)).unwrap().position,
            Vec2::new(100.0, 100.0)
        );
        assert!(store.is_selectable(PieceId(0)));
    }

    #[test]
    fn cancelled_box_restores_selection_and_clears_preview() {
        let mut app = input_app();
        pointer_frame(&mut app, Vec2::new(100.0, 100.0), true, true, &[0]);
        pointer_frame(&mut app, Vec2::new(100.0, 100.0), false, true, &[0]);
        pointer_frame(&mut app, Vec2::new(50.0, 50.0), true, false, &[]);
        pointer_frame(&mut app, Vec2::new(400.0, 200.0), true, false, &[]);
        app.world_mut().resource_mut::<InputState>().window_focused = false;
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .clear();
        app.update();
        let store = app.world().resource::<PieceDataStore>();
        assert_eq!(
            store.selected_pieces.iter().collect::<HashSet<_>>(),
            HashSet::from([PieceId(0)])
        );
        assert!(
            !app.world()
                .resource::<crate::selection::PuzzleSelection>()
                .preview_active
        );
        assert!(app
            .world()
            .resource::<crate::interaction::PieceInteraction>()
            .screen_selection_rect()
            .is_none());
    }

    #[test]
    fn selected_group_keeps_relative_stacking_and_depth_stays_bounded() {
        let mut app = input_app();
        for (id, point) in [Vec2::new(100.0, 100.0), Vec2::new(300.0, 100.0)]
            .into_iter()
            .enumerate()
        {
            pointer_frame(&mut app, point, true, true, &[id as u32]);
            pointer_frame(&mut app, point, false, true, &[id as u32]);
        }
        // Force depth compaction during a group grab.
        app.world_mut()
            .resource_mut::<PieceDataStore>()
            .next_z_order = crate::resources::pieces::MAX_Z;
        pointer_frame(&mut app, Vec2::new(100.0, 100.0), true, false, &[0]);
        pointer_frame(&mut app, Vec2::new(100.0, 100.0), false, false, &[0]);
        let store = app.world().resource::<PieceDataStore>();
        let z0 = store.states[0].z_order;
        let z1 = store.states[1].z_order;
        assert!(z0 < z1 && z1 < 50);
    }

    #[test]
    fn delayed_point_result_preserves_press_and_release_coordinates() {
        use crate::{interaction::*, selection::*};
        let mut app = input_app();
        let mut store = app.world_mut().remove_resource::<PieceDataStore>().unwrap();
        let mut selection = PuzzleSelection::default();
        let mut interaction = PieceInteraction::default();
        let frame = |position, pressed, just_pressed| PointerFrame {
            position: Some(position),
            screen_position: Some(position),
            pressed,
            just_pressed,
            ctrl: false,
            over_ui: false,
            focused: true,
        };
        assert!(interaction
            .update(
                frame(Vec2::new(107., 103.), true, true),
                &mut store,
                &mut selection,
                jigsall_core::LOCAL_PLAYER
            )
            .is_empty());
        let request = selection.latest.unwrap();
        assert!(interaction
            .update(
                frame(Vec2::new(120., 130.), true, false),
                &mut store,
                &mut selection,
                jigsall_core::LOCAL_PLAYER
            )
            .is_empty());
        assert!(interaction
            .update(
                frame(Vec2::new(9., 5.), false, false),
                &mut store,
                &mut selection,
                jigsall_core::LOCAL_PLAYER
            )
            .is_empty());
        selection.completed = Some(SelectionResult {
            request_id: request.request_id,
            mode: request.mode,
            payload: SelectionPayload::from_ids(request.mode, 2, vec![PieceId(0)]),
            error: None,
        });
        // Later pointer motion after release must not move the piece again.
        let commands = interaction.update(
            frame(Vec2::splat(500.), false, false),
            &mut store,
            &mut selection,
            jigsall_core::LOCAL_PLAYER,
        );
        assert_eq!(
            commands,
            vec![
                PieceCommand::GrabGroup {
                    members: {
                        let mut set = PieceBitSet::new(2);
                        set.insert(PieceId(0));
                        set
                    }
                },
                PieceCommand::ReleaseGroup {
                    members: {
                        let mut set = PieceBitSet::new(2);
                        set.insert(PieceId(0));
                        set
                    },
                    delta: Vec2::new(-98.0, -98.0)
                }
            ]
        );
        assert!(!interaction.is_dragging());
        assert!(selection.latest.is_none());
    }

    #[test]
    fn final_rectangle_supersedes_in_flight_preview() {
        use crate::{interaction::*, selection::*};
        let mut app = input_app();
        let mut store = app.world_mut().remove_resource::<PieceDataStore>().unwrap();
        let mut selection = PuzzleSelection::default();
        let mut interaction = PieceInteraction::default();
        let frame = |position, pressed, just_pressed| PointerFrame {
            position: Some(position),
            screen_position: Some(position),
            pressed,
            just_pressed,
            ctrl: false,
            over_ui: false,
            focused: true,
        };
        interaction.update(
            frame(Vec2::splat(50.), true, true),
            &mut store,
            &mut selection,
            jigsall_core::LOCAL_PLAYER,
        );
        let point = selection.latest.unwrap();
        selection.completed = Some(SelectionResult {
            request_id: point.request_id,
            mode: point.mode,
            payload: SelectionPayload::from_ids(point.mode, 2, vec![]),
            error: None,
        });
        interaction.update(
            frame(Vec2::new(400., 200.), true, false),
            &mut store,
            &mut selection,
            jigsall_core::LOCAL_PLAYER,
        );
        let preview = selection.latest.unwrap();
        interaction.update(
            frame(Vec2::new(410., 210.), false, false),
            &mut store,
            &mut selection,
            jigsall_core::LOCAL_PLAYER,
        );
        let final_request = selection.latest.unwrap();
        assert!(final_request.request_id > preview.request_id);
        selection.completed = Some(SelectionResult {
            request_id: preview.request_id,
            mode: preview.mode,
            payload: SelectionPayload::from_ids(preview.mode, 2, vec![PieceId(0)]),
            error: None,
        });
        interaction.update(
            frame(Vec2::splat(500.), false, false),
            &mut store,
            &mut selection,
            jigsall_core::LOCAL_PLAYER,
        );
        assert!(store.selected_pieces.is_empty());
        selection.completed = Some(SelectionResult {
            request_id: final_request.request_id,
            mode: final_request.mode,
            payload: SelectionPayload::from_ids(
                final_request.mode,
                2,
                vec![PieceId(0), PieceId(1)],
            ),
            error: None,
        });
        interaction.update(
            frame(Vec2::splat(500.), false, false),
            &mut store,
            &mut selection,
            jigsall_core::LOCAL_PLAYER,
        );
        assert_eq!(
            store.selected_pieces.iter().collect::<HashSet<_>>(),
            HashSet::from([PieceId(0), PieceId(1)])
        );
        assert!(interaction.screen_selection_rect().is_none());
    }
}

#[cfg(test)]
mod rotation_input_tests {
    use super::*;

    #[test]
    fn disabled_rotation_keys_emit_no_commands_or_hover_pick_for_selection_or_drag() {
        for mode in 0..3 {
            let mut app = super::tests::input_app();
            app.insert_resource(PuzzleDefinition {
                generator_version: GENERATOR_VERSION,
                seed: 42,
                grid_size: UVec2::new(2, 1),
                image_size: UVec2::new(120, 40),
                snap_distance: 5.0,
                rotation_enabled: false,
            });
            if mode != 0 {
                app.world_mut()
                    .resource_mut::<PieceDataStore>()
                    .selected_pieces
                    .fill();
            }
            if mode == 2 {
                super::tests::pointer_frame(&mut app, Vec2::new(100., 100.), true, false, &[0]);
            }
            let mut input = app.world_mut().resource_mut::<InputState>();
            input.window_focused = true;
            input.cursor_screen_position = Some(Vec2::new(100., 100.));
            let before = app.world().resource::<PieceDataStore>().states.clone();
            let mut reader = bevy::ecs::message::MessageCursor::<ClientCommand>::default();
            reader.clear(app.world().resource::<Messages<ClientCommand>>());
            for key in [KeyCode::KeyQ, KeyCode::KeyE] {
                app.world_mut()
                    .resource_mut::<ButtonInput<KeyCode>>()
                    .press(key);
                app.update();
                assert_eq!(
                    reader
                        .read(app.world().resource::<Messages<ClientCommand>>())
                        .count(),
                    0
                );
                assert_eq!(app.world().resource::<PieceDataStore>().states, before);
                assert!(app
                    .world()
                    .resource::<crate::selection::PuzzleSelection>()
                    .latest
                    .is_none());
                let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
                keys.release(key);
                keys.clear();
            }
        }
    }

    #[test]
    fn q_e_rotate_selection_and_focus_ui_and_gestures_gate_commands() {
        let mut app = super::tests::input_app();
        app.world_mut().insert_resource(PuzzleDefinition {
            generator_version: GENERATOR_VERSION,
            seed: 42,
            grid_size: UVec2::new(2, 1),
            image_size: UVec2::new(120, 40),
            snap_distance: 5.0,
            rotation_enabled: true,
        });
        app.world_mut()
            .resource_mut::<PieceDataStore>()
            .selected_pieces
            .fill();
        app.world_mut().resource_mut::<InputState>().window_focused = true;
        let original = app.world().resource::<PieceDataStore>().states.to_vec();
        for (key, rotation) in [(KeyCode::KeyQ, 1), (KeyCode::KeyE, 0)] {
            app.world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .press(key);
            app.update();
            for (state, before) in app
                .world()
                .resource::<PieceDataStore>()
                .states
                .iter()
                .zip(&original)
            {
                assert_eq!(decode_rotation(state.flags), rotation);
                assert_eq!(state.position, before.position);
            }
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.release(key);
            keys.clear();
        }
        for gate in 0..3 {
            app.world_mut().resource_mut::<InputState>().window_focused = gate != 0;
            app.world_mut()
                .resource_mut::<GameUiPointerCapture>()
                .over_hud = gate == 1;
            if gate == 2 {
                let members = app
                    .world()
                    .resource::<PieceDataStore>()
                    .selected_pieces
                    .words()
                    .clone();
                app.world_mut()
                    .resource_mut::<PieceDataStore>()
                    .drag
                    .members = members;
            }
            app.world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .press(KeyCode::KeyQ);
            app.update();
            assert!(app
                .world()
                .resource::<PieceDataStore>()
                .states
                .iter()
                .all(|s| decode_rotation(s.flags) == 0));
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.release(KeyCode::KeyQ);
            keys.clear();
        }
    }
}

#[cfg(test)]
mod local_identity_tests {
    use super::tests::{input_app, pointer_frame};
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    #[derive(Resource, Default)]
    struct ObservedCommands(Vec<ClientCommand>);

    fn observe(mut messages: MessageReader<ClientCommand>, mut observed: ResMut<ObservedCommands>) {
        observed.0.extend(messages.read().cloned());
    }

    fn app() -> App {
        let mut app = input_app();
        app.insert_resource(LocalPlayerId(PlayerId(42)))
            .init_resource::<ObservedCommands>()
            .add_systems(
                Update,
                observe
                    .after(handle_piece_input)
                    .before(crate::systems::game_logic::apply_piece_commands),
            );
        app
    }

    #[test]
    fn click_drag_release_and_both_rotation_inputs_use_session_local_identity() {
        let mut app = app();
        app.insert_resource(PuzzleDefinition {
            generator_version: GENERATOR_VERSION,
            seed: 42,
            grid_size: UVec2::new(2, 1),
            image_size: UVec2::new(80, 40),
            snap_distance: 5.0,
            rotation_enabled: true,
        });
        pointer_frame(&mut app, Vec2::new(100.0, 100.0), true, false, &[0]);
        assert_eq!(
            app.world()
                .resource::<PieceDataStore>()
                .held_by
                .get(&PieceId(0)),
            Some(&PlayerId(42))
        );
        pointer_frame(&mut app, Vec2::new(120.0, 130.0), true, false, &[]);
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .clear();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyQ);
        app.update();
        assert_eq!(
            decode_rotation(app.world().resource::<PieceDataStore>().states[0].flags),
            1
        );
        *app.world_mut().resource_mut::<ButtonInput<KeyCode>>() = default();
        pointer_frame(&mut app, Vec2::new(120.0, 130.0), false, false, &[]);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyE);
        app.update();
        let commands = &app.world().resource::<ObservedCommands>().0;
        assert!(!commands.is_empty());
        assert!(commands.iter().all(|c| c.player == PlayerId(42)));
        for kind in 0..4 {
            assert!(commands.iter().any(|c| matches!(
                (&c.command, kind),
                (PieceCommand::GrabGroup { .. }, 0)
                    | (PieceCommand::RotateDrag { .. }, 1)
                    | (PieceCommand::ReleaseGroup { .. }, 2)
                    | (PieceCommand::Rotate { .. }, 3)
            )));
        }
        assert!(app.world().resource::<PieceDataStore>().held_by.is_empty());
        assert_eq!(
            decode_rotation(app.world().resource::<PieceDataStore>().states[0].flags),
            0
        );
    }

    #[test]
    fn pause_and_focus_loss_release_42_and_leave_remote_zero_held() {
        for pause in [false, true] {
            let mut app = app();
            app.world_mut()
                .resource_mut::<PieceDataStore>()
                .apply_command(
                    PlayerId(0),
                    &PieceCommand::Grab(PieceId(1)),
                    None,
                    PlayerId(42),
                );
            pointer_frame(&mut app, Vec2::new(100.0, 100.0), true, false, &[0]);
            app.world_mut()
                .resource_mut::<ButtonInput<MouseButton>>()
                .clear();
            if pause {
                app.world_mut().run_system_once(release_local_drag).unwrap();
            } else {
                app.world_mut().resource_mut::<InputState>().window_focused = false;
            }
            app.update();
            let store = app.world().resource::<PieceDataStore>();
            assert_eq!(store.held_by.get(&PieceId(0)), None);
            assert_eq!(store.held_by.get(&PieceId(1)), Some(&PlayerId(0)));
            let commands = &app.world().resource::<ObservedCommands>().0;
            assert!(commands.iter().all(|c| c.player == PlayerId(42)));
            assert!(commands.iter().any(
                |c| matches!(&c.command, PieceCommand::ReleaseGroup { members, .. }
                if members.iter().collect::<Vec<_>>() == vec![PieceId(0)])
            ));
        }
    }
}
