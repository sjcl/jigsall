//! Bevy game lifecycle, input, presentation and GPU selection.
pub mod asset_reader;
mod components;
mod game;
mod interaction;
pub mod multiplayer;
#[cfg(any(test, feature = "cpu-picking-debug"))]
#[allow(dead_code)]
mod piece_geometry;
pub mod render;
pub mod resources;
mod selection;
mod systems;
pub use game::GamePlugin;
