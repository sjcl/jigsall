//! Session-local sender. Membership work happens only at Reliable boundaries.
use crate::{interaction::PieceInteraction, resources::PieceDataStore};
use bevy::math::Vec2;
use jigsall_core::{protocol::*, session::*, PieceBitSet, PieceCommand, PlayerId};
use std::collections::VecDeque;
use std::sync::Arc;

#[cfg(test)]
#[path = "bridge_tests.rs"]
mod pending_release_tests;
#[cfg(test)]
#[path = "rotation_prediction_tests.rs"]
mod rotation_prediction_tests;

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
    use jigsall_core::PieceId;
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
    /// Exact local intent for this in-flight control, separate from wire basis.
    predicted: Option<PieceCommand>,
}
struct PendingRelease {
    delta: Vec2,
    token: Arc<()>,
}
#[derive(Default)]
pub(super) struct CommandBridge {
    pub prediction_enabled: bool,
    next_control: Option<u64>,
    started: bool,
    active: Option<LocalDrag>,
    pending: Option<PendingControl>,
    queue: VecDeque<(PieceCommand, Option<Vec2>, Arc<()>)>,
    release: Option<PendingRelease>,
    scope: Option<(SessionId, AuthorityEpoch, u64)>,
}

impl CommandBridge {
    /// Replay the uncommitted suffix only at input / Reliable boundaries. The
    /// pending envelope identifies the prefix to retire; queued controls retain
    /// ordering, gesture token and the existing ACK-adjusted protocol deltas.
    pub fn refresh_prediction(
        &self,
        player: PlayerId,
        definition: Option<&jigsall_core::PuzzleDefinition>,
        interaction: &PieceInteraction,
        store: &mut PieceDataStore,
    ) {
        if !self.prediction_enabled {
            return;
        }
        let Some(definition) = definition else {
            store.clear_local_rotation();
            return;
        };
        let current = interaction.network_gesture_token();
        let mut optimistic_grab = self
            .pending
            .as_ref()
            .is_some_and(|p| p.requested.is_some() && Arc::ptr_eq(&p.token, &current));
        let mut poses = std::collections::HashMap::new();
        let mut consumed = Vec2::ZERO;
        let mut drag_members = None;
        let pending = self
            .pending
            .as_ref()
            .and_then(|p| p.predicted.as_ref().map(|command| (command, &p.token)));
        for (command, token) in pending
            .into_iter()
            .chain(self.queue.iter().map(|(c, _, t)| (c, t)))
        {
            match command {
                PieceCommand::Grab(_) | PieceCommand::GrabGroup { .. }
                    if Arc::ptr_eq(token, &current) =>
                {
                    optimistic_grab = true;
                }
                PieceCommand::Rotate {
                    target,
                    quarter_turns,
                } => {
                    store.predict_rotation(target, *quarter_turns, definition, &mut poses);
                }
                PieceCommand::RotateDrag {
                    members,
                    delta,
                    quarter_turns,
                } if Arc::ptr_eq(token, &current) => {
                    let active = self
                        .active
                        .as_ref()
                        .filter(|a| Arc::ptr_eq(&a.token, token));
                    // Until Grab ACK, requested members are optimistic only.
                    // After ACK, the exact accepted mask always replaces them.
                    let accepted = active.map_or(members, |a| &a.members);
                    let optimistic = active.is_none() && optimistic_grab;
                    if active.is_none() && !optimistic {
                        continue;
                    }
                    if let Some(delta) = store.predict_drag_rotation(
                        crate::resources::pieces::local_rotation::PredictionDrag {
                            members: accepted,
                            player,
                            optimistic_grab: optimistic,
                        },
                        *delta - consumed,
                        *quarter_turns,
                        definition,
                        &mut poses,
                    ) {
                        consumed += delta;
                        drag_members = Some(accepted);
                    }
                }
                _ => {}
            }
        }
        // Keep the protocol/pointer delta scalar hot path. The override is a
        // rotated base minus consumed translation, so the shader adds it once.
        if let Some(members) = drag_members {
            for (id, pose) in &mut poses {
                if members.contains(id) {
                    pose.position -= consumed;
                }
            }
        }
        store.set_local_rotation(poses);
    }
    pub fn enqueue(
        &mut self,
        command: PieceCommand,
        pointer: Option<Vec2>,
        token: Arc<()>,
    ) -> Result<(), BridgeError> {
        if self.queue.len() == 64 {
            return Err(BridgeError::Capacity);
        }
        if self.release.is_none() {
            let delta = match &command {
                PieceCommand::ReleaseGroup { delta, .. } => Some(*delta),
                PieceCommand::Release(_) => self.active.as_ref().map(|drag| drag.scalar_delta),
                _ => None,
            };
            if let Some(delta) = delta {
                self.release = Some(PendingRelease {
                    delta,
                    token: token.clone(),
                });
            }
        }
        self.queue.push_back((command, pointer, token));
        Ok(())
    }
    pub fn synchronize(
        &mut self,
        session: &AuthoritySession,
        interaction: &mut PieceInteraction,
        store: &mut PieceDataStore,
    ) {
        let scope = (session.session_id(), session.cursor().epoch, store.epoch);
        if !session.is_active() {
            store.clear_local_rotation();
            self.clear_release(interaction, store);
            self.active = None;
            self.pending = None;
            self.queue.clear();
            *interaction = PieceInteraction::default();
            store.drag = Default::default();
        }
        if let Some(old) = self.scope.filter(|old| *old != scope) {
            store.clear_local_rotation();
            // A store reinstall in the same authority scope must preserve the
            // sender's consumed Control history, while discarding presentation.
            let same_authority = (old.0, old.1) == (scope.0, scope.1);
            *self = Self {
                next_control: same_authority.then_some(self.next_control).flatten(),
                started: same_authority && self.started,
                prediction_enabled: self.prediction_enabled,
                ..Default::default()
            };
            *interaction = PieceInteraction::default();
            store.drag = Default::default();
        }
        self.scope = Some(scope);
    }
    /// Shares the accepted COW mask; idle pending frames touch only scalars/Arcs.
    pub fn present_release(
        &mut self,
        interaction: &mut PieceInteraction,
        store: &mut PieceDataStore,
    ) {
        if self.active.is_none() && self.pending.is_none() && self.queue.is_empty() {
            self.clear_release(interaction, store);
        }
        let Some(release) = &self.release else { return };
        if !interaction.hold_network_release(&release.token) {
            return;
        }
        if let Some(active) = &self.active {
            if Arc::ptr_eq(&active.token, &release.token) {
                store.drag.members = active.members.words().clone();
                store.drag.delta = release.delta;
            }
        } else if self.prediction_enabled {
            if let Some(pending) = self
                .pending
                .as_ref()
                .filter(|p| Arc::ptr_eq(&p.token, &release.token))
            {
                if let Some(requested) = &pending.requested {
                    store.drag.members = requested.words().clone();
                    store.drag.delta = release.delta;
                }
            }
        }
    }
    fn clear_release(&mut self, interaction: &mut PieceInteraction, store: &mut PieceDataStore) {
        if let Some(release) = self.release.take() {
            interaction.clear_network_release(&release.token, store);
        }
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
            let predicted = (self.prediction_enabled
                && matches!(
                    &command,
                    PieceCommand::Rotate { .. } | PieceCommand::RotateDrag { .. }
                ))
            .then(|| command.clone());
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
            if let ProtocolPieceCommand::Release { final_delta, .. } = &envelope.command {
                // Freeze the actual wire input, after all queued basis adjustments.
                if self.release.is_none() {
                    self.release = Some(PendingRelease {
                        delta: *final_delta,
                        token: token.clone(),
                    });
                }
                if let Some(release) = &mut self.release {
                    if Arc::ptr_eq(&release.token, &token) {
                        release.delta = *final_delta;
                    }
                }
            }
            self.pending = Some(PendingControl {
                envelope: envelope.clone(),
                requested,
                pointer,
                token,
                predicted,
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
        if self.release.is_some() || self.pending.is_some() || !self.queue.is_empty() {
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
        store.clear_local_rotation();
        if let Some(pending) = self.pending.take() {
            match pending.envelope.command {
                ProtocolPieceCommand::Grab { .. } => {
                    self.clear_release(interaction, store);
                    self.queue.clear();
                    self.active = None;
                    *interaction = PieceInteraction::default();
                    store.drag = Default::default();
                }
                ProtocolPieceCommand::Release { .. } => {
                    self.clear_release(interaction, store);
                    self.active = None;
                }
                _ => {}
            }
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
        if let ProtocolAuthorityEvent::DragCancelled(cancel) = event {
            let Some(active) = self
                .active
                .as_ref()
                .filter(|a| a.grab == cancel.grab_sequence)
            else {
                return Ok(());
            };
            let token = active.token.clone();
            if self
                .release
                .as_ref()
                .is_some_and(|r| Arc::ptr_eq(&r.token, &token))
            {
                self.clear_release(interaction, store);
            }
            self.active = None;
            if self
                .pending
                .as_ref()
                .is_some_and(|p| Arc::ptr_eq(&p.token, &token))
            {
                self.pending = None;
            }
            self.queue.retain(|(_, _, t)| !Arc::ptr_eq(t, &token));
            if Arc::ptr_eq(&interaction.network_gesture_token(), &token) {
                store.clear_local_rotation();
                *interaction = PieceInteraction::default();
                store.drag = Default::default();
            }
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
                if self.active.is_none() {
                    self.clear_release(interaction, store);
                }
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
                // Validate the accepted members in their committed basis,
                // including when the gesture ended with a queued Release.
                let validation = interaction.validation_for_members(store, &active.members);
                // Controls sampled while waiting still carry the previous basis.
                for (command, _, token) in &mut self.queue {
                    let releasing = matches!(command, PieceCommand::ReleaseGroup { .. });
                    match command {
                        PieceCommand::ReleaseGroup { delta, .. }
                        | PieceCommand::RotateDrag { delta, .. } => {
                            let residual = *delta - commit.final_delta;
                            *delta =
                                validation.map_or(residual, |v| v.pivots.clamp(v.area, residual));
                            if releasing {
                                if let Some(release) = &mut self.release {
                                    if Arc::ptr_eq(&release.token, token) {
                                        release.delta = *delta;
                                    }
                                }
                            }
                        }
                        PieceCommand::Grab(_) | PieceCommand::GrabGroup { .. } => break,
                        _ => {}
                    }
                }
            }
            (
                ProtocolAuthorityEvent::ReleaseCommitted(commit),
                ProtocolPieceCommand::Release { grab_sequence, .. },
            ) if commit.grab_sequence == *grab_sequence => {
                // ClientRouter has already committed the canonical release.
                self.clear_release(interaction, store);
                self.active = None;
            }
            (ProtocolAuthorityEvent::RotationCommitted(_), ProtocolPieceCommand::Rotate { .. }) => {
            }
            _ => return Err(BridgeError::UnexpectedAuthorityResult),
        }
        self.present_release(interaction, store);
        Ok(())
    }
}
