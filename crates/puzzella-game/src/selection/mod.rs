//! GPU request lifecycle; rendering shares the normal Mesh2d buffers.
mod api;
mod coordinates;
mod render;
use crate::resources::PieceDataStore;
pub use api::{
    PuzzlePieceId, PuzzleSelection, SelectionMode, SelectionRequest, SelectionResult,
    ATTRIBUTE_PIECE_ID,
};
use bevy::prelude::*;
use crossbeam::channel::{unbounded, Receiver};
use puzzella_core::PieceId;
use std::collections::HashMap;
pub struct PuzzleSelectionPlugin;
impl Plugin for PuzzleSelectionPlugin {
    fn build(&self, app: &mut App) {
        let (tx, rx) = unbounded();
        app.init_resource::<PuzzleSelection>()
            .insert_resource(ResultInbox(rx))
            .add_systems(PreUpdate, receive_results);
        render::install(app, tx);
    }
}
#[derive(Resource)]
struct ResultInbox(Receiver<RawResult>);
struct RawResult {
    request: SelectionRequest,
    bytes: Vec<u8>,
    error: Option<String>,
}

fn decode_ids(mode: SelectionMode, bytes: &[u8]) -> Vec<PieceId> {
    if mode == SelectionMode::Point {
        return bytes
            .get(..4)
            .and_then(|b| u32::from_le_bytes(b.try_into().unwrap()).checked_sub(1))
            .map(PieceId)
            .into_iter()
            .collect();
    }
    let mut ids = Vec::new();
    for (word_index, bytes) in bytes.chunks_exact(4).enumerate() {
        let mut word = u32::from_le_bytes(bytes.try_into().unwrap());
        while word != 0 {
            ids.push(PieceId(word_index as u32 * 32 + word.trailing_zeros()));
            word &= word - 1;
        }
    }
    ids
}
fn receive_results(
    inbox: Res<ResultInbox>,
    mut selection: ResMut<PuzzleSelection>,
    ids: Query<(Entity, &PuzzlePieceId)>,
    store: Res<PieceDataStore>,
) {
    for raw in inbox.0.try_iter() {
        if !selection
            .latest
            .is_some_and(|r| r.request_id == raw.request.request_id)
        {
            continue;
        }
        let piece_ids: Vec<_> = decode_ids(raw.request.mode, &raw.bytes)
            .into_iter()
            .filter(|id| store.pieces.contains_key(id))
            .collect();
        let mapping: HashMap<_, _> = ids.iter().map(|(entity, id)| (id.0, entity)).collect();
        let entities = piece_ids
            .iter()
            .filter_map(|id| mapping.get(id).copied())
            .collect();
        let result = SelectionResult {
            request_id: raw.request.request_id,
            mode: raw.request.mode,
            piece_ids,
            entities,
            error: raw.error,
        };
        if selection.debug {
            info!(request_id = result.request_id, ids = ?result.piece_ids, entities = ?result.entities, region = ?selection.latest.map(|r| r.region), "GPU selection result");
        }
        selection.completed = Some(result);
    }
}
