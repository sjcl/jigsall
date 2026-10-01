use bevy::prelude::*;
use puzzella_core::{PieceId, PieceState, PuzzlePiece};
#[cfg(any(test, feature = "cpu-picking-debug"))]
pub use puzzella_puzzle::PieceShape as PieceShapeData;
use std::collections::{HashMap, HashSet};

/// Canonical gameplay records plus separate, local presentation caches.
#[derive(Resource, Default)]
pub struct PieceDataStore {
    pub pieces: HashMap<PieceId, StoredPieceData>,
    pub transforms: HashMap<PieceId, Transform>,
    pub selected_pieces: HashSet<PieceId>,
    pub preview_pieces: HashSet<PieceId>,
    pub temporary_entities: HashMap<PieceId, Entity>,
    pub dirty_pieces: HashSet<PieceId>,
    pub held_pieces: HashSet<PieceId>,
    pub placed_pieces: HashSet<PieceId>,
    pub next_z_order: f32,
}

#[derive(Clone, Debug)]
pub struct StoredPieceData {
    pub definition: PuzzlePiece,
    pub state: PieceState,
    pub render: PieceRenderData,
}

/// Mesh, bounds, shape and handles are never included in gameplay snapshots.
#[derive(Clone, Debug)]
pub struct PieceRenderData {
    #[allow(dead_code)] // CPU debug picking reads bounds; GPU picking uses the mesh.
    pub bounds: Rect,
    #[cfg(any(test, feature = "cpu-picking-debug"))]
    pub shape: PieceShapeData,
    pub mesh: Handle<Mesh>,
    pub stroke: Handle<Mesh>,
    pub material: Handle<ColorMaterial>,
}
impl PieceDataStore {
    pub(crate) fn is_selectable(&self, id: PieceId) -> bool {
        self.pieces
            .get(&id)
            .is_some_and(|piece| !piece.state.placed && piece.state.held_by.is_none())
    }

    pub fn add_piece(&mut self, piece: StoredPieceData) {
        let id = piece.definition.id;
        let z = id.0 as f32 * 0.001;
        self.next_z_order = self.next_z_order.max(z);
        self.transforms.insert(
            id,
            Transform::from_translation(piece.state.position.extend(z)),
        );
        self.pieces.insert(id, piece);
    }

    /// Keep stacking below overlays and inside the camera's depth range even
    /// after thousands of grabs. Returns whether existing batches need rebuilding.
    pub fn bring_piece_to_front(&mut self, id: PieceId) -> bool {
        let compacted = self.next_z_order >= 50.0;
        if compacted {
            let mut ids: Vec<_> = self
                .pieces
                .iter()
                .filter_map(|(&id, piece)| (!piece.state.placed).then_some(id))
                .collect();
            ids.sort_by(|a, b| {
                self.transforms[a]
                    .translation
                    .z
                    .total_cmp(&self.transforms[b].translation.z)
                    .then(a.cmp(b))
            });
            let step = 10.0 / (ids.len() + 1) as f32;
            for (index, id) in ids.into_iter().enumerate() {
                if let Some(transform) = self.transforms.get_mut(&id) {
                    transform.translation.z = (index + 1) as f32 * step;
                    self.dirty_pieces.insert(id);
                }
            }
            self.next_z_order = 10.0;
        }
        self.next_z_order += 0.1;
        if let Some(transform) = self.transforms.get_mut(&id) {
            transform.translation.z = self.next_z_order;
        }
        compacted
    }
}
