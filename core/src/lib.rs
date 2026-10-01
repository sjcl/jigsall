//! Puzzle identities, state and authoritative command decisions.
mod bitset;
mod commands;
mod connectivity;
pub use bitset::{PieceBitSet, MAX_PIECES};
pub use connectivity::PieceConnectivity;
mod gameplay;
mod snapping;
pub use snapping::{matches_translation, offset_distance_squared, SnapCandidate};
pub mod session;
pub use commands::ClientCommand;
pub use gameplay::{
    apply_piece_command, snap_piece, CommandOutcome, PieceCommand, PieceId, PieceState, PlayerId,
    PuzzleDefinition, PuzzleGeometry, PuzzlePiece, GENERATOR_VERSION, LOCAL_PLAYER,
};
