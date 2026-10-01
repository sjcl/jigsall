//! egui screens and their state-specific schedule registration.
mod game_play;
mod game_setup;
mod grid;
mod menu;
mod overlays;
use bevy::prelude::*;
use bevy_egui::{EguiPlugin, EguiPrimaryContextPass};
use puzzella_game::resources::{AppState, GameSubState};

pub struct GameUiPlugin;
impl Plugin for GameUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(EguiPlugin::default()).add_systems(
            EguiPrimaryContextPass,
            (
                menu::draw_menu_ui.run_if(in_state(AppState::Menu)),
                game_setup::draw_game_setup_ui.run_if(in_state(AppState::GameSetup)),
                game_play::draw_game_ui.run_if(in_state(AppState::InGame)),
                game_play::draw_players_overlay
                    .run_if(in_state(AppState::InGame).and_then(tab_pressed)),
                overlays::draw_in_game_menu_ui.run_if(in_state(GameSubState::Paused)),
                overlays::draw_generation_progress_ui.run_if(in_state(AppState::InGame)),
                game_play::draw_completion_ui.run_if(in_state(AppState::GameComplete)),
            ),
        );
    }
}
fn tab_pressed(keys: Res<ButtonInput<KeyCode>>) -> bool {
    keys.pressed(KeyCode::Tab)
}
