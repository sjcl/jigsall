//! Puzzle identities, state and authoritative command decisions.
mod bitset;
mod commands;
pub use bitset::{PieceBitSet, MAX_PIECES};
mod gameplay;
pub mod session;
pub use commands::ClientCommand;
pub use gameplay::{
    apply_piece_command, snap_piece, CommandOutcome, PieceCommand, PieceId, PieceState, PlayerId,
    PuzzleDefinition, PuzzlePiece, GENERATOR_VERSION, LOCAL_PLAYER,
};
