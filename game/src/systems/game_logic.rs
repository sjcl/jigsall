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
    definition: Option<Res<PuzzleDefinition>>,
    mut perf: ResMut<PerformanceMonitor>,
) {
    let start = perf.start_system_timing("apply_piece_commands");
    for request in commands.read() {
        store.apply_command(request.player, &request.command, definition.as_deref());
    }
    perf.end_system_timing("apply_piece_commands", start);
}

// Kept for independent producers of single release notifications. Bulk authority
// applies snap synchronously without a per-member PieceMoveCompleted/Placed event.
pub fn check_piece_placement_event_driven(
    mut moves: MessageReader<PieceMoveCompleted>,
    definition: Option<Res<PuzzleDefinition>>,
    mut store: ResMut<PieceDataStore>,
) {
    let Some(definition) = definition else {
        return;
    };
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
            store.selected_pieces.remove(&event.id);
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
    for event in placed.read() {
        let _ = event.id;
    }
    // Cached placed_count changes only on authority commits: O(1) even for 1M.
    if !store.is_empty() {
        game.puzzle_progress = store.placed_count as f32 / store.len() as f32;
        game.puzzle_completed = store.placed_count == store.len();
        if game.puzzle_completed {
            next.set(AppState::GameComplete);
        }
    }
    perf.end_system_timing("update_game_state_event_driven", start);
}
