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
        if outcome == CommandOutcome::Grabbed {
            store.next_z_order += 0.1;
            let z = store.next_z_order + 10.0;
            if let Some(transform) = store.transforms.get_mut(&id) {
                transform.translation.z = z;
            }
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
    mut collision: ResMut<PieceCollisionSystem>,
    mut entities: Query<(&PuzzlePiece, &mut Transform)>,
    perf: Res<PerformanceMonitor>,
) {
    let mut ids: Vec<_> = store.dirty_pieces.drain().collect();
    ids.sort_unstable();
    for id in ids {
        let Some(piece) = store.pieces.get(&id) else {
            continue;
        };
        let state = piece.state;
        if let Some(transform) = store.transforms.get_mut(&id) {
            transform.translation.x = state.position.x;
            transform.translation.y = state.position.y;
            if state.placed {
                transform.translation.z = -20.0;
            }
        }
        if state.placed {
            collision.dragging_pieces.remove(&id);
            collision.remove_piece(id);
        } else {
            if state.held_by.is_some() {
                collision.start_dragging_piece(id, &perf.debug_level);
            }
            collision.update_piece_position(id, state.position);
            if state.held_by.is_none() {
                collision.stop_dragging_piece(id, &perf.debug_level);
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
