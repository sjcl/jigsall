//! Opt-in CPU authority adapter. No ECS systems, transport, or renderer changes.
use crate::play_area::{DragValidation, PivotEnvelope};
use crate::resources::{pieces::AppliedCommand, PieceDataStore};
use jigsall_core::{
    protocol::{
        ActiveDrag, ActiveDragTarget, DragCancelled, DragRotationCommitted, GrabAccepted,
        ProtocolAuthorityEvent, ProtocolAuthorityEventEnvelope, ProtocolCommandEnvelope,
        ProtocolPieceCommand, RejectedComponentRef, ReleaseCommitted, ReleaseResultFingerprint,
        RemoteDragUpdate, ResolvedPieceTarget, RotationCommitted, TargetError,
    },
    session::{
        AuthorityEpoch, AuthoritySession, ClientCommandSequence, CommandSequenceStatus,
        ProtocolError, SessionId,
    },
    PlayerId, PuzzleDefinition,
};
use jigsall_puzzle::placement::LogicalPlayArea;
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
    InvalidDefinition,
    RotationDisabled,
    InvalidRebaseTick,
    InconsistentDragTarget,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ProtocolCommandResult {
    Grabbed {
        applied: AppliedCommand,
        ack: GrabAccepted,
    },
    DragUpdated {
        sequence: CommandSequenceStatus,
    },
    Released {
        applied: AppliedCommand,
        rejected: Vec<RejectedComponentRef>,
        result: ReleaseResultFingerprint,
    },
    Rotated {
        applied: AppliedCommand,
        rejected: Vec<RejectedComponentRef>,
        commit: RotationCommitted,
    },
    DragRotated {
        applied: AppliedCommand,
        result: ReleaseResultFingerprint,
    },
}

/// Mutation is complete before returning publication objects. A transport owns
/// delivery/retention; no callback or send can fail halfway through gameplay.
#[derive(Debug)]
pub struct HostCommandOutcome {
    pub result: ProtocolCommandResult,
    pub authority_event: Option<ProtocolAuthorityEventEnvelope>,
    pub drag_update: Option<RemoteDragUpdate>,
}

/// Already-applied lifecycle action; retain/publish this event without reapplying
/// cancellation on send failure. Client command sequence history is unchanged.
#[derive(Debug)]
pub struct HostCancellationOutcome {
    pub applied: AppliedCommand,
    pub authority_event: ProtocolAuthorityEventEnvelope,
}

/// Per-player accepted sets, scoped to both authority epoch and store generation.
/// Only validated, accepted membership is retained. Transient updates touch only a scalar delta.
#[derive(Default, Debug)]
pub struct ProtocolDragContexts {
    scope: Option<(SessionId, AuthorityEpoch, u64)>,
    players: HashMap<PlayerId, ActiveDrag>,
    // Authority-private, constant-size metadata per accepted drag. Never on wire.
    validation: HashMap<PlayerId, DragValidation>,
}

impl ProtocolDragContexts {
    /// Use this entrypoint consistently for a replicated authority session.
    /// Reliable outcomes advance the cursor; transient presentation never does.
    pub fn apply_replicated(
        &mut self,
        session: &mut AuthoritySession,
        store: &mut PieceDataStore,
        authenticated_player: PlayerId,
        envelope: &ProtocolCommandEnvelope,
        definition: Option<&PuzzleDefinition>,
        local_player: PlayerId,
    ) -> Result<HostCommandOutcome, ProtocolCommandError> {
        // Publication must be possible BEFORE mutation. Counters never wrap.
        if !matches!(envelope.command, ProtocolPieceCommand::DragUpdate { .. })
            && session.cursor().sequence.0 == u64::MAX
        {
            return Err(ProtocolCommandError::Sequence(
                ProtocolError::CounterExhausted,
            ));
        }
        let result = self.apply(
            session,
            store,
            authenticated_player,
            envelope,
            definition,
            local_player,
        )?;
        let event = match &result {
            ProtocolCommandResult::Grabbed { ack, .. } => {
                Some(ProtocolAuthorityEvent::GrabAccepted(ack.clone()))
            }
            ProtocolCommandResult::Released { result, .. } => {
                let ProtocolPieceCommand::Release {
                    grab_sequence,
                    final_delta,
                } = envelope.command
                else {
                    unreachable!()
                };
                Some(ProtocolAuthorityEvent::ReleaseCommitted(ReleaseCommitted {
                    player: authenticated_player,
                    grab_sequence,
                    final_delta,
                    result: *result,
                }))
            }
            ProtocolCommandResult::DragUpdated { .. } => None,
            ProtocolCommandResult::Rotated { commit, .. } => {
                Some(ProtocolAuthorityEvent::RotationCommitted(commit.clone()))
            }
            ProtocolCommandResult::DragRotated { result, .. } => {
                let ProtocolPieceCommand::RotateDrag {
                    grab_sequence,
                    final_delta,
                    through_tick,
                    quarter_turns,
                } = envelope.command
                else {
                    unreachable!()
                };
                let ClientCommandSequence::Control(basis_sequence) = envelope.sequence else {
                    unreachable!()
                };
                Some(ProtocolAuthorityEvent::DragRotationCommitted(
                    DragRotationCommitted {
                        player: authenticated_player,
                        grab_sequence,
                        basis_sequence,
                        through_tick,
                        final_delta,
                        quarter_turns: jigsall_core::add_quarter_turns(0, quarter_turns) as i8,
                        result: *result,
                    },
                ))
            }
        };
        let authority_event = event.map(|event| ProtocolAuthorityEventEnvelope {
            session: session.session_id(),
            host: session.host(),
            cursor: session
                .advance_authority()
                .expect("preflighted active cursor"),
            event,
        });
        let drag_update = if let ProtocolPieceCommand::DragUpdate { delta } = envelope.command {
            let ClientCommandSequence::Move {
                after_control_sequence,
                tick,
            } = envelope.sequence
            else {
                unreachable!()
            };
            Some(RemoteDragUpdate {
                session: session.session_id(),
                authority_epoch: session.cursor().epoch,
                player: authenticated_player,
                grab_sequence: self.players[&authenticated_player].grab_sequence,
                basis_sequence: after_control_sequence,
                tick,
                delta,
            })
        } else {
            None
        };
        Ok(HostCommandOutcome {
            result,
            authority_event,
            drag_update,
        })
    }
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

    /// Read-only join capture at a complete command boundary. Old session,
    /// authority epoch or store-generation contexts are never exposed.
    pub(crate) fn active_drags<'a>(
        &'a self,
        session: &AuthoritySession,
        store: &PieceDataStore,
    ) -> impl Iterator<Item = (PlayerId, &'a ActiveDrag)> {
        let visible = session.is_active() && self.scope == Some(Self::scope(session, store));
        self.players
            .iter()
            .filter(move |_| visible)
            .map(|(&player, drag)| (player, drag))
    }

    /// Unreplicated emergency cleanup for migration/repair/tests. Normal
    /// disconnects in an active replicated session must use cancel_replicated.
    pub fn cancel_player(&mut self, store: &mut PieceDataStore, player: PlayerId) {
        self.players.remove(&player);
        self.validation.remove(&player);
        store.clear_player_holds(player);
    }

    /// Cancel only a current-scope drag as one Reliable authority event. A player
    /// without a context or holds is a no-op; orphan holds require repair/resync.
    /// All fallible checks precede store/context/cursor mutation.
    pub fn cancel_replicated(
        &mut self,
        session: &mut AuthoritySession,
        store: &mut PieceDataStore,
        player: PlayerId,
    ) -> Result<Option<HostCancellationOutcome>, ProtocolCommandError> {
        if !session.is_active() {
            return Err(ProtocolCommandError::Sequence(ProtocolError::Frozen));
        }
        if session.cursor().sequence.0 == u64::MAX {
            return Err(ProtocolCommandError::Sequence(
                ProtocolError::CounterExhausted,
            ));
        }
        let Some(drag) = self.active_drag(session, store, player) else {
            return if store.held_by.has_player(player) {
                Err(ProtocolCommandError::InconsistentDragTarget)
            } else {
                Ok(None)
            };
        };
        let grab_sequence = drag.grab_sequence;
        let applied = store
            .cancel_drag_target(player, &drag.target)
            .ok_or(ProtocolCommandError::InconsistentDragTarget)?;
        self.players.remove(&player);
        self.validation.remove(&player);
        let cursor = session
            .advance_authority()
            .expect("preflighted active cursor");
        Ok(Some(HostCancellationOutcome {
            applied,
            authority_event: ProtocolAuthorityEventEnvelope {
                session: session.session_id(),
                host: session.host(),
                cursor,
                event: ProtocolAuthorityEvent::DragCancelled(DragCancelled {
                    player,
                    grab_sequence,
                }),
            },
        }))
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
        local_player: PlayerId,
    ) -> Result<ProtocolCommandResult, ProtocolCommandError> {
        if envelope.player != authenticated_player {
            return Err(ProtocolCommandError::WrongPlayer);
        }
        let scope = Self::scope(session, store);
        if self.scope != Some(scope) {
            self.players.clear();
            self.validation.clear();
            self.scope = Some(scope);
        }
        session
            .accept_command(envelope)
            .map_err(ProtocolCommandError::Sequence)?;
        let player = authenticated_player;
        if matches!(
            envelope.command,
            ProtocolPieceCommand::Rotate { .. } | ProtocolPieceCommand::RotateDrag { .. }
        ) && definition.is_none_or(|d| !d.rotation_enabled)
        {
            return Err(ProtocolCommandError::RotationDisabled);
        }
        match &envelope.command {
            ProtocolPieceCommand::RotateDrag {
                grab_sequence,
                final_delta,
                through_tick,
                quarter_turns,
            } => {
                if !final_delta.is_finite() {
                    return Err(ProtocolCommandError::InvalidDelta);
                }
                let definition = definition
                    .filter(|d| d.validate().is_ok() && d.piece_count() == store.len())
                    .ok_or(ProtocolCommandError::InvalidDefinition)?;
                let drag = self
                    .players
                    .get_mut(&player)
                    .ok_or(ProtocolCommandError::NoActiveDrag)?;
                if *grab_sequence != drag.grab_sequence {
                    return Err(ProtocolCommandError::WrongDragContext);
                }
                if !valid_rebase_tick(drag.last_tick, *through_tick) {
                    return Err(ProtocolCommandError::InvalidRebaseTick);
                }
                let validation = self
                    .validation
                    .get(&player)
                    .ok_or(ProtocolCommandError::InvalidDefinition)?;
                if !validation.accepts(*final_delta) {
                    return Err(ProtocolCommandError::InvalidDelta);
                }
                let area = validation.area;
                let ClientCommandSequence::Control(basis_sequence) = envelope.sequence else {
                    unreachable!()
                };
                let turns = jigsall_core::add_quarter_turns(0, *quarter_turns) as i8;
                let (applied, roots) = store
                    .rotate_drag_target(player, &drag.target, *final_delta, turns, definition)
                    .ok_or(ProtocolCommandError::InconsistentDragTarget)?;
                self.validation.get_mut(&player).unwrap().pivots =
                    PivotEnvelope::from_roots(store, roots.iter().copied(), area)
                        .expect("rotation plans preflight finite legal pivots");
                drag.delta = bevy::math::Vec2::ZERO;
                drag.last_tick = *through_tick;
                drag.basis_sequence = basis_sequence;
                session.rebase_drag_sequence(player, basis_sequence, *through_tick);
                let result = super::release::drag_rotation_fingerprint(
                    store, &roots, definition, &applied, player, drag,
                );
                Ok(ProtocolCommandResult::DragRotated { applied, result })
            }
            ProtocolPieceCommand::Rotate {
                target,
                quarter_turns,
            } => {
                if self.players.contains_key(&player) {
                    return Err(ProtocolCommandError::ActiveDragExists);
                }
                let definition = definition
                    .filter(|d| d.validate().is_ok() && d.piece_count() == store.len())
                    .ok_or(ProtocolCommandError::InvalidDefinition)?;
                let turns = jigsall_core::add_quarter_turns(0, *quarter_turns) as i8;
                let rotation = store
                    .rotate_target(target, turns, definition)
                    .map_err(ProtocolCommandError::Target)?;
                let result = super::release::result_fingerprint(
                    store,
                    &rotation.roots,
                    Some(definition),
                    &rotation.applied,
                );
                Ok(ProtocolCommandResult::Rotated {
                    applied: rotation.applied,
                    rejected: rotation.rejected,
                    commit: RotationCommitted {
                        player,
                        accepted: rotation.accepted,
                        quarter_turns: turns,
                        result,
                    },
                })
            }
            ProtocolPieceCommand::Grab { target } => {
                if self.players.contains_key(&player) {
                    return Err(ProtocolCommandError::ActiveDragExists);
                }
                let definition = definition
                    .filter(|d| d.validate().is_ok() && d.piece_count() == store.len())
                    .ok_or(ProtocolCommandError::InvalidDefinition)?;
                let area = LogicalPlayArea::from_definition(definition)
                    .map_err(|_| ProtocolCommandError::InvalidDefinition)?;
                let resolved = target
                    .resolve(&store.connectivity)
                    .map_err(ProtocolCommandError::Target)?;
                let (applied, target, pivots) = match resolved.target {
                    ResolvedPieceTarget::Sparse(mut refs) => {
                        refs.retain(|reference| {
                            store
                                .connectivity
                                .iter_component(reference.member)
                                .all(|id| store.is_selectable(id))
                        });
                        let nonempty = !refs.is_empty();
                        let target = ActiveDragTarget::Sparse(refs);
                        let pivots = PivotEnvelope::from_target(store, &target, area);
                        if nonempty
                            && pivots.is_none_or(|p| !p.accepts(area, bevy::math::Vec2::ZERO))
                        {
                            return Err(ProtocolCommandError::InvalidDelta);
                        }
                        let ActiveDragTarget::Sparse(refs) = &target else {
                            unreachable!()
                        };
                        let applied = store.grab_resolved_components(
                            player,
                            refs.iter().map(|r| r.member),
                            local_player,
                        );
                        (applied, target, pivots)
                    }
                    ResolvedPieceTarget::Dense(members) => {
                        let accepted = store.selectable_members(&members);
                        let target =
                            ActiveDragTarget::from_accepted_members(&store.connectivity, &accepted)
                                .map_err(ProtocolCommandError::Target)?;
                        let pivots = PivotEnvelope::from_target(store, &target, area);
                        if !accepted.is_empty()
                            && pivots.is_none_or(|p| !p.accepts(area, bevy::math::Vec2::ZERO))
                        {
                            return Err(ProtocolCommandError::InvalidDelta);
                        }
                        (
                            store.grab_accepted_members(player, &accepted, local_player),
                            target,
                            pivots,
                        )
                    }
                };
                let ClientCommandSequence::Control(grab_sequence) = envelope.sequence else {
                    unreachable!()
                };
                let ack = GrabAccepted {
                    player,
                    grab_sequence,
                    accepted: target.to_piece_target(),
                    rejected: resolved.rejected,
                };
                if applied.grabbed != 0 {
                    self.validation.insert(
                        player,
                        DragValidation {
                            area,
                            pivots: pivots
                                .expect("nonempty accepted components have finite centers"),
                        },
                    );
                    self.players.insert(
                        player,
                        ActiveDrag {
                            grab_sequence,
                            basis_sequence: grab_sequence,
                            last_tick: None,
                            target,
                            delta: bevy::math::Vec2::ZERO,
                        },
                    );
                }
                Ok(ProtocolCommandResult::Grabbed { applied, ack })
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
                    tick,
                } = envelope.sequence
                else {
                    unreachable!()
                };
                if after_control_sequence != drag.basis_sequence {
                    return Err(ProtocolCommandError::WrongDragContext);
                }
                if let Some(last) = drag.last_tick {
                    if tick <= last {
                        return Err(ProtocolCommandError::Sequence(if tick == last {
                            ProtocolError::DuplicateCommand
                        } else {
                            ProtocolError::StaleCommand
                        }));
                    }
                }
                if !self
                    .validation
                    .get(&player)
                    .is_some_and(|validation| validation.accepts(*delta))
                {
                    return Err(ProtocolCommandError::InvalidDelta);
                }
                let expected = drag
                    .last_tick
                    .map_or(Some(0), |last| last.checked_add(1))
                    .ok_or(ProtocolCommandError::Sequence(
                        ProtocolError::CounterExhausted,
                    ))?;
                drag.last_tick = Some(tick);
                drag.delta = *delta;
                let sequence = if tick == expected {
                    CommandSequenceStatus::InOrder
                } else {
                    CommandSequenceStatus::Gap { expected }
                };
                Ok(ProtocolCommandResult::DragUpdated { sequence })
            }
            ProtocolPieceCommand::Release {
                grab_sequence,
                final_delta,
            } => {
                if !final_delta.is_finite() {
                    return Err(ProtocolCommandError::InvalidDelta);
                }
                let definition =
                    definition.filter(|d| d.validate().is_ok() && d.piece_count() == store.len());
                let drag = self
                    .players
                    .get(&player)
                    .ok_or(ProtocolCommandError::NoActiveDrag)?;
                if *grab_sequence != drag.grab_sequence {
                    return Err(ProtocolCommandError::WrongDragContext);
                }
                if !self
                    .validation
                    .get(&player)
                    .is_some_and(|validation| validation.accepts(*final_delta))
                {
                    return Err(ProtocolCommandError::InvalidDelta);
                }
                if !store.release_target_fits(
                    &drag.target,
                    *final_delta,
                    definition,
                    self.validation[&player].area,
                ) {
                    return Err(ProtocolCommandError::InvalidDelta);
                }
                let (applied, rejected, result) = super::release::release_drag(
                    store,
                    player,
                    &drag.target,
                    *final_delta,
                    definition,
                    local_player,
                )
                .map_err(ProtocolCommandError::Target)?;
                self.players.remove(&player);
                self.validation.remove(&player);
                Ok(ProtocolCommandResult::Released {
                    applied,
                    rejected,
                    result,
                })
            }
        }
    }
}

pub(super) fn valid_rebase_tick(last: Option<u64>, through: Option<u64>) -> bool {
    last.is_none_or(|last| through.is_some_and(|through| through >= last))
}

#[cfg(test)]
#[path = "protocol_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "play_area_tests.rs"]
mod play_area_tests;
