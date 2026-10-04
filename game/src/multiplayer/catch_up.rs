//! Bounded same-epoch join history, independent of transport and runtime schedules.
//!
//! Capture only at complete authority command boundaries. Record each already-applied
//! outcome immediately, before publishing it; send retries must reuse the publication
//! object without recording or applying gameplay again. This is not a Ready protocol.
use super::{
    protocol::HostCommandOutcome, protocol::ProtocolDragContexts, JoinBaseline, JoinBaselineError,
    MAX_BASELINE_DRAGS,
};
use crate::resources::PieceDataStore;
use jigsall_core::{
    protocol::{
        ComponentRef, PieceTarget, ProtocolAuthorityEvent, ProtocolAuthorityEventEnvelope,
        RejectedComponentRef, RemoteDragUpdate,
    },
    session::{AuthorityCursor, AuthorityEpoch, AuthoritySession, SessionId},
    PlayerId, PuzzleDefinition,
};
use std::{
    collections::{BTreeMap, VecDeque},
    mem::size_of,
    sync::Arc,
};

pub const MAX_CATCH_UP_EVENTS: usize = 4096;
pub const MAX_CATCH_UP_RETAINED_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_PENDING_JOIN_SYNCS: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CatchUpLimits {
    pub max_events: usize,
    /// Per-peer logical history budget, even though event payloads are shared.
    pub max_retained_bytes: usize,
}
impl Default for CatchUpLimits {
    fn default() -> Self {
        Self {
            max_events: MAX_CATCH_UP_EVENTS,
            max_retained_bytes: MAX_CATCH_UP_RETAINED_BYTES,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatchUpRestartReason {
    EventCountLimit,
    RetainedByteLimit,
    TooManyActiveDrags,
    InconsistentDragContext,
    MissedAuthorityEvent,
    ScopeChanged,
    AuthorityFrozen,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinCatchUpPhase {
    BaselinePending,
    CatchingUp,
    RestartRequired(CatchUpRestartReason),
}

#[derive(Debug)]
pub struct JoinCatchUpStart {
    pub generation: u64,
    pub baseline: JoinBaseline,
}

/// Read-only progress; inspecting it does not acknowledge or retire a join.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JoinCatchUpStatus {
    pub generation: u64,
    pub baseline_cursor: AuthorityCursor,
    pub acknowledged_cursor: AuthorityCursor,
    pub last_recorded_cursor: AuthorityCursor,
    pub phase: JoinCatchUpPhase,
    pub retained_events: usize,
    pub retained_bytes: usize,
    pub latest_drag_updates: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CatchUpError {
    Baseline(JoinBaselineError),
    AlreadyJoining,
    NotJoining,
    HostCannotJoin,
    TooManyPendingJoins,
    GenerationMismatch { expected: u64, received: u64 },
    GenerationExhausted,
    WrongPhase(JoinCatchUpPhase),
    RestartRequired(CatchUpRestartReason),
    WrongBaselineCursor,
    WrongSession,
    WrongHost,
    WrongEpoch,
    EventCursorMismatch,
    DuplicateEvent,
    StaleEvent,
    StaleAcknowledgement,
    FutureAcknowledgement,
    MissingDragContext,
    WrongDragContext,
    DuplicateUpdate,
    StaleUpdate,
    InvalidDelta,
}
impl std::fmt::Display for CatchUpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Join catch-up rejected: {self:?}")
    }
}
impl std::error::Error for CatchUpError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HostScope {
    session: SessionId,
    authority_epoch: AuthorityEpoch,
    host: PlayerId,
    store_epoch: u64,
}
impl HostScope {
    fn current(session: &AuthoritySession, store: &PieceDataStore) -> Self {
        Self {
            session: session.session_id(),
            authority_epoch: session.cursor().epoch,
            host: session.host(),
            store_epoch: store.epoch,
        }
    }
}

#[derive(Debug)]
struct RetainedAuthorityEvent {
    envelope: ProtocolAuthorityEventEnvelope,
    logical_bytes: usize,
}

/// Scalar validation metadata, never a second retained target or baseline update.
#[derive(Clone, Copy, Debug)]
struct DragBasis {
    grab_sequence: u64,
    basis_sequence: u64,
    last_tick: Option<u64>,
}

#[derive(Debug)]
struct PendingJoin {
    generation: u64,
    baseline_cursor: AuthorityCursor,
    acknowledged_cursor: AuthorityCursor,
    last_recorded_cursor: AuthorityCursor,
    phase: JoinCatchUpPhase,
    events: VecDeque<Arc<RetainedAuthorityEvent>>,
    retained_bytes: usize,
    latest_updates: BTreeMap<u64, RemoteDragUpdate>,
    drag_bases: BTreeMap<u64, DragBasis>,
}
impl PendingJoin {
    fn new(generation: u64, baseline: &JoinBaseline) -> Self {
        let cursor = baseline.snapshot.cursor;
        Self {
            generation,
            baseline_cursor: cursor,
            acknowledged_cursor: cursor,
            last_recorded_cursor: cursor,
            phase: JoinCatchUpPhase::BaselinePending,
            events: VecDeque::new(),
            retained_bytes: 0,
            latest_updates: BTreeMap::new(),
            drag_bases: baseline
                .active_drags
                .iter()
                .map(|drag| {
                    (
                        drag.player.0,
                        DragBasis {
                            grab_sequence: drag.grab_sequence,
                            basis_sequence: drag.basis_sequence,
                            last_tick: drag.last_tick,
                        },
                    )
                })
                .collect(),
        }
    }
    fn restart_required(&mut self, reason: CatchUpRestartReason) {
        if !matches!(self.phase, JoinCatchUpPhase::RestartRequired(_)) {
            self.phase = JoinCatchUpPhase::RestartRequired(reason);
        }
        // Drop the queue's backing allocation too, not just its event references.
        self.events = VecDeque::new();
        self.retained_bytes = 0;
        self.latest_updates.clear();
        self.drag_bases.clear();
    }
    fn require_phase(&self, phase: JoinCatchUpPhase) -> Result<(), CatchUpError> {
        match self.phase {
            JoinCatchUpPhase::RestartRequired(reason) => Err(CatchUpError::RestartRequired(reason)),
            actual if actual != phase => Err(CatchUpError::WrongPhase(actual)),
            _ => Ok(()),
        }
    }
    fn update_drag_basis(&mut self, event: &ProtocolAuthorityEvent) {
        let context = match event {
            ProtocolAuthorityEvent::DragRotationCommitted(commit) => {
                Some((commit.player, commit.grab_sequence))
            }
            ProtocolAuthorityEvent::ReleaseCommitted(commit) => {
                Some((commit.player, commit.grab_sequence))
            }
            ProtocolAuthorityEvent::DragCancelled(cancel) => {
                Some((cancel.player, cancel.grab_sequence))
            }
            _ => None,
        };
        if let Some((player, grab_sequence)) = context {
            if self
                .drag_bases
                .get(&player.0)
                .is_none_or(|basis| basis.grab_sequence != grab_sequence)
            {
                self.restart_required(CatchUpRestartReason::InconsistentDragContext);
                return;
            }
        }
        match event {
            ProtocolAuthorityEvent::GrabAccepted(ack) => {
                self.latest_updates.remove(&ack.player.0);
                if target_is_empty(&ack.accepted) {
                    self.drag_bases.remove(&ack.player.0);
                } else {
                    if !self.drag_bases.contains_key(&ack.player.0)
                        && self.drag_bases.len() == MAX_BASELINE_DRAGS
                    {
                        self.restart_required(CatchUpRestartReason::TooManyActiveDrags);
                        return;
                    }
                    self.drag_bases.insert(
                        ack.player.0,
                        DragBasis {
                            grab_sequence: ack.grab_sequence,
                            basis_sequence: ack.grab_sequence,
                            last_tick: None,
                        },
                    );
                }
            }
            ProtocolAuthorityEvent::DragRotationCommitted(commit) => {
                self.latest_updates.remove(&commit.player.0);
                let basis = self
                    .drag_bases
                    .get_mut(&commit.player.0)
                    .expect("validated lifecycle drag context");
                basis.basis_sequence = commit.basis_sequence;
                basis.last_tick = commit.through_tick;
            }
            ProtocolAuthorityEvent::ReleaseCommitted(commit) => {
                self.latest_updates.remove(&commit.player.0);
                self.drag_bases.remove(&commit.player.0);
            }
            ProtocolAuthorityEvent::DragCancelled(cancel) => {
                self.latest_updates.remove(&cancel.player.0);
                self.drag_bases.remove(&cancel.player.0);
            }
            ProtocolAuthorityEvent::RotationCommitted(_) => {}
        }
    }
}

#[derive(Default, Debug)]
pub struct JoinCatchUpCoordinator {
    limits: CatchUpLimits,
    scope: Option<HostScope>,
    observed_cursor: Option<AuthorityCursor>,
    // PlayerId has no Ord; numeric keys preserve its deterministic identity order.
    peers: BTreeMap<u64, PendingJoin>,
}

impl JoinCatchUpCoordinator {
    /// Zero limits safely require restart on the first event.
    pub fn new(limits: CatchUpLimits) -> Self {
        Self {
            limits,
            ..Self::default()
        }
    }

    /// Capture cursor, canonical pieces and active drags with one shared borrowed
    /// view at a command boundary. Retention starts before returning the baseline.
    /// A fresh capture can recover global observation after invalidating old joins.
    pub fn begin_join(
        &mut self,
        joining_player: PlayerId,
        session: &AuthoritySession,
        store: &PieceDataStore,
        contexts: &ProtocolDragContexts,
        definition: &PuzzleDefinition,
    ) -> Result<JoinCatchUpStart, CatchUpError> {
        if joining_player == session.host() {
            return Err(CatchUpError::HostCannotJoin);
        }
        if self.peers.contains_key(&joining_player.0) {
            return Err(CatchUpError::AlreadyJoining);
        }
        if self.peers.len() == MAX_PENDING_JOIN_SYNCS {
            return Err(CatchUpError::TooManyPendingJoins);
        }
        // Detect a missed hook BEFORE a new baseline moves global observation.
        let _ = self.observe_host_state(session, store);
        let baseline = JoinBaseline::capture(session, store, contexts, definition)
            .map_err(CatchUpError::Baseline)?;
        self.scope = Some(HostScope::current(session, store));
        self.observed_cursor = Some(baseline.snapshot.cursor);
        self.peers
            .insert(joining_player.0, PendingJoin::new(0, &baseline));
        Ok(JoinCatchUpStart {
            generation: 0,
            baseline,
        })
    }

    /// Only a failed join may restart. Capture failure/counter exhaustion preserves
    /// its old generation and RestartRequired state; capture never waits for drags.
    pub fn restart_join(
        &mut self,
        joining_player: PlayerId,
        session: &AuthoritySession,
        store: &PieceDataStore,
        contexts: &ProtocolDragContexts,
        definition: &PuzzleDefinition,
    ) -> Result<JoinCatchUpStart, CatchUpError> {
        if !self.peers.contains_key(&joining_player.0) {
            return Err(CatchUpError::NotJoining);
        }
        let _ = self.observe_host_state(session, store);
        if joining_player == session.host() {
            return Err(CatchUpError::HostCannotJoin);
        }
        let peer = &self.peers[&joining_player.0];
        if !matches!(peer.phase, JoinCatchUpPhase::RestartRequired(_)) {
            return Err(CatchUpError::WrongPhase(peer.phase));
        }
        let generation = peer
            .generation
            .checked_add(1)
            .ok_or(CatchUpError::GenerationExhausted)?;
        let baseline = JoinBaseline::capture(session, store, contexts, definition)
            .map_err(CatchUpError::Baseline)?;
        self.scope = Some(HostScope::current(session, store));
        self.observed_cursor = Some(baseline.snapshot.cursor);
        self.peers
            .insert(joining_player.0, PendingJoin::new(generation, &baseline));
        Ok(JoinCatchUpStart {
            generation,
            baseline,
        })
    }

    pub fn status(&self, joining_player: PlayerId) -> Result<JoinCatchUpStatus, CatchUpError> {
        let peer = self
            .peers
            .get(&joining_player.0)
            .ok_or(CatchUpError::NotJoining)?;
        Ok(JoinCatchUpStatus {
            generation: peer.generation,
            baseline_cursor: peer.baseline_cursor,
            acknowledged_cursor: peer.acknowledged_cursor,
            last_recorded_cursor: peer.last_recorded_cursor,
            phase: peer.phase,
            retained_events: peer.events.len(),
            retained_bytes: peer.retained_bytes,
            latest_drag_updates: peer.latest_updates.len(),
        })
    }

    /// Explicit lifecycle cleanup, not Ready promotion. The caller must retire old
    /// transfers before removing/reusing an authenticated PlayerId's join lifetime.
    pub fn remove_join(
        &mut self,
        joining_player: PlayerId,
        generation: u64,
    ) -> Result<(), CatchUpError> {
        self.peer(joining_player, generation)?;
        self.peers.remove(&joining_player.0);
        if self.peers.is_empty() {
            self.scope = None;
            self.observed_cursor = None;
        }
        Ok(())
    }

    /// Call on migration/restore boundaries too. Invalidates every pending join on
    /// a scope change, freeze, cursor gap or rollback. Does not capture a snapshot.
    pub fn observe_host_state(
        &mut self,
        session: &AuthoritySession,
        store: &PieceDataStore,
    ) -> Result<(), CatchUpError> {
        if self.peers.is_empty() {
            return Ok(());
        }
        self.require_scope(session, store)?;
        if self.observed_cursor != Some(session.cursor()) {
            return Err(self.invalidate_all(CatchUpRestartReason::MissedAuthorityEvent));
        }
        Ok(())
    }

    /// Record exactly once immediately after apply_replicated/cancel_replicated.
    /// Already-applied semantic events only: no gameplay, encoding or network calls.
    /// Duplicate/stale cursors reject without mutation, including publication retries.
    pub fn record_authority_event(
        &mut self,
        session: &AuthoritySession,
        store: &PieceDataStore,
        event: &ProtocolAuthorityEventEnvelope,
    ) -> Result<(), CatchUpError> {
        if self.peers.is_empty() {
            return Ok(());
        }
        self.require_scope(session, store)?;
        if event.session != session.session_id() {
            return Err(CatchUpError::WrongSession);
        }
        if event.host != session.host() {
            return Err(CatchUpError::WrongHost);
        }
        if event.cursor.epoch != session.cursor().epoch {
            return Err(CatchUpError::WrongEpoch);
        }
        let observed = self
            .observed_cursor
            .expect("pending joins bind an observed cursor");
        if event.cursor == observed {
            return Err(CatchUpError::DuplicateEvent);
        }
        if event.cursor < observed {
            return Err(CatchUpError::StaleEvent);
        }
        if event.cursor != session.cursor() {
            self.invalidate_all(CatchUpRestartReason::MissedAuthorityEvent);
            return Err(CatchUpError::EventCursorMismatch);
        }
        if observed.sequence.0.checked_add(1) != Some(event.cursor.sequence.0) {
            return Err(self.invalidate_all(CatchUpRestartReason::MissedAuthorityEvent));
        }
        self.observed_cursor = Some(event.cursor);
        // Evaluate dynamic lengths once, without serialization or target traversal.
        let logical_bytes = logical_retained_bytes(event);
        let mut shared = None;
        for peer in self.peers.values_mut() {
            if matches!(peer.phase, JoinCatchUpPhase::RestartRequired(_)) {
                continue;
            }
            peer.last_recorded_cursor = event.cursor;
            if peer.events.len() >= self.limits.max_events {
                peer.restart_required(CatchUpRestartReason::EventCountLimit);
            } else if logical_bytes
                > self
                    .limits
                    .max_retained_bytes
                    .saturating_sub(peer.retained_bytes)
            {
                peer.restart_required(CatchUpRestartReason::RetainedByteLimit);
            } else {
                peer.update_drag_basis(&event.event);
                if matches!(peer.phase, JoinCatchUpPhase::RestartRequired(_)) {
                    continue;
                }
                let retained = shared.get_or_insert_with(|| {
                    Arc::new(RetainedAuthorityEvent {
                        envelope: event.clone(),
                        logical_bytes,
                    })
                });
                peer.events.push_back(Arc::clone(retained));
                peer.retained_bytes = peer.retained_bytes.saturating_add(logical_bytes);
            }
        }
        Ok(())
    }

    /// Forward the already-produced publication objects. No outcomes means no-op;
    /// normal gameplay with zero joins returns before even examining an outcome.
    pub fn record_command_outcome(
        &mut self,
        session: &AuthoritySession,
        store: &PieceDataStore,
        outcome: &HostCommandOutcome,
    ) -> Result<(), CatchUpError> {
        if self.peers.is_empty() {
            return Ok(());
        }
        if let Some(event) = &outcome.authority_event {
            self.record_authority_event(session, store, event)?;
        }
        if let Some(update) = &outcome.drag_update {
            self.record_drag_update(session, store, update)?;
        }
        Ok(())
    }

    /// Keep only post-capture absolute presentation, never transient history.
    /// Basis/tick checks include the baseline and survive ACK pruning/rebase cleanup.
    pub fn record_drag_update(
        &mut self,
        session: &AuthoritySession,
        store: &PieceDataStore,
        update: &RemoteDragUpdate,
    ) -> Result<(), CatchUpError> {
        if self.peers.is_empty() {
            return Ok(());
        }
        self.observe_host_state(session, store)?;
        if update.session != session.session_id() {
            return Err(CatchUpError::WrongSession);
        }
        if update.authority_epoch != session.cursor().epoch {
            return Err(CatchUpError::WrongEpoch);
        }
        if !update.delta.is_finite() {
            return Err(CatchUpError::InvalidDelta);
        }
        // Preflight all live peers, so a rejected update cannot partly replace latest.
        for peer in self.peers.values() {
            if matches!(peer.phase, JoinCatchUpPhase::RestartRequired(_)) {
                continue;
            }
            let basis = peer
                .drag_bases
                .get(&update.player.0)
                .ok_or(CatchUpError::MissingDragContext)?;
            if basis.grab_sequence != update.grab_sequence
                || basis.basis_sequence != update.basis_sequence
            {
                return Err(CatchUpError::WrongDragContext);
            }
            if let Some(tick) = basis.last_tick {
                if update.tick == tick {
                    return Err(CatchUpError::DuplicateUpdate);
                }
                if update.tick < tick {
                    return Err(CatchUpError::StaleUpdate);
                }
            }
        }
        for peer in self.peers.values_mut() {
            if matches!(peer.phase, JoinCatchUpPhase::RestartRequired(_)) {
                continue;
            }
            if !peer.latest_updates.contains_key(&update.player.0)
                && peer.latest_updates.len() >= MAX_BASELINE_DRAGS
            {
                peer.restart_required(CatchUpRestartReason::TooManyActiveDrags);
                continue;
            }
            peer.drag_bases
                .get_mut(&update.player.0)
                .expect("preflighted basis")
                .last_tick = Some(update.tick);
            peer.latest_updates.insert(update.player.0, update.clone());
        }
        Ok(())
    }

    pub fn mark_baseline_installed(
        &mut self,
        joining_player: PlayerId,
        generation: u64,
        cursor: AuthorityCursor,
    ) -> Result<(), CatchUpError> {
        let peer = self.peer_mut(joining_player, generation)?;
        peer.require_phase(JoinCatchUpPhase::BaselinePending)?;
        if cursor != peer.baseline_cursor {
            return Err(CatchUpError::WrongBaselineCursor);
        }
        peer.acknowledged_cursor = cursor;
        peer.phase = JoinCatchUpPhase::CatchingUp;
        Ok(())
    }

    /// Retryable ascending contiguous batch, starting at ACK + 1. Reading never
    /// pops history. Dense masks share existing COW words in the returned clones.
    pub fn pending_events(
        &self,
        joining_player: PlayerId,
        generation: u64,
        max_events: usize,
    ) -> Result<Vec<ProtocolAuthorityEventEnvelope>, CatchUpError> {
        let peer = self.peer(joining_player, generation)?;
        peer.require_phase(JoinCatchUpPhase::CatchingUp)?;
        Ok(peer
            .events
            .iter()
            .take(max_events)
            .map(|retained| retained.envelope.clone())
            .collect())
    }

    /// Same-epoch cumulative ACK of applied events, not a send notification.
    /// Duplicate ACK is idempotent; rollback and unretained future cursors reject.
    pub fn acknowledge_through(
        &mut self,
        joining_player: PlayerId,
        generation: u64,
        cursor: AuthorityCursor,
    ) -> Result<(), CatchUpError> {
        let peer = self.peer_mut(joining_player, generation)?;
        peer.require_phase(JoinCatchUpPhase::CatchingUp)?;
        if cursor.epoch != peer.baseline_cursor.epoch {
            return Err(CatchUpError::WrongEpoch);
        }
        if cursor < peer.acknowledged_cursor {
            return Err(CatchUpError::StaleAcknowledgement);
        }
        if cursor > peer.last_recorded_cursor {
            return Err(CatchUpError::FutureAcknowledgement);
        }
        while peer
            .events
            .front()
            .is_some_and(|event| event.envelope.cursor <= cursor)
        {
            let event = peer.events.pop_front().expect("checked front");
            peer.retained_bytes -= event.logical_bytes;
        }
        peer.acknowledged_cursor = cursor;
        Ok(())
    }

    /// Deterministic PlayerId order. Send/apply after the corresponding reliable
    /// basis; this retrieval is not a final refresh ACK or Ready handoff.
    pub fn latest_drag_updates(
        &self,
        joining_player: PlayerId,
        generation: u64,
    ) -> Result<Vec<RemoteDragUpdate>, CatchUpError> {
        let peer = self.peer(joining_player, generation)?;
        peer.require_phase(JoinCatchUpPhase::CatchingUp)?;
        Ok(peer.latest_updates.values().cloned().collect())
    }

    /// Detect unrecorded gameplay even with an empty queue. True means reliable
    /// catch-up only; caller still owns drag refresh, final ACK and Ready promotion.
    pub fn is_reliable_caught_up(
        &mut self,
        joining_player: PlayerId,
        generation: u64,
        session: &AuthoritySession,
        store: &PieceDataStore,
    ) -> Result<bool, CatchUpError> {
        self.peer(joining_player, generation)?;
        self.observe_host_state(session, store)?;
        let peer = self.peer(joining_player, generation)?;
        if let JoinCatchUpPhase::RestartRequired(reason) = peer.phase {
            return Err(CatchUpError::RestartRequired(reason));
        }
        Ok(peer.phase == JoinCatchUpPhase::CatchingUp
            && peer.acknowledged_cursor == session.cursor()
            && peer.events.is_empty()
            && self.observed_cursor == Some(session.cursor()))
    }

    fn peer(&self, player: PlayerId, generation: u64) -> Result<&PendingJoin, CatchUpError> {
        let peer = self.peers.get(&player.0).ok_or(CatchUpError::NotJoining)?;
        if peer.generation != generation {
            return Err(CatchUpError::GenerationMismatch {
                expected: peer.generation,
                received: generation,
            });
        }
        Ok(peer)
    }
    fn peer_mut(
        &mut self,
        player: PlayerId,
        generation: u64,
    ) -> Result<&mut PendingJoin, CatchUpError> {
        self.peer(player, generation)?;
        Ok(self.peers.get_mut(&player.0).expect("checked peer"))
    }
    fn invalidate_all(&mut self, reason: CatchUpRestartReason) -> CatchUpError {
        for peer in self.peers.values_mut() {
            peer.restart_required(reason);
        }
        CatchUpError::RestartRequired(reason)
    }
    fn require_scope(
        &mut self,
        session: &AuthoritySession,
        store: &PieceDataStore,
    ) -> Result<(), CatchUpError> {
        if self.scope != Some(HostScope::current(session, store)) {
            return Err(self.invalidate_all(CatchUpRestartReason::ScopeChanged));
        }
        if !session.is_active() {
            return Err(self.invalidate_all(CatchUpRestartReason::AuthorityFrozen));
        }
        Ok(())
    }
}

fn target_is_empty(target: &PieceTarget) -> bool {
    match target {
        PieceTarget::Component(_) => false,
        PieceTarget::Components(refs) => refs.is_empty(),
        PieceTarget::Dense(dense) => dense.members.is_empty(),
    }
}

/// Logical live storage, not serialized wire length or allocator overhead.
/// All dynamic lengths are O(1); arithmetic saturates instead of wrapping.
fn logical_retained_bytes(event: &ProtocolAuthorityEventEnvelope) -> usize {
    let dynamic = match &event.event {
        ProtocolAuthorityEvent::GrabAccepted(ack) => target_dynamic_bytes(&ack.accepted)
            .saturating_add(
                ack.rejected
                    .len()
                    .saturating_mul(size_of::<RejectedComponentRef>()),
            ),
        ProtocolAuthorityEvent::RotationCommitted(commit) => target_dynamic_bytes(&commit.accepted),
        ProtocolAuthorityEvent::ReleaseCommitted(_)
        | ProtocolAuthorityEvent::DragRotationCommitted(_)
        | ProtocolAuthorityEvent::DragCancelled(_) => 0,
    };
    size_of::<ProtocolAuthorityEventEnvelope>().saturating_add(dynamic)
}

fn target_dynamic_bytes(target: &PieceTarget) -> usize {
    match target {
        PieceTarget::Component(_) => 0,
        PieceTarget::Components(refs) => refs.len().saturating_mul(size_of::<ComponentRef>()),
        PieceTarget::Dense(dense) => dense.members.words().len().saturating_mul(size_of::<u32>()),
    }
}

#[cfg(test)]
#[path = "catch_up_tests.rs"]
mod tests;
