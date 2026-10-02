//! Opt-in CPU authority adapter. No ECS systems, transport, or renderer changes.
use crate::resources::{
    pieces::{AppliedCommand, ENABLED, HELD, PLACED},
    PieceDataStore,
};
use puzzella_core::{
    protocol::{
        ActiveDrag, ProtocolCommandEnvelope, ProtocolPieceCommand, RejectedComponentRef,
        TargetError,
    },
    session::{
        AuthorityEpoch, AuthoritySession, ClientCommandSequence, CommandSequenceStatus,
        ProtocolError, SessionId,
    },
    PlayerId, PuzzleDefinition,
};
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProtocolCommandError {
    Sequence(ProtocolError),
    WrongPlayer,
    Target(TargetError),
    InvalidDelta,
    ActiveDragExists,
    NoActiveDrag,
    WrongDragContext,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ProtocolCommandResult {
    Grabbed {
        applied: AppliedCommand,
        rejected: Vec<RejectedComponentRef>,
    },
    DragUpdated {
        sequence: CommandSequenceStatus,
    },
    Released {
        applied: AppliedCommand,
        rejected: Vec<RejectedComponentRef>,
    },
}

/// Per-player accepted sets, scoped to both authority epoch and store generation.
/// No wire membership is retained. Transient updates touch only a scalar delta.
#[derive(Default, Debug)]
pub struct ProtocolDragContexts {
    scope: Option<(SessionId, AuthorityEpoch, u64)>,
    players: HashMap<PlayerId, ActiveDrag>,
}

impl ProtocolDragContexts {
    fn scope(
        session: &AuthoritySession,
        store: &PieceDataStore,
    ) -> (SessionId, AuthorityEpoch, u64) {
        (session.session_id(), session.cursor().epoch, store.epoch)
    }

    pub fn active_drag(
        &self,
        session: &AuthoritySession,
        store: &PieceDataStore,
        player: PlayerId,
    ) -> Option<&ActiveDrag> {
        if session.is_active() && self.scope == Some(Self::scope(session, store)) {
            self.players.get(&player)
        } else {
            None
        }
    }

    /// Explicit cancel/disconnect: release holds without translating or snapping.
    /// Call this when ownership is cleared externally, before reusing a player.
    pub fn cancel_player(&mut self, store: &mut PieceDataStore, player: PlayerId) {
        self.players.remove(&player);
        store.clear_player_holds(player);
    }

    /// Authenticate externally, then pass that identity separately from the wire
    /// claim. Reliable controls are consumed even when gameplay validation rejects
    /// them; callers must process sequence validation and gameplay synchronously.
    pub fn apply(
        &mut self,
        session: &mut AuthoritySession,
        store: &mut PieceDataStore,
        authenticated_player: PlayerId,
        envelope: &ProtocolCommandEnvelope,
        definition: Option<&PuzzleDefinition>,
    ) -> Result<ProtocolCommandResult, ProtocolCommandError> {
        if envelope.player != authenticated_player {
            return Err(ProtocolCommandError::WrongPlayer);
        }
        let scope = Self::scope(session, store);
        if self.scope != Some(scope) {
            self.players.clear();
            self.scope = Some(scope);
        }
        let status = session
            .accept_command(envelope)
            .map_err(ProtocolCommandError::Sequence)?;
        let player = authenticated_player;
        match &envelope.command {
            ProtocolPieceCommand::Grab { target } => {
                if self.players.contains_key(&player) {
                    return Err(ProtocolCommandError::ActiveDragExists);
                }
                let mut resolved = target
                    .resolve(&store.connectivity)
                    .map_err(ProtocolCommandError::Target)?;
                resolved.components.retain(|reference| {
                    store
                        .connectivity
                        .iter_component(reference.member)
                        .all(|id| store.is_selectable(id))
                });
                let applied = store
                    .grab_resolved_components(player, resolved.components.iter().map(|r| r.member));
                let ClientCommandSequence::Control(grab_sequence) = envelope.sequence else {
                    unreachable!()
                };
                if applied.grabbed != 0 {
                    self.players.insert(
                        player,
                        ActiveDrag {
                            grab_sequence,
                            components: resolved.components,
                            delta: bevy::math::Vec2::ZERO,
                        },
                    );
                }
                Ok(ProtocolCommandResult::Grabbed {
                    applied,
                    rejected: resolved.rejected,
                })
            }
            ProtocolPieceCommand::DragUpdate { delta } => {
                if !delta.is_finite() {
                    return Err(ProtocolCommandError::InvalidDelta);
                }
                let drag = self
                    .players
                    .get_mut(&player)
                    .ok_or(ProtocolCommandError::NoActiveDrag)?;
                let ClientCommandSequence::Move {
                    after_control_sequence,
                    ..
                } = envelope.sequence
                else {
                    unreachable!()
                };
                if after_control_sequence != drag.grab_sequence {
                    return Err(ProtocolCommandError::WrongDragContext);
                }
                drag.delta = *delta;
                Ok(ProtocolCommandResult::DragUpdated { sequence: status })
            }
            ProtocolPieceCommand::Release {
                grab_sequence,
                final_delta,
            } => {
                if !final_delta.is_finite() {
                    return Err(ProtocolCommandError::InvalidDelta);
                }
                let drag = self
                    .players
                    .get(&player)
                    .ok_or(ProtocolCommandError::NoActiveDrag)?;
                if *grab_sequence != drag.grab_sequence {
                    return Err(ProtocolCommandError::WrongDragContext);
                }
                let drag = self.players.remove(&player).unwrap();
                let mut roots = Vec::with_capacity(drag.components.len());
                let mut rejected = Vec::new();
                for reference in drag.components {
                    match reference.resolve(&store.connectivity) {
                        Ok(minimum) => {
                            // Recheck the complete current component, including both
                            // owner occupancy and its presentation flags.
                            if store.connectivity.iter_component(minimum).all(|id| {
                                store.states[id.0 as usize].flags & (PLACED | ENABLED | HELD)
                                    == (ENABLED | HELD)
                                    && store.held_by.get(&id) == Some(&player)
                            }) {
                                roots.push(minimum);
                            }
                        }
                        Err(reason) => rejected.push(RejectedComponentRef { reference, reason }),
                    }
                }
                let definition =
                    definition.filter(|d| d.validate().is_ok() && d.piece_count() == store.len());
                let applied = store.release_roots(player, roots, *final_delta, definition);
                Ok(ProtocolCommandResult::Released { applied, rejected })
            }
        }
    }
}

#[cfg(test)]
#[path = "protocol_tests.rs"]
mod tests;
