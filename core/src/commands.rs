//! Local Bevy command boundary; compact transport-facing commands live in protocol.
use crate::{PieceCommand, PlayerId};
use bevy_ecs::prelude::Message;

/// A future host supplies an authenticated player identity here.
#[derive(Message, Clone, Debug)]
pub struct ClientCommand {
    pub player: PlayerId,
    pub command: PieceCommand,
}

// Transports must validate sequence numbers and publish only host-approved
// PieceState. Never trust a client-provided placed flag.
