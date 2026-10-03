//! Asynchronous GPU request lifecycle keyed by stable PieceId.
pub(crate) mod api;
pub(crate) mod coordinates;
use crate::resources::PieceDataStore;
pub use api::{
    PuzzleSelection, SelectionMode, SelectionPayload, SelectionRequest, SelectionResult,
};
use bevy::prelude::*;
use crossbeam::channel::{unbounded, Receiver};
use puzzella_core::{PieceBitSet, PieceId};
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

pub(crate) fn decode_payload(
    mode: SelectionMode,
    bytes: &[u8],
    count: usize,
) -> Result<SelectionPayload, String> {
    // A clipped/empty ROI is completed by the renderer without a GPU copy/map.
    if bytes.is_empty() {
        return Ok(match mode {
            SelectionMode::Point => SelectionPayload::Point(None),
            SelectionMode::Rectangle => SelectionPayload::Rectangle(PieceBitSet::new(count)),
        });
    }
    if mode == SelectionMode::Point {
        if bytes.len() != 4 {
            return Err("Invalid point readback size".into());
        }
        let id = u32::from_le_bytes(bytes.try_into().unwrap())
            .checked_sub(1)
            .map(PieceId)
            .filter(|id| (id.0 as usize) < count);
        return Ok(SelectionPayload::Point(id));
    }
    // O(N/32) bytes/words; never expand GPU membership into an ID Vec.
    if bytes.len() != count.div_ceil(32) * 4 {
        return Err("Invalid rectangle readback size".into());
    }
    let words = bytes
        .chunks_exact(4)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
        .collect();
    PieceBitSet::from_words(count, words)
        .map(SelectionPayload::Rectangle)
        .map_err(str::to_owned)
}
fn receive_results(
    inbox: Res<ResultInbox>,
    mut selection: ResMut<PuzzleSelection>,
    store: Res<PieceDataStore>,
) {
    for raw in inbox.0.try_iter() {
        if !raw.request.readback {
            continue;
        }
        if selection
            .latest
            .is_none_or(|r| r.request_id != raw.request.request_id)
        {
            continue;
        }
        #[cfg(test)]
        let receive_start = std::time::Instant::now();
        let (payload, error) = match decode_payload(raw.request.mode, &raw.bytes, store.len()) {
            Ok(payload) => (payload, raw.error),
            Err(error) => (SelectionPayload::Point(None), raw.error.or(Some(error))),
        };
        let result = SelectionResult {
            request_id: raw.request.request_id,
            mode: raw.request.mode,
            payload,
            error,
        };
        if selection.debug {
            info!(request_id = result.request_id, region = ?selection.latest.map(|r| r.region), "GPU selection result");
        }
        selection.completed = Some(result);
        #[cfg(test)]
        {
            selection.receive_cpu_ns = receive_start.elapsed().as_nanos() as u64;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bitset_sizes_and_zero_id_and_word_boundaries() {
        assert!(decode_payload(SelectionMode::Rectangle, &[], 1_000_000)
            .unwrap()
            .ids()
            .is_empty());
        assert!(decode_payload(SelectionMode::Point, &[], 1_000_000)
            .unwrap()
            .ids()
            .is_empty());
        assert!(decode_payload(SelectionMode::Rectangle, &[0; 3], 1_000_000).is_err());
        let mut bytes = vec![0; 1252];
        for id in [0u32, 31, 32, 9999] {
            let offset = (id / 32 * 4) as usize;
            let mut word = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
            word |= 1 << (id % 32);
            bytes[offset..offset + 4].copy_from_slice(&word.to_le_bytes());
        }
        assert_eq!(
            decode_payload(SelectionMode::Rectangle, &bytes, 10_000)
                .unwrap()
                .ids(),
            [0, 31, 32, 9999].map(PieceId)
        );
        assert_eq!(
            decode_payload(SelectionMode::Point, &1u32.to_le_bytes(), 10_000)
                .unwrap()
                .ids(),
            vec![PieceId(0)]
        );
        assert!(
            decode_payload(SelectionMode::Point, &0u32.to_le_bytes(), 10_000)
                .unwrap()
                .ids()
                .is_empty()
        );
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
