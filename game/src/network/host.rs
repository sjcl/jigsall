use super::{
    session::SessionConnections,
    transport::{ConnectionId, Transport, TransportError, TransportEvent},
    wire::{self, WireError, WireMessage},
};
use crate::{
    multiplayer::protocol::{HostCommandOutcome, ProtocolCommandError, ProtocolDragContexts},
    resources::PieceDataStore,
};
use puzzella_core::{
    protocol::ProtocolAuthorityEventEnvelope, session::AuthoritySession, PlayerId, PuzzleDefinition,
};

#[derive(Debug, PartialEq)]
pub enum HostRouteError {
    NotMessage,
    Unauthenticated,
    HostConnection,
    WrongDirection,
    Wire(WireError),
    Command(ProtocolCommandError),
}

#[derive(Debug)]
pub enum HostRouteOutcome {
    Applied(Box<HostCommandOutcome>),
    BulkChunk(Vec<u8>),
}

/// Borrow existing authority state for a frame; does not create a second authority.
pub struct HostRouter<'a> {
    pub local_player: PlayerId,
    pub connections: &'a SessionConnections,
    pub contexts: &'a mut ProtocolDragContexts,
    pub session: &'a mut AuthoritySession,
    pub store: &'a mut PieceDataStore,
    pub definition: Option<&'a PuzzleDefinition>,
}

impl HostRouter<'_> {
    /// Apply exactly once using the assigned identity, separate from envelope.player.
    /// Publication is explicit so failed sends cannot hide an already applied result.
    pub fn route(&mut self, event: &TransportEvent) -> Result<HostRouteOutcome, HostRouteError> {
        let TransportEvent::Message {
            connection,
            class,
            payload,
        } = event
        else {
            return Err(HostRouteError::NotMessage);
        };
        let player = self
            .connections
            .player(*connection)
            .ok_or(HostRouteError::Unauthenticated)?;
        if player == self.session.host() {
            return Err(HostRouteError::HostConnection);
        }
        match wire::decode_for_class(payload, *class).map_err(HostRouteError::Wire)? {
            WireMessage::ClientCommand(command) => self
                .contexts
                .apply_replicated(
                    self.session,
                    self.store,
                    player,
                    &command,
                    self.definition,
                    self.local_player,
                )
                .map(|outcome| HostRouteOutcome::Applied(Box::new(outcome)))
                .map_err(HostRouteError::Command),
            WireMessage::BulkChunk(bytes) => Ok(HostRouteOutcome::BulkChunk(bytes)),
            _ => Err(HostRouteError::WrongDirection),
        }
    }

    /// Authority controls echo to every Ready remote peer (including sender).
    /// Transient presentation goes to other peers only. No host self-application.
    /// Collect each send failure and still attempt the other peers. The caller must
    /// retain/retry a failed control publication or disconnect/resynchronize its peer;
    /// never re-apply the command to generate a replacement event.
    pub fn publish(
        &self,
        transport: &mut dyn Transport,
        source: Option<ConnectionId>,
        outcome: &HostCommandOutcome,
    ) -> Result<Vec<(ConnectionId, TransportError)>, WireError> {
        if let Some(event) = &outcome.authority_event {
            return self.publish_authority_event(transport, event);
        }
        let Some(update) = &outcome.drag_update else {
            return Ok(Vec::new());
        };
        self.broadcast(transport, WireMessage::DragUpdate(update.clone()), source)
    }

    /// Publish an already-applied lifecycle/control event to all assigned Ready
    /// remote peers as Reliable Control. No source exclusion or host loopback.
    /// Return per-peer failures; retain/retry this envelope, never reapply gameplay.
    pub fn publish_authority_event(
        &self,
        transport: &mut dyn Transport,
        event: &ProtocolAuthorityEventEnvelope,
    ) -> Result<Vec<(ConnectionId, TransportError)>, WireError> {
        self.broadcast(transport, WireMessage::AuthorityEvent(event.clone()), None)
    }

    fn broadcast(
        &self,
        transport: &mut dyn Transport,
        message: WireMessage,
        source: Option<ConnectionId>,
    ) -> Result<Vec<(ConnectionId, TransportError)>, WireError> {
        let payload = wire::encode(&message)?;
        let transient = matches!(message, WireMessage::DragUpdate(_));
        let mut failures = Vec::new();
        for peer in self.connections.peers() {
            if peer.player.is_none()
                || peer.player == Some(self.session.host())
                || (transient && source == Some(peer.connection))
            {
                continue;
            }
            if let Err(error) = transport.send(peer.connection, message.class(), &payload) {
                failures.push((peer.connection, error));
            }
        }
        Ok(failures)
    }
}
