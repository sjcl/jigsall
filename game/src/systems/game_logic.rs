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
    persistence: Res<crate::persistence::runtime::PersistenceState>,
) {
    if persistence.busy || persistence.title_dialog_open {
        return;
    }
    match *state.get() {
        GameSubState::Playing => next.set(GameSubState::Paused),
        GameSubState::Paused => next.set(GameSubState::Playing),
        GameSubState::Initializing => {}
    }
}

pub fn toggle_completed_puzzle_menu(
    state: Res<State<GameCompleteSubState>>,
    mut next: ResMut<NextState<GameCompleteSubState>>,
    persistence: Res<crate::persistence::runtime::PersistenceState>,
) {
    if persistence.busy || persistence.title_dialog_open {
        return;
    }
    match *state.get() {
        GameCompleteSubState::Viewing => next.set(GameCompleteSubState::Paused),
        GameCompleteSubState::Paused => next.set(GameCompleteSubState::Viewing),
        GameCompleteSubState::Summary => {}
    }
}

pub fn apply_piece_commands(
    local_player: Res<LocalPlayerId>,
    mut commands: MessageReader<ClientCommand>,
    mut store: ResMut<PieceDataStore>,
    definition: Option<Res<PuzzleDefinition>>,
    mut interaction: Option<ResMut<crate::interaction::PieceInteraction>>,
    input: Option<Res<InputState>>,
    mut perf: ResMut<PerformanceMonitor>,
) {
    let start = perf.start_system_timing("apply_piece_commands");
    for request in commands.read() {
        let applied = store.apply_command(
            request.player,
            &request.command,
            definition.as_deref(),
            local_player.0,
        );
        if applied.drag_rebased {
            if let (Some(interaction), Some(pointer)) = (
                interaction.as_deref_mut(),
                input.as_ref().and_then(|input| input.mouse_position),
            ) {
                interaction.accept_drag_rotation(&request.command, pointer);
            }
        }
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
        store.snap_unheld_component(event.id, &definition);
    }
}

pub fn update_game_state_event_driven(
    mut placed: MessageReader<PiecePlacedEvent>,
    state: Option<Res<State<GameSubState>>>,
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
        if game.puzzle_completed
            && state
                .as_ref()
                .is_none_or(|s| *s.get() != GameSubState::Initializing)
        {
            next.set(AppState::GameComplete);
        }
    }
    perf.end_system_timing("update_game_state_event_driven", start);
}
