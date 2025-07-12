use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Component, Clone, Debug, Serialize, Deserialize)]
pub struct PuzzlePiece {
    pub id: Uuid,
    pub original_position: Vec2,
    pub current_position: Vec2,
    pub correct_position: Vec2,
    pub texture_coords: Vec4,
    pub is_placed: bool,
    pub grid_x: usize,
    pub grid_y: usize,
    pub bounds: Rect,  // 実際のジグソー形状のバウンディングボックス
}


// Picking system用の新しいコンポーネント
#[derive(Component)]
pub struct PickablePiece {
    pub drag_offset: Vec2,
}

#[derive(Component)]
pub struct PieceShape {
    pub vertices: Vec<[f32; 2]>,  // メッシュの頂点（2D）
    pub indices: Vec<u32>,        // 三角形インデックス
    pub shape_hash: String,       // 形状のハッシュ（ストロークメッシュキャッシュのキー）
}

#[derive(Component)]
pub struct Player {
}

#[derive(Component)]
pub struct MainCamera;

#[derive(Component)]
pub struct GameUI;

#[derive(Component)]
pub struct MenuUI;

#[derive(Component)]
pub struct GridReference;

/// Frustum culling用のコンポーネント

/// 選択されたピースをマークするコンポーネント
#[derive(Component)]
pub struct SelectedPiece;

/// 選択範囲の可視化用コンポーネント
#[derive(Component)]
pub struct SelectionBox;

/// 選択されたピースの枠線表示用コンポーネント
#[derive(Component)]
pub struct PieceOutline {
    pub piece_entity: Entity,
}

/// 選択範囲プレビュー中のピースをマークするコンポーネント
#[derive(Component)]
pub struct SelectionPreview;

/// ピース移動完了イベント
#[derive(Event)]
pub struct PieceMoveCompleted {
    pub entity: Entity,
    pub new_position: Vec2,
}

/// ピース配置イベント
#[derive(Event)]
pub struct PiecePlacedEvent {
    pub entity: Entity,
    pub grid_x: usize,
    pub grid_y: usize,
}

