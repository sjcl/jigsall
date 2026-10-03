//! Opt-in runtime foundation for authenticated host-to-client joins.
//! A connection owns one offered transfer; no completed-payload queue exists.
//! Reliable completion enters Finalizing. Only the authoritative drag-set
//! reconciliation/barrier may issue a Ready permit.
use super::session::SessionConnections;
use super::{
    bootstrap::{BootstrapError, ClientBootstrap, ConnectionState, HostBootstrap},
    bulk::*,
    secure::SecureTransport,
    session_control::SessionMetadata,
    sync_control::{SyncControlMessage as Control, SyncFinalization, SyncTransferBinding},
    transport::{
        ConnectionId, DisconnectReason, MessageClass, Transport, TransportError, TransportEvent,
    },
    wire::{self, WireError, WireMessage},
};
use crate::{
    multiplayer::{
        catch_up::{
            CatchUpError, JoinCatchUpCoordinator, JoinCatchUpPhase, MAX_PENDING_JOIN_SYNCS,
        },
        finalization::{FinalDragError, FinalDragSet},
        protocol::{HostCommandOutcome, ProtocolDragContexts},
        replication::{PeerReplicationState, ReplicationError},
        JoinBaseline, JoinBaselineError, SnapshotExpectation,
    },
    resources::PieceDataStore,
};
use puzzella_core::{
    protocol::ProtocolAuthorityEventEnvelope,
    session::{AuthorityCursor, AuthoritySession},
    PlayerId, PuzzleDefinition,
};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
    time::{Duration, Instant},
};

/// Authority-wide capture/transfer cap, separate from the 64 joining connections.
pub const MAX_CONCURRENT_BASELINE_TRANSFERS: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncPhase {
    ImageNegotiation,
    ImageTransfer,
    AwaitingImageReady,
    /// Host-only FIFO wait: verified image, no baseline payload or new retention.
    AwaitingBaselineSlot,
    BaselineTransfer,
    CatchingUp,
    /// Reliable stream is current; final full-set reconciliation/ACK is pending.
    Finalizing,
    Ready,
    RestartRequired,
}

#[derive(Clone, Copy, Debug)]
pub struct SyncTiming {
    pub started: Instant,
    pub last_progress: Instant,
    pub transfer_progress: Option<Instant>,
}
impl SyncTiming {
    fn new(now: Instant) -> Self {
        Self {
            started: now,
            last_progress: now,
            transfer_progress: None,
        }
    }
    pub fn timed_out(self, now: Instant, syncing: Duration, transfer: Duration) -> bool {
        now.saturating_duration_since(self.started) >= syncing
            || self
                .transfer_progress
                .is_some_and(|last| now.saturating_duration_since(last) >= transfer)
    }
}

#[derive(Debug, PartialEq)]
pub enum SyncError {
    Bootstrap(BootstrapError),
    NotSyncing,
    TooManyJoins,
    WrongConnection,
    WrongDirection,
    WrongPhase,
    WrongIdentity,
    WrongGeneration,
    WrongFinalization,
    FinalizationExhausted,
    FinalDrag(FinalDragError),
    UnofferedTransfer,
    TransferMismatch,
    ImageHashMismatch,
    MalformedBaseline,
    Baseline(JoinBaselineError),
    CatchUp(CatchUpError),
    Replication(ReplicationError),
    Bulk(BulkTransferError),
    Wire(WireError),
    Transport(TransportError),
}

/// Private construction: only a validated final barrier or its host commit can
/// certify readiness. Bound to the original authenticated connection identity.
pub struct SyncReadyPermit {
    connection: ConnectionId,
    metadata: SessionMetadata,
    player: PlayerId,
}
impl SyncReadyPermit {
    pub(crate) fn matches(
        &self,
        connection: ConnectionId,
        metadata: SessionMetadata,
        player: PlayerId,
    ) -> bool {
        self.connection == connection && self.metadata == metadata && self.player == player
    }
    #[cfg(test)]
    pub(crate) fn fixture(
        connection: ConnectionId,
        metadata: SessionMetadata,
        player: PlayerId,
    ) -> Self {
        Self {
            connection,
            metadata,
            player,
        }
    }
}

/// Borrow the authority at complete command boundaries, including capture.
pub struct SyncAuthority<'a> {
    pub session: &'a AuthoritySession,
    pub store: &'a PieceDataStore,
    pub contexts: &'a ProtocolDragContexts,
    pub definition: &'a PuzzleDefinition,
}

/// Mutably borrow both routing gates throughout final validation/commit.
pub struct SyncHost<'a> {
    pub bootstrap: &'a mut HostBootstrap,
    pub connections: &'a mut SessionConnections,
}
impl SyncAuthority<'_> {
    fn metadata(&self) -> SessionMetadata {
        SessionMetadata {
            definition: self.session.session_definition(),
            cursor: self.session.cursor(),
            host: self.session.host(),
        }
    }
}

struct SendingTransfer {
    binding: SyncTransferBinding,
    start: Option<BulkTransferMessage>,
    outbound: OutboundBulkTransfer,
    accepted: bool,
}
impl SendingTransfer {
    fn new(
        sender: &mut BulkTransferSender,
        kind: BulkTransferKind,
        bytes: Arc<[u8]>,
    ) -> Result<Self, SyncError> {
        let mut outbound = sender.begin(kind, bytes).map_err(SyncError::Bulk)?;
        let start = outbound
            .next_message()
            .map_err(SyncError::Bulk)?
            .expect("new sender starts");
        let BulkTransferMessage::Start {
            transfer_id,
            kind,
            total_size,
            sha256,
        } = start
        else {
            unreachable!()
        };
        Ok(Self {
            binding: SyncTransferBinding {
                transfer_id,
                kind,
                total_size,
                sha256,
            },
            start: Some(start),
            outbound,
            accepted: false,
        })
    }
}

struct HostSyncPeer {
    player: PlayerId,
    authenticated: SessionMetadata,
    phase: SyncPhase,
    image_ready: bool,
    generation: Option<u64>,
    baseline_cursor: Option<AuthorityCursor>,
    binding: Option<SyncTransferBinding>,
    sender: BulkTransferSender,
    sending: Option<SendingTransfer>,
    transfer_finished: bool,
    sent_cursor: Option<AuthorityCursor>,
    obsolete: Option<(u64, Option<TransferId>)>,
    final_revision: u64,
    candidate: Option<FinalizationCandidate>,
    timing: SyncTiming,
}

struct FinalizationCandidate {
    token: SyncFinalization,
    drags: FinalDragSet,
    store_epoch: u64,
}

/// One coordinator for the authority, with state only for joining connections.
/// Keep this alongside bootstrap; remove a connection here on every disconnect.
#[derive(Default)]
pub struct HostSyncCoordinator {
    catch_up: JoinCatchUpCoordinator,
    peers: BTreeMap<ConnectionId, HostSyncPeer>,
    baseline_waiters: VecDeque<ConnectionId>,
}
impl HostSyncCoordinator {
    pub fn new(catch_up: JoinCatchUpCoordinator) -> Self {
        Self {
            catch_up,
            peers: BTreeMap::new(),
            baseline_waiters: VecDeque::new(),
        }
    }
    pub fn catch_up(&self) -> &JoinCatchUpCoordinator {
        &self.catch_up
    }
    pub fn transfer_binding(&self, connection: ConnectionId) -> Option<SyncTransferBinding> {
        self.peers.get(&connection).and_then(|p| p.binding)
    }
    pub fn phase(&self, connection: ConnectionId) -> Option<SyncPhase> {
        self.peers.get(&connection).map(|p| p.phase)
    }
    pub fn timing(&self, connection: ConnectionId) -> Option<SyncTiming> {
        self.peers.get(&connection).map(|p| p.timing)
    }
    /// Slots remain occupied through the baseline installation ACK, even after
    /// Finish has released the host's serialized payload.
    pub fn active_baseline_transfers(&self) -> usize {
        self.peers
            .values()
            .filter(|peer| peer.phase == SyncPhase::BaselineTransfer)
            .count()
    }

    pub fn start<T: Transport>(
        &mut self,
        bootstrap: &mut HostBootstrap,
        connection: ConnectionId,
        transport: &mut SecureTransport<T>,
        authority: &SyncAuthority<'_>,
        now: Instant,
    ) -> Result<(), SyncError> {
        if self.peers.len() >= MAX_PENDING_JOIN_SYNCS {
            return Err(SyncError::TooManyJoins);
        }
        if self.peers.contains_key(&connection) || !transport.has_channel(connection) {
            return Err(SyncError::NotSyncing);
        }
        let authenticated = bootstrap
            .metadata(connection)
            .ok_or(SyncError::NotSyncing)?;
        require_identity(authenticated, authority.metadata())?;
        if !authority.session.is_active() {
            return Err(SyncError::WrongPhase);
        }
        let player = bootstrap
            .assigned_player(connection)
            .ok_or(SyncError::NotSyncing)?;
        bootstrap
            .begin_sync(connection)
            .map_err(SyncError::Bootstrap)?;
        self.peers.insert(
            connection,
            HostSyncPeer {
                player,
                authenticated,
                phase: SyncPhase::ImageNegotiation,
                image_ready: false,
                generation: None,
                baseline_cursor: None,
                binding: None,
                sender: BulkTransferSender::default(),
                sending: None,
                transfer_finished: false,
                sent_cursor: None,
                obsolete: None,
                final_revision: 0,
                candidate: None,
                timing: SyncTiming::new(now),
            },
        );
        send(
            transport,
            connection,
            Control::Session {
                metadata: authority.metadata(),
                definition: authority.definition.clone(),
            },
        )
    }

    /// Runtime apply -> record -> normal Ready publication. A failed catch-up hook
    /// must never hide the already-applied outcome or stop Ready publication.
    pub fn record_command_outcome(
        &mut self,
        session: &AuthoritySession,
        store: &PieceDataStore,
        outcome: &HostCommandOutcome,
    ) -> Result<(), CatchUpError> {
        let result = self
            .catch_up
            .record_command_outcome(session, store, outcome);
        self.invalidate_restarts();
        if outcome.authority_event.is_some() {
            for peer in self.peers.values_mut() {
                if peer.phase == SyncPhase::Finalizing {
                    peer.phase = SyncPhase::CatchingUp;
                    peer.candidate = None;
                }
            }
        }
        result
    }
    /// Includes Reliable DragCancelled generated by disconnect cleanup.
    pub fn record_authority_event(
        &mut self,
        session: &AuthoritySession,
        store: &PieceDataStore,
        event: &ProtocolAuthorityEventEnvelope,
    ) -> Result<(), CatchUpError> {
        let result = self.catch_up.record_authority_event(session, store, event);
        self.invalidate_restarts();
        for peer in self.peers.values_mut() {
            if peer.phase == SyncPhase::Finalizing {
                peer.phase = SyncPhase::CatchingUp;
                peer.candidate = None;
            }
        }
        result
    }
    pub fn observe_host_state(
        &mut self,
        session: &AuthoritySession,
        store: &PieceDataStore,
    ) -> Result<(), CatchUpError> {
        let result = self.catch_up.observe_host_state(session, store);
        self.invalidate_restarts();
        result
    }
    fn invalidate_restarts(&mut self) {
        for peer in self.peers.values_mut() {
            if peer.phase == SyncPhase::RestartRequired
                || peer.phase == SyncPhase::AwaitingBaselineSlot
            {
                continue;
            }
            if let Ok(status) = self.catch_up.status(peer.player) {
                if matches!(status.phase, JoinCatchUpPhase::RestartRequired(_)) {
                    // Only acceptance proves the client has seen the Control
                    // offer. Otherwise Bulk Abort could overtake that offer.
                    // Finish removes `sending`, but was also gated on acceptance.
                    let abort_known_transfer = peer.transfer_finished
                        || peer
                            .sending
                            .as_ref()
                            .is_some_and(|sending| sending.accepted);
                    peer.obsolete = Some((
                        status.generation,
                        peer.binding
                            .take()
                            .filter(|_| abort_known_transfer)
                            .map(|b| b.transfer_id),
                    ));
                    peer.sending = None;
                    peer.transfer_finished = false;
                    peer.baseline_cursor = None;
                    peer.sent_cursor = None;
                    peer.candidate = None;
                    peer.timing.transfer_progress = None;
                    peer.phase = SyncPhase::RestartRequired;
                }
            }
        }
    }
    pub fn disconnect(&mut self, connection: ConnectionId) {
        self.baseline_waiters.retain(|&id| id != connection);
        if let Some(peer) = self.peers.remove(&connection) {
            if let Some(generation) = peer.generation {
                let _ = self.catch_up.remove_join(peer.player, generation);
            }
        }
    }

    /// Only client Reliable Control ACK/status messages are permitted. No inbound
    /// Bulk receiver exists on the host. Image bytes are caller-owned immutable
    /// session content, supplied only when an image is actually missing.
    pub fn route<T: Transport>(
        &mut self,
        host: &mut SyncHost<'_>,
        event: &TransportEvent,
        transport: &mut SecureTransport<T>,
        authority: &SyncAuthority<'_>,
        image: Option<Arc<[u8]>>,
        now: Instant,
    ) -> Result<(), SyncError> {
        let result = self.route_inner(host, event, transport, authority, image, now);
        if result.is_err() {
            if let TransportEvent::Message { connection, .. } = event {
                self.disconnect(*connection);
                let _ = host.bootstrap.reject(
                    *connection,
                    DisconnectReason::ProtocolViolation,
                    transport,
                    host.connections,
                );
            }
        }
        result
    }

    fn route_inner<T: Transport>(
        &mut self,
        host: &mut SyncHost<'_>,
        event: &TransportEvent,
        transport: &mut SecureTransport<T>,
        authority: &SyncAuthority<'_>,
        image: Option<Arc<[u8]>>,
        now: Instant,
    ) -> Result<(), SyncError> {
        let SyncHost {
            bootstrap,
            connections,
        } = host;
        let (connection, message) = sync_message(event)?;
        if bootstrap.state(connection) != Some(ConnectionState::Syncing)
            || !transport.has_channel(connection)
        {
            return Err(SyncError::NotSyncing);
        }
        let _ = self.observe_host_state(authority.session, authority.store);
        let peer = self
            .peers
            .get_mut(&connection)
            .ok_or(SyncError::NotSyncing)?;
        require_identity(peer.authenticated, authority.metadata())?;
        let WireMessage::SyncControl(control) = message else {
            return Err(SyncError::WrongDirection);
        };
        match control {
            Control::ImageAvailability {
                image_hash,
                available,
            } => {
                if peer.phase != SyncPhase::ImageNegotiation {
                    return Err(SyncError::WrongPhase);
                }
                if image_hash != peer.authenticated.definition.image_hash {
                    return Err(SyncError::ImageHashMismatch);
                }
                if available {
                    peer.phase = SyncPhase::AwaitingImageReady;
                } else {
                    let sending = SendingTransfer::new(
                        &mut peer.sender,
                        BulkTransferKind::PuzzleImage,
                        image.ok_or(SyncError::WrongPhase)?,
                    )?;
                    if sending.binding.sha256 != image_hash.0 {
                        return Err(SyncError::ImageHashMismatch);
                    }
                    let binding = sending.binding;
                    send(transport, connection, Control::ImageOffer(binding))?;
                    peer.binding = Some(binding);
                    peer.sending = Some(sending);
                    peer.phase = SyncPhase::ImageTransfer;
                    peer.timing.transfer_progress = Some(now);
                }
            }
            Control::TransferAccepted { transfer_id } => {
                if peer.phase == SyncPhase::RestartRequired
                    || (peer.phase == SyncPhase::AwaitingBaselineSlot && peer.generation.is_some())
                {
                    return Ok(());
                }
                let sending = peer.sending.as_mut().ok_or(SyncError::WrongPhase)?;
                if sending.binding.transfer_id != transfer_id {
                    if transfer_id < sending.binding.transfer_id {
                        return Ok(());
                    }
                    return Err(SyncError::UnofferedTransfer);
                }
                if sending.accepted {
                    return Err(SyncError::WrongPhase);
                }
                sending.accepted = true;
            }
            Control::ImageReady { image_hash } => {
                if image_hash != peer.authenticated.definition.image_hash {
                    return Err(SyncError::ImageHashMismatch);
                }
                if peer.phase != SyncPhase::AwaitingImageReady
                    && !(peer.phase == SyncPhase::ImageTransfer && peer.transfer_finished)
                {
                    return Err(SyncError::WrongPhase);
                }
                peer.image_ready = true;
                peer.binding = None;
                peer.transfer_finished = false;
                peer.timing.transfer_progress = None;
                peer.phase = SyncPhase::AwaitingBaselineSlot;
                self.baseline_waiters.push_back(connection);
            }
            Control::BaselineInstalled {
                generation,
                cursor,
                transfer_id,
            } => {
                if obsolete_ack(peer, generation)? {
                    return Ok(());
                }
                if peer.phase != SyncPhase::BaselineTransfer || !peer.transfer_finished {
                    return Err(SyncError::WrongPhase);
                }
                if peer.binding.map(|b| b.transfer_id) != Some(transfer_id)
                    || peer.baseline_cursor != Some(cursor)
                {
                    return Err(SyncError::TransferMismatch);
                }
                self.catch_up
                    .mark_baseline_installed(peer.player, generation, cursor)
                    .map_err(SyncError::CatchUp)?;
                peer.binding = None;
                peer.phase = SyncPhase::CatchingUp;
                peer.timing.transfer_progress = None;
            }
            Control::CatchUpAck { generation, cursor } => {
                if obsolete_ack(peer, generation)? {
                    return Ok(());
                }
                if peer.phase != SyncPhase::CatchingUp && peer.phase != SyncPhase::Finalizing {
                    return Err(SyncError::WrongPhase);
                }
                if peer
                    .sent_cursor
                    .is_none_or(|sent| cursor.epoch != sent.epoch || cursor > sent)
                {
                    return Err(SyncError::TransferMismatch);
                }
                self.catch_up
                    .acknowledge_through(peer.player, generation, cursor)
                    .map_err(SyncError::CatchUp)?;
            }
            Control::FinalizeAck { token } => {
                if obsolete_ack(peer, token.generation)? {
                    return Ok(());
                }
                // ACKs can outlive a Reliable fallback or a Transient retry.
                if token.revision < peer.final_revision
                    || (token.revision == peer.final_revision && peer.candidate.is_none())
                {
                    return Ok(());
                }
                let candidate = peer
                    .candidate
                    .as_ref()
                    .ok_or(SyncError::WrongFinalization)?;
                if peer.phase != SyncPhase::Finalizing || candidate.token != token {
                    return Err(SyncError::WrongFinalization);
                }
                if !peer.image_ready
                    || candidate.store_epoch != authority.store.epoch
                    || !authority.session.is_active()
                {
                    return Err(SyncError::WrongIdentity);
                }
                let current = self
                    .catch_up
                    .is_reliable_caught_up(
                        peer.player,
                        token.generation,
                        authority.session,
                        authority.store,
                    )
                    .map_err(SyncError::CatchUp)?;
                if !current || token.cursor != authority.session.cursor() {
                    peer.candidate = None;
                    peer.phase = SyncPhase::CatchingUp;
                    return Ok(());
                }
                let drags =
                    FinalDragSet::capture(authority.contexts, authority.session, authority.store)
                        .map_err(SyncError::FinalDrag)?;
                if drags != candidate.drags {
                    peer.candidate = None;
                    // No Reliable replay is needed. The next pump sends a fresh revision.
                    return Ok(());
                }
                let permit = SyncReadyPermit {
                    connection,
                    metadata: peer.authenticated,
                    player: peer.player,
                };
                // Register before the client can receive commit and send on another lane.
                // The public route's failure path rolls registration back if send fails.
                bootstrap
                    .promote_ready(connection, connections, &permit)
                    .map_err(SyncError::Bootstrap)?;
                send(transport, connection, Control::ReadyCommit { token })?;
                self.disconnect(connection);
                return Ok(());
            }
            _ => return Err(SyncError::WrongDirection),
        }
        peer.timing.last_progress = now;
        self.invalidate_restarts();
        self.try_start_baseline(connection, transport, authority, now)?;
        Ok(())
    }

    fn try_start_baseline<T: Transport>(
        &mut self,
        connection: ConnectionId,
        transport: &mut SecureTransport<T>,
        authority: &SyncAuthority<'_>,
        now: Instant,
    ) -> Result<(), SyncError> {
        if self.baseline_waiters.front() != Some(&connection)
            || self.active_baseline_transfers() >= MAX_CONCURRENT_BASELINE_TRANSFERS
        {
            return Ok(());
        }
        let peer = self.peers.get_mut(&connection).expect("queued sync peer");
        require_identity(peer.authenticated, authority.metadata())?;
        if !authority.session.is_active() {
            return Err(SyncError::WrongPhase);
        }
        // Reserve before capture/encoding. Even a failed send keeps its slot
        // until runtime teardown, rather than leaving uncapped retained state.
        self.baseline_waiters.pop_front();
        peer.phase = SyncPhase::BaselineTransfer;
        let start = if peer.generation.is_some() {
            self.catch_up.restart_join(
                peer.player,
                authority.session,
                authority.store,
                authority.contexts,
                authority.definition,
            )
        } else {
            // The ONLY initial begin_join call: both image and slot are ready.
            self.catch_up.begin_join(
                peer.player,
                authority.session,
                authority.store,
                authority.contexts,
                authority.definition,
            )
        }
        .map_err(SyncError::CatchUp)?;
        Self::offer_baseline(peer, connection, transport, start, now)
    }

    fn offer_baseline<T: Transport>(
        peer: &mut HostSyncPeer,
        connection: ConnectionId,
        transport: &mut SecureTransport<T>,
        start: crate::multiplayer::catch_up::JoinCatchUpStart,
        now: Instant,
    ) -> Result<(), SyncError> {
        let generation = start.generation;
        let cursor = start.baseline.snapshot.cursor;
        // Bind generation immediately, even if encoding or sending fails; cleanup
        // must still remove the coordinator's retention entry.
        peer.generation = Some(generation);
        peer.baseline_cursor = Some(cursor);
        let bytes =
            postcard::to_allocvec(&start.baseline).map_err(|_| SyncError::MalformedBaseline)?;
        let sending = SendingTransfer::new(
            &mut peer.sender,
            BulkTransferKind::JoinBaseline,
            bytes.into(),
        )?;
        let transfer = sending.binding;
        send(
            transport,
            connection,
            Control::BaselineOffer {
                generation,
                cursor,
                transfer,
            },
        )?;
        peer.binding = Some(transfer);
        peer.sending = Some(sending);
        peer.transfer_finished = false;
        peer.sent_cursor = Some(cursor);
        peer.phase = SyncPhase::BaselineTransfer;
        peer.timing.transfer_progress = Some(now);
        peer.timing.last_progress = now;
        Ok(())
    }

    fn flush_obsolete<T: Transport>(
        peer: &mut HostSyncPeer,
        connection: ConnectionId,
        transport: &mut SecureTransport<T>,
    ) -> Result<(), SyncError> {
        if let Some((generation, transfer)) = peer.obsolete {
            if let Some(transfer_id) = transfer {
                send_bulk(
                    transport,
                    connection,
                    BulkTransferMessage::Abort { transfer_id },
                )?;
            }
            send(transport, connection, Control::Restart { generation })?;
            peer.obsolete = None;
        }
        Ok(())
    }
    pub fn restart<T: Transport>(
        &mut self,
        bootstrap: &HostBootstrap,
        connection: ConnectionId,
        transport: &mut SecureTransport<T>,
        authority: &SyncAuthority<'_>,
        now: Instant,
    ) -> Result<(), SyncError> {
        let _ = self.observe_host_state(authority.session, authority.store);
        let peer = self
            .peers
            .get_mut(&connection)
            .ok_or(SyncError::NotSyncing)?;
        if bootstrap.state(connection) != Some(ConnectionState::Syncing)
            || peer.phase != SyncPhase::RestartRequired
            || !peer.image_ready
        {
            return Err(SyncError::WrongPhase);
        }
        // PAKE's immutable session/host/epoch binding needs fresh authentication if
        // it changes. A same-session store restore can reuse the verified image.
        require_identity(peer.authenticated, authority.metadata())?;
        Self::flush_obsolete(peer, connection, transport)?;
        peer.phase = SyncPhase::AwaitingBaselineSlot;
        peer.timing.last_progress = now;
        self.baseline_waiters.push_back(connection);
        self.try_start_baseline(connection, transport, authority, now)
    }

    /// Bounded frame work: at most one Bulk or catch-up event per call. A transfer
    /// cannot start until its Control offer was accepted, so cross-lane reorder
    /// never requires buffering an unoffered Start/payload.
    pub fn pump<T: Transport>(
        &mut self,
        bootstrap: &HostBootstrap,
        connection: ConnectionId,
        transport: &mut SecureTransport<T>,
        authority: &SyncAuthority<'_>,
        now: Instant,
    ) -> Result<(), SyncError> {
        if bootstrap.state(connection) != Some(ConnectionState::Syncing)
            || !transport.has_channel(connection)
        {
            return Err(SyncError::NotSyncing);
        }
        let _ = self.observe_host_state(authority.session, authority.store);
        self.try_start_baseline(connection, transport, authority, now)?;
        let peer = self
            .peers
            .get_mut(&connection)
            .ok_or(SyncError::NotSyncing)?;
        Self::flush_obsolete(peer, connection, transport)?;
        require_identity(peer.authenticated, authority.metadata())?;
        if let Some(sending) = peer.sending.as_mut() {
            if !sending.accepted {
                return Ok(());
            }
            let message = if let Some(start) = sending.start.take() {
                Some(start)
            } else {
                sending.outbound.next_message().map_err(SyncError::Bulk)?
            };
            if let Some(message) = message {
                let finished = matches!(message, BulkTransferMessage::Finish { .. });
                send_bulk(transport, connection, message)?;
                peer.timing.transfer_progress = Some(now);
                peer.timing.last_progress = now;
                if finished {
                    peer.sending = None;
                    peer.transfer_finished = true;
                }
            }
            return Ok(());
        }
        if peer.phase == SyncPhase::CatchingUp {
            let generation = peer.generation.expect("catch-up has a baseline");
            let status = self
                .catch_up
                .status(peer.player)
                .map_err(SyncError::CatchUp)?;
            // One outstanding Reliable event limits publication work and ACKs to
            // cursors actually sent. History remains owned by the CPU coordinator.
            if peer.sent_cursor == Some(status.acknowledged_cursor) {
                if let Some(event) = self
                    .catch_up
                    .pending_events(peer.player, generation, 1)
                    .map_err(SyncError::CatchUp)?
                    .pop()
                {
                    let cursor = event.cursor;
                    send(
                        transport,
                        connection,
                        Control::CatchUpEvent { generation, event },
                    )?;
                    peer.sent_cursor = Some(cursor);
                    peer.timing.last_progress = now;
                } else if self
                    .catch_up
                    .is_reliable_caught_up(
                        peer.player,
                        generation,
                        authority.session,
                        authority.store,
                    )
                    .map_err(SyncError::CatchUp)?
                {
                    send(
                        transport,
                        connection,
                        Control::ReliableComplete {
                            generation,
                            cursor: authority.session.cursor(),
                        },
                    )?;
                    peer.phase = SyncPhase::Finalizing;
                    peer.timing.last_progress = now;
                }
            }
        } else if peer.phase == SyncPhase::Finalizing && peer.candidate.is_none() {
            let generation = peer.generation.ok_or(SyncError::WrongGeneration)?;
            if !self
                .catch_up
                .is_reliable_caught_up(peer.player, generation, authority.session, authority.store)
                .map_err(SyncError::CatchUp)?
            {
                peer.phase = SyncPhase::CatchingUp;
                return Ok(());
            }
            let revision = peer
                .final_revision
                .checked_add(1)
                .ok_or(SyncError::FinalizationExhausted)?;
            let token = SyncFinalization {
                generation,
                cursor: authority.session.cursor(),
                revision,
            };
            let drags =
                FinalDragSet::capture(authority.contexts, authority.session, authority.store)
                    .map_err(SyncError::FinalDrag)?;
            send(
                transport,
                connection,
                Control::Finalize {
                    token,
                    drags: drags.clone(),
                },
            )?;
            peer.final_revision = revision;
            peer.candidate = Some(FinalizationCandidate {
                token,
                drags,
                store_epoch: authority.store.epoch,
            });
            peer.timing.last_progress = now;
        }
        Ok(())
    }
}

fn obsolete_ack(peer: &HostSyncPeer, generation: u64) -> Result<bool, SyncError> {
    let current = peer.generation.ok_or(SyncError::WrongPhase)?;
    if generation > current {
        return Err(SyncError::WrongGeneration);
    }
    Ok(generation < current
        || peer.phase == SyncPhase::RestartRequired
        || peer.phase == SyncPhase::AwaitingBaselineSlot)
}

#[derive(Clone, Copy)]
enum ReceivingPurpose {
    Image,
    Baseline {
        generation: u64,
        cursor: AuthorityCursor,
    },
}
struct ReceivingTransfer {
    binding: SyncTransferBinding,
    purpose: ReceivingPurpose,
    started: bool,
}

pub enum ClientSyncOutcome {
    Consumed,
    Obsolete,
    /// Caller-owned verified bytes, handed off immediately; never queued here.
    ImageReady(CompletedBulkTransfer),
    BaselineInstalled,
    CatchUpApplied,
    FinalizationPending,
    Finalized,
    Ready,
}

pub struct ClientSyncRouter {
    connection: ConnectionId,
    player: PlayerId,
    authenticated: SessionMetadata,
    definition: Option<PuzzleDefinition>,
    phase: SyncPhase,
    image_ready: bool,
    generation: Option<u64>,
    baseline_cursor: Option<AuthorityCursor>,
    store_epoch: Option<u64>,
    last_offered_id: Option<TransferId>,
    receiving: Option<ReceivingTransfer>,
    bulk: BulkTransferReceiver,
    final_revision: u64,
    candidate: Option<FinalizationCandidate>,
    timing: SyncTiming,
}
/// The existing replica/store/session are borrowed only while routing sync work.
pub struct SyncReplica<'a> {
    pub replica: &'a mut PeerReplicationState,
    pub session: &'a mut AuthoritySession,
    pub store: &'a mut PieceDataStore,
}
impl ClientSyncRouter {
    /// Cache possession is established by hashing caller-owned bytes against the
    /// PAKE-authenticated SessionDefinition, never by trusting a cache filename.
    pub fn start(
        bootstrap: &mut ClientBootstrap,
        cached_image: Option<&[u8]>,
        now: Instant,
    ) -> Result<Self, SyncError> {
        let authenticated = bootstrap.metadata().ok_or(SyncError::NotSyncing)?;
        let player = bootstrap.assigned_player().ok_or(SyncError::NotSyncing)?;
        let image_ready = cached_image.is_some_and(|bytes| {
            <[u8; 32]>::from(Sha256::digest(bytes)) == authenticated.definition.image_hash.0
        });
        bootstrap.begin_sync().map_err(SyncError::Bootstrap)?;
        Ok(Self {
            connection: bootstrap.host_connection(),
            player,
            authenticated,
            definition: None,
            phase: SyncPhase::ImageNegotiation,
            image_ready,
            generation: None,
            baseline_cursor: None,
            store_epoch: None,
            last_offered_id: None,
            receiving: None,
            bulk: BulkTransferReceiver::default(),
            final_revision: 0,
            candidate: None,
            timing: SyncTiming::new(now),
        })
    }
    pub fn phase(&self) -> SyncPhase {
        self.phase
    }
    /// Negotiated definition, available before baseline installation and Ready.
    pub fn definition(&self) -> Option<&PuzzleDefinition> {
        self.definition.as_ref()
    }
    pub fn timing(&self) -> SyncTiming {
        self.timing
    }
    pub fn generation(&self) -> Option<u64> {
        self.generation
    }
    pub fn baseline_cursor(&self) -> Option<AuthorityCursor> {
        self.baseline_cursor
    }
    pub fn image_ready(&self) -> bool {
        self.image_ready
    }
    pub fn declared_in_flight_bytes(&self) -> u64 {
        self.bulk.declared_in_flight_bytes()
    }

    pub fn route<T: Transport>(
        &mut self,
        bootstrap: &mut ClientBootstrap,
        connections: &mut SessionConnections,
        event: &TransportEvent,
        transport: &mut SecureTransport<T>,
        state: &mut SyncReplica<'_>,
        now: Instant,
    ) -> Result<ClientSyncOutcome, SyncError> {
        let result = self.route_inner(bootstrap, connections, event, transport, state, now);
        if result.is_err() {
            self.invalidate();
            let _ = bootstrap.reject(DisconnectReason::ProtocolViolation, transport, connections);
        }
        result
    }

    fn route_inner<T: Transport>(
        &mut self,
        bootstrap: &mut ClientBootstrap,
        connections: &mut SessionConnections,
        event: &TransportEvent,
        transport: &mut SecureTransport<T>,
        state: &mut SyncReplica<'_>,
        now: Instant,
    ) -> Result<ClientSyncOutcome, SyncError> {
        let (connection, message) = sync_message(event)?;
        if connection != self.connection {
            return Err(SyncError::WrongConnection);
        }
        if bootstrap.state() != Some(ConnectionState::Syncing) || !transport.has_channel(connection)
        {
            return Err(SyncError::NotSyncing);
        }
        let SyncReplica {
            replica,
            session,
            store,
        } = state;
        let identity = require_identity(
            self.authenticated,
            SessionMetadata {
                definition: session.session_definition(),
                cursor: session.cursor(),
                host: session.host(),
            },
        );
        if identity.is_err()
            || !session.is_active()
            || self.store_epoch.is_some_and(|epoch| epoch != store.epoch)
        {
            self.invalidate();
            return Err(identity.err().unwrap_or(SyncError::WrongPhase));
        }
        let outcome = match message {
            WireMessage::SyncControl(control) => match control {
                Control::Session {
                    metadata,
                    definition,
                } => {
                    if self.phase != SyncPhase::ImageNegotiation || self.definition.is_some() {
                        return Err(SyncError::WrongPhase);
                    }
                    require_identity(self.authenticated, metadata)?;
                    if definition.validate().is_err() {
                        return Err(SyncError::WrongIdentity);
                    }
                    self.definition = Some(definition);
                    send(
                        transport,
                        connection,
                        Control::ImageAvailability {
                            image_hash: self.authenticated.definition.image_hash,
                            available: self.image_ready,
                        },
                    )?;
                    if self.image_ready {
                        send(
                            transport,
                            connection,
                            Control::ImageReady {
                                image_hash: self.authenticated.definition.image_hash,
                            },
                        )?;
                        self.phase = SyncPhase::AwaitingImageReady;
                    }
                    ClientSyncOutcome::Consumed
                }
                Control::ImageOffer(binding) => {
                    if self.phase != SyncPhase::ImageNegotiation
                        || self.definition.is_none()
                        || self.image_ready
                    {
                        return Err(SyncError::WrongPhase);
                    }
                    if binding.kind != BulkTransferKind::PuzzleImage
                        || binding.sha256 != self.authenticated.definition.image_hash.0
                    {
                        return Err(SyncError::ImageHashMismatch);
                    }
                    self.accept_offer(binding, ReceivingPurpose::Image, now)?;
                    send(
                        transport,
                        connection,
                        Control::TransferAccepted {
                            transfer_id: binding.transfer_id,
                        },
                    )?;
                    self.phase = SyncPhase::ImageTransfer;
                    ClientSyncOutcome::Consumed
                }
                Control::BaselineOffer {
                    generation,
                    cursor,
                    transfer,
                } => {
                    if !self.image_ready
                        || self.definition.is_none()
                        || (self.phase != SyncPhase::AwaitingImageReady
                            && self.phase != SyncPhase::RestartRequired)
                    {
                        return Err(SyncError::WrongPhase);
                    }
                    if self
                        .generation
                        .is_some_and(|old| old.checked_add(1) != Some(generation))
                        || (self.generation.is_none() && generation != 0)
                    {
                        return Err(SyncError::WrongGeneration);
                    }
                    if cursor.epoch != self.authenticated.cursor.epoch
                        || cursor < session.cursor()
                        || transfer.kind != BulkTransferKind::JoinBaseline
                    {
                        return Err(SyncError::TransferMismatch);
                    }
                    self.accept_offer(
                        transfer,
                        ReceivingPurpose::Baseline { generation, cursor },
                        now,
                    )?;
                    self.generation = Some(generation);
                    self.baseline_cursor = Some(cursor);
                    self.store_epoch = Some(store.epoch);
                    self.phase = SyncPhase::BaselineTransfer;
                    send(
                        transport,
                        connection,
                        Control::TransferAccepted {
                            transfer_id: transfer.transfer_id,
                        },
                    )?;
                    ClientSyncOutcome::Consumed
                }
                Control::Restart { generation } => {
                    if self.stale_generation(generation)? {
                        return Ok(ClientSyncOutcome::Obsolete);
                    }
                    self.invalidate();
                    ClientSyncOutcome::Consumed
                }
                Control::CatchUpEvent { generation, event } => {
                    if self.stale_generation(generation)?
                        || self.phase == SyncPhase::RestartRequired
                    {
                        return Ok(ClientSyncOutcome::Obsolete);
                    }
                    if self.phase != SyncPhase::CatchingUp && self.phase != SyncPhase::Finalizing {
                        return Err(SyncError::WrongPhase);
                    }
                    replica
                        .apply_event(
                            session,
                            store,
                            self.authenticated.host,
                            &event,
                            self.definition.as_ref(),
                            self.player,
                        )
                        .map_err(SyncError::Replication)?;
                    send(
                        transport,
                        connection,
                        Control::CatchUpAck {
                            generation,
                            cursor: session.cursor(),
                        },
                    )?;
                    self.phase = SyncPhase::CatchingUp;
                    self.candidate = None;
                    ClientSyncOutcome::CatchUpApplied
                }
                Control::ReliableComplete { generation, cursor } => {
                    if self.stale_generation(generation)?
                        || self.phase == SyncPhase::RestartRequired
                    {
                        return Ok(ClientSyncOutcome::Obsolete);
                    }
                    if self.phase != SyncPhase::CatchingUp || cursor != session.cursor() {
                        return Err(SyncError::TransferMismatch);
                    }
                    self.phase = SyncPhase::Finalizing;
                    ClientSyncOutcome::FinalizationPending
                }
                Control::Finalize { token, drags } => {
                    if self.stale_generation(token.generation)?
                        || self.phase == SyncPhase::RestartRequired
                        || token.revision <= self.final_revision
                    {
                        return Ok(ClientSyncOutcome::Obsolete);
                    }
                    if self.phase != SyncPhase::Finalizing
                        || !self.image_ready
                        || self.baseline_cursor.is_none()
                        || token.cursor != session.cursor()
                    {
                        return Err(SyncError::WrongFinalization);
                    }
                    replica
                        .reconcile_final_drags(session, store, &drags)
                        .map_err(SyncError::FinalDrag)?;
                    send(transport, connection, Control::FinalizeAck { token })?;
                    self.final_revision = token.revision;
                    self.candidate = Some(FinalizationCandidate {
                        token,
                        drags,
                        store_epoch: store.epoch,
                    });
                    ClientSyncOutcome::Finalized
                }
                Control::ReadyCommit { token } => {
                    if self.stale_generation(token.generation)?
                        || token.revision < self.final_revision
                        || self.phase == SyncPhase::RestartRequired
                    {
                        return Ok(ClientSyncOutcome::Obsolete);
                    }
                    let candidate = self
                        .candidate
                        .as_ref()
                        .ok_or(SyncError::WrongFinalization)?;
                    if self.phase != SyncPhase::Finalizing
                        || candidate.token != token
                        || token.cursor != session.cursor()
                        || candidate.store_epoch != store.epoch
                    {
                        return Err(SyncError::WrongFinalization);
                    }
                    replica
                        .reconcile_final_drags(session, store, &candidate.drags)
                        .map_err(SyncError::FinalDrag)?;
                    let permit = SyncReadyPermit {
                        connection,
                        metadata: self.authenticated,
                        player: self.player,
                    };
                    bootstrap
                        .promote_ready(connections, &permit)
                        .map_err(SyncError::Bootstrap)?;
                    self.invalidate();
                    self.definition = None;
                    self.generation = None;
                    self.last_offered_id = None;
                    self.bulk = BulkTransferReceiver::default();
                    self.final_revision = 0;
                    self.phase = SyncPhase::Ready;
                    ClientSyncOutcome::Ready
                }
                _ => return Err(SyncError::WrongDirection),
            },
            WireMessage::BulkTransfer(message) => {
                self.receive_bulk(message, transport, replica, session, store, now)?
            }
            _ => return Err(SyncError::WrongDirection),
        };
        self.timing.last_progress = now;
        Ok(outcome)
    }
    fn stale_generation(&self, generation: u64) -> Result<bool, SyncError> {
        let current = self.generation.ok_or(SyncError::WrongGeneration)?;
        if generation > current {
            return Err(SyncError::WrongGeneration);
        }
        Ok(generation < current)
    }
    /// Called on scope/migration changes too. ID high-water marks survive restarts.
    pub fn invalidate(&mut self) {
        self.bulk.clear();
        self.receiving = None;
        self.baseline_cursor = None;
        self.store_epoch = None;
        self.candidate = None;
        self.phase = SyncPhase::RestartRequired;
        self.timing.transfer_progress = None;
    }
    fn accept_offer(
        &mut self,
        binding: SyncTransferBinding,
        purpose: ReceivingPurpose,
        now: Instant,
    ) -> Result<(), SyncError> {
        let cap = match binding.kind {
            BulkTransferKind::PuzzleImage => MAX_PUZZLE_IMAGE_TRANSFER_BYTES,
            BulkTransferKind::JoinBaseline => MAX_JOIN_BASELINE_TRANSFER_BYTES,
        };
        if binding.total_size == 0
            || binding.total_size > cap
            || self.receiving.is_some()
            || self
                .last_offered_id
                .is_some_and(|last| binding.transfer_id <= last)
        {
            return Err(SyncError::TransferMismatch);
        }
        self.last_offered_id = Some(binding.transfer_id);
        self.receiving = Some(ReceivingTransfer {
            binding,
            purpose,
            started: false,
        });
        self.timing.transfer_progress = Some(now);
        Ok(())
    }
    fn receive_bulk<T: Transport>(
        &mut self,
        message: BulkTransferMessage,
        transport: &mut SecureTransport<T>,
        replica: &mut PeerReplicationState,
        session: &mut AuthoritySession,
        store: &mut PieceDataStore,
        now: Instant,
    ) -> Result<ClientSyncOutcome, SyncError> {
        let id = match &message {
            BulkTransferMessage::Start { transfer_id, .. }
            | BulkTransferMessage::Chunk { transfer_id, .. }
            | BulkTransferMessage::Finish { transfer_id }
            | BulkTransferMessage::Abort { transfer_id } => *transfer_id,
        };
        let Some(receiving) = self
            .receiving
            .as_mut()
            .filter(|r| r.binding.transfer_id == id)
        else {
            // Bounded scalar retirement, not a tombstone/payload queue. Monotonic
            // offered IDs can never be rebound to another generation.
            if self.last_offered_id.is_some_and(|last| id <= last) {
                return Ok(ClientSyncOutcome::Obsolete);
            }
            return Err(SyncError::UnofferedTransfer);
        };
        if let BulkTransferMessage::Start {
            kind,
            total_size,
            sha256,
            ..
        } = &message
        {
            if receiving.started
                || receiving.binding.kind != *kind
                || receiving.binding.total_size != *total_size
                || receiving.binding.sha256 != *sha256
            {
                return Err(SyncError::TransferMismatch);
            }
        } else if matches!(message, BulkTransferMessage::Abort { .. }) {
            self.invalidate();
            return Ok(ClientSyncOutcome::Consumed);
        } else if !receiving.started {
            return Err(SyncError::TransferMismatch);
        }
        let starting = matches!(message, BulkTransferMessage::Start { .. });
        let completed = self.bulk.receive(message).map_err(SyncError::Bulk)?;
        if starting {
            receiving.started = true;
        }
        self.timing.transfer_progress = Some(now);
        let Some(completed) = completed else {
            return Ok(ClientSyncOutcome::Consumed);
        };
        let receiving = self.receiving.take().expect("authorized completion");
        self.timing.transfer_progress = None;
        match receiving.purpose {
            ReceivingPurpose::Image => {
                // This is the receiver's actual verified digest. Bulk Start/offer
                // hashes alone never authorize image identity.
                if completed.sha256 != self.authenticated.definition.image_hash.0 {
                    return Err(SyncError::ImageHashMismatch);
                }
                self.image_ready = true;
                self.phase = SyncPhase::AwaitingImageReady;
                send(
                    transport,
                    self.connection,
                    Control::ImageReady {
                        image_hash: self.authenticated.definition.image_hash,
                    },
                )?;
                Ok(ClientSyncOutcome::ImageReady(completed))
            }
            ReceivingPurpose::Baseline { generation, cursor } => {
                if self.generation != Some(generation) || self.phase != SyncPhase::BaselineTransfer
                {
                    return Err(SyncError::WrongGeneration);
                }
                let transfer_id = completed.transfer_id;
                let bytes = completed.into_bytes().map_err(SyncError::Bulk)?;
                let (baseline, rest): (JoinBaseline, _) =
                    postcard::take_from_bytes(&bytes).map_err(|_| SyncError::MalformedBaseline)?;
                if !rest.is_empty() {
                    return Err(SyncError::MalformedBaseline);
                }
                replica
                    .install_join_baseline(
                        session,
                        store,
                        &baseline,
                        SnapshotExpectation {
                            session: self.authenticated.definition.id,
                            image_hash: self.authenticated.definition.image_hash,
                            cursor,
                            definition: self.definition.as_ref().expect("session negotiated"),
                        },
                    )
                    .map_err(SyncError::Baseline)?;
                self.phase = SyncPhase::CatchingUp;
                self.store_epoch = Some(store.epoch);
                send(
                    transport,
                    self.connection,
                    Control::BaselineInstalled {
                        generation,
                        cursor,
                        transfer_id,
                    },
                )?;
                Ok(ClientSyncOutcome::BaselineInstalled)
            }
        }
    }
}

fn require_identity(
    authenticated: SessionMetadata,
    current: SessionMetadata,
) -> Result<(), SyncError> {
    if authenticated.definition != current.definition
        || authenticated.host != current.host
        || authenticated.cursor.epoch != current.cursor.epoch
        || current.cursor < authenticated.cursor
    {
        return Err(SyncError::WrongIdentity);
    }
    Ok(())
}
fn sync_message(event: &TransportEvent) -> Result<(ConnectionId, WireMessage), SyncError> {
    let TransportEvent::Message {
        connection,
        class,
        payload,
    } = event
    else {
        return Err(SyncError::WrongDirection);
    };
    if wire::frame_route_for_class(payload, *class).map_err(SyncError::Wire)?
        != wire::FrameRoute::Syncing
    {
        return Err(SyncError::WrongDirection);
    }
    Ok((
        *connection,
        wire::decode_for_class(payload, *class).map_err(SyncError::Wire)?,
    ))
}
fn send<T: Transport>(
    transport: &mut SecureTransport<T>,
    connection: ConnectionId,
    control: Control,
) -> Result<(), SyncError> {
    let bytes = wire::encode(&WireMessage::SyncControl(control)).map_err(SyncError::Wire)?;
    transport
        .send(connection, MessageClass::Control, &bytes)
        .map_err(SyncError::Transport)
}
fn send_bulk<T: Transport>(
    transport: &mut SecureTransport<T>,
    connection: ConnectionId,
    message: BulkTransferMessage,
) -> Result<(), SyncError> {
    let bytes = wire::encode(&WireMessage::BulkTransfer(message)).map_err(SyncError::Wire)?;
    transport
        .send(connection, MessageClass::Bulk, &bytes)
        .map_err(SyncError::Transport)
}
