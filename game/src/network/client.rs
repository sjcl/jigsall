use super::{
    session::SessionConnections,
    transient::{replication_drop, TransientDrop},
    transport::{ConnectionId, Transport, TransportError, TransportEvent},
    wire::{self, WireError, WireMessage},
};
use crate::{
    multiplayer::replication::{PeerReplicationState, ReplicationError},
    resources::{pieces::AppliedCommand, PieceDataStore},
};
use jigsall_core::{
    protocol::ProtocolCommandEnvelope,
    session::{AuthoritySession, CommandSequenceStatus},
    PlayerId, PuzzleDefinition,
};

#[derive(Debug, PartialEq)]
pub enum ClientRouteError {
    NotMessage,
    WrongConnection,
    UnauthenticatedHost,
    WrongDirection,
    Wire(WireError),
    Replication(ReplicationError),
    Roster(crate::players::RosterError),
}

#[derive(Debug)]
pub enum ClientRouteOutcome {
    Authority(AppliedCommand),
    Presence,
    Drag(CommandSequenceStatus),
    DroppedTransient(TransientDrop),
}

#[derive(Debug, PartialEq, Eq)]
pub enum ClientSendError {
    UnauthenticatedHost,
    Wire(WireError),
    Transport(TransportError),
}

/// A star-topology peer has one designated host connection. The trusted session
/// mapping, not a wire host claim, provides the authenticated_host argument.
pub struct ClientRouter<'a> {
    pub roster: &'a mut crate::players::PlayerRoster,
    pub local_player: PlayerId,
    pub host_connection: ConnectionId,
    pub connections: &'a SessionConnections,
    pub replica: &'a mut PeerReplicationState,
    pub session: &'a mut AuthoritySession,
    pub store: &'a mut PieceDataStore,
    pub definition: Option<&'a PuzzleDefinition>,
}

impl ClientRouter<'_> {
    pub fn route(
        &mut self,
        event: &TransportEvent,
    ) -> Result<ClientRouteOutcome, ClientRouteError> {
        let TransportEvent::Message {
            connection,
            class,
            payload,
        } = event
        else {
            return Err(ClientRouteError::NotMessage);
        };
        if *connection != self.host_connection {
            return Err(ClientRouteError::WrongConnection);
        }
        let host = self
            .connections
            .player(*connection)
            .filter(|player| *player == self.session.host())
            .ok_or(ClientRouteError::UnauthenticatedHost)?;
        match wire::decode_for_class(payload, *class).map_err(ClientRouteError::Wire)? {
            WireMessage::AuthorityEvent(envelope) => self
                .replica
                .apply_event(
                    self.session,
                    self.store,
                    host,
                    &envelope,
                    self.definition,
                    self.local_player,
                )
                .map(ClientRouteOutcome::Authority)
                .map_err(ClientRouteError::Replication),
            WireMessage::DragUpdate(update) => {
                match self
                    .replica
                    .apply_drag_update(self.session, self.store, host, &update)
                {
                    Ok(status) => Ok(ClientRouteOutcome::Drag(status)),
                    Err(error) => match replication_drop(&error) {
                        Some(reason) => Ok(ClientRouteOutcome::DroppedTransient(reason)),
                        None => Err(ClientRouteError::Replication(error)),
                    },
                }
            }
            WireMessage::Presence(event) => self
                .roster
                .apply_presence(event)
                .map(|()| ClientRouteOutcome::Presence)
                .map_err(ClientRouteError::Roster),
            _ => Err(ClientRouteError::WrongDirection),
        }
    }

    pub fn send_command(
        &self,
        transport: &mut dyn Transport,
        command: &ProtocolCommandEnvelope,
    ) -> Result<(), ClientSendError> {
        if self.connections.player(self.host_connection) != Some(self.session.host()) {
            return Err(ClientSendError::UnauthenticatedHost);
        }
        let message = WireMessage::ClientCommand(command.clone());
        let payload = wire::encode(&message).map_err(ClientSendError::Wire)?;
        transport
            .send(self.host_connection, message.class(), &payload)
            .map_err(ClientSendError::Transport)
    }
}
