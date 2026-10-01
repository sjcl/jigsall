use crate::{components::*, resources::*};
use bevy::prelude::*;
use bevy_egui::EguiContexts;
use puzzella_core::ClientCommand;
use puzzella_core::*;
use std::collections::HashSet;

/// UI adapters only sample input and publish the gesture's gameplay commands.
#[allow(clippy::too_many_arguments)]
pub fn handle_piece_input(
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    input: Res<InputState>,
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
    let frame = crate::interaction::PointerFrame {
        position: input.mouse_position,
        screen_position: input.cursor_screen_position,
        pressed: mouse.pressed(MouseButton::Left),
        just_pressed: mouse.just_pressed(MouseButton::Left),
        ctrl: keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight),
        over_ui,
        focused: input.window_focused,
    };
    for command in interaction.update(frame, &mut store, &mut selection) {
        commands.write(ClientCommand {
            player: LOCAL_PLAYER,
            command,
        });
    }
    perf.end_system_timing("handle_piece_input", start);
}

pub fn release_local_drag(
    mut selection: ResMut<crate::selection::PuzzleSelection>,
    mut input: ResMut<InputState>,
    mut interaction: ResMut<crate::interaction::PieceInteraction>,
    mut store: ResMut<PieceDataStore>,
    mut commands: MessageWriter<ClientCommand>,
) {
    for command in interaction.cancel(&mut store, &mut selection) {
        commands.write(ClientCommand {
            player: LOCAL_PLAYER,
            command,
        });
    }
    input.is_camera_dragging = false;
    input.last_cursor_position = None;
}

/// Selection markers are local presentation only; selection is keyed by stable ID.
pub fn sync_selection_markers(
    mut commands: Commands,
    store: Res<PieceDataStore>,
    pieces: Query<(
        Entity,
        &PuzzlePiece,
        Has<SelectedPiece>,
        Has<SelectionPreview>,
    )>,
) {
    for (entity, piece, selected, preview) in &pieces {
        let want_selected = store.selected_pieces.contains(&piece.id);
        let want_preview = store.preview_pieces.contains(&piece.id) && !want_selected;
        if want_selected != selected {
            if want_selected {
                commands.entity(entity).insert(SelectedPiece);
            } else {
                commands.entity(entity).remove::<SelectedPiece>();
            }
        }
        if want_preview != preview {
            if want_preview {
                commands.entity(entity).insert(SelectionPreview);
            } else {
                commands.entity(entity).remove::<SelectionPreview>();
            }
        }
    }
}
#[allow(clippy::type_complexity)]
pub fn render_selection_box(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
    interaction: Res<crate::interaction::PieceInteraction>,
    cameras: Query<(&Camera, &Transform), (With<MainCamera>, Without<SelectionBox>)>,
    mut boxes: Query<(Entity, &mut Transform), With<SelectionBox>>,
) {
    let rect = interaction
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
    if let Some(rect) = rect {
        let transform = Transform::from_translation(rect.center().extend(200.0))
            .with_scale(Vec3::new(rect.width(), rect.height(), 1.0));
        if let Ok((_, mut current)) = boxes.single_mut() {
            if *current != transform {
                *current = transform;
            }
        } else {
            commands.spawn((
                Mesh2d(meshes.add(Rectangle::new(1.0, 1.0))),
                MeshMaterial2d(
                    materials.add(ColorMaterial::from(Color::srgba(0.3, 0.6, 1.0, 0.3))),
                ),
                transform,
                SelectionBox,
            ));
        }
    } else {
        for (entity, _) in &boxes {
            commands.entity(entity).despawn();
        }
    }
}

/// Cached child outlines follow piece transforms without changing the shared
/// image material or rebuilding meshes on every pointer movement.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn highlight_selected_pieces(
    mut commands: Commands,
    pieces: Query<(
        Entity,
        &PieceStroke,
        Has<SelectedPiece>,
        Has<SelectionPreview>,
    )>,
    outlines: Query<Entity, With<PieceOutline>>,
    mut state: ResMut<HighlightState>,
    materials: Res<HighlightMaterials>,
) {
    let selected: HashSet<_> = pieces.iter().filter(|p| p.2).map(|p| p.0).collect();
    let preview: HashSet<_> = pieces.iter().filter(|p| p.3).map(|p| p.0).collect();
    if selected == state.last_selected_pieces && preview == state.last_preview_pieces {
        return;
    }
    for entity in &outlines {
        commands.entity(entity).despawn();
    }
    for (entity, stroke, selected, preview) in &pieces {
        if !selected && !preview {
            continue;
        }
        let material = if selected {
            materials.selected_material.clone()
        } else {
            materials.preview_material.clone()
        };
        commands.spawn((
            Mesh2d(stroke.0.clone()),
            MeshMaterial2d(material),
            // Outlines are overlays above every piece, below the selection box.
            Transform::from_xyz(0.0, 0.0, 60.0),
            PieceOutline,
            ChildOf(entity),
        ));
    }
    state.last_selected_pieces = selected;
    state.last_preview_pieces = preview;
}

pub fn should_render_selection_box(
    interaction: Res<crate::interaction::PieceInteraction>,
    boxes: Query<Entity, With<SelectionBox>>,
) -> bool {
    interaction.selection_rect().is_some() || !boxes.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::systems::game_logic::{apply_piece_commands, project_piece_states};

    #[test]
    fn outlines_follow_the_piece_owned_stroke_handle_and_clear_on_deselection() {
        let mut app = App::new();
        app.init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<ColorMaterial>>()
            .init_resource::<HighlightState>()
            .add_systems(Update, highlight_selected_pieces);
        let stroke = app
            .world_mut()
            .resource_mut::<Assets<Mesh>>()
            .add(Rectangle::new(10.0, 10.0));
        let selected_material = app
            .world_mut()
            .resource_mut::<Assets<ColorMaterial>>()
            .add(ColorMaterial::default());
        app.insert_resource(HighlightMaterials {
            preview_material: default(),
            selected_material: selected_material.clone(),
        });
        let parent = app
            .world_mut()
            .spawn((PieceStroke(stroke.clone()), SelectedPiece))
            .id();
        app.update();
        let mut query = app.world_mut().query_filtered::<(Entity, &Mesh2d, &MeshMaterial2d<ColorMaterial>, &ChildOf), With<PieceOutline>>();
        let (entity, mesh, material, child) = query.single(app.world()).unwrap();
        assert_eq!(mesh.0, stroke);
        assert_eq!(material.0, selected_material);
        assert_eq!(child.parent(), parent);
        app.update();
        assert!(
            app.world().get_entity(entity).is_ok(),
            "unchanged selection reuses outline"
        );
        app.world_mut().entity_mut(parent).remove::<SelectedPiece>();
        app.update();
        assert!(app.world().get_entity(entity).is_err());
    }

    fn pointer_frame(app: &mut App, point: Vec2, pressed: bool, ctrl: bool) {
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
        // Gesture tests inject a GPU response independently of render entities.
        // The legacy debug oracle is only a test fixture, never a runtime fallback.
        for _ in 0..4 {
            app.world_mut()
                .resource_mut::<ButtonInput<MouseButton>>()
                .clear();
            if let Some(request) = app
                .world()
                .resource::<crate::selection::PuzzleSelection>()
                .latest
            {
                use crate::selection::*;
                let mut collision = app.world_mut().resource_mut::<PieceCollisionSystem>();
                let ids = match request.mode {
                    SelectionMode::Point => collision
                        .find_piece_at_position(request.region.min)
                        .into_iter()
                        .collect(),
                    SelectionMode::Rectangle => {
                        let rect = Rect {
                            min: request.region.min.min(request.region.max),
                            max: request.region.min.max(request.region.max),
                        };
                        if rect.width() > 0.0 && rect.height() > 0.0 {
                            collision.find_pieces_with_detailed_rect_intersection(rect)
                        } else {
                            vec![]
                        }
                    }
                };
                app.world_mut().resource_mut::<PuzzleSelection>().completed =
                    Some(SelectionResult {
                        request_id: request.request_id,
                        mode: request.mode,
                        piece_ids: ids,
                        entities: vec![],
                        error: None,
                    });
            }
            app.update();
        }
    }

    fn input_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<crate::selection::PuzzleSelection>()
            .init_resource::<InputState>()
            .init_resource::<GameUiPointerCapture>()
            .init_resource::<crate::interaction::PieceInteraction>()
            .init_resource::<BatchManager>()
            .init_resource::<PieceDataStore>()
            .init_resource::<PieceCollisionSystem>()
            .init_resource::<PerformanceMonitor>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<bevy_egui::EguiUserTextures>()
            .add_message::<ClientCommand>()
            .add_message::<PieceMoveCompleted>()
            .add_message::<PiecePlacedEvent>()
            .add_systems(
                Update,
                (
                    handle_piece_input,
                    apply_piece_commands,
                    crate::systems::game_logic::check_piece_placement_event_driven,
                    project_piece_states,
                )
                    .chain(),
            );
        for (index, position) in [Vec2::new(100.0, 100.0), Vec2::new(300.0, 100.0)]
            .into_iter()
            .enumerate()
        {
            let id = PieceId(index as u32);
            let vertices = vec![[-20.0, -20.0], [20.0, -20.0], [20.0, 20.0], [-20.0, 20.0]];
            let indices = vec![0, 1, 2, 0, 2, 3];
            app.world_mut()
                .resource_mut::<PieceCollisionSystem>()
                .add_piece(PieceCollisionData {
                    piece_id: id,
                    position,
                    z_order: index as f32 * 0.001,
                    bounding_box: Rect {
                        min: position - Vec2::splat(20.0),
                        max: position + Vec2::splat(20.0),
                    },
                    vertices: vertices.iter().map(|&vertex| Vec2::from(vertex)).collect(),
                    indices: indices.clone(),
                });
            app.world_mut()
                .resource_mut::<PieceDataStore>()
                .add_piece(StoredPieceData {
                    definition: PuzzlePiece {
                        id,
                        grid_position: UVec2::new(index as u32, 0),
                        correct_position: Vec2::ZERO,
                        initial_position: position,
                    },
                    state: PieceState::new(position),
                    render: PieceRenderData {
                        bounds: Rect::new(-20.0, -20.0, 20.0, 20.0),
                        shape: PieceShapeData { vertices, indices },
                        mesh: default(),
                        stroke: default(),
                        material: default(),
                    },
                });
        }
        app
    }

    #[test]
    fn ctrl_box_selection_and_multi_drag_work_without_render_entities() {
        let mut app = input_app();
        for point in [Vec2::new(100.0, 100.0), Vec2::new(300.0, 100.0)] {
            pointer_frame(&mut app, point, true, true);
            pointer_frame(&mut app, point, false, true);
        }
        assert_eq!(
            app.world()
                .resource::<PieceDataStore>()
                .selected_pieces
                .len(),
            2
        );
        pointer_frame(&mut app, Vec2::new(100.0, 100.0), true, false);
        assert_eq!(
            app.world().resource::<PieceDataStore>().held_pieces.len(),
            2
        );
        pointer_frame(&mut app, Vec2::new(120.0, 130.0), true, false);
        pointer_frame(&mut app, Vec2::new(120.0, 130.0), false, false);
        let store = app.world().resource::<PieceDataStore>();
        assert_eq!(
            store.pieces[&PieceId(0)].state.position,
            Vec2::new(120.0, 130.0)
        );
        assert_eq!(
            store.pieces[&PieceId(1)].state.position,
            Vec2::new(320.0, 130.0)
        );
        assert!(store.held_pieces.is_empty());
        assert!(app
            .world()
            .resource::<PieceCollisionSystem>()
            .dragging_pieces
            .is_empty());

        pointer_frame(&mut app, Vec2::new(50.0, 50.0), true, false);
        assert!(app
            .world()
            .resource::<PieceDataStore>()
            .selected_pieces
            .is_empty());
        pointer_frame(&mut app, Vec2::new(400.0, 200.0), true, false);
        assert_eq!(
            app.world()
                .resource::<PieceDataStore>()
                .preview_pieces
                .len(),
            2
        );
        pointer_frame(&mut app, Vec2::new(400.0, 200.0), false, false);
        let store = app.world().resource::<PieceDataStore>();
        assert_eq!(store.selected_pieces.len(), 2);
        assert!(store.preview_pieces.is_empty());
        assert!(store.temporary_entities.is_empty());
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
        });
        // Grab off-center, then move and release in one frame.
        pointer_frame(&mut app, Vec2::new(107.0, 103.0), true, false);
        pointer_frame(&mut app, Vec2::new(9.0, 5.0), false, false);
        let store = app.world().resource::<PieceDataStore>();
        assert_eq!(store.pieces[&PieceId(0)].state.position, Vec2::ZERO);
        assert!(store.pieces[&PieceId(0)].state.placed);
        assert!(store.selected_pieces.is_empty());
        assert!(store.held_pieces.is_empty());
        let collision = app.world().resource::<PieceCollisionSystem>();
        assert!(!collision.pieces.contains_key(&PieceId(0)));
        assert!(collision.dragging_pieces.is_empty());
    }

    #[test]
    fn focus_loss_and_pause_release_ownership_and_restore_picking() {
        use bevy::ecs::system::RunSystemOnce;
        let mut app = input_app();
        for pause in [false, true] {
            pointer_frame(&mut app, Vec2::new(100.0, 100.0), true, false);
            app.world_mut()
                .resource_mut::<ButtonInput<MouseButton>>()
                .clear();
            if pause {
                app.world_mut().run_system_once(release_local_drag).unwrap();
            } else {
                app.world_mut().resource_mut::<InputState>().window_focused = false;
            }
            app.update();
            assert!(app
                .world()
                .resource::<PieceDataStore>()
                .held_pieces
                .is_empty());
            assert!(!app
                .world()
                .resource::<crate::interaction::PieceInteraction>()
                .is_dragging());
            let mut collision = app.world_mut().resource_mut::<PieceCollisionSystem>();
            assert!(collision.dragging_pieces.is_empty());
            assert_eq!(
                collision.find_piece_at_position(Vec2::new(100.0, 100.0)),
                Some(PieceId(0))
            );
            assert_eq!(collision.rtree.size(), 2);
            pointer_frame(&mut app, Vec2::new(100.0, 100.0), false, false);
        }
    }

    #[test]
    fn reverse_box_selection_uses_the_release_frame_and_ctrl_is_additive() {
        let mut app = input_app();
        pointer_frame(&mut app, Vec2::new(100.0, 100.0), true, true);
        pointer_frame(&mut app, Vec2::new(100.0, 100.0), false, true);
        pointer_frame(&mut app, Vec2::new(400.0, 200.0), true, true);
        // Modifier is frozen at press, even when released mid-gesture.
        pointer_frame(&mut app, Vec2::new(250.0, 50.0), false, false);
        assert_eq!(
            app.world()
                .resource::<PieceDataStore>()
                .selected_pieces
                .len(),
            2
        );
        // Ctrl toggles membership without grabbing or moving.
        pointer_frame(&mut app, Vec2::new(100.0, 100.0), true, true);
        pointer_frame(&mut app, Vec2::new(150.0, 150.0), false, true);
        let store = app.world().resource::<PieceDataStore>();
        assert_eq!(store.selected_pieces, HashSet::from([PieceId(1)]));
        assert_eq!(
            store.pieces[&PieceId(0)].state.position,
            Vec2::new(100.0, 100.0)
        );
    }

    #[test]
    fn ui_press_never_starts_a_gesture_but_canvas_drag_can_finish_over_ui() {
        let mut app = input_app();
        app.world_mut()
            .resource_mut::<GameUiPointerCapture>()
            .over_hud = true;
        pointer_frame(&mut app, Vec2::new(100.0, 100.0), true, false);
        app.world_mut()
            .resource_mut::<GameUiPointerCapture>()
            .over_hud = false;
        pointer_frame(&mut app, Vec2::new(105.0, 105.0), true, false);
        pointer_frame(&mut app, Vec2::new(105.0, 105.0), false, false);
        assert!(app
            .world()
            .resource::<PieceDataStore>()
            .selected_pieces
            .is_empty());
        pointer_frame(&mut app, Vec2::new(105.0, 105.0), true, false);
        app.world_mut()
            .resource_mut::<GameUiPointerCapture>()
            .over_hud = true;
        pointer_frame(&mut app, Vec2::new(155.0, 135.0), false, false);
        let store = app.world().resource::<PieceDataStore>();
        assert_eq!(
            store.pieces[&PieceId(0)].state.position,
            Vec2::new(150.0, 130.0)
        );
        assert!(store.held_pieces.is_empty());
    }

    #[test]
    fn missing_or_invalid_pointer_never_moves_a_piece_and_release_still_works() {
        let mut app = input_app();
        pointer_frame(&mut app, Vec2::new(100.0, 100.0), true, false);
        app.world_mut().resource_mut::<InputState>().mouse_position = None;
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .clear();
        app.update();
        assert_eq!(
            app.world().resource::<PieceDataStore>().pieces[&PieceId(0)]
                .state
                .position,
            Vec2::new(100.0, 100.0)
        );
        pointer_frame(&mut app, Vec2::splat(f32::NAN), false, false);
        assert!(app
            .world()
            .resource::<PieceDataStore>()
            .held_pieces
            .is_empty());
        assert_eq!(
            app.world_mut()
                .resource_mut::<PieceCollisionSystem>()
                .find_piece_at_position(Vec2::new(100.0, 100.0)),
            Some(PieceId(0))
        );
    }

    #[test]
    fn cancelled_box_restores_selection_and_clears_preview() {
        let mut app = input_app();
        pointer_frame(&mut app, Vec2::new(100.0, 100.0), true, true);
        pointer_frame(&mut app, Vec2::new(100.0, 100.0), false, true);
        pointer_frame(&mut app, Vec2::new(50.0, 50.0), true, false);
        pointer_frame(&mut app, Vec2::new(400.0, 200.0), true, false);
        app.world_mut().resource_mut::<InputState>().window_focused = false;
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .clear();
        app.update();
        let store = app.world().resource::<PieceDataStore>();
        assert_eq!(store.selected_pieces, HashSet::from([PieceId(0)]));
        assert!(store.preview_pieces.is_empty());
        assert!(app
            .world()
            .resource::<crate::interaction::PieceInteraction>()
            .selection_rect()
            .is_none());
    }

    #[test]
    fn selected_group_keeps_relative_stacking_and_depth_stays_bounded() {
        let mut app = input_app();
        for point in [Vec2::new(100.0, 100.0), Vec2::new(300.0, 100.0)] {
            pointer_frame(&mut app, point, true, true);
            pointer_frame(&mut app, point, false, true);
        }
        // Force depth compaction during a group grab.
        app.world_mut()
            .resource_mut::<PieceDataStore>()
            .next_z_order = 50.0;
        pointer_frame(&mut app, Vec2::new(100.0, 100.0), true, false);
        pointer_frame(&mut app, Vec2::new(100.0, 100.0), false, false);
        let store = app.world().resource::<PieceDataStore>();
        let z0 = store.transforms[&PieceId(0)].translation.z;
        let z1 = store.transforms[&PieceId(1)].translation.z;
        assert!(z0 < z1 && z1 < 50.0);
        let collision = app.world().resource::<PieceCollisionSystem>();
        assert_eq!(collision.pieces[&PieceId(0)].z_order, z0);
        assert_eq!(collision.pieces[&PieceId(1)].z_order, z1);
        assert_eq!(collision.rtree.size(), 2);
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
                &mut selection
            )
            .is_empty());
        let request = selection.latest.unwrap();
        assert!(interaction
            .update(
                frame(Vec2::new(120., 130.), true, false),
                &mut store,
                &mut selection
            )
            .is_empty());
        assert!(interaction
            .update(
                frame(Vec2::new(9., 5.), false, false),
                &mut store,
                &mut selection
            )
            .is_empty());
        selection.completed = Some(SelectionResult {
            request_id: request.request_id,
            mode: request.mode,
            piece_ids: vec![PieceId(0)],
            entities: vec![],
            error: None,
        });
        // Later pointer motion after release must not move the piece again.
        let commands = interaction.update(
            frame(Vec2::splat(500.), false, false),
            &mut store,
            &mut selection,
        );
        assert_eq!(
            commands,
            vec![
                PieceCommand::Grab(PieceId(0)),
                PieceCommand::Move {
                    id: PieceId(0),
                    position: Vec2::splat(2.)
                },
                PieceCommand::Release(PieceId(0))
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
        );
        let point = selection.latest.unwrap();
        selection.completed = Some(SelectionResult {
            request_id: point.request_id,
            mode: point.mode,
            piece_ids: vec![],
            entities: vec![],
            error: None,
        });
        interaction.update(
            frame(Vec2::new(400., 200.), true, false),
            &mut store,
            &mut selection,
        );
        let preview = selection.latest.unwrap();
        interaction.update(
            frame(Vec2::new(410., 210.), false, false),
            &mut store,
            &mut selection,
        );
        let final_request = selection.latest.unwrap();
        assert!(final_request.request_id > preview.request_id);
        selection.completed = Some(SelectionResult {
            request_id: preview.request_id,
            mode: preview.mode,
            piece_ids: vec![PieceId(0)],
            entities: vec![],
            error: None,
        });
        interaction.update(
            frame(Vec2::splat(500.), false, false),
            &mut store,
            &mut selection,
        );
        assert!(store.selected_pieces.is_empty());
        selection.completed = Some(SelectionResult {
            request_id: final_request.request_id,
            mode: final_request.mode,
            piece_ids: vec![PieceId(0), PieceId(1)],
            entities: vec![],
            error: None,
        });
        interaction.update(
            frame(Vec2::splat(500.), false, false),
            &mut store,
            &mut selection,
        );
        assert_eq!(
            store.selected_pieces,
            HashSet::from([PieceId(0), PieceId(1)])
        );
        assert!(interaction.selection_rect().is_none());
    }
}
