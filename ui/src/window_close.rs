//! Route native close requests through the same save flow as the puzzle menu.
//! The app disables WindowPlugin's automatic close_when_requested system.
use crate::{localization::Localization, persistence::SaveDialogs};
use bevy::{
    prelude::*,
    window::{ClosingWindow, PrimaryWindow, WindowCloseRequested},
};
use jigsall_core::PuzzleDefinition;
use jigsall_game::{
    network::runtime::{NetworkStatus, RuntimeRole},
    persistence::runtime::PersistenceState,
    resources::{AppState, GameCompleteSubState, GameSubState, PieceDataStore},
};

pub(crate) fn exit_dialog_pending(dialogs: Res<SaveDialogs>, status: Res<NetworkStatus>) -> bool {
    dialogs.exit_pending()
        && (status.role != Some(RuntimeRole::Client) || status.has_disconnected_game())
}

/// PreUpdate runs before state transitions, so pausing releases the local drag
/// before the save dialog can request a checkpoint.
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_close_requests(
    mut commands: Commands,
    mut requests: MessageReader<WindowCloseRequested>,
    windows: Query<Option<&PrimaryWindow>, With<Window>>,
    closing: Query<Entity, (With<ClosingWindow>, Without<PrimaryWindow>)>,
    app_state: Res<State<AppState>>,
    game_state: Option<Res<State<GameSubState>>>,
    definition: Option<Res<PuzzleDefinition>>,
    store: Res<PieceDataStore>,
    status: Res<NetworkStatus>,
    i18n: Res<Localization>,
    mut dialogs: ResMut<SaveDialogs>,
    mut persistence: ResMut<PersistenceState>,
    mut next_game: ResMut<NextState<GameSubState>>,
    mut next_complete: ResMut<NextState<GameCompleteSubState>>,
    mut exit: MessageWriter<AppExit>,
) {
    // Preserve Bevy's two-frame closing behavior for secondary windows.
    for window in &closing {
        commands.entity(window).despawn();
    }
    let mut primary_requested = false;
    for request in requests.read() {
        let Ok(primary) = windows.get(request.window) else {
            continue;
        };
        if primary.is_some() {
            primary_requested = true;
        } else {
            commands.entity(request.window).try_insert(ClosingWindow);
        }
    }
    if !primary_requested {
        return;
    }
    let playing = *app_state.get() == AppState::InGame
        && (status.has_disconnected_game()
            || game_state.is_some_and(|state| {
                matches!(state.get(), GameSubState::Playing | GameSubState::Paused)
            }));
    let completed = *app_state.get() == AppState::GameComplete;
    if (status.role == Some(RuntimeRole::Client) && !status.has_disconnected_game())
        || definition.is_none()
        || store.is_empty()
        || !(playing || completed)
    {
        exit.write(AppExit::Success);
        return;
    }
    dialogs.request_window_exit(&mut persistence, &i18n);
    if completed {
        next_complete.set(GameCompleteSubState::Paused);
    } else {
        next_game.set(GameSubState::Paused);
    }
}
