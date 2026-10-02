//! egui screens and their state-specific schedule registration.
mod completion;
mod game_play;
mod game_setup;
mod grid;
mod menu;
mod overlays;
mod performance;
use bevy::prelude::*;
use bevy_egui::{EguiPlugin, EguiPrimaryContextPass};
use puzzella_game::resources::{AppState, GameCompleteSubState, GameSubState};

pub struct GameUiPlugin;
impl Plugin for GameUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(EguiPlugin::default()).add_systems(
            EguiPrimaryContextPass,
            (
                menu::draw_menu_ui.run_if(in_state(AppState::Menu)),
                game_setup::draw_game_setup_ui.run_if(in_state(AppState::GameSetup)),
                game_play::draw_game_ui.run_if(in_state(AppState::InGame)),
                performance::draw_performance_overlay.run_if(in_state(AppState::InGame)),
                game_play::draw_players_overlay
                    .run_if(in_state(AppState::InGame).and_then(tab_pressed)),
                overlays::draw_in_game_menu_ui.run_if(
                    in_state(GameSubState::Paused).or_else(in_state(GameCompleteSubState::Paused)),
                ),
                overlays::draw_generation_progress_ui.run_if(in_state(AppState::InGame)),
                completion::draw_completion_ui.run_if(in_state(GameCompleteSubState::Summary)),
                completion::draw_completed_puzzle_ui.run_if(
                    in_state(GameCompleteSubState::Viewing)
                        .or_else(in_state(GameCompleteSubState::Paused)),
                ),
            ),
        );
    }
}
fn tab_pressed(keys: Res<ButtonInput<KeyCode>>) -> bool {
    keys.pressed(KeyCode::Tab)
}
