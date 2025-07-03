mod game;
mod networking;
mod puzzle;
mod ui;
mod components;
mod resources;
mod systems;
// mod jigsaw_shapes; // 一時的にコメントアウト

use bevy::prelude::*;
use bevy_egui::EguiPlugin;
// use bevy_prototype_lyon::prelude::*; // 一時的にコメントアウト
use game::GamePlugin;
use networking::NetworkingPlugin;
use ui::UiPlugin;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(EguiPlugin)
        // .add_plugins(ShapePlugin) // 一時的にコメントアウト
        .add_plugins(GamePlugin)
        .add_plugins(NetworkingPlugin)
        .add_plugins(UiPlugin)
        .run();
}
