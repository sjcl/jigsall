use crate::{components::*, gameplay::*, networking::ClientCommand, resources::*};
use bevy::prelude::*;
use bevy_egui::EguiContexts;
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
    mut collision: ResMut<PieceCollisionSystem>,
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
        pressed: mouse.pressed(MouseButton::Left),
        just_pressed: mouse.just_pressed(MouseButton::Left),
        ctrl: keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight),
        over_ui,
        focused: input.window_focused,
    };
    for command in interaction.update(frame, &mut store, &mut collision) {
        commands.write(ClientCommand {
            player: LOCAL_PLAYER,
            command,
        });
    }
    perf.end_system_timing("handle_piece_input", start);
}

pub fn release_local_drag(
    mut input: ResMut<InputState>,
    mut interaction: ResMut<crate::interaction::PieceInteraction>,
    mut store: ResMut<PieceDataStore>,
    mut commands: MessageWriter<ClientCommand>,
) {
    for command in interaction.cancel(&mut store) {
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
pub fn render_selection_box(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
    interaction: Res<crate::interaction::PieceInteraction>,
    mut boxes: Query<(Entity, &mut Transform), With<SelectionBox>>,
) {
    let rect = interaction
        .selection_rect()
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
        &PieceShape,
        Has<SelectedPiece>,
        Has<SelectionPreview>,
    )>,
    outlines: Query<Entity, With<PieceOutline>>,
    stroke_cache: Res<StrokeMeshCache>,
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
    for (entity, shape, selected, preview) in &pieces {
        if !selected && !preview {
            continue;
        }
        if let Some(mesh) = stroke_cache.stroke_meshes.get(&shape.shape_hash) {
            let material = if selected {
                materials.selected_material.clone()
            } else {
                materials.preview_material.clone()
            };
            commands.spawn((
                Mesh2d(mesh.clone()),
                MeshMaterial2d(material),
                // Outlines are overlays: they stay above every piece and below
                // the selection rectangle throughout depth compaction.
                Transform::from_xyz(0.0, 0.0, 60.0),
                PieceOutline,
                ChildOf(entity),
            ));
        }
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

    fn pointer_frame(app: &mut App, point: Vec2, pressed: bool, ctrl: bool) {
        let mut input = app.world_mut().resource_mut::<InputState>();
        input.mouse_position = Some(point);
        input.window_focused = true;
        input.cursor_screen_position = Some(Vec2::ZERO);
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
    }

    fn input_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
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
                        shape: PieceShapeData {
                            vertices,
                            indices,
                            shape_hash: String::new(),
                        },
                        mesh: default(),
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
}
