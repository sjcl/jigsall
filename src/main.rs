mod game;
// mod networking; // 一時的に無効化
mod puzzle;
// mod ui; // 一時的に無効化
mod components;
mod resources;
mod systems;
mod jigsaw_shapes;

use bevy::prelude::*;
// use bevy_egui;
use game::GamePlugin;
// use networking::NetworkingPlugin;
// use ui::UiPlugin;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        // .add_plugins(bevy_egui::EguiPlugin) // 一時的に無効化
        .add_plugins(GamePlugin)
        // .add_plugins(NetworkingPlugin) // 一時的に無効化
        // .add_plugins(UiPlugin) // 一時的に無効化
        .run();
}
