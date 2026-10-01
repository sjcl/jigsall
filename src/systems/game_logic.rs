use crate::{components::*, gameplay::*, networking::ClientCommand, resources::*};
use bevy::prelude::*;

pub fn escape_just_pressed(keys: Res<ButtonInput<KeyCode>>) -> bool {
    keys.just_pressed(KeyCode::Escape)
}
pub fn tab_pressed(keys: Res<ButtonInput<KeyCode>>) -> bool {
    keys.pressed(KeyCode::Tab)
}
pub fn toggle_game_menu(
    state: Res<State<GameSubState>>,
    mut next: ResMut<NextState<GameSubState>>,
) {
    match *state.get() {
        GameSubState::Playing => next.set(GameSubState::Paused),
        GameSubState::Paused => next.set(GameSubState::Playing),
        GameSubState::Initializing => {}
    }
}

pub fn apply_piece_commands(
    mut commands: MessageReader<ClientCommand>,
    mut store: ResMut<PieceDataStore>,
    mut batch: ResMut<BatchManager>,
    mut moves: MessageWriter<PieceMoveCompleted>,
    mut perf: ResMut<PerformanceMonitor>,
) {
    let start = perf.start_system_timing("apply_piece_commands");
    for request in commands.read() {
        let id = request.command.piece_id();
        let Some(piece) = store.pieces.get_mut(&id) else {
            continue;
        };
        let Some(outcome) = apply_piece_command(&mut piece.state, request.player, &request.command)
        else {
            continue;
        };
        store.dirty_pieces.insert(id);
        if outcome == CommandOutcome::Grabbed {
            store.held_pieces.insert(id);
        }
        if outcome == CommandOutcome::Released {
            store.held_pieces.remove(&id);
        }
        if outcome == CommandOutcome::Grabbed && store.bring_piece_to_front(id) {
            batch.needs_rebuild = true;
        }
        if outcome == CommandOutcome::Released {
            moves.write(PieceMoveCompleted { id });
        }
    }
    perf.end_system_timing("apply_piece_commands", start);
}

pub fn check_piece_placement_event_driven(
    mut moves: MessageReader<PieceMoveCompleted>,
    mut placed: MessageWriter<PiecePlacedEvent>,
    definition: Option<Res<PuzzleDefinition>>,
    mut store: ResMut<PieceDataStore>,
    mut perf: ResMut<PerformanceMonitor>,
) {
    let Some(definition) = definition else {
        return;
    };
    let start = perf.start_system_timing("check_piece_placement_event_driven");
    for event in moves.read() {
        let Some(piece) = store.pieces.get_mut(&event.id) else {
            continue;
        };
        if snap_piece(
            &piece.definition,
            &mut piece.state,
            definition.snap_distance,
        ) {
            store.placed_pieces.insert(event.id);
            store.selected_pieces.remove(&event.id);
            store.preview_pieces.remove(&event.id);
            store.dirty_pieces.insert(event.id);
            placed.write(PiecePlacedEvent { id: event.id });
        }
    }
    perf.end_system_timing("check_piece_placement_event_driven", start);
}

/// Only changed IDs update local render/collision caches. State is the authority.
pub fn project_piece_states(
    mut store: ResMut<PieceDataStore>,
    #[cfg(any(test, feature = "cpu-picking-debug"))] mut collision: ResMut<PieceCollisionSystem>,
    mut entities: Query<(&PuzzlePiece, &mut Transform)>,
    #[cfg(any(test, feature = "cpu-picking-debug"))] perf: Res<PerformanceMonitor>,
) {
    let mut ids: Vec<_> = store.dirty_pieces.drain().collect();
    ids.sort_unstable();
    for id in ids {
        let Some(piece) = store.pieces.get(&id) else {
            continue;
        };
        let state = piece.state;
        #[cfg(any(test, feature = "cpu-picking-debug"))]
        let bounds = piece.render.bounds;
        if let Some(transform) = store.transforms.get_mut(&id) {
            transform.translation.x = state.position.x;
            transform.translation.y = state.position.y;
            if state.placed {
                transform.translation.z = -20.0;
            }
        }
        #[cfg(any(test, feature = "cpu-picking-debug"))]
        {
            if state.placed {
                collision.dragging_pieces.remove(&id);
                collision.remove_piece(id);
            } else {
                if let Some(transform) = store.transforms.get(&id) {
                    collision.update_piece_z_order(id, transform.translation.z);
                }
                if state.held_by.is_some() {
                    collision.start_dragging_piece(id, &perf.debug_level);
                }
                collision.update_piece_position(id, state.position, bounds);
                if state.held_by.is_none() {
                    collision.stop_dragging_piece(id, &perf.debug_level);
                }
            }
        }
        if let Some(&entity) = store.temporary_entities.get(&id) {
            if let Ok((_, mut transform)) = entities.get_mut(entity) {
                if let Some(projected) = store.transforms.get(&id) {
                    *transform = *projected;
                }
            }
        }
    }
}

pub fn update_game_state_event_driven(
    mut placed: MessageReader<PiecePlacedEvent>,
    store: Res<PieceDataStore>,
    mut game: ResMut<GameData>,
    mut next: ResMut<NextState<AppState>>,
    mut perf: ResMut<PerformanceMonitor>,
) {
    let start = perf.start_system_timing("update_game_state_event_driven");
    let placed_count = placed
        .read()
        .filter(|event| store.placed_pieces.contains(&event.id))
        .count();
    if placed_count > 0 && !store.pieces.is_empty() {
        game.puzzle_progress = store.placed_pieces.len() as f32 / store.pieces.len() as f32;
        game.puzzle_completed = store.placed_pieces.len() == store.pieces.len();
        if game.puzzle_completed {
            next.set(AppState::GameComplete);
        }
    }
    perf.end_system_timing("update_game_state_event_driven", start);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::systems::batching::*;
    use bevy::state::app::StatesPlugin;

    /// Exercise the command -> snap -> state -> batch path without a GPU.
    #[test]
    fn snap_counts_all_pieces_and_survives_missing_temporary_entities() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, StatesPlugin))
            .init_state::<AppState>()
            .init_resource::<PieceDataStore>()
            .init_resource::<PieceCollisionSystem>()
            .init_resource::<PieceIdManager>()
            .init_resource::<BatchManager>()
            .init_resource::<PerformanceMonitor>()
            .init_resource::<GameData>()
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<ColorMaterial>>()
            .add_message::<ClientCommand>()
            .add_message::<PieceMoveCompleted>()
            .add_message::<PiecePlacedEvent>()
            .add_message::<BatchRebuildRequest>()
            .insert_resource(PuzzleDefinition {
                generator_version: GENERATOR_VERSION,
                seed: 42,
                grid_size: UVec2::new(2, 1),
                image_size: UVec2::new(200, 100),
                snap_distance: 10.0,
            })
            .add_systems(
                Update,
                (
                    apply_piece_commands,
                    check_piece_placement_event_driven,
                    project_piece_states,
                    update_game_state_event_driven,
                    reconcile_piece_rendering,
                    cleanup_temporary_entities,
                    create_temporary_entities,
                    handle_batch_rebuild_requests,
                )
                    .chain(),
            );
        let mesh = app
            .world_mut()
            .resource_mut::<Assets<Mesh>>()
            .add(Mesh::from(Rectangle::new(100.0, 100.0)));
        let material = app
            .world_mut()
            .resource_mut::<Assets<ColorMaterial>>()
            .add(ColorMaterial::default());
        for index in 0..2 {
            let piece = app
                .world()
                .resource::<PuzzleDefinition>()
                .piece(index, Vec2::splat(500.0));
            app.world_mut()
                .resource_mut::<PieceDataStore>()
                .add_piece(StoredPieceData {
                    definition: piece,
                    state: PieceState::new(Vec2::splat(500.0)),
                    render: PieceRenderData {
                        bounds: Rect::new(-50.0, -50.0, 50.0, 50.0),
                        shape: PieceShapeData {
                            vertices: vec![],
                            indices: vec![],
                            shape_hash: String::new(),
                        },
                        mesh: mesh.clone(),
                        material: material.clone(),
                    },
                });
            app.world_mut()
                .resource_mut::<BatchManager>()
                .add_piece(PieceId(index));
        }
        app.update();
        assert!(app
            .world()
            .resource::<PieceDataStore>()
            .temporary_entities
            .is_empty());
        for index in 0..2 {
            let id = PieceId(index);
            let position = app.world().resource::<PieceDataStore>().pieces[&id]
                .definition
                .correct_position;
            for command in [
                PieceCommand::Grab(id),
                PieceCommand::Move { id, position },
                PieceCommand::Release(id),
            ] {
                app.world_mut().write_message(ClientCommand {
                    player: LOCAL_PLAYER,
                    command,
                });
            }
            app.update();
            let store = app.world().resource::<PieceDataStore>();
            assert!(store.pieces[&id].state.placed);
            assert_eq!(store.transforms[&id].translation.truncate(), position);
            assert_eq!(
                app.world().resource::<GameData>().puzzle_progress,
                (index + 1) as f32 / 2.0
            );
            assert_eq!(
                app.world().resource::<BatchManager>().batched_pieces.len(),
                2
            );
        }
        assert!(app.world().resource::<GameData>().puzzle_completed);
        app.update();
        assert_eq!(
            *app.world().resource::<State<AppState>>().get(),
            AppState::GameComplete
        );
    }
}
