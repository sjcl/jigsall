mod game;
// mod networking; // 一時的に無効化
mod puzzle;
mod ui;
mod components;
mod resources;
mod systems;
mod jigsaw_shapes;

use bevy::prelude::*;
use bevy_egui::{self, EguiPrimaryContextPass};
use game::GamePlugin;
// use networking::NetworkingPlugin;
use ui::*;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(MeshPickingPlugin) // 2D meshes用
        .add_plugins(bevy_egui::EguiPlugin::default())
        .add_plugins(GamePlugin)
        // .add_plugins(NetworkingPlugin) // 一時的に無効化
        .add_systems(
            EguiPrimaryContextPass,
            (
                draw_menu_ui,
                draw_game_ui,
                draw_host_setup_ui,
                draw_join_game_ui,
                draw_completion_ui,
            ),
        )
        .run();
}
