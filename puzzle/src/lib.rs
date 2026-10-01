//! Seeded puzzle geometry and placement; no game World, renderer or UI.
pub mod generation;
pub mod grid;
pub mod placement;
#[cfg(any(test, feature = "cpu-picking-debug"))]
pub mod shape_data;
pub mod shapes;
pub use generation::{
    generate_pieces, GenerationError, PieceCreationResult, PieceData, TessellationWorker,
};
#[cfg(any(test, feature = "cpu-picking-debug"))]
pub use shape_data::PieceShape;
