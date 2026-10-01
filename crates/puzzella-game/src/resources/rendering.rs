use bevy::prelude::*;
use puzzella_core::PieceId;
use std::collections::{HashMap, HashSet};

/// ストロークメッシュのキャッシュリソース
#[derive(Resource, Default)]
pub struct StrokeMeshCache {
    pub stroke_meshes: HashMap<String, Handle<Mesh>>, // shape_hash -> stroke mesh handle
}

/// ハイライト表示用の共有マテリアルリソース
#[derive(Resource)]
pub struct HighlightMaterials {
    pub preview_material: Handle<ColorMaterial>, // プレビュー用（薄い青色）
    pub selected_material: Handle<ColorMaterial>, // 選択用（黄色）
}

/// ハイライト状態変更検出リソース
#[derive(Resource, Default)]
pub struct HighlightState {
    pub last_selected_pieces: HashSet<Entity>, // 前フレームの選択ピース
    pub last_preview_pieces: HashSet<Entity>,  // 前フレームのプレビューピース
}

/// Local presentation mapping, never part of a network snapshot.
#[derive(Resource, Default)]
pub struct PieceIdManager {
    id_to_entity: HashMap<PieceId, Entity>,
    entity_to_id: HashMap<Entity, PieceId>,
}
impl PieceIdManager {
    #[cfg(any(test, feature = "cpu-picking-debug"))]
    pub fn len(&self) -> usize {
        self.id_to_entity.len()
    }
    #[cfg(any(test, feature = "cpu-picking-debug"))]
    pub fn is_empty(&self) -> bool {
        self.id_to_entity.is_empty()
    }
    pub fn register_piece(&mut self, entity: Entity, id: PieceId) {
        self.id_to_entity.insert(id, entity);
        self.entity_to_id.insert(entity, id);
    }
    pub fn unregister_entity(&mut self, entity: Entity) {
        if let Some(id) = self.entity_to_id.remove(&entity) {
            self.id_to_entity.remove(&id);
        }
    }
}
