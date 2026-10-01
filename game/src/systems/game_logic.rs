use crate::{components::*, resources::*};
use bevy::prelude::*;
use puzzella_core::ClientCommand;
use puzzella_core::*;

pub fn escape_just_pressed(keys: Res<ButtonInput<KeyCode>>) -> bool {
    keys.just_pressed(KeyCode::Escape)
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
        let Some(mut state) = store.state(id) else {
            continue;
        };
        let Some(outcome) = apply_piece_command(&mut state, request.player, &request.command)
        else {
            continue;
        };
        store.set_state(id, state);
        if outcome == CommandOutcome::Grabbed {
            store.bring_piece_to_front(id);
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
        let Some(mut state) = store.state(event.id) else {
            continue;
        };
        if snap_piece(
            &definition.piece(event.id.0, Vec2::ZERO),
            &mut state,
            definition.snap_distance,
        ) {
            store.set_state(event.id, state);
            store.highlights_dirty = true;
            store.selected_pieces.remove(&event.id);
            store.preview_pieces.remove(&event.id);
            store.dirty_pieces.insert(event.id);
            placed.write(PiecePlacedEvent { id: event.id });
        }
    }
    perf.end_system_timing("check_piece_placement_event_driven", start);
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
        .filter(|event| store.state(event.id).is_some_and(|s| s.placed))
        .count();
    if placed_count > 0 && !store.is_empty() {
        game.puzzle_progress = store.placed_count as f32 / store.len() as f32;
        game.puzzle_completed = store.placed_count == store.len();
        if game.puzzle_completed {
            next.set(AppState::GameComplete);
        }
    }
    perf.end_system_timing("update_game_state_event_driven", start);
}
