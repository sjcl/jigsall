use super::{
    policy::HostFailurePolicy,
    session::SessionConnections,
    transient::{command_drop, TransientDrop},
    transport::{ConnectionId, DisconnectReason, Transport, TransportError, TransportEvent},
    wire::{self, WireError, WireMessage},
};
use crate::{
    multiplayer::protocol::{
        HostCommandOutcome, ProtocolCommandError, ProtocolCommandResult, ProtocolDragContexts,
    },
    resources::PieceDataStore,
};
use jigsall_core::{
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

impl HostRouteError {
    pub fn policy(&self) -> HostFailurePolicy {
        match self {
            Self::NotMessage => {
                HostFailurePolicy::InternalFailure(DisconnectReason::BackendFailure)
            }
            Self::Unauthenticated | Self::HostConnection | Self::WrongDirection | Self::Wire(_) => {
                HostFailurePolicy::Disconnect(DisconnectReason::ProtocolViolation)
            }
            Self::Command(error) => command_failure_policy(error),
        }
    }
}

/// Only unhandled command errors reach this classifier. Benign Transient lane
/// reorder is handled by command_drop; safe refusals already carry normal ACKs.
pub(crate) fn command_failure_policy(error: &ProtocolCommandError) -> HostFailurePolicy {
    use jigsall_core::session::ProtocolError;
    use ProtocolCommandError::*;
    match error {
        InvalidDefinition
        | InconsistentDragTarget
        | Sequence(
            ProtocolError::Frozen
            | ProtocolError::InvalidTransition
            | ProtocolError::CounterExhausted,
        ) => HostFailurePolicy::InternalFailure(DisconnectReason::BackendFailure),
        WrongPlayer
        | Target(_)
        | InvalidDelta
        | ActiveDragExists
        | NoActiveDrag
        | WrongDragContext
        | RotationDisabled
        | InvalidRebaseTick
        | Sequence(
            ProtocolError::WrongSession
            | ProtocolError::WrongEpoch
            | ProtocolError::WrongHost
            | ProtocolError::DuplicateCommand
            | ProtocolError::StaleCommand
            | ProtocolError::WrongCommandStream
            | ProtocolError::ControlGap { .. }
            | ProtocolError::ControlNotProcessed { .. }
            | ProtocolError::StaleMoveContext
            | ProtocolError::StaleEvent
            | ProtocolError::EventGap { .. }
            | ProtocolError::WrongSnapshotCursor
            | ProtocolError::MissingAcknowledgement,
        ) => {
            // Reliable errors have no safe acknowledgement in the current protocol.
            HostFailurePolicy::Disconnect(DisconnectReason::ProtocolViolation)
        }
    }
}

#[derive(Debug)]
pub enum HostRouteOutcome {
    Applied(Box<HostCommandOutcome>),
    /// A normal empty ACK/result, retained and published exactly like an apply.
    Rejected(Box<HostCommandOutcome>),
    DroppedTransient(TransientDrop),
}

/// Retention failures remain separate from the already-applied publication.
pub struct HostSyncRouteOutcome {
    pub gameplay: HostRouteOutcome,
    pub retention: Result<(), crate::multiplayer::catch_up::CatchUpError>,
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
            WireMessage::ClientCommand(command) => {
                let transient = matches!(
                    command.command,
                    jigsall_core::protocol::ProtocolPieceCommand::DragUpdate { .. }
                );
                // Reject invalid authenticated scalars even when an old context
                // would otherwise yield a benign sequencing error first.
                if matches!(command.command, jigsall_core::protocol::ProtocolPieceCommand::DragUpdate { delta } if !delta.is_finite())
                {
                    return Err(HostRouteError::Command(ProtocolCommandError::InvalidDelta));
                }
                match self.contexts.apply_replicated(
                    self.session,
                    self.store,
                    player,
                    &command,
                    self.definition,
                    self.local_player,
                ) {
                    Ok(outcome) => {
                        let rejected = match &outcome.result {
                            ProtocolCommandResult::Grabbed { applied, .. } => applied.grabbed == 0,
                            ProtocolCommandResult::Rotated { commit, .. } => match &commit.accepted
                            {
                                jigsall_core::protocol::PieceTarget::Component(_) => false,
                                jigsall_core::protocol::PieceTarget::Components(refs) => {
                                    refs.is_empty()
                                }
                                jigsall_core::protocol::PieceTarget::Dense(dense) => {
                                    dense.members.is_empty()
                                }
                            },
                            ProtocolCommandResult::DragUpdated { .. }
                            | ProtocolCommandResult::Released { .. }
                            | ProtocolCommandResult::DragRotated { .. } => false,
                        };
                        Ok(if rejected {
                            HostRouteOutcome::Rejected(Box::new(outcome))
                        } else {
                            HostRouteOutcome::Applied(Box::new(outcome))
                        })
                    }
                    Err(error) => {
                        if transient {
                            if let Some(reason) = command_drop(&error) {
                                return Ok(HostRouteOutcome::DroppedTransient(reason));
                            }
                        }
                        Err(HostRouteError::Command(error))
                    }
                }
            }
            _ => Err(HostRouteError::WrongDirection),
        }
    }

    /// Use for a runtime with pending joins. Recording always precedes caller
    /// publication; a retention failure cannot discard an applied command.
    pub fn route_with_sync(
        &mut self,
        event: &TransportEvent,
        sync: &mut super::syncing::HostSyncCoordinator,
    ) -> Result<HostSyncRouteOutcome, HostRouteError> {
        let gameplay = self.route(event)?;
        let retention = match &gameplay {
            HostRouteOutcome::Applied(outcome) | HostRouteOutcome::Rejected(outcome) => {
                sync.record_command_outcome(self.session, self.store, outcome)
            }
            HostRouteOutcome::DroppedTransient(_) => Ok(()),
        };
        Ok(HostSyncRouteOutcome {
            gameplay,
            retention,
        })
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
