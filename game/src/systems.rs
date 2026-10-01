// Module declarations for systems
pub mod batching;
#[cfg(any(test, feature = "cpu-picking-debug"))]
pub mod collision;
pub mod game_logic;
pub mod image_loading;
pub mod input_camera;
pub mod performance;
pub mod piece_interaction;
pub mod puzzle_generation;

// Re-export all public functions from submodules
pub use batching::*;
#[cfg(any(test, feature = "cpu-picking-debug"))]
pub use collision::*;
pub use game_logic::*;
pub use image_loading::*;
pub use input_camera::*;
pub use performance::*;
pub use piece_interaction::*;
pub use puzzle_generation::*;
