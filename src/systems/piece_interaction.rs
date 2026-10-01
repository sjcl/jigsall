use crate::{components::*, gameplay::*, networking::ClientCommand, resources::*};
use bevy::prelude::*;
use bevy_egui::EguiContexts;
use std::collections::HashSet;

/// Reuses R-tree/triangle hit testing; generates commands, never moves a Transform.
// Bevy system parameters describe independent ECS access.
#[allow(clippy::too_many_arguments)]
pub fn handle_piece_input(
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut input: ResMut<InputState>,
    mut store: ResMut<PieceDataStore>,
    mut collision: ResMut<PieceCollisionSystem>,
    mut commands: MessageWriter<ClientCommand>,
    mut contexts: EguiContexts,
    mut perf: ResMut<PerformanceMonitor>,
) {
    let start = perf.start_system_timing("handle_piece_input");
    let point = input.mouse_position;
    let ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
    let ui_wants_pointer = contexts
        .ctx_mut()
        .is_ok_and(|ctx| ctx.is_pointer_over_egui() || ctx.egui_wants_pointer_input());
    if mouse.just_pressed(MouseButton::Left)
        && !ui_wants_pointer
        && input.cursor_screen_position.is_some()
    {
        let hit = collision.find_piece_at_position(point);
        if let Some(id) = hit {
            if ctrl {
                if !store.selected_pieces.remove(&id) {
                    store.selected_pieces.insert(id);
                }
            } else {
                if !store.selected_pieces.contains(&id) {
                    store.selected_pieces.clear();
                    store.selected_pieces.insert(id);
                }
                let mut ids: Vec<_> = store.selected_pieces.iter().copied().collect();
                ids.sort_unstable();
                for id in ids {
                    if let Some(piece) = store.pieces.get(&id) {
                        if !piece.state.placed && piece.state.held_by.is_none() {
                            input.drag_offsets.insert(id, piece.state.position - point);
                            commands.write(ClientCommand {
                                player: LOCAL_PLAYER,
                                command: PieceCommand::Grab(id),
                            });
                        }
                    }
                }
                input.is_dragging_piece = !input.drag_offsets.is_empty();
                input.selection_mode = if input.drag_offsets.len() > 1 {
                    SelectionMode::MultiDrag
                } else {
                    SelectionMode::Single
                };
            }
        } else {
            if !ctrl {
                store.selected_pieces.clear();
            }
            input.selection_mode = SelectionMode::BoxSelection;
            input.selection_start = Some(point);
            input.selection_current = Some(point);
        }
    }
    if mouse.pressed(MouseButton::Left) {
        if input.selection_mode == SelectionMode::BoxSelection {
            input.selection_current = Some(point);
            if let Some(begin) = input.selection_start {
                let rect = Rect {
                    min: begin.min(point),
                    max: begin.max(point),
                };
                store.preview_pieces = collision
                    .find_pieces_with_detailed_rect_intersection(rect)
                    .into_iter()
                    .collect();
            }
        }
        let mut moves: Vec<_> = input
            .drag_offsets
            .iter()
            .map(|(&id, &offset)| (id, point + offset))
            .collect();
        moves.sort_unstable_by_key(|(id, _)| *id);
        for (id, position) in moves {
            commands.write(ClientCommand {
                player: LOCAL_PLAYER,
                command: PieceCommand::Move { id, position },
            });
        }
    } else {
        // Also release after focus loss, even if just_released was missed.
        if input.selection_mode == SelectionMode::BoxSelection {
            let preview = std::mem::take(&mut store.preview_pieces);
            store.selected_pieces.extend(preview);
            input.selection_start = None;
            input.selection_current = None;
        }
        let mut ids: Vec<_> = input.drag_offsets.drain().map(|(id, _)| id).collect();
        ids.sort_unstable();
        for id in ids {
            commands.write(ClientCommand {
                player: LOCAL_PLAYER,
                command: PieceCommand::Release(id),
            });
        }
        input.is_dragging_piece = false;
        input.selection_mode = SelectionMode::Single;
    }
    perf.end_system_timing("handle_piece_input", start);
}

pub fn release_local_drag(
    mut input: ResMut<InputState>,
    mut store: ResMut<PieceDataStore>,
    mut commands: MessageWriter<ClientCommand>,
) {
    let mut ids: Vec<_> = input.drag_offsets.drain().map(|(id, _)| id).collect();
    ids.sort_unstable();
    for id in ids {
        commands.write(ClientCommand {
            player: LOCAL_PLAYER,
            command: PieceCommand::Release(id),
        });
    }
    input.is_dragging_piece = false;
    input.selection_mode = SelectionMode::Single;
    input.selection_start = None;
    input.selection_current = None;
    store.preview_pieces.clear();
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
    input_state: Res<InputState>,
    selection_box_query: Query<(Entity, &mut Transform), (With<SelectionBox>, Without<MainCamera>)>,
    mut perf_monitor: ResMut<PerformanceMonitor>,
) {
    let start_time = perf_monitor.start_system_timing("render_selection_box");

    // 既存の選択ボックスを削除して毎フレーム更新
    for (entity, _) in selection_box_query.iter() {
        commands.entity(entity).despawn();
    }

    // 範囲選択中の選択ボックスを描画
    if let (Some(start), Some(current)) =
        (input_state.selection_start, input_state.selection_current)
    {
        // 矩形の大きさを計算
        let width = (current.x - start.x).abs();
        let height = (current.y - start.y).abs();
        let center_x = (start.x + current.x) / 2.0;
        let center_y = (start.y + current.y) / 2.0;

        if width > 1.0 && height > 1.0 {
            // 選択範囲の矩形メッシュを作成
            let mesh = Mesh::from(Rectangle::new(width, height));
            let material = ColorMaterial::from(Color::srgba(0.3, 0.6, 1.0, 0.3)); // 半透明の青

            // 選択ボックスエンティティを生成
            let selection_box_entity = commands
                .spawn((
                    Mesh2d(meshes.add(mesh)),
                    MeshMaterial2d(materials.add(material)),
                    Transform::from_translation(Vec3::new(center_x, center_y, 100.0)), // 最前面に表示
                    SelectionBox,
                ))
                .id();
            println!("🖼️ Created selection box {:?}", selection_box_entity);
        }
    }

    perf_monitor.end_system_timing("render_selection_box", start_time);
}

/// 選択されたピースのハイライト表示システム（最適化版）
#[allow(clippy::too_many_arguments)]
pub fn highlight_selected_pieces(
    mut commands: Commands,
    selected_pieces_query: Query<Entity, (With<SelectedPiece>, With<PuzzlePiece>)>,
    preview_pieces_query: Query<Entity, (With<SelectionPreview>, With<PuzzlePiece>)>,
    mut piece_query: Query<&mut MeshMaterial2d<ColorMaterial>, With<PuzzlePiece>>,
    piece_transform_query: Query<&Transform, With<PuzzlePiece>>,
    piece_shape_query: Query<&PieceShape, With<PuzzlePiece>>,
    existing_outline_query: Query<Entity, With<PieceOutline>>,
    stroke_cache: Res<StrokeMeshCache>,
    store: Res<PieceDataStore>,
    mut perf_monitor: ResMut<PerformanceMonitor>,
    mut highlight_state: ResMut<HighlightState>,
    highlight_materials: Res<HighlightMaterials>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    let start_time = perf_monitor.start_system_timing("highlight_selected_pieces");

    // 現在の選択状態を取得
    let change_detection_start = perf_monitor.start_system_timing("highlight_change_detection");
    let current_selected: HashSet<Entity> = selected_pieces_query.iter().collect();
    let current_preview: HashSet<Entity> = preview_pieces_query.iter().collect();

    // 選択状態変更を検出
    let selection_changed = current_selected != highlight_state.last_selected_pieces;
    let preview_changed = current_preview != highlight_state.last_preview_pieces;

    highlight_state.frame_count += 1;
    highlight_state.selection_changed = selection_changed;
    highlight_state.preview_changed = preview_changed;

    perf_monitor.end_system_timing("highlight_change_detection", change_detection_start);

    // 変更がない場合は早期リターン
    if !selection_changed && !preview_changed {
        // 定期的にスキップ情報を出力
        if perf_monitor.debug_level == PerformanceDebugLevel::High
            && highlight_state.frame_count.is_multiple_of(300)
        {
            println!(
                "🚀 HIGHLIGHT OPTIMIZATION: Skipped {} frames (no changes)",
                highlight_state.frame_count
            );
        }
        perf_monitor.end_system_timing("highlight_selected_pieces", start_time);
        return;
    }

    // 変更があった場合のみ処理を実行
    if perf_monitor.debug_level == PerformanceDebugLevel::Medium {
        println!(
            "🔄 HIGHLIGHT UPDATE: Selection changed={}, Preview changed={}, Frame={}",
            selection_changed, preview_changed, highlight_state.frame_count
        );
    }

    // 変更されたピースのみ通常の色に戻す（最適化）
    let reset_color_start = perf_monitor.start_system_timing("highlight_reset_colors");
    let mut reset_count = 0;

    // 前回選択されていたが今回選択されていないピースの色をリセット
    for &entity in &highlight_state.last_selected_pieces {
        if !current_selected.contains(&entity) {
            if let Ok(material_handle) = piece_query.get_mut(entity) {
                if let Some(mut material) = materials.get_mut(&material_handle.0) {
                    material.color = Color::srgba(1.0, 1.0, 1.0, 1.0);
                    reset_count += 1;
                }
            }
        }
    }

    // 前回プレビューだったが今回プレビューでないピースの色をリセット
    for &entity in &highlight_state.last_preview_pieces {
        if !current_preview.contains(&entity) {
            if let Ok(material_handle) = piece_query.get_mut(entity) {
                if let Some(mut material) = materials.get_mut(&material_handle.0) {
                    material.color = Color::srgba(1.0, 1.0, 1.0, 1.0);
                    reset_count += 1;
                }
            }
        }
    }

    perf_monitor.end_system_timing("highlight_reset_colors", reset_color_start);

    // 既存の枠線を削除（変更があった場合のみ）
    let outline_removal_start = perf_monitor.start_system_timing("highlight_outline_removal");
    for outline_entity in existing_outline_query.iter() {
        commands.entity(outline_entity).despawn();
    }
    perf_monitor.end_system_timing("highlight_outline_removal", outline_removal_start);

    // プレビュー中のピースにストロークハイライトを追加（共有マテリアル使用）
    let preview_outline_start = perf_monitor.start_system_timing("highlight_preview_outlines");
    for entity in preview_pieces_query.iter() {
        if let (Ok(_transform), Ok(piece_shape)) = (
            piece_transform_query.get(entity),
            piece_shape_query.get(entity),
        ) {
            // キャッシュからストロークメッシュを取得
            if let Some(stroke_mesh_handle) =
                stroke_cache.stroke_meshes.get(&piece_shape.shape_hash)
            {
                // 共有マテリアルを使用（マテリアル作成のオーバーヘッドを削減）
                let outline_entity = commands
                    .spawn((
                        Mesh2d(stroke_mesh_handle.clone()),
                        MeshMaterial2d(highlight_materials.preview_material.clone()),
                        Transform {
                            translation: Vec3::new(0.0, 0.0, -0.1), // 親からの相対位置
                            rotation: Quat::IDENTITY,
                            scale: Vec3::ONE,
                        },
                        PieceOutline {},
                    ))
                    .id();

                // 輪郭線をピースの子エンティティとして設定
                commands.entity(entity).add_child(outline_entity);
            }
        }
    }
    perf_monitor.end_system_timing("highlight_preview_outlines", preview_outline_start);

    // 選択されたピースにストロークハイライトを追加（共有マテリアル使用）
    let selected_outline_start = perf_monitor.start_system_timing("highlight_selected_outlines");
    for entity in selected_pieces_query.iter() {
        if let (Ok(_transform), Ok(piece_shape)) = (
            piece_transform_query.get(entity),
            piece_shape_query.get(entity),
        ) {
            // キャッシュからストロークメッシュを取得
            if let Some(stroke_mesh_handle) =
                stroke_cache.stroke_meshes.get(&piece_shape.shape_hash)
            {
                // 共有マテリアルを使用（マテリアル作成のオーバーヘッドを削減）
                let outline_entity = commands
                    .spawn((
                        Mesh2d(stroke_mesh_handle.clone()),
                        MeshMaterial2d(highlight_materials.selected_material.clone()),
                        Transform {
                            translation: Vec3::new(0.0, 0.0, -0.05), // 親からの相対位置（プレビューより上）
                            rotation: Quat::IDENTITY,
                            scale: Vec3::ONE,
                        },
                        PieceOutline {},
                    ))
                    .id();

                // 輪郭線をピースの子エンティティとして設定
                commands.entity(entity).add_child(outline_entity);
            }
        }
    }
    perf_monitor.end_system_timing("highlight_selected_outlines", selected_outline_start);

    // 最適化の統計情報を出力（移動前に値を取得）
    if perf_monitor.debug_level == PerformanceDebugLevel::High {
        let total_pieces = store.pieces.len();
        let selected_count = current_selected.len();
        let preview_count = current_preview.len();
        println!("🔥 HIGHLIGHT OPTIMIZATION: Reset {} pieces, Current selected: {}, Current preview: {}, Total pieces: {}",
            reset_count, selected_count, preview_count, total_pieces);
    }

    // 次フレーム用に現在の選択状態を保存
    let state_update_start = perf_monitor.start_system_timing("highlight_state_update");
    highlight_state.last_selected_pieces = current_selected;
    highlight_state.last_preview_pieces = current_preview;
    perf_monitor.end_system_timing("highlight_state_update", state_update_start);

    perf_monitor.end_system_timing("highlight_selected_pieces", start_time);
}

/// システム条件: ボックス選択モード中、または選択ボックスのクリーンアップが必要
pub fn should_render_selection_box(
    input_state: Res<InputState>,
    selection_box_query: Query<Entity, With<SelectionBox>>,
) -> bool {
    matches!(input_state.selection_mode, SelectionMode::BoxSelection)
        || !selection_box_query.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::systems::game_logic::{apply_piece_commands, project_piece_states};

    fn pointer_frame(app: &mut App, point: Vec2, pressed: bool, ctrl: bool) {
        let mut input = app.world_mut().resource_mut::<InputState>();
        input.mouse_position = point;
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

    #[test]
    fn ctrl_box_selection_and_multi_drag_work_without_render_entities() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<InputState>()
            .init_resource::<PieceDataStore>()
            .init_resource::<PieceCollisionSystem>()
            .init_resource::<PerformanceMonitor>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<bevy_egui::EguiUserTextures>()
            .add_message::<ClientCommand>()
            .add_message::<PieceMoveCompleted>()
            .add_systems(
                Update,
                (
                    handle_piece_input,
                    apply_piece_commands,
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
}
