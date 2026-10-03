//! Bevy game lifecycle, input, presentation and GPU selection.
pub mod asset_reader;
pub mod checkpoint;
mod components;
mod game;
mod gpu_memory;
pub mod image_settings;
mod interaction;
pub mod keybindings;
pub mod multiplayer;
pub mod network;
pub mod persistence;
#[cfg(any(test, feature = "cpu-picking-debug"))]
#[allow(dead_code)]
mod piece_geometry;
pub mod render;
pub mod resources;
mod selection;
pub mod settings;
pub mod settings_file;
mod systems;
#[cfg(test)]
mod test_logging;
pub use game::GamePlugin;
