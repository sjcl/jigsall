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
}

#[derive(Component)]
pub struct Draggable {
    pub is_dragging: bool,
    pub drag_offset: Vec2,
}

#[derive(Component)]
pub struct Player {
    pub id: Uuid,
    pub name: String,
    pub is_host: bool,
}

#[derive(Component)]
pub struct MainCamera;

#[derive(Component)]
pub struct GameUI;

#[derive(Component)]
pub struct MenuUI;

#[derive(Component)]
pub struct GridReference;