//! Game resources grouped by responsibility.
pub mod app;
#[cfg(any(test, feature = "cpu-picking-debug"))]
pub mod collision;
pub mod config;
pub mod generation;
pub mod images;
pub mod input;
pub mod performance;
pub mod pieces;
pub mod remote_drag;

pub use app::{
    AppState, GameCompleteSubState, GameData, GameSubState, LocalPlayerId, PlayerInfo,
    SessionHostId,
};
#[cfg(any(test, feature = "cpu-picking-debug"))]
pub use collision::{PieceCollisionData, PieceCollisionSystem};
pub use config::{PieceMode, PuzzleConfig};
pub use generation::{GenerationError, GenerationPhase, PieceGenerationProgress};
pub use images::{
    ImageDecodeLimits, ImageLoadChannels, ImageLoadError, ImageLoadSender, PuzzleImage,
    PuzzleImageLimits,
};
pub use input::{GameUiPointerCapture, InputState};
pub use performance::{PerformanceDebugLevel, PerformanceMonitor, SystemTiming};
pub use pieces::{DensePieceStates, GpuPieceState, PieceDataStore, PieceUpload};
pub use puzzella_core::PieceId;
