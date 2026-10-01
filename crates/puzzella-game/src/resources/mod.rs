//! Game resources grouped by responsibility.
pub mod app;
pub mod batching;
#[cfg(any(test, feature = "cpu-picking-debug"))]
pub mod collision;
pub mod config;
pub mod generation;
pub mod images;
pub mod input;
pub mod performance;
pub mod pieces;
pub mod rendering;

pub use app::{AppState, GameData, GameSubState, PlayerInfo};
pub use batching::BatchManager;
#[cfg(any(test, feature = "cpu-picking-debug"))]
pub use collision::{PieceCollisionData, PieceCollisionSystem};
pub use config::{PieceMode, PuzzleConfig};
pub use generation::{GenerationPhase, PieceGenerationProgress};
pub use images::{ImageLoadChannels, ImageLoadSender, PuzzleImage};
pub use input::{GameUiPointerCapture, InputState};
pub use performance::{PerformanceDebugLevel, PerformanceMonitor, SystemTiming};
pub use pieces::{PieceDataStore, PieceRenderData, PieceShapeData, StoredPieceData};
pub use puzzella_core::PieceId;
pub use rendering::{HighlightMaterials, HighlightState, PieceIdManager, StrokeMeshCache};
