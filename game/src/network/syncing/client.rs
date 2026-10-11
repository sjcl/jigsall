//! Client-owned receiving, replica installation and finalization.
use super::*;

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
    display_name: Option<jigsall_core::PlayerDisplayName>,
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
    policy: SyncPolicy,
}
/// The existing replica/store/session are borrowed only while routing sync work.
pub struct SyncReplica<'a> {
    pub roster: &'a mut PlayerRoster,
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
        display_name: Option<jigsall_core::PlayerDisplayName>,
        now: Instant,
    ) -> Result<Self, SyncError> {
        let authenticated = bootstrap.metadata().ok_or(SyncError::NotSyncing)?;
        let player = bootstrap.assigned_player().ok_or(SyncError::NotSyncing)?;
        let image_ready = cached_image.is_some_and(|bytes| {
            <[u8; 32]>::from(Sha256::digest(bytes)) == authenticated.definition.image_hash.0
        });
        bootstrap.begin_sync().map_err(SyncError::Bootstrap)?;
        let mut timing = SyncTiming::new(now);
        timing.wait(ResponseWait::Session, now);
        Ok(Self {
            display_name,
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
            timing,
            policy: SyncPolicy::default(),
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
    pub fn set_policy(&mut self, policy: SyncPolicy) {
        self.policy = policy;
    }
    pub fn timeout(&self, now: Instant) -> Option<SyncError> {
        self.timing.timeout(now, self.policy)
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
        let result = if let Some(error) = self.timeout(now) {
            Err(error)
        } else {
            self.route_inner(bootstrap, connections, event, transport, state, now)
        };
        if result.is_err() {
            self.invalidate();
            let _ = bootstrap.reject(
                result.as_ref().err().unwrap().disconnect_reason(),
                transport,
                connections,
            );
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
            roster,
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
            return Err(SyncError::AuthorityChanged);
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
                        Control::ClientProfile {
                            display_name: self.display_name.clone(),
                        },
                    )?;
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
                    if self.stale_generation(generation)?
                        || self.phase == SyncPhase::RestartRequired
                    {
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
                    let previous = session.cursor();
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
                    if session.cursor() == previous {
                        return Ok(ClientSyncOutcome::Obsolete);
                    }
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
                Control::ReadyCommit {
                    token,
                    roster: snapshot,
                    drags,
                } => {
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
                    if !drags.same_reliable_structure(&candidate.drags) {
                        return Err(SyncError::FinalDrag(FinalDragError::ContextMismatch));
                    }
                    let next_roster = PlayerRoster::validated_snapshot(
                        snapshot,
                        self.authenticated.host,
                        self.player,
                    )
                    .map_err(SyncError::Roster)?;
                    // Syncing may have dropped intervening Transient updates.
                    // Reapply the fresh commit scalars, never the old candidate.
                    replica
                        .reconcile_final_drags(session, store, &drags)
                        .map_err(SyncError::FinalDrag)?;
                    let permit = SyncReadyPermit {
                        connection,
                        metadata: self.authenticated,
                        player: self.player,
                    };
                    bootstrap
                        .promote_ready(connections, &permit)
                        .map_err(SyncError::Bootstrap)?;
                    transport
                        .mark_ready(connection)
                        .map_err(SyncError::Transport)?;
                    **roster = next_roster;
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
        if !matches!(outcome, ClientSyncOutcome::Obsolete) {
            self.timing.peer_progress(now);
            self.timing.phase_entered = now;
            if self.phase == SyncPhase::Finalizing {
                self.timing.wait(ResponseWait::ReadyCommit, now);
            } else if self.phase == SyncPhase::CatchingUp {
                self.timing.wait(ResponseWait::CatchUpAck, now);
            }
        }
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
        self.timing.bulk = None;
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
        self.timing
            .budget(binding.kind, binding.total_size, self.policy);
        self.timing.start_bulk(binding.total_size, 0, now);
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
        let chunk_bytes = match &message {
            BulkTransferMessage::Chunk { data, .. } => data.len() as u64,
            _ => 0,
        };
        let completed = self.bulk.receive(message).map_err(SyncError::Bulk)?;
        if starting {
            receiving.started = true;
        }
        if chunk_bytes > 0 {
            if let Some(bulk) = self.timing.bulk {
                self.timing
                    .observe_delivery(bulk.delivered + chunk_bytes, now);
            }
        }
        let Some(completed) = completed else {
            return Ok(ClientSyncOutcome::Consumed);
        };
        let receiving = self.receiving.take().expect("authorized completion");
        self.timing.bulk = None;
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
