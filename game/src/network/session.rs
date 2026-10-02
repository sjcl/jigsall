//! Session authentication is deliberately separate from backend addressing.
use super::transport::{ConnectionId, TransportEvent};
use puzzella_core::PlayerId;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionConnectionError {
    UnknownConnection,
    PlayerAlreadyAssigned,
    ConnectionAlreadyAssigned,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PeerConnection {
    pub connection: ConnectionId,
    pub player: Option<PlayerId>,
}

#[derive(Default, Debug)]
pub struct SessionConnections {
    peers: BTreeMap<ConnectionId, PeerConnection>,
}

impl SessionConnections {
    /// Observe lifecycle BEFORE routing messages from the same poll batch.
    /// Connected starts unassigned; a trusted session action assigns PlayerId.
    pub fn observe(&mut self, event: &TransportEvent) {
        match *event {
            TransportEvent::Connected { connection } => {
                self.peers.entry(connection).or_insert(PeerConnection {
                    connection,
                    player: None,
                });
            }
            TransportEvent::Disconnected { connection, .. }
            | TransportEvent::ConnectionFailed { connection, .. } => {
                self.peers.remove(&connection);
            }
            TransportEvent::Message { .. } => {}
        }
    }

    pub fn assign_player(
        &mut self,
        connection: ConnectionId,
        player: PlayerId,
    ) -> Result<(), SessionConnectionError> {
        let peer = self
            .peers
            .get(&connection)
            .ok_or(SessionConnectionError::UnknownConnection)?;
        if peer.player == Some(player) {
            return Ok(());
        }
        if peer.player.is_some() {
            return Err(SessionConnectionError::ConnectionAlreadyAssigned);
        }
        if self.peers.values().any(|peer| peer.player == Some(player)) {
            return Err(SessionConnectionError::PlayerAlreadyAssigned);
        }
        self.peers
            .get_mut(&connection)
            .expect("checked above")
            .player = Some(player);
        Ok(())
    }

    pub fn player(&self, connection: ConnectionId) -> Option<PlayerId> {
        self.peers.get(&connection)?.player
    }
    pub fn peers(&self) -> impl Iterator<Item = &PeerConnection> {
        self.peers.values()
    }
}
