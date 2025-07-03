use bevy::prelude::*;
use bevy_renet::renet::{RenetServer, RenetClient, ConnectionConfig, DefaultChannel};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::components::*;
use crate::resources::NetworkInfo;

pub struct NetworkingPlugin;

impl Plugin for NetworkingPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (
                handle_server_events,
                handle_client_events,
                send_piece_updates,
            ));
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub enum GameMessage {
    PlayerJoined { player_id: Uuid, name: String },
    PlayerLeft { player_id: Uuid },
    PieceUpdate { piece_id: Uuid, position: Vec2, is_placed: bool },
    GameState { progress: f32, completed: bool },
    StartGame,
    ResetGame,
}

const CHANNEL_ID: u8 = 0;

pub fn start_server(
    commands: &mut Commands,
    _network_info: &NetworkInfo,
) -> Result<(), Box<dyn std::error::Error>> {
    let connection_config = ConnectionConfig::default();
    let server = RenetServer::new(connection_config);
    
    commands.insert_resource(server);
    
    Ok(())
}

pub fn start_client(
    commands: &mut Commands,
    _network_info: &NetworkInfo,
) -> Result<(), Box<dyn std::error::Error>> {
    let connection_config = ConnectionConfig::default();
    let client = RenetClient::new(connection_config);
    
    commands.insert_resource(client);
    
    Ok(())
}

fn handle_server_events(
    server: Option<ResMut<RenetServer>>,
    mut game_state: ResMut<crate::resources::GameState>,
) {
    let Some(mut server) = server else { return; };
    for client_id in server.clients_id() {
        while let Some(message) = server.receive_message(client_id, DefaultChannel::ReliableOrdered) {
            let game_message: GameMessage = bincode::deserialize(&message).unwrap();
            
            match game_message {
                GameMessage::PlayerJoined { player_id, name } => {
                    game_state.players.push(crate::resources::PlayerInfo {
                        id: player_id,
                        name,
                        score: 0,
                    });
                }
                GameMessage::PieceUpdate { piece_id, position, is_placed } => {
                    let response = GameMessage::PieceUpdate { piece_id, position, is_placed };
                    let message = bincode::serialize(&response).unwrap();
                    server.broadcast_message(DefaultChannel::ReliableOrdered, message);
                }
                _ => {}
            }
        }
    }
}

fn handle_client_events(
    client: Option<ResMut<RenetClient>>,
    mut piece_query: Query<(&mut Transform, &mut PuzzlePiece)>,
) {
    let Some(mut client) = client else { return; };
    while let Some(message) = client.receive_message(DefaultChannel::ReliableOrdered) {
        let game_message: GameMessage = bincode::deserialize(&message).unwrap();
        
        match game_message {
            GameMessage::PieceUpdate { piece_id, position, is_placed } => {
                for (mut transform, mut piece) in piece_query.iter_mut() {
                    if piece.id == piece_id {
                        transform.translation = position.extend(transform.translation.z);
                        piece.current_position = position;
                        piece.is_placed = is_placed;
                        break;
                    }
                }
            }
            _ => {}
        }
    }
}

fn send_piece_updates(
    mut server: Option<ResMut<RenetServer>>,
    mut client: Option<ResMut<RenetClient>>,
    piece_query: Query<&PuzzlePiece, Changed<PuzzlePiece>>,
) {
    for piece in piece_query.iter() {
        let message = GameMessage::PieceUpdate {
            piece_id: piece.id,
            position: piece.current_position,
            is_placed: piece.is_placed,
        };
        
        let serialized = bincode::serialize(&message).unwrap();
        
        if let Some(ref mut server) = server {
            server.broadcast_message(DefaultChannel::ReliableOrdered, serialized);
        } else if let Some(ref mut client) = client {
            client.send_message(DefaultChannel::ReliableOrdered, serialized);
        }
    }
}