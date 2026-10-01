use bevy::prelude::*;
use puzzella_core::PieceId;
#[derive(Component)]
pub struct MainCamera;
#[derive(Component)]
pub struct GridReference;
#[derive(Component)]
pub struct SelectionBox;
#[derive(Message)]
pub struct PieceMoveCompleted {
    pub id: PieceId,
}
#[derive(Message)]
pub struct PiecePlacedEvent {
    pub id: PieceId,
}
