//! Session-local sender. Membership work happens only at Reliable boundaries.
use crate::{interaction::PieceInteraction, resources::PieceDataStore};
use bevy::math::Vec2;
use puzzella_core::{protocol::*, session::*, PieceBitSet, PieceCommand, PlayerId};
use std::collections::VecDeque;
use std::sync::Arc;

#[derive(Debug, PartialEq, Eq)]
pub enum BridgeError {
    Capacity,
    CounterExhausted,
    InvalidPiece,
    Target(TargetError),
    UnexpectedAuthorityResult,
}

#[cfg(test)]
mod tests {
    use super::*;
    use puzzella_core::PieceId;
    fn session() -> AuthoritySession {
        AuthoritySession::new(
            SessionDefinition {
                id: SessionId(1),
                image_hash: ImageHash([0; 32]),
            },
            PlayerId(37),
            AuthorityCursor::new(0, 0),
        )
    }
    #[test]
    fn exhausted_control_and_move_counters_never_wrap() {
        let mut bridge = CommandBridge {
            started: true,
            next_control: None,
            ..Default::default()
        };
        let mut store = PieceDataStore::default();
        store.initialize(vec![Vec2::ZERO]);
        bridge
            .enqueue(PieceCommand::Grab(PieceId(0)), None, Arc::new(()))
            .unwrap();
        assert_eq!(
            bridge.next(&session(), PlayerId(37), &store),
            Err(BridgeError::CounterExhausted)
        );
        let mut members = PieceBitSet::new(1);
        members.insert(PieceId(0));
        bridge.active = Some(LocalDrag {
            grab: 0,
            basis: 0,
            last_tick: Some(u64::MAX),
            members,
            last_delta: None,
            scalar_delta: Vec2::ONE,
            token: Arc::new(()),
        });
        assert_eq!(
            bridge.drag_update(&session(), PlayerId(37), &store),
            Err(BridgeError::CounterExhausted)
        );
    }
}

struct LocalDrag {
    grab: u64,
    basis: u64,
    last_tick: Option<u64>,
    members: PieceBitSet,
    last_delta: Option<Vec2>,
    scalar_delta: Vec2,
    token: Arc<()>,
}
struct PendingControl {
    envelope: ProtocolCommandEnvelope,
    requested: Option<PieceBitSet>,
    pointer: Option<Vec2>,
    token: Arc<()>,
}
#[derive(Default)]
pub(super) struct CommandBridge {
    next_control: Option<u64>,
    started: bool,
    active: Option<LocalDrag>,
    pending: Option<PendingControl>,
    queue: VecDeque<(PieceCommand, Option<Vec2>, Arc<()>)>,
}

impl CommandBridge {
    pub fn enqueue(
        &mut self,
        command: PieceCommand,
        pointer: Option<Vec2>,
        token: Arc<()>,
    ) -> Result<(), BridgeError> {
        if self.queue.len() == 64 {
            return Err(BridgeError::Capacity);
        }
        self.queue.push_back((command, pointer, token));
        Ok(())
    }
    fn envelope(
        session: &AuthoritySession,
        player: PlayerId,
        sequence: ClientCommandSequence,
        command: ProtocolPieceCommand,
    ) -> ProtocolCommandEnvelope {
        ProtocolCommandEnvelope {
            session: session.session_id(),
            authority_epoch: session.cursor().epoch,
            player,
            sequence,
            command,
        }
    }
    pub fn next(
        &mut self,
        session: &AuthoritySession,
        player: PlayerId,
        store: &PieceDataStore,
    ) -> Result<Option<ProtocolCommandEnvelope>, BridgeError> {
        if self.pending.is_some() {
            return Ok(None);
        }
        while let Some((command, pointer, token)) = self.queue.pop_front() {
            let mut requested = None;
            let command = match command {
                PieceCommand::Grab(id) => {
                    let reference = ComponentRef::from_member(&store.connectivity, id)
                        .map_err(BridgeError::Target)?;
                    let mut members = PieceBitSet::new(store.len());
                    members.extend(store.connectivity.iter_component(id));
                    requested = Some(members);
                    ProtocolPieceCommand::Grab {
                        target: PieceTarget::Component(reference),
                    }
                }
                PieceCommand::GrabGroup { members } => {
                    let target = PieceTarget::from_selection(&store.connectivity, &members)
                        .map_err(BridgeError::Target)?;
                    requested = Some(members);
                    ProtocolPieceCommand::Grab { target }
                }
                PieceCommand::Move { id, position } => {
                    if let Some(active) = &mut self.active {
                        if active.members.contains(&id) {
                            active.scalar_delta = position
                                - store.state(id).ok_or(BridgeError::InvalidPiece)?.position;
                        }
                    }
                    continue;
                }
                PieceCommand::Release(_) => {
                    let Some(active) = &self.active else { continue };
                    ProtocolPieceCommand::Release {
                        grab_sequence: active.grab,
                        final_delta: active.scalar_delta,
                    }
                }
                PieceCommand::ReleaseGroup { delta, .. } => {
                    let Some(active) = &self.active else { continue };
                    ProtocolPieceCommand::Release {
                        grab_sequence: active.grab,
                        final_delta: delta,
                    }
                }
                PieceCommand::Rotate {
                    target,
                    quarter_turns,
                } => ProtocolPieceCommand::Rotate {
                    target,
                    quarter_turns,
                },
                PieceCommand::RotateDrag {
                    delta,
                    quarter_turns,
                    ..
                } => {
                    let Some(active) = &self.active else { continue };
                    ProtocolPieceCommand::RotateDrag {
                        grab_sequence: active.grab,
                        final_delta: delta,
                        through_tick: active.last_tick,
                        quarter_turns,
                    }
                }
            };
            let sequence = if self.started {
                self.next_control.ok_or(BridgeError::CounterExhausted)?
            } else {
                self.started = true;
                0
            };
            self.next_control = sequence.checked_add(1);
            let envelope = Self::envelope(
                session,
                player,
                ClientCommandSequence::Control(sequence),
                command,
            );
            self.pending = Some(PendingControl {
                envelope: envelope.clone(),
                requested,
                pointer,
                token,
            });
            return Ok(Some(envelope));
        }
        Ok(None)
    }
    /// Once per frame after controls. Pending ACKs suppress uncommitted bases.
    /// Pointer frames access scalars only; no mask access or serialization.
    pub fn drag_update(
        &mut self,
        session: &AuthoritySession,
        player: PlayerId,
        store: &PieceDataStore,
    ) -> Result<Option<ProtocolCommandEnvelope>, BridgeError> {
        if self.pending.is_some() || !self.queue.is_empty() {
            return Ok(None);
        }
        let Some(active) = &mut self.active else {
            return Ok(None);
        };
        let delta = if store.drag.members.is_empty() {
            active.scalar_delta
        } else {
            store.drag.delta
        };
        if !delta.is_finite() || active.last_delta == Some(delta) {
            return Ok(None);
        }
        let tick = active
            .last_tick
            .map_or(Some(0), |tick| tick.checked_add(1))
            .ok_or(BridgeError::CounterExhausted)?;
        active.last_tick = Some(tick);
        active.last_delta = Some(delta);
        Ok(Some(Self::envelope(
            session,
            player,
            ClientCommandSequence::Move {
                after_control_sequence: active.basis,
                tick,
            },
            ProtocolPieceCommand::DragUpdate { delta },
        )))
    }
    pub fn reject(&mut self, interaction: &mut PieceInteraction, store: &mut PieceDataStore) {
        if self.pending.take().is_some_and(|pending| {
            matches!(pending.envelope.command, ProtocolPieceCommand::Grab { .. })
        }) {
            *interaction = PieceInteraction::default();
            store.drag = Default::default();
        }
        // A consumed rejection never rewinds Control history or the old basis.
    }
    pub fn reconcile(
        &mut self,
        player: PlayerId,
        event: &ProtocolAuthorityEvent,
        interaction: &mut PieceInteraction,
        store: &mut PieceDataStore,
    ) -> Result<(), BridgeError> {
        let event_player = match event {
            ProtocolAuthorityEvent::GrabAccepted(e) => e.player,
            ProtocolAuthorityEvent::ReleaseCommitted(e) => e.player,
            ProtocolAuthorityEvent::RotationCommitted(e) => e.player,
            ProtocolAuthorityEvent::DragRotationCommitted(e) => e.player,
            ProtocolAuthorityEvent::DragCancelled(e) => e.player,
        };
        if event_player != player {
            return Ok(());
        }
        if matches!(event, ProtocolAuthorityEvent::DragCancelled(_)) {
            self.active = None;
            self.pending = None;
            self.queue.clear();
            *interaction = PieceInteraction::default();
            store.drag = Default::default();
            return Ok(());
        }
        let pending = self
            .pending
            .take()
            .ok_or(BridgeError::UnexpectedAuthorityResult)?;
        let ClientCommandSequence::Control(sequence) = pending.envelope.sequence else {
            unreachable!()
        };
        match (event, &pending.envelope.command) {
            (ProtocolAuthorityEvent::GrabAccepted(ack), ProtocolPieceCommand::Grab { .. })
                if ack.grab_sequence == sequence =>
            {
                let accepted = match &ack.accepted {
                    PieceTarget::Dense(target) => target.members.clone(),
                    target => {
                        let resolved = target
                            .resolve(&store.connectivity)
                            .map_err(BridgeError::Target)?;
                        let ResolvedPieceTarget::Sparse(refs) = resolved.target else {
                            unreachable!()
                        };
                        let mut members = PieceBitSet::new(store.len());
                        for reference in refs {
                            members.extend(store.connectivity.iter_component(reference.member));
                        }
                        members
                    }
                };
                interaction.reconcile_network_grab(
                    &pending.token,
                    pending.requested.as_ref().expect("Grab request"),
                    accepted.clone(),
                    store,
                );
                self.active = (!accepted.is_empty()).then_some(LocalDrag {
                    grab: sequence,
                    basis: sequence,
                    last_tick: None,
                    members: accepted,
                    last_delta: None,
                    scalar_delta: Vec2::ZERO,
                    token: pending.token,
                });
            }
            (
                ProtocolAuthorityEvent::DragRotationCommitted(commit),
                ProtocolPieceCommand::RotateDrag { grab_sequence, .. },
            ) if commit.grab_sequence == *grab_sequence && commit.basis_sequence == sequence => {
                let active = self
                    .active
                    .as_mut()
                    .ok_or(BridgeError::UnexpectedAuthorityResult)?;
                interaction.rebase_network_drag(
                    &active.token,
                    &active.members,
                    pending.pointer,
                    commit.final_delta,
                    store,
                );
                active.basis = sequence;
                active.last_delta = None;
                active.scalar_delta = Vec2::ZERO;
                // Controls sampled while waiting still carry the previous basis.
                for (command, _, _) in &mut self.queue {
                    match command {
                        PieceCommand::ReleaseGroup { delta, .. }
                        | PieceCommand::RotateDrag { delta, .. } => *delta -= commit.final_delta,
                        PieceCommand::Grab(_) | PieceCommand::GrabGroup { .. } => break,
                        _ => {}
                    }
                }
            }
            (
                ProtocolAuthorityEvent::ReleaseCommitted(commit),
                ProtocolPieceCommand::Release { grab_sequence, .. },
            ) if commit.grab_sequence == *grab_sequence => self.active = None,
            (ProtocolAuthorityEvent::RotationCommitted(_), ProtocolPieceCommand::Rotate { .. }) => {
            }
            _ => return Err(BridgeError::UnexpectedAuthorityResult),
        }
        Ok(())
    }
}
