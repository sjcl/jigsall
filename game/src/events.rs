//! Local presentation messages from committed lifecycle transitions.
//! These are never part of the wire protocol, checkpoints or save format.
use bevy::prelude::*;

use crate::{
    network::transport::{ConnectionId, DisconnectReason},
    persistence::runtime::PersistenceError,
    players::{PlayerRoster, PresenceMessage, RosterPlayer},
};

/// Session-local correlation only. This cannot be used to route network traffic.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct JoinAttemptId(pub u64);

impl From<ConnectionId> for JoinAttemptId {
    fn from(connection: ConnectionId) -> Self {
        Self(connection.local_token())
    }
}

#[derive(Message, Clone, Debug)]
pub enum GameEvent {
    /// Discard the previous game's/session's presentation history.
    SessionReset,
    PlayerJoining {
        connection: JoinAttemptId,
    },
    PlayerJoinCancelled {
        connection: JoinAttemptId,
        reason: DisconnectReason,
    },
    PlayerJoined {
        player: RosterPlayer,
        /// Only the host has a local joining operation to finish.
        connection: Option<JoinAttemptId>,
    },
    PlayerLeft {
        player: RosterPlayer,
    },
    AutosaveStarted,
    AutosaveSucceeded,
    AutosaveFailed {
        error: PersistenceError,
    },
    AutosaveRotationFailed {
        error: PersistenceError,
    },
}

impl GameEvent {
    /// Capture the name before PlayerLeft removes it. The caller emits this
    /// only after apply_presence succeeds; snapshots never use this path.
    pub(crate) fn presence(roster: &PlayerRoster, event: &PresenceMessage) -> Self {
        match event {
            PresenceMessage::PlayerJoined { player, .. } => Self::PlayerJoined {
                player: player.clone(),
                connection: None,
            },
            PresenceMessage::PlayerLeft { player, .. } => Self::PlayerLeft {
                player: RosterPlayer {
                    player: *player,
                    display_name: roster.get(*player).and_then(|p| p.display_name.clone()),
                },
            },
        }
    }
}

pub(crate) fn reset_history(mut events: MessageWriter<GameEvent>) {
    events.write(GameEvent::SessionReset);
}

pub(crate) fn reset_world_history(world: &mut World) {
    if let Some(mut events) = world.get_resource_mut::<Messages<GameEvent>>() {
        events.write(GameEvent::SessionReset);
    }
}
