use bevy::prelude::*;
use puzzella_game::{asset_reader::DirectFileAssetPlugin, GamePlugin, WindowIconPlugin};
use puzzella_ui::GameUiPlugin;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Puzzella".into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(WindowIconPlugin)
        .add_plugins(GameUiPlugin)
        .add_plugins(DirectFileAssetPlugin)
        .add_plugins(GamePlugin)
        .run();
}
