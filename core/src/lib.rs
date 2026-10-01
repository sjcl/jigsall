//! Puzzle identities, state and authoritative command decisions.
mod commands;
mod gameplay;
pub use commands::ClientCommand;
pub use gameplay::{
    apply_piece_command, snap_piece, CommandOutcome, PieceCommand, PieceId, PieceState, PlayerId,
    PuzzleDefinition, PuzzlePiece, GENERATOR_VERSION, LOCAL_PLAYER,
};
