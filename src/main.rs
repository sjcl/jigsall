mod asset_reader;
mod components;
mod game;
mod gameplay;
mod interaction;
mod jigsaw_shapes;
mod networking;
#[cfg(any(test, feature = "cpu-picking-debug"))]
#[allow(dead_code)]
mod piece_geometry;
mod puzzle;
mod puzzle_utils;
mod resources;
mod selection;
mod systems;
mod ui;

use asset_reader::DirectFileAssetPlugin;
use bevy::prelude::*;
use bevy_egui::{self, EguiPrimaryContextPass};
use game::GamePlugin;
use resources::{AppState, GameSubState};
use systems::tab_pressed;
use ui::*;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(bevy_egui::EguiPlugin::default())
        .add_plugins(DirectFileAssetPlugin)
        .add_plugins(GamePlugin)
        .add_systems(
            EguiPrimaryContextPass,
            (
                // Menu state UI
                draw_menu_ui.run_if(in_state(AppState::Menu)),
                // GameSetup state UI
                draw_game_setup_ui.run_if(in_state(AppState::GameSetup)),
                // InGame state UI
                draw_game_ui.run_if(in_state(AppState::InGame)),
                draw_players_overlay.run_if(in_state(AppState::InGame).and_then(tab_pressed)),
                draw_in_game_menu_ui.run_if(in_state(GameSubState::Paused)),
                draw_generation_progress_ui.run_if(in_state(AppState::InGame)),
                // GameComplete state UI
                draw_completion_ui.run_if(in_state(AppState::GameComplete)),
            ),
        )
        .run();
}
