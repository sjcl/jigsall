//! Seeded puzzle geometry and placement; no game World, renderer or UI.
#[cfg(any(test, feature = "cpu-geometry-reference"))]
pub mod fingerprint;
#[cfg(any(test, feature = "cpu-geometry-reference"))]
pub mod generation;
pub mod grid;
pub mod placement;
pub mod procedural;
#[cfg(any(test, feature = "cpu-picking-debug"))]
pub mod shape_data;
#[cfg(any(test, feature = "cpu-geometry-reference"))]
pub mod shapes;
#[cfg(any(test, feature = "cpu-geometry-reference"))]
pub use generation::{
    generate_pieces, GenerationError, PieceCreationResult, PieceData, TessellationWorker,
};
#[cfg(any(test, feature = "cpu-picking-debug"))]
pub use shape_data::PieceShape;
