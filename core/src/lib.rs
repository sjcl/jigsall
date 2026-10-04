//! Puzzle identities, state and authoritative command decisions.
mod bitset;
mod commands;
mod connectivity;
pub mod protocol;
mod scratch;
pub use bitset::{PieceBitSet, MAX_PIECES};
pub use connectivity::PieceConnectivity;
pub use scratch::PieceScratchSet;
mod gameplay;
mod player;
pub use player::{
    DisplayNameError, PlayerDisplayName, MAX_PLAYER_DISPLAY_NAME_BYTES,
    MAX_PLAYER_DISPLAY_NAME_CHARS,
};
mod rotation;
pub use rotation::{
    add_quarter_turns, decode_rotation, rotate_quarter, with_rotation, ROTATION_MASK,
    ROTATION_SHIFT,
};
mod snapping;
pub use snapping::{
    matches_transform, matches_translation, offset_distance_squared, SnapCandidate,
};
pub mod session;
pub use commands::ClientCommand;
pub use gameplay::{
    fit_image_size, PieceCommand, PieceId, PieceState, PlayerId, PuzzleDefinition, PuzzleGeometry,
    GENERATOR_VERSION, LOCAL_PLAYER, MAX_PUZZLE_IMAGE_DIMENSION,
};
