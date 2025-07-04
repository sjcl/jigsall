mod game;
mod networking;
mod puzzle;
mod ui;
mod components;
mod resources;
mod systems;
mod jigsaw_shapes;

use bevy::prelude::*;
use bevy_egui::EguiPlugin;
use game::GamePlugin;
use networking::NetworkingPlugin;
use ui::UiPlugin;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(EguiPlugin)
        .add_plugins(GamePlugin)
        .add_plugins(NetworkingPlugin)
        .add_plugins(UiPlugin)
        .run();
}
