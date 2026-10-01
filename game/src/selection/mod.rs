//! Asynchronous GPU request lifecycle keyed by stable PieceId.
pub(crate) mod api;
pub(crate) mod coordinates;
use crate::resources::PieceDataStore;
pub use api::{PuzzleSelection, SelectionMode, SelectionRequest, SelectionResult};
use bevy::prelude::*;
use crossbeam::channel::{unbounded, Receiver};
use puzzella_core::PieceId;
pub struct PuzzleSelectionPlugin;
impl Plugin for PuzzleSelectionPlugin {
    fn build(&self, app: &mut App) {
        let (tx, rx) = unbounded();
        app.init_resource::<PuzzleSelection>()
            .insert_resource(ResultInbox(rx))
            .add_systems(PreUpdate, receive_results);
        crate::render::install(app, tx);
    }
}
#[derive(Resource)]
struct ResultInbox(Receiver<RawResult>);
pub(crate) struct RawResult {
    pub(crate) request: SelectionRequest,
    pub(crate) bytes: Vec<u8>,
    pub(crate) error: Option<String>,
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
            .filter(|id| store.contains(*id))
            .collect();
        let result = SelectionResult {
            request_id: raw.request.request_id,
            mode: raw.request.mode,
            piece_ids,
            entities: vec![],
            error: raw.error,
        };
        if selection.debug {
            info!(request_id = result.request_id, ids = ?result.piece_ids, entities = ?result.entities, region = ?selection.latest.map(|r| r.region), "GPU selection result");
        }
        selection.completed = Some(result);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bitset_sizes_and_zero_id_and_word_boundaries() {
        let mut bytes = vec![0; 1252];
        for id in [0u32, 31, 32, 9999] {
            let offset = (id / 32 * 4) as usize;
            let mut word = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
            word |= 1 << (id % 32);
            bytes[offset..offset + 4].copy_from_slice(&word.to_le_bytes());
        }
        assert_eq!(
            decode_ids(SelectionMode::Rectangle, &bytes),
            [0, 31, 32, 9999].map(PieceId)
        );
        assert_eq!(
            decode_ids(SelectionMode::Point, &1u32.to_le_bytes()),
            vec![PieceId(0)]
        );
        assert!(decode_ids(SelectionMode::Point, &0u32.to_le_bytes()).is_empty());
    }
    #[test]
    fn stale_callbacks_do_not_overwrite_latest_or_cross_sessions() {
        let mut app = App::new();
        let (tx, rx) = unbounded();
        app.init_resource::<PuzzleSelection>()
            .init_resource::<PieceDataStore>()
            .insert_resource(ResultInbox(rx))
            .add_systems(Update, receive_results);
        let mut requests = app.world_mut().resource_mut::<PuzzleSelection>();
        let first = requests.request(Rect::default(), SelectionMode::Point);
        let old = requests.latest.unwrap();
        requests.cancel();
        let second = requests.request(Rect::default(), SelectionMode::Point);
        assert!(second > first);
        tx.send(RawResult {
            request: old,
            bytes: 1u32.to_le_bytes().to_vec(),
            error: None,
        })
        .unwrap();
        app.update();
        assert!(app
            .world()
            .resource::<PuzzleSelection>()
            .completed
            .is_none());
    }
}
