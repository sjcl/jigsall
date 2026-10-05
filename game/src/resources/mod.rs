//! Game resources grouped by responsibility.
pub mod app;
pub mod config;
pub mod generation;
pub mod images;
pub mod input;
pub mod performance;
pub mod pieces;
pub mod remote_cursor;
pub mod remote_drag;
pub(crate) mod rotation_visual;

pub use crate::players::{PlayerInfo, PlayerRoster};
pub use app::{
    AppState, GameCompleteSubState, GameData, GameSubState, LocalPlayerId, SessionHostId,
};
pub use config::{PieceMode, PuzzleConfig};
pub use generation::{GenerationError, GenerationPhase, PieceGenerationProgress};
pub use images::{
    ImageDecodeLimits, ImageLoadChannels, ImageLoadError, ImageLoadSender, PuzzleImage,
    PuzzleImageLimits,
};
pub(crate) use input::local_gameplay_just_blocked;
pub use input::{local_gameplay_enabled, GameUiPointerCapture, InputState, LocalGameplayBlocked};
pub use jigsall_core::PieceId;
pub use performance::{PerformanceDebugLevel, PerformanceMonitor, SystemTiming};
pub use pieces::{DensePieceStates, GpuPieceState, PieceDataStore, PieceUpload};
