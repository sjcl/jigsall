use bevy::prelude::*;
use crossbeam::channel;
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum GenerationPhase {
    #[default]
    NotStarted,
    GeneratingState,
    UploadingGpu,
    Completed,
    Failed,
}
#[derive(Resource, Default)]
pub struct PieceGenerationProgress {
    pub is_generating: bool,
    pub total_pieces: usize,
    pub generation_phase: GenerationPhase,
    pub grid_size: (usize, usize),
    pub pieces_created: usize,
    pub receiver: Option<channel::Receiver<Result<Vec<Vec2>, String>>>,
    pub error: Option<String>,
}
