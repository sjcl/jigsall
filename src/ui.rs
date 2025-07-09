// Module declarations for UI
pub mod common;
pub mod menu;
pub mod game_setup;
pub mod game_play;
pub mod overlays;

// Re-export all public functions from submodules
pub use menu::*;
pub use game_setup::*;
pub use game_play::*;
pub use overlays::*;