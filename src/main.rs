mod game;
// mod networking; // 一時的に無効化
mod puzzle;
mod ui;
mod components;
mod resources;
mod systems;
mod jigsaw_shapes;
mod puzzle_utils;
mod asset_reader;

use bevy::prelude::*;
use bevy_egui::{self, EguiPrimaryContextPass};
use game::GamePlugin;
// use networking::NetworkingPlugin;
use resources::AppState;
use ui::*;
use systems::tab_pressed;
use asset_reader::DirectFileAssetPlugin;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        // .add_plugins(MeshPickingPlugin) // 一時的に無効化 - DefaultPluginsに含まれている可能性
        .add_plugins(bevy_egui::EguiPlugin::default())
        .add_plugins(DirectFileAssetPlugin)
        .add_plugins(GamePlugin)
        // .add_plugins(NetworkingPlugin) // 一時的に無効化
        .add_systems(
            EguiPrimaryContextPass,
            (
                // Menu state UI
                draw_menu_ui.run_if(in_state(AppState::Menu)),
                
                // GameSetup state UI
                draw_host_setup_ui.run_if(in_state(AppState::GameSetup)),
                draw_join_game_ui.run_if(in_state(AppState::GameSetup)),
                
                // InGame state UI
                draw_game_ui.run_if(in_state(AppState::InGame)),
                draw_players_overlay.run_if(in_state(AppState::InGame).and(tab_pressed)),
                draw_in_game_menu_ui.run_if(in_state(AppState::InGame)),
                draw_generation_progress_ui.run_if(in_state(AppState::InGame)),
                
                // GameComplete state UI
                draw_completion_ui.run_if(in_state(AppState::GameComplete)),
            ),
        )
        .run();
}
