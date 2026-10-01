//! Seeded puzzle geometry and placement; no game World, renderer or UI.
pub mod generation;
pub mod grid;
pub mod placement;
pub mod shape_data;
pub mod shapes;
pub use generation::{
    create_all_pieces_sync, generate_shapes, PieceCreationResult, PieceData, ShapeGenerationResult,
};
pub use shape_data::PieceShape;
