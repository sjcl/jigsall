//! Pure session protocol and authority handoff. Backend identity is deliberately absent.
use crate::{PieceCommand, PlayerId};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SessionId(pub u128);
/// SHA-256 of the retained encoded image bytes, computed/verified by the backend.
/// Content identity only: this digest does not authenticate a snapshot or its sender.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ImageHash(pub [u8; 32]);
/// Immutable session identity. Image dimensions alone cannot identify the puzzle image.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionDefinition {
    pub id: SessionId,
    pub image_hash: ImageHash,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AuthorityEpoch(pub u64);
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AuthoritySequence(pub u64);
/// Lexicographic order: every cursor in a newer epoch follows the previous epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AuthorityCursor {
    pub epoch: AuthorityEpoch,
    pub sequence: AuthoritySequence,
}
impl AuthorityCursor {
    pub fn new(epoch: u64, sequence: u64) -> Self {
        Self {
            epoch: AuthorityEpoch(epoch),
            sequence: AuthoritySequence(sequence),
        }
    }
}

/// Independent reliable controls and best-effort moves within one player/epoch.
/// Move ticks increment per command; after_control_sequence is the latest control sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientCommandSequence {
    Control(u64),
    Move {
        after_control_sequence: u64,
        tick: u64,
    },
}
/// Separate from the local Bevy ClientCommand. Authenticate player at the backend.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClientCommandEnvelope {
    pub session: SessionId,
    pub authority_epoch: AuthorityEpoch,
    pub player: PlayerId,
    pub sequence: ClientCommandSequence,
    pub command: PieceCommand,
}
/// A backend must authenticate the sender as host before applying this envelope.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AuthorityEventEnvelope<T> {
    pub session: SessionId,
    pub host: PlayerId,
    pub cursor: AuthorityCursor,
    pub event: T,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtocolError {
    WrongSession,
    WrongEpoch,
    WrongHost,
    DuplicateCommand,
    StaleCommand,
    WrongCommandStream,
    ControlGap { expected: u64 },
    ControlNotProcessed { required: u64 },
    StaleMoveContext,
    StaleEvent,
    EventGap { expected: AuthoritySequence },
    Frozen,
    InvalidTransition,
    WrongSnapshotCursor,
    MissingAcknowledgement,
    CounterExhausted,
}

/// Only Move gaps are accepted. Reliable controls must be consumed contiguously.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandSequenceStatus {
    InOrder,
    Gap { expected: u64 },
}
#[derive(Clone, Copy, Debug, Default)]
struct PlayerCommandSequences {
    control: Option<u64>,
    move_tick: Option<u64>,
}
#[derive(Clone, Debug)]
pub struct CommandSequenceTracker {
    session: SessionId,
    epoch: AuthorityEpoch,
    players: HashMap<PlayerId, PlayerCommandSequences>,
}
impl CommandSequenceTracker {
    pub fn new(session: SessionId, epoch: AuthorityEpoch) -> Self {
        Self {
            session,
            epoch,
            players: HashMap::new(),
        }
    }

    /// Rejections alter neither stream. Consume controls before gameplay validation,
    /// then apply/reject gameplay synchronously before processing another envelope.
    pub fn validate_and_record(
        &mut self,
        envelope: &ClientCommandEnvelope,
    ) -> Result<CommandSequenceStatus, ProtocolError> {
        if envelope.session != self.session {
            return Err(ProtocolError::WrongSession);
        }
        if envelope.authority_epoch != self.epoch {
            return Err(ProtocolError::WrongEpoch);
        }
        let mut player = self
            .players
            .get(&envelope.player)
            .copied()
            .unwrap_or_default();
        let status = match envelope.sequence {
            ClientCommandSequence::Control(sequence) => {
                if matches!(envelope.command, PieceCommand::Move { .. }) {
                    return Err(ProtocolError::WrongCommandStream);
                }
                let expected = next_sequence(player.control, sequence)?;
                if sequence != expected {
                    return Err(ProtocolError::ControlGap { expected });
                }
                player.control = Some(sequence);
                // Move ticks are scoped to the latest consumed control.
                player.move_tick = None;
                CommandSequenceStatus::InOrder
            }
            ClientCommandSequence::Move {
                after_control_sequence,
                tick,
            } => {
                if !matches!(envelope.command, PieceCommand::Move { .. }) {
                    return Err(ProtocolError::WrongCommandStream);
                }
                match player.control {
                    None => {
                        return Err(ProtocolError::ControlNotProcessed {
                            required: after_control_sequence,
                        })
                    }
                    Some(control) if after_control_sequence > control => {
                        return Err(ProtocolError::ControlNotProcessed {
                            required: after_control_sequence,
                        })
                    }
                    Some(control) if after_control_sequence < control => {
                        return Err(ProtocolError::StaleMoveContext)
                    }
                    Some(_) => {}
                }
                let expected = next_sequence(player.move_tick, tick)?;
                player.move_tick = Some(tick);
                if tick == expected {
                    CommandSequenceStatus::InOrder
                } else {
                    CommandSequenceStatus::Gap { expected }
                }
            }
        };
        self.players.insert(envelope.player, player);
        Ok(status)
    }
}
/// Validate a stream without changing its state; counters never wrap.
fn next_sequence(last: Option<u64>, sequence: u64) -> Result<u64, ProtocolError> {
    if let Some(last) = last {
        if sequence == last {
            return Err(ProtocolError::DuplicateCommand);
        }
        if sequence < last {
            return Err(ProtocolError::StaleCommand);
        }
        last.checked_add(1).ok_or(ProtocolError::CounterExhausted)
    } else {
        Ok(0)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoverySource {
    pub session: SessionId,
    pub player: PlayerId,
    pub cursor: AuthorityCursor,
}
/// Latest cursor wins; the lowest PlayerId breaks ties regardless of input order.
/// This chooses snapshot data, never the new host. Authenticate candidates externally.
/// Trusted-peer policy: cursors are peer claims, with no old-host provenance proof.
pub fn select_recovery_source(
    session: SessionId,
    candidates: impl IntoIterator<Item = RecoverySource>,
) -> Option<RecoverySource> {
    candidates
        .into_iter()
        .filter(|source| source.session == session)
        .max_by(|a, b| {
            a.cursor
                .cmp(&b.cursor)
                .then_with(|| b.player.0.cmp(&a.player.0))
        })
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MigrationState {
    Active,
    /// Final cursor is frozen until snapshot receipt/validation is acknowledged.
    Graceful {
        new_host: PlayerId,
        final_cursor: AuthorityCursor,
        acknowledged: bool,
    },
    /// Host loss can precede the external owner notification.
    Recovering {
        new_host: Option<PlayerId>,
        source: Option<RecoverySource>,
    },
}

/// Opt-in pure state; no ECS schedule, socket, host election or GPU lifecycle.
#[derive(Clone, Debug)]
pub struct AuthoritySession {
    definition: SessionDefinition,
    host: PlayerId,
    cursor: AuthorityCursor,
    migration: MigrationState,
    commands: CommandSequenceTracker,
}
impl AuthoritySession {
    pub fn new(definition: SessionDefinition, host: PlayerId, cursor: AuthorityCursor) -> Self {
        Self {
            definition,
            host,
            cursor,
            migration: MigrationState::Active,
            commands: CommandSequenceTracker::new(definition.id, cursor.epoch),
        }
    }
    pub fn session_id(&self) -> SessionId {
        self.definition.id
    }
    pub fn session_definition(&self) -> SessionDefinition {
        self.definition
    }
    pub fn image_hash(&self) -> ImageHash {
        self.definition.image_hash
    }
    pub fn host(&self) -> PlayerId {
        self.host
    }
    pub fn cursor(&self) -> AuthorityCursor {
        self.cursor
    }
    pub fn migration(&self) -> &MigrationState {
        &self.migration
    }
    pub fn is_active(&self) -> bool {
        self.migration == MigrationState::Active
    }
    fn require_active(&self) -> Result<(), ProtocolError> {
        if self.is_active() {
            Ok(())
        } else {
            Err(ProtocolError::Frozen)
        }
    }
    pub fn accept_command(
        &mut self,
        envelope: &ClientCommandEnvelope,
    ) -> Result<CommandSequenceStatus, ProtocolError> {
        self.require_active()?;
        self.commands.validate_and_record(envelope)
    }
    /// Publish after a host-approved authoritative change; zero is the epoch baseline.
    pub fn advance_authority(&mut self) -> Result<AuthorityCursor, ProtocolError> {
        self.require_active()?;
        let sequence = self
            .cursor
            .sequence
            .0
            .checked_add(1)
            .ok_or(ProtocolError::CounterExhausted)?;
        self.cursor.sequence = AuthoritySequence(sequence);
        Ok(self.cursor)
    }
    /// Check before applying an event; record its cursor only after gameplay succeeds.
    pub fn validate_event<T>(
        &self,
        envelope: &AuthorityEventEnvelope<T>,
    ) -> Result<(), ProtocolError> {
        self.require_active()?;
        if envelope.session != self.definition.id {
            return Err(ProtocolError::WrongSession);
        }
        if envelope.cursor.epoch != self.cursor.epoch {
            return Err(ProtocolError::WrongEpoch);
        }
        if envelope.host != self.host {
            return Err(ProtocolError::WrongHost);
        }
        if envelope.cursor.sequence <= self.cursor.sequence {
            return Err(ProtocolError::StaleEvent);
        }
        let expected = AuthoritySequence(
            self.cursor
                .sequence
                .0
                .checked_add(1)
                .ok_or(ProtocolError::CounterExhausted)?,
        );
        if envelope.cursor.sequence != expected {
            return Err(ProtocolError::EventGap { expected });
        }
        Ok(())
    }
    pub fn record_applied_event<T>(
        &mut self,
        envelope: &AuthorityEventEnvelope<T>,
    ) -> Result<(), ProtocolError> {
        self.validate_event(envelope)?;
        self.cursor = envelope.cursor;
        Ok(())
    }
    pub fn begin_graceful(&mut self, new_host: PlayerId) -> Result<(), ProtocolError> {
        self.require_active()?;
        if new_host == self.host {
            return Err(ProtocolError::WrongHost);
        }
        self.migration = MigrationState::Graceful {
            new_host,
            final_cursor: self.cursor,
            acknowledged: false,
        };
        Ok(())
    }
    /// Only call after the target validates/installs the final snapshot.
    pub fn acknowledge_snapshot(
        &mut self,
        session: SessionId,
        player: PlayerId,
        cursor: AuthorityCursor,
    ) -> Result<(), ProtocolError> {
        if session != self.definition.id {
            return Err(ProtocolError::WrongSession);
        }
        let MigrationState::Graceful {
            new_host,
            final_cursor,
            acknowledged,
        } = &mut self.migration
        else {
            return Err(ProtocolError::InvalidTransition);
        };
        if player != *new_host {
            return Err(ProtocolError::WrongHost);
        }
        if cursor != *final_cursor {
            return Err(ProtocolError::WrongSnapshotCursor);
        }
        *acknowledged = true;
        Ok(())
    }
    /// Immediately freeze after loss, even when the backend has not chosen an owner.
    /// Also recovers an interrupted graceful transfer.
    pub fn host_lost(&mut self) -> Result<(), ProtocolError> {
        if matches!(self.migration, MigrationState::Recovering { .. }) {
            return Err(ProtocolError::InvalidTransition);
        }
        self.migration = MigrationState::Recovering {
            new_host: None,
            source: None,
        };
        Ok(())
    }
    /// External lobby/session backend supplies the owner; core never elects one.
    pub fn host_changed(&mut self, new_host: PlayerId) -> Result<(), ProtocolError> {
        if new_host == self.host {
            return Err(ProtocolError::WrongHost);
        }
        let source = match self.migration {
            MigrationState::Active => None,
            MigrationState::Graceful {
                new_host: target,
                final_cursor,
                acknowledged,
            } => {
                if new_host != target {
                    return Err(ProtocolError::WrongHost);
                }
                if !acknowledged {
                    return Err(ProtocolError::MissingAcknowledgement);
                }
                Some(RecoverySource {
                    session: self.definition.id,
                    player: new_host,
                    cursor: final_cursor,
                })
            }
            MigrationState::Recovering {
                new_host: Some(_), ..
            } => return Err(ProtocolError::InvalidTransition),
            MigrationState::Recovering { source, .. } => source,
        };
        self.migration = MigrationState::Recovering {
            new_host: Some(new_host),
            source,
        };
        Ok(())
    }
    /// Candidates must belong to this session and the lost authority generation.
    pub fn choose_recovery_source(
        &mut self,
        candidates: impl IntoIterator<Item = RecoverySource>,
    ) -> Result<RecoverySource, ProtocolError> {
        let MigrationState::Recovering { source, .. } = &mut self.migration else {
            return Err(ProtocolError::InvalidTransition);
        };
        let chosen = select_recovery_source(
            self.definition.id,
            candidates
                .into_iter()
                .filter(|candidate| candidate.cursor.epoch == self.cursor.epoch),
        )
        .ok_or(ProtocolError::WrongSnapshotCursor)?;
        // A graceful ACK pins the exact final snapshot; do not silently replace it.
        if source.is_some() {
            return Err(ProtocolError::InvalidTransition);
        }
        // Incomplete peer responses must not roll back locally applied authority.
        if chosen.cursor < self.cursor {
            return Err(ProtocolError::WrongSnapshotCursor);
        }
        *source = Some(chosen);
        Ok(chosen)
    }
    pub fn recovery_source(&self) -> Option<RecoverySource> {
        if let MigrationState::Recovering { source, .. } = self.migration {
            source
        } else {
            None
        }
    }
    /// Pure final transition. Caller must validate/install the chosen snapshot first.
    /// game::multiplayer::install_migration_snapshot makes that operation transactional.
    pub fn complete_migration(
        &mut self,
        session: SessionId,
        snapshot_cursor: AuthorityCursor,
    ) -> Result<AuthorityCursor, ProtocolError> {
        if session != self.definition.id {
            return Err(ProtocolError::WrongSession);
        }
        let MigrationState::Recovering {
            new_host: Some(new_host),
            source: Some(source),
        } = self.migration
        else {
            return Err(ProtocolError::InvalidTransition);
        };
        if snapshot_cursor != source.cursor || snapshot_cursor < self.cursor {
            return Err(ProtocolError::WrongSnapshotCursor);
        }
        let epoch = AuthorityEpoch(
            self.cursor
                .epoch
                .0
                .checked_add(1)
                .ok_or(ProtocolError::CounterExhausted)?,
        );
        self.cursor = AuthorityCursor {
            epoch,
            sequence: AuthoritySequence(0),
        };
        self.host = new_host;
        self.commands = CommandSequenceTracker::new(self.definition.id, epoch);
        self.migration = MigrationState::Active;
        Ok(self.cursor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PieceId;
    const SESSION: SessionId = SessionId(7);
    const SESSION_DEFINITION: SessionDefinition = SessionDefinition {
        id: SESSION,
        image_hash: ImageHash([0x42; 32]),
    };
    const A: PlayerId = PlayerId(1);
    const B: PlayerId = PlayerId(2);
    fn command(player: PlayerId, sequence: u64) -> ClientCommandEnvelope {
        ClientCommandEnvelope {
            session: SESSION,
            authority_epoch: AuthorityEpoch(3),
            player,
            sequence: ClientCommandSequence::Control(sequence),
            command: PieceCommand::Grab(PieceId(0)),
        }
    }

    fn move_command(
        player: PlayerId,
        after_control_sequence: u64,
        tick: u64,
    ) -> ClientCommandEnvelope {
        ClientCommandEnvelope {
            sequence: ClientCommandSequence::Move {
                after_control_sequence,
                tick,
            },
            command: PieceCommand::Move {
                id: PieceId(0),
                position: bevy_math::Vec2::ONE,
            },
            ..command(player, 0)
        }
    }

    #[test]
    fn command_replay_gaps_sessions_and_epochs_are_checked_per_player() {
        let mut tracker = CommandSequenceTracker::new(SESSION, AuthorityEpoch(3));
        assert_eq!(
            tracker.validate_and_record(&command(A, 0)),
            Ok(CommandSequenceStatus::InOrder)
        );
        assert_eq!(
            tracker.validate_and_record(&command(A, 0)),
            Err(ProtocolError::DuplicateCommand)
        );
        assert_eq!(
            tracker.validate_and_record(&command(A, 3)),
            Err(ProtocolError::ControlGap { expected: 1 })
        );
        assert_eq!(
            tracker.validate_and_record(&command(A, 1)),
            Ok(CommandSequenceStatus::InOrder)
        );
        assert_eq!(
            tracker.validate_and_record(&command(A, 0)),
            Err(ProtocolError::StaleCommand)
        );
        assert_eq!(
            tracker.validate_and_record(&command(A, 2)),
            Ok(CommandSequenceStatus::InOrder)
        );
        assert_eq!(
            tracker.validate_and_record(&command(B, 0)),
            Ok(CommandSequenceStatus::InOrder)
        );
        for epoch in [2, 4] {
            let mut envelope = command(A, 3);
            envelope.authority_epoch = AuthorityEpoch(epoch);
            assert_eq!(
                tracker.validate_and_record(&envelope),
                Err(ProtocolError::WrongEpoch)
            );
        }
        let mut envelope = command(A, 3);
        envelope.session = SessionId(8);
        assert_eq!(
            tracker.validate_and_record(&envelope),
            Err(ProtocolError::WrongSession)
        );
        assert_eq!(
            tracker.validate_and_record(&command(A, 3)),
            Ok(CommandSequenceStatus::InOrder)
        );
        assert_eq!(
            tracker.validate_and_record(&command(B, 2)),
            Err(ProtocolError::ControlGap { expected: 1 })
        );
        assert_eq!(
            tracker.validate_and_record(&command(B, 1)),
            Ok(CommandSequenceStatus::InOrder)
        );
    }

    #[test]
    fn moves_do_not_consume_delayed_reliable_grab_or_release() {
        let mut tracker = CommandSequenceTracker::new(SESSION, AuthorityEpoch(3));
        let early_move = move_command(A, 0, 100);
        assert_eq!(
            tracker.validate_and_record(&early_move),
            Err(ProtocolError::ControlNotProcessed { required: 0 })
        );
        assert_eq!(
            tracker.validate_and_record(&command(A, 0)),
            Ok(CommandSequenceStatus::InOrder)
        );
        assert_eq!(
            tracker.validate_and_record(&early_move),
            Ok(CommandSequenceStatus::Gap { expected: 0 })
        );
        let release = ClientCommandEnvelope {
            command: PieceCommand::Release(PieceId(0)),
            ..command(A, 1)
        };
        assert_eq!(
            tracker.validate_and_record(&release),
            Ok(CommandSequenceStatus::InOrder)
        );
        assert_eq!(
            tracker.validate_and_record(&release),
            Err(ProtocolError::DuplicateCommand)
        );
        assert_eq!(
            tracker.validate_and_record(&early_move),
            Err(ProtocolError::StaleMoveContext)
        );
    }

    #[test]
    fn move_replay_is_scoped_to_latest_control_and_does_not_cross_regrabs() {
        let mut tracker = CommandSequenceTracker::new(SESSION, AuthorityEpoch(3));
        tracker.validate_and_record(&command(A, 0)).unwrap();
        assert_eq!(
            tracker.validate_and_record(&move_command(A, 0, 0)),
            Ok(CommandSequenceStatus::InOrder)
        );
        assert_eq!(
            tracker.validate_and_record(&move_command(A, 0, 0)),
            Err(ProtocolError::DuplicateCommand)
        );
        assert_eq!(
            tracker.validate_and_record(&move_command(A, 0, 5)),
            Ok(CommandSequenceStatus::Gap { expected: 1 })
        );
        assert_eq!(
            tracker.validate_and_record(&move_command(A, 0, 4)),
            Err(ProtocolError::StaleCommand)
        );
        assert_eq!(
            tracker.validate_and_record(&move_command(A, 0, 6)),
            Ok(CommandSequenceStatus::InOrder)
        );
        // A Move after a pending control cannot advance either stream.
        assert_eq!(
            tracker.validate_and_record(&move_command(A, 2, 99)),
            Err(ProtocolError::ControlNotProcessed { required: 2 })
        );
        tracker.validate_and_record(&command(A, 1)).unwrap();
        tracker.validate_and_record(&command(A, 2)).unwrap();
        assert_eq!(
            tracker.validate_and_record(&move_command(A, 0, 7)),
            Err(ProtocolError::StaleMoveContext)
        );
        assert_eq!(
            tracker.validate_and_record(&move_command(A, 2, 0)),
            Ok(CommandSequenceStatus::InOrder)
        );
        // The other player's control and move counters are independent.
        tracker.validate_and_record(&command(B, 0)).unwrap();
        assert_eq!(
            tracker.validate_and_record(&move_command(B, 0, 0)),
            Ok(CommandSequenceStatus::InOrder)
        );
    }

    #[test]
    fn commands_require_matching_sequence_streams_without_poisoning_counters() {
        let mut tracker = CommandSequenceTracker::new(SESSION, AuthorityEpoch(3));
        let wrong_control = ClientCommandEnvelope {
            sequence: ClientCommandSequence::Control(0),
            ..move_command(A, 0, 0)
        };
        assert_eq!(
            tracker.validate_and_record(&wrong_control),
            Err(ProtocolError::WrongCommandStream)
        );
        tracker.validate_and_record(&command(A, 0)).unwrap();
        let wrong_move = ClientCommandEnvelope {
            sequence: ClientCommandSequence::Move {
                after_control_sequence: 0,
                tick: 100,
            },
            ..command(A, 1)
        };
        assert_eq!(
            tracker.validate_and_record(&wrong_move),
            Err(ProtocolError::WrongCommandStream)
        );
        assert_eq!(
            tracker.validate_and_record(&move_command(A, 0, 0)),
            Ok(CommandSequenceStatus::InOrder)
        );
        assert_eq!(
            tracker.validate_and_record(&command(A, 1)),
            Ok(CommandSequenceStatus::InOrder)
        );
    }

    #[test]
    fn player_control_and_move_counters_do_not_wrap() {
        let mut tracker = CommandSequenceTracker::new(SESSION, AuthorityEpoch(3));
        tracker.players.insert(
            A,
            PlayerCommandSequences {
                control: Some(u64::MAX - 1),
                move_tick: None,
            },
        );
        assert_eq!(
            tracker.validate_and_record(&command(A, u64::MAX)),
            Ok(CommandSequenceStatus::InOrder)
        );
        assert_eq!(
            tracker.validate_and_record(&command(A, u64::MAX)),
            Err(ProtocolError::DuplicateCommand)
        );
        assert_eq!(
            tracker.validate_and_record(&command(A, 0)),
            Err(ProtocolError::StaleCommand)
        );
        tracker
            .validate_and_record(&move_command(A, u64::MAX, u64::MAX))
            .unwrap();
        assert_eq!(
            tracker.validate_and_record(&move_command(A, u64::MAX, u64::MAX)),
            Err(ProtocolError::DuplicateCommand)
        );
        assert_eq!(
            tracker.validate_and_record(&move_command(A, u64::MAX, 0)),
            Err(ProtocolError::StaleCommand)
        );
    }
    #[test]
    fn graceful_transfer_freezes_and_requires_exact_ack_and_owner() {
        let mut state = AuthoritySession::new(SESSION_DEFINITION, A, AuthorityCursor::new(3, 100));
        state.accept_command(&command(A, 0)).unwrap();
        state.accept_command(&move_command(A, 0, 100)).unwrap();
        state.begin_graceful(B).unwrap();
        assert_eq!(
            state.accept_command(&command(A, 0)),
            Err(ProtocolError::Frozen)
        );
        assert_eq!(state.advance_authority(), Err(ProtocolError::Frozen));
        assert_eq!(
            state.host_changed(B),
            Err(ProtocolError::MissingAcknowledgement)
        );
        assert_eq!(
            state.acknowledge_snapshot(SessionId(8), B, state.cursor()),
            Err(ProtocolError::WrongSession)
        );
        assert_eq!(
            state.acknowledge_snapshot(SESSION, A, state.cursor()),
            Err(ProtocolError::WrongHost)
        );
        assert_eq!(
            state.acknowledge_snapshot(SESSION, B, AuthorityCursor::new(3, 99)),
            Err(ProtocolError::WrongSnapshotCursor)
        );
        state
            .acknowledge_snapshot(SESSION, B, state.cursor())
            .unwrap();
        assert_eq!(
            state.host_changed(PlayerId(3)),
            Err(ProtocolError::WrongHost)
        );
        state.host_changed(B).unwrap();
        assert_eq!(
            state.choose_recovery_source([]),
            Err(ProtocolError::WrongSnapshotCursor)
        );
        assert_eq!(
            state.complete_migration(SESSION, AuthorityCursor::new(3, 99)),
            Err(ProtocolError::WrongSnapshotCursor)
        );
        assert_eq!(
            state.complete_migration(SESSION, AuthorityCursor::new(3, 100)),
            Ok(AuthorityCursor::new(4, 0))
        );
        assert_eq!(state.host(), B);
        assert_eq!(state.session_definition(), SESSION_DEFINITION);
        assert_eq!(state.image_hash(), SESSION_DEFINITION.image_hash);
        assert_eq!(
            state.accept_command(&command(A, 0)),
            Err(ProtocolError::WrongEpoch)
        );
        let mut fresh_move = move_command(A, 0, 0);
        fresh_move.authority_epoch = AuthorityEpoch(4);
        assert_eq!(
            state.accept_command(&fresh_move),
            Err(ProtocolError::ControlNotProcessed { required: 0 })
        );
        let mut envelope = command(A, 0);
        envelope.authority_epoch = AuthorityEpoch(4);
        assert_eq!(
            state.accept_command(&envelope),
            Ok(CommandSequenceStatus::InOrder)
        );
        assert_eq!(
            state.accept_command(&fresh_move),
            Ok(CommandSequenceStatus::InOrder)
        );
        assert_eq!(state.advance_authority(), Ok(AuthorityCursor::new(4, 1)));
    }

    #[test]
    fn abrupt_loss_waits_for_external_host_and_same_epoch_recovery() {
        let mut state = AuthoritySession::new(SESSION_DEFINITION, A, AuthorityCursor::new(4, 801));
        state.host_lost().unwrap();
        assert_eq!(
            state.accept_command(&command(B, 0)),
            Err(ProtocolError::Frozen)
        );
        assert_eq!(
            state.complete_migration(SESSION, state.cursor()),
            Err(ProtocolError::InvalidTransition)
        );
        assert_eq!(
            state.choose_recovery_source([]),
            Err(ProtocolError::WrongSnapshotCursor)
        );
        let candidates = [
            RecoverySource {
                session: SESSION,
                player: B,
                cursor: AuthorityCursor::new(4, 801),
            },
            RecoverySource {
                session: SESSION,
                player: PlayerId(3),
                cursor: AuthorityCursor::new(4, 805),
            },
            RecoverySource {
                session: SESSION,
                player: PlayerId(4),
                cursor: AuthorityCursor::new(4, 805),
            },
            RecoverySource {
                session: SESSION,
                player: B,
                cursor: AuthorityCursor::new(5, 0),
            },
            RecoverySource {
                session: SessionId(8),
                player: B,
                cursor: AuthorityCursor::new(4, 999),
            },
        ];
        let source = state.choose_recovery_source(candidates).unwrap();
        assert_eq!(source.player, PlayerId(3));
        // Snapshot source and authority owner are independent.
        state.host_changed(B).unwrap();
        assert_eq!(
            state.complete_migration(SESSION, source.cursor),
            Ok(AuthorityCursor::new(5, 0))
        );
        assert_eq!(state.host(), B);
    }

    #[test]
    fn recovery_rejects_incomplete_candidates_behind_local_cursor_without_pinning_source() {
        let local = AuthorityCursor::new(4, 806);
        let mut state = AuthoritySession::new(SESSION_DEFINITION, A, local);
        state.host_lost().unwrap();
        state.host_changed(B).unwrap();
        let candidates = [
            RecoverySource {
                session: SESSION,
                player: B,
                cursor: AuthorityCursor::new(4, 801),
            },
            RecoverySource {
                session: SESSION,
                player: PlayerId(3),
                cursor: AuthorityCursor::new(4, 805),
            },
        ];
        let pending = state.migration().clone();
        assert_eq!(
            state.choose_recovery_source(candidates),
            Err(ProtocolError::WrongSnapshotCursor)
        );
        assert_eq!(state.cursor(), local);
        assert_eq!(state.host(), A);
        assert_eq!(state.migration(), &pending);
        assert_eq!(state.recovery_source(), None);
        assert!(!state.is_active());

        // A later complete response at the local cursor is still accepted.
        let current = RecoverySource {
            session: SESSION,
            player: B,
            cursor: local,
        };
        assert_eq!(state.choose_recovery_source([current]), Ok(current));
        assert_eq!(
            state.complete_migration(SESSION, local),
            Ok(AuthorityCursor::new(5, 0))
        );
    }

    #[test]
    fn completion_rechecks_local_cursor_even_when_snapshot_matches_selected_source() {
        for sequence in [805, 806, 807] {
            let local = AuthorityCursor::new(4, 806);
            let mut state = AuthoritySession::new(SESSION_DEFINITION, A, local);
            let source = RecoverySource {
                session: SESSION,
                player: B,
                cursor: AuthorityCursor::new(4, sequence),
            };
            // Seed the negotiated state directly to exercise completion independently
            // of selection, including a stale source accepted by an older implementation.
            state.migration = MigrationState::Recovering {
                new_host: Some(B),
                source: Some(source),
            };
            let pending = state.migration().clone();
            let result = state.complete_migration(SESSION, source.cursor);
            if sequence < local.sequence.0 {
                assert_eq!(result, Err(ProtocolError::WrongSnapshotCursor));
                assert_eq!(state.cursor(), local);
                assert_eq!(state.host(), A);
                assert_eq!(state.migration(), &pending);
                assert!(!state.is_active());
            } else {
                assert_eq!(result, Ok(AuthorityCursor::new(5, 0)));
                assert_eq!(state.host(), B);
                assert!(state.is_active());
            }
        }
    }

    #[test]
    fn recovery_choice_is_order_independent_and_cursor_order_is_lexicographic() {
        let candidates = [
            RecoverySource {
                session: SESSION,
                player: A,
                cursor: AuthorityCursor::new(3, u64::MAX),
            },
            RecoverySource {
                session: SESSION,
                player: B,
                cursor: AuthorityCursor::new(4, 0),
            },
            RecoverySource {
                session: SESSION,
                player: PlayerId(3),
                cursor: AuthorityCursor::new(4, 0),
            },
        ];
        assert_eq!(
            select_recovery_source(SESSION, candidates).unwrap().player,
            B
        );
        assert_eq!(
            select_recovery_source(SESSION, candidates.into_iter().rev())
                .unwrap()
                .player,
            B
        );
        assert_eq!(select_recovery_source(SessionId(8), candidates), None);
    }

    #[test]
    fn events_reject_replays_wrong_sender_epoch_and_order_without_advancing() {
        let mut state = AuthoritySession::new(SESSION_DEFINITION, A, AuthorityCursor::new(3, 100));
        let event = |session, host, epoch, sequence| AuthorityEventEnvelope {
            session,
            host,
            cursor: AuthorityCursor::new(epoch, sequence),
            event: (),
        };
        assert_eq!(
            state.record_applied_event(&event(SESSION, A, 2, 101)),
            Err(ProtocolError::WrongEpoch)
        );
        assert_eq!(
            state.record_applied_event(&event(SESSION, A, 4, 101)),
            Err(ProtocolError::WrongEpoch)
        );
        assert_eq!(
            state.record_applied_event(&event(SessionId(8), A, 3, 101)),
            Err(ProtocolError::WrongSession)
        );
        assert_eq!(
            state.record_applied_event(&event(SESSION, B, 3, 101)),
            Err(ProtocolError::WrongHost)
        );
        assert_eq!(
            state.record_applied_event(&event(SESSION, A, 3, 100)),
            Err(ProtocolError::StaleEvent)
        );
        assert_eq!(
            state.record_applied_event(&event(SESSION, A, 3, 102)),
            Err(ProtocolError::EventGap {
                expected: AuthoritySequence(101)
            })
        );
        assert_eq!(state.cursor(), AuthorityCursor::new(3, 100));
        state
            .record_applied_event(&event(SESSION, A, 3, 101))
            .unwrap();
        assert_eq!(
            state.record_applied_event(&event(SESSION, A, 3, 101)),
            Err(ProtocolError::StaleEvent)
        );
        state.host_lost().unwrap();
        assert_eq!(
            state.validate_event(&event(SESSION, A, 3, 102)),
            Err(ProtocolError::Frozen)
        );
    }

    #[test]
    fn counters_never_wrap_and_interrupted_graceful_transfer_can_recover() {
        let mut state = AuthoritySession::new(
            SESSION_DEFINITION,
            A,
            AuthorityCursor::new(u64::MAX, u64::MAX),
        );
        assert_eq!(
            state.advance_authority(),
            Err(ProtocolError::CounterExhausted)
        );
        state.begin_graceful(B).unwrap();
        state.host_lost().unwrap();
        state.host_changed(B).unwrap();
        state
            .choose_recovery_source([RecoverySource {
                session: SESSION,
                player: B,
                cursor: state.cursor(),
            }])
            .unwrap();
        assert_eq!(
            state.complete_migration(SESSION, state.cursor()),
            Err(ProtocolError::CounterExhausted)
        );
        assert!(!state.is_active());
        assert_eq!(state.host(), A);
    }
}
