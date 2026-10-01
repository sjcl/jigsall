use bevy::prelude::*;
use puzzella_game::{asset_reader::DirectFileAssetPlugin, GamePlugin};
use puzzella_ui::GameUiPlugin;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(GameUiPlugin)
        .add_plugins(DirectFileAssetPlugin)
        .add_plugins(GamePlugin)
        .run();
}
