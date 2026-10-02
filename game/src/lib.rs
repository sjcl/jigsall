//! Bevy game lifecycle, input, presentation and GPU selection.
pub mod asset_reader;
pub mod checkpoint;
mod components;
mod game;
mod interaction;
pub mod multiplayer;
pub mod network;
pub mod persistence;
#[cfg(any(test, feature = "cpu-picking-debug"))]
#[allow(dead_code)]
mod piece_geometry;
pub mod render;
pub mod resources;
mod selection;
mod systems;
pub use game::GamePlugin;
