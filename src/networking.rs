//! Transport-independent command boundary. No sockets or backend yet.
use crate::gameplay::{PieceCommand, PlayerId};
use bevy::prelude::Message;

/// A future host supplies an authenticated player identity here.
#[derive(Message, Clone, Debug)]
pub struct ClientCommand {
    pub player: PlayerId,
    pub command: PieceCommand,
}

// Transports must validate sequence numbers and publish only host-approved
// PieceState. Never trust a client-provided placed flag.
