use crate::{components::*, resources::*};
use bevy::prelude::*;
use puzzella_core::PuzzleDefinition;
#[allow(clippy::too_many_arguments)]
pub fn generate_puzzle_state(
    definition: Option<Res<PuzzleDefinition>>,
    mut progress: ResMut<PieceGenerationProgress>,
    mut store: ResMut<PieceDataStore>,
    mut next: ResMut<NextState<GameSubState>>,
    ready: Res<crate::render::RenderReady>,
    game: Res<GameData>,
    mut next_app: ResMut<NextState<AppState>>,
) {
    let Some(definition) = definition else {
        return;
    };
    if progress.generation_phase == GenerationPhase::NotStarted {
        progress.is_generating = true;
        progress.total_pieces = definition.piece_count();
        progress.grid_size = (
            definition.grid_size.x as usize,
            definition.grid_size.y as usize,
        );
        progress.generation_phase = GenerationPhase::GeneratingState;
        let def = definition.clone();
        let (tx, rx) = crossbeam::channel::bounded(1);
        std::thread::spawn(move || {
            let states = DensePieceStates::generate(&def);
            let _ = tx.send(Ok(states));
        });
        progress.receiver = Some(rx);
    }
    if let Some(rx) = &progress.receiver {
        match rx.try_recv() {
            Ok(Ok(states)) => {
                store.initialize_dense(states);
                progress.pieces_created = store.len();
                progress.receiver = None;
                progress.generation_phase = GenerationPhase::UploadingGpu;
            }
            Ok(Err(error)) => {
                progress.error = Some(error);
                progress.generation_phase = GenerationPhase::Failed;
                progress.receiver = None;
            }
            Err(crossbeam::channel::TryRecvError::Disconnected) => {
                progress.error = Some("State worker stopped".into());
                progress.generation_phase = GenerationPhase::Failed;
                progress.receiver = None;
            }
            Err(crossbeam::channel::TryRecvError::Empty) => {}
        }
    }
    if progress.generation_phase == GenerationPhase::UploadingGpu && ready.is_ready(store.epoch) {
        progress.generation_phase = GenerationPhase::Completed;
        progress.is_generating = false;
        if game.puzzle_completed {
            next_app.set(AppState::GameComplete);
        } else {
            next.set(GameSubState::Playing);
        }
    }
    if let Some(error) = ready.error(store.epoch) {
        progress.error = Some(error);
        progress.generation_phase = GenerationPhase::Failed;
        progress.is_generating = false;
    }
    if progress.generation_phase == GenerationPhase::Failed {
        progress.is_generating = false;
    }
}
pub fn spawn_grid_reference(mut commands: Commands, image: Res<PuzzleImage>) {
    commands.spawn((
        Sprite {
            image: image.handle.clone(),
            custom_size: Some(image.size),
            color: Color::WHITE.with_alpha(0.3),
            ..default()
        },
        Transform::from_xyz(0.0, 0.0, -30.0),
        GridReference,
    ));
}
