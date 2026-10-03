//! Runs the join foundation through bootstrap and encrypted Transport messages.
use super::*;
use crate::{
    multiplayer::catch_up::*,
    network::{bulk::*, sync_control::SyncControlMessage as Control, syncing::*},
};
use sha2::{Digest, Sha256};
use std::sync::Arc;

struct Harness {
    p: Pair,
    s: Scenario,
    host: HostSyncCoordinator,
    client: ClientSyncRouter,
    image: Arc<[u8]>,
    images: usize,
}
fn authority(s: &Scenario) -> SyncAuthority<'_> {
    SyncAuthority {
        session: &s.host.session,
        store: &s.host.store,
        contexts: &s.contexts,
        definition: &s.definition,
    }
}
impl Harness {
    fn new(cached: bool, limits: CatchUpLimits) -> Self {
        let image: Arc<[u8]> = Arc::from(b"immutable session image".as_slice());
        let definition = SessionDefinition {
            id: SESSION.id,
            image_hash: ImageHash(Sha256::digest(&image).into()),
        };
        let metadata = SessionMetadata {
            definition,
            ..metadata()
        };
        let mut p = Pair::new("correct password");
        p.host = HostBootstrap::new(password("correct password"), metadata, [], p.now);
        p.authenticate();
        let mut s = Scenario::new();
        s.host.session = AuthoritySession::new(definition, HOST, metadata.cursor);
        for peer in &mut s.peers {
            peer.session = AuthoritySession::new(definition, HOST, metadata.cursor);
        }
        let mut host = HostSyncCoordinator::new(JoinCatchUpCoordinator::new(limits));
        let client =
            ClientSyncRouter::start(&mut p.client, cached.then_some(image.as_ref()), p.now)
                .unwrap();
        host.start(&mut p.host, HA, &mut p.ht, &authority(&s), p.now)
            .unwrap();
        Self {
            p,
            s,
            host,
            client,
            image,
            images: 0,
        }
    }
    fn host_to_client(&mut self) -> Result<Vec<ClientSyncOutcome>, SyncError> {
        let messages = std::mem::take(&mut self.p.ht.backend_mut().sent);
        self.p
            .ct
            .backend_mut()
            .inbox
            .extend(messages.into_iter().map(|e| remap(e, CLIENT_HOST)));
        let mut events = Vec::new();
        self.p.ct.poll(&mut events).unwrap();
        let mut outcomes = Vec::new();
        for event in events {
            assert_eq!(
                self.p
                    .client
                    .process(
                        &event,
                        &mut self.p.ct,
                        &mut self.p.client_connections,
                        self.p.now
                    )
                    .unwrap(),
                BootstrapOutcome::Syncing
            );
            let peer = &mut self.s.peers[0];
            let outcome = self.client.route(
                &self.p.client,
                &event,
                &mut self.p.ct,
                &mut SyncReplica {
                    replica: &mut peer.replica,
                    session: &mut peer.session,
                    store: &mut peer.store,
                },
                self.p.now,
            )?;
            if let ClientSyncOutcome::ImageReady(completed) = &outcome {
                assert_eq!(
                    completed.sha256,
                    self.p.client.metadata().unwrap().definition.image_hash.0
                );
                assert_eq!(
                    completed.chunks().flatten().copied().collect::<Vec<_>>(),
                    self.image.as_ref()
                );
                self.images += 1;
            }
            outcomes.push(outcome);
        }
        Ok(outcomes)
    }
    fn client_to_host(&mut self) -> Result<(), SyncError> {
        let messages = std::mem::take(&mut self.p.ct.backend_mut().sent);
        self.p
            .ht
            .backend_mut()
            .inbox
            .extend(messages.into_iter().map(|e| remap(e, HA)));
        let mut events = Vec::new();
        self.p.ht.poll(&mut events).unwrap();
        for event in events {
            assert_eq!(
                self.p
                    .host
                    .process(
                        &event,
                        &mut self.p.ht,
                        &mut self.p.host_connections,
                        self.p.now
                    )
                    .unwrap(),
                BootstrapOutcome::Syncing
            );
            self.host.route(
                &self.p.host,
                &event,
                &mut self.p.ht,
                &authority(&self.s),
                Some(self.image.clone()),
                self.p.now,
            )?;
        }
        Ok(())
    }
    fn step(&mut self) {
        self.host_to_client().unwrap();
        self.client_to_host().unwrap();
        self.host
            .pump(
                &self.p.host,
                HA,
                &mut self.p.ht,
                &authority(&self.s),
                self.p.now,
            )
            .unwrap();
    }
    fn reach(&mut self, phase: SyncPhase) {
        for _ in 0..100 {
            if self.client.phase() == phase {
                return;
            }
            self.step();
        }
        panic!("did not reach {phase:?}: {:?}", self.client.phase());
    }
    fn send_host(&mut self, message: WireMessage) -> Result<Vec<ClientSyncOutcome>, SyncError> {
        self.p
            .ht
            .send(HA, message.class(), &wire::encode(&message).unwrap())
            .unwrap();
        self.host_to_client()
    }
    fn send_client(&mut self, control: Control) -> Result<(), SyncError> {
        let message = WireMessage::SyncControl(control);
        self.p
            .ct
            .send(
                CLIENT_HOST,
                message.class(),
                &wire::encode(&message).unwrap(),
            )
            .unwrap();
        self.client_to_host()
    }
    fn apply_grab(&mut self) {
        connected(&mut self.s.host.connections, HB, B);
        let mut command = grab();
        command.player = B;
        let routed = self
            .s
            .host_router()
            .route_with_sync(
                &message_event(HB, &WireMessage::ClientCommand(command)),
                &mut self.host,
            )
            .unwrap();
        assert!(routed.retention.is_ok());
        let HostRouteOutcome::Applied(outcome) = routed.gameplay else {
            panic!("grab applies");
        };
        // Publication follows recording, even if a joining peer needs restart.
        assert_eq!(self.s.host.session.cursor().sequence.0, 1);
        assert!(outcome.authority_event.is_some());
    }
}

#[test]
fn missing_image_prepares_before_begin_join_then_installs_and_stops_at_finalization() {
    let mut h = Harness::new(false, CatchUpLimits::default());
    let player = h.p.host.assigned_player(HA).unwrap();
    h.host_to_client().unwrap();
    h.client_to_host().unwrap();
    assert_eq!(h.host.phase(HA), Some(SyncPhase::ImageTransfer));
    assert_eq!(
        h.host.catch_up().status(player),
        Err(CatchUpError::NotJoining)
    );
    h.apply_grab();
    assert_eq!(
        h.host.catch_up().status(player),
        Err(CatchUpError::NotJoining)
    );
    // Arbitrarily long image preparation retains zero authority events.
    h.p.now += Duration::from_secs(60 * 60);
    for _ in 0..20 {
        h.host_to_client().unwrap();
        if h.client.phase() == SyncPhase::AwaitingImageReady {
            break;
        }
        h.client_to_host().unwrap();
        h.host
            .pump(&h.p.host, HA, &mut h.p.ht, &authority(&h.s), h.p.now)
            .unwrap();
    }
    assert_eq!(h.client.phase(), SyncPhase::AwaitingImageReady);
    assert_eq!(h.images, 1);
    assert!(h.client.image_ready());
    assert_eq!(
        h.host.catch_up().status(player),
        Err(CatchUpError::NotJoining)
    );
    h.client_to_host().unwrap();
    let status = h.host.catch_up().status(player).unwrap();
    assert_eq!(status.baseline_cursor, AuthorityCursor::new(3, 1));
    assert_eq!(status.retained_events, 0);
    assert_eq!(h.p.host_connections.player(HA), None);
    h.reach(SyncPhase::Finalizing);
    assert_eq!(h.client.baseline_cursor(), Some(status.baseline_cursor));
    assert_eq!(h.s.peers[0].store.states, h.s.host.store.states);
    assert!(h.s.peers[0]
        .replica
        .remote_drag(&h.s.peers[0].session, &h.s.peers[0].store, B)
        .is_some());
    assert_eq!(h.p.client.state(), Some(ConnectionState::Syncing));
    assert_eq!(h.p.host.state(HA), Some(ConnectionState::Syncing));
    assert_eq!(h.p.client_connections.player(CLIENT_HOST), None);
}

#[test]
fn verified_cache_skips_image_and_records_reliable_events_immediately_after_begin() {
    let mut h = Harness::new(true, CatchUpLimits::default());
    h.host_to_client().unwrap();
    h.client_to_host().unwrap();
    assert_eq!(h.host.phase(HA), Some(SyncPhase::BaselineTransfer));
    h.apply_grab();
    let status = h
        .host
        .catch_up()
        .status(h.p.host.assigned_player(HA).unwrap())
        .unwrap();
    assert_eq!(status.baseline_cursor.sequence.0, 0);
    assert_eq!(status.retained_events, 1);
    h.reach(SyncPhase::Finalizing);
    assert_eq!(h.images, 0);
    assert_eq!(h.s.peers[0].session.cursor(), h.s.host.session.cursor());
    assert_eq!(h.s.peers[0].store.states, h.s.host.store.states);
    assert!(h
        .host
        .catch_up()
        .latest_drag_updates(h.p.host.assigned_player(HA).unwrap(), 0)
        .unwrap()
        .is_empty());
    // Finalizing still consumes/ACKs newer Reliable events before another candidate.
    let cancelled =
        h.s.contexts
            .cancel_replicated(&mut h.s.host.session, &mut h.s.host.store, B)
            .unwrap()
            .unwrap();
    h.host
        .record_authority_event(
            &h.s.host.session,
            &h.s.host.store,
            &cancelled.authority_event,
        )
        .unwrap();
    assert_eq!(h.host.phase(HA), Some(SyncPhase::CatchingUp));
    h.step();
    h.host_to_client().unwrap();
    assert_eq!(h.client.phase(), SyncPhase::CatchingUp);
    h.reach(SyncPhase::Finalizing);
    assert_eq!(h.s.peers[0].session.cursor(), h.s.host.session.cursor());
}

#[test]
fn restart_before_offer_delivery_cannot_send_abort_ahead_of_control() {
    let mut h = Harness::new(
        true,
        CatchUpLimits {
            max_events: 0,
            ..CatchUpLimits::default()
        },
    );
    h.host_to_client().unwrap();
    h.client_to_host().unwrap();
    // begin_join queued G0's offer, but the client has not seen it or sent an ACK.
    assert_eq!(h.host.phase(HA), Some(SyncPhase::BaselineTransfer));
    assert_eq!(h.client.generation(), None);
    assert_eq!(h.client.baseline_cursor(), None);
    assert_eq!(h.client.declared_in_flight_bytes(), 0);
    h.apply_grab();
    assert_eq!(h.host.phase(HA), Some(SyncPhase::RestartRequired));
    h.host
        .restart(&h.p.host, HA, &mut h.p.ht, &authority(&h.s), h.p.now)
        .unwrap();
    // Preserve Reliable order inside each lane, but let any Bulk Abort overtake
    // the still-undelivered G0 offer, Restart and G1 offer on Control.
    h.p.ht.backend_mut().sent.sort_by_key(|event| {
        !matches!(
            event,
            TransportEvent::Message {
                class: MessageClass::Bulk,
                ..
            }
        )
    });
    let only_control = h.p.ht.backend_mut().sent.iter().all(|event| {
        matches!(
            event,
            TransportEvent::Message {
                class: MessageClass::Control,
                ..
            }
        )
    });
    h.host_to_client().unwrap();
    assert!(
        only_control,
        "unaccepted transfers have no Bulk packets to cancel"
    );
    assert_eq!(h.client.generation(), Some(1));
    assert_eq!(h.client.baseline_cursor(), Some(AuthorityCursor::new(3, 1)));
    assert_eq!(h.client.declared_in_flight_bytes(), 0);
    // The late T0 acceptance cannot accept T1 or start obsolete Bulk.
    h.client_to_host().unwrap();
    h.reach(SyncPhase::Finalizing);
    assert_eq!(h.s.peers[0].session.cursor(), h.s.host.session.cursor());
    assert_eq!(h.s.peers[0].store.states, h.s.host.store.states);
}

#[test]
fn restart_aborts_accepted_offers_before_start_and_after_finish() {
    for finished in [false, true] {
        let mut h = Harness::new(
            true,
            CatchUpLimits {
                max_events: 0,
                ..CatchUpLimits::default()
            },
        );
        h.host_to_client().unwrap();
        h.client_to_host().unwrap();
        h.host_to_client().unwrap();
        h.client_to_host().unwrap();
        // TransferAccepted has reached the host, but no Bulk has been generated.
        assert_eq!(h.client.generation(), Some(0));
        assert!(h.p.ht.backend_mut().sent.is_empty());
        if finished {
            // This small baseline fits one chunk. Queue Start/Chunk/Finish without
            // delivery: sending is now gone, but the install ACK is still pending.
            for _ in 0..3 {
                h.host
                    .pump(&h.p.host, HA, &mut h.p.ht, &authority(&h.s), h.p.now)
                    .unwrap();
            }
        }
        h.apply_grab();
        h.host
            .restart(&h.p.host, HA, &mut h.p.ht, &authority(&h.s), h.p.now)
            .unwrap();
        assert_eq!(
            h.p.ht
                .backend_mut()
                .sent
                .iter()
                .filter(|event| matches!(
                    event,
                    TransportEvent::Message {
                        class: MessageClass::Bulk,
                        ..
                    }
                ))
                .count(),
            if finished { 4 } else { 1 },
            "accepted transfers still receive Abort"
        );
        h.p.ht.backend_mut().sent.sort_by_key(|event| {
            !matches!(
                event,
                TransportEvent::Message {
                    class: MessageClass::Bulk,
                    ..
                }
            )
        });
        h.host_to_client().unwrap();
        assert_eq!(h.client.generation(), Some(1));
        assert_eq!(h.client.declared_in_flight_bytes(), 0);
        h.client_to_host().unwrap();
        h.reach(SyncPhase::Finalizing);
        assert_eq!(h.s.peers[0].session.cursor(), h.s.host.session.cursor());
        assert_eq!(h.s.peers[0].store.states, h.s.host.store.states);
    }
}

#[test]
fn restart_obsoletes_partial_bulk_old_acks_and_late_completion_across_lanes() {
    let mut h = Harness::new(
        true,
        CatchUpLimits {
            max_events: 0,
            ..CatchUpLimits::default()
        },
    );
    h.reach(SyncPhase::BaselineTransfer);
    // Accept offer and deliver Start, reserving the old transfer's bytes.
    h.client_to_host().unwrap();
    h.host
        .pump(&h.p.host, HA, &mut h.p.ht, &authority(&h.s), h.p.now)
        .unwrap();
    h.host_to_client().unwrap();
    assert!(h.client.declared_in_flight_bytes() > 0);
    let old = h.host.transfer_binding(HA).unwrap();
    let old_cursor = h.client.baseline_cursor().unwrap();
    h.apply_grab();
    assert_eq!(h.host.phase(HA), Some(SyncPhase::RestartRequired));
    assert_eq!(
        h.send_client(Control::BaselineInstalled {
            generation: 0,
            cursor: old_cursor,
            transfer_id: old.transfer_id
        }),
        Ok(())
    );
    h.host
        .restart(&h.p.host, HA, &mut h.p.ht, &authority(&h.s), h.p.now)
        .unwrap();
    // New-generation Control overtakes old Bulk Abort on independent lanes.
    let mut sent = std::mem::take(&mut h.p.ht.backend_mut().sent);
    sent.sort_by_key(|e| {
        matches!(
            e,
            TransportEvent::Message {
                class: MessageClass::Bulk,
                ..
            }
        )
    });
    h.p.ht.backend_mut().sent = sent;
    h.host_to_client().unwrap();
    assert_eq!(h.client.generation(), Some(1));
    assert_eq!(h.client.declared_in_flight_bytes(), 0);
    for message in [
        BulkTransferMessage::Start {
            transfer_id: old.transfer_id,
            kind: old.kind,
            total_size: old.total_size,
            sha256: old.sha256,
        },
        BulkTransferMessage::Chunk {
            transfer_id: old.transfer_id,
            offset: 0,
            data: vec![1],
        },
        BulkTransferMessage::Finish {
            transfer_id: old.transfer_id,
        },
    ] {
        assert!(matches!(
            h.send_host(WireMessage::BulkTransfer(message))
                .unwrap()
                .pop(),
            Some(ClientSyncOutcome::Obsolete)
        ));
    }
    assert_eq!(
        h.send_client(Control::CatchUpAck {
            generation: 0,
            cursor: old_cursor
        }),
        Ok(())
    );
    h.reach(SyncPhase::Finalizing);
    assert_eq!(h.client.baseline_cursor(), Some(AuthorityCursor::new(3, 1)));
    assert_eq!(h.s.peers[0].session.cursor(), h.s.host.session.cursor());
    assert_eq!(h.s.peers[0].store.states, h.s.host.store.states);
}

#[test]
fn preoffer_future_bulk_wrong_direction_and_phase_cannot_install() {
    let mut h = Harness::new(true, CatchUpLimits::default());
    assert!(matches!(
        h.send_host(WireMessage::BulkTransfer(BulkTransferMessage::Start {
            transfer_id: TransferId(9),
            kind: BulkTransferKind::JoinBaseline,
            total_size: 1,
            sha256: [0; 32]
        })),
        Err(SyncError::UnofferedTransfer)
    ));
    h.p.ct.backend_mut().sent.clear();
    let message = WireMessage::BulkTransfer(BulkTransferMessage::Start {
        transfer_id: TransferId(1),
        kind: BulkTransferKind::PuzzleImage,
        total_size: MAX_PUZZLE_IMAGE_TRANSFER_BYTES,
        sha256: h.p.client.metadata().unwrap().definition.image_hash.0,
    });
    h.p.ct
        .send(
            CLIENT_HOST,
            message.class(),
            &wire::encode(&message).unwrap(),
        )
        .unwrap();
    assert_eq!(h.client_to_host(), Err(SyncError::WrongDirection));
    assert_eq!(h.s.host.session.cursor().sequence.0, 0);
    assert_eq!(
        h.host
            .catch_up()
            .status(h.p.host.assigned_player(HA).unwrap()),
        Err(CatchUpError::NotJoining)
    );
}

#[test]
fn offered_image_hash_is_bound_to_authenticated_session_and_content_is_verified() {
    let mut h = Harness::new(false, CatchUpLimits::default());
    h.host_to_client().unwrap();
    let offer = crate::network::sync_control::SyncTransferBinding {
        transfer_id: TransferId(1),
        kind: BulkTransferKind::PuzzleImage,
        total_size: 3,
        sha256: Sha256::digest(b"bad").into(),
    };
    assert!(matches!(
        h.send_host(WireMessage::SyncControl(Control::ImageOffer(offer))),
        Err(SyncError::ImageHashMismatch)
    ));
    h.client_to_host().unwrap();
    h.host_to_client().unwrap();
    h.client_to_host().unwrap();
    let transfer = h.host.transfer_binding(HA).unwrap();
    h.host
        .pump(&h.p.host, HA, &mut h.p.ht, &authority(&h.s), h.p.now)
        .unwrap();
    h.host_to_client().unwrap();
    h.send_host(WireMessage::BulkTransfer(BulkTransferMessage::Chunk {
        transfer_id: transfer.transfer_id,
        offset: 0,
        data: vec![0; h.image.len()],
    }))
    .unwrap();
    assert!(matches!(
        h.send_host(WireMessage::BulkTransfer(BulkTransferMessage::Finish {
            transfer_id: transfer.transfer_id
        })),
        Err(SyncError::Bulk(BulkTransferError::HashMismatch))
    ));
    assert!(!h.client.image_ready());
    assert_eq!(
        h.host
            .catch_up()
            .status(h.p.host.assigned_player(HA).unwrap()),
        Err(CatchUpError::NotJoining)
    );
}

#[test]
fn store_restore_and_authority_freeze_invalidate_sync_and_timeout_slots_are_visible() {
    let mut h = Harness::new(true, CatchUpLimits::default());
    h.reach(SyncPhase::BaselineTransfer);
    h.client_to_host().unwrap();
    h.host
        .pump(&h.p.host, HA, &mut h.p.ht, &authority(&h.s), h.p.now)
        .unwrap();
    h.host_to_client().unwrap();
    let timing = h.client.timing();
    assert!(timing.timed_out(
        h.p.now + Duration::from_secs(11),
        Duration::from_secs(60),
        Duration::from_secs(10)
    ));
    assert!(timing.timed_out(
        h.p.now + Duration::from_secs(61),
        Duration::from_secs(60),
        Duration::from_secs(100)
    ));
    h.s.host.store.epoch += 1;
    assert_eq!(
        h.host
            .observe_host_state(&h.s.host.session, &h.s.host.store),
        Err(CatchUpError::RestartRequired(
            CatchUpRestartReason::ScopeChanged
        ))
    );
    h.host
        .restart(&h.p.host, HA, &mut h.p.ht, &authority(&h.s), h.p.now)
        .unwrap();
    h.host_to_client().unwrap();
    assert_eq!(h.client.generation(), Some(1));
    h.s.host.session.begin_graceful(B).unwrap();
    assert_eq!(
        h.host
            .observe_host_state(&h.s.host.session, &h.s.host.store),
        Err(CatchUpError::RestartRequired(
            CatchUpRestartReason::AuthorityFrozen
        ))
    );
    assert!(h
        .host
        .restart(&h.p.host, HA, &mut h.p.ht, &authority(&h.s), h.p.now)
        .is_err());
    h.host.disconnect(HA);
    assert_eq!(h.host.phase(HA), None);
}

#[test]
fn offered_baseline_is_validated_against_control_identity_and_cursor_before_ack() {
    let mut h = Harness::new(true, CatchUpLimits::default());
    h.host_to_client().unwrap();
    h.p.ct.backend_mut().sent.clear();
    let mut baseline = crate::multiplayer::JoinBaseline::capture(
        &h.s.host.session,
        &h.s.host.store,
        &h.s.contexts,
        &h.s.definition,
    )
    .unwrap();
    baseline.snapshot.session = SessionId(999);
    let bytes: Arc<[u8]> = postcard::to_allocvec(&baseline).unwrap().into();
    let mut sender = BulkTransferSender::default();
    let mut sending = sender
        .begin(BulkTransferKind::JoinBaseline, bytes.clone())
        .unwrap();
    let transfer = crate::network::sync_control::SyncTransferBinding {
        transfer_id: sending.transfer_id(),
        kind: BulkTransferKind::JoinBaseline,
        total_size: bytes.len() as u64,
        sha256: Sha256::digest(&bytes).into(),
    };
    h.send_host(WireMessage::SyncControl(Control::BaselineOffer {
        generation: 0,
        cursor: AuthorityCursor::new(3, 0),
        transfer,
    }))
    .unwrap();
    let before = h.s.peers[0].store.states.clone();
    let mut failed = false;
    while let Some(message) = sending.next_message().unwrap() {
        let result = h.send_host(WireMessage::BulkTransfer(message));
        if matches!(result, Err(SyncError::Baseline(_))) {
            failed = true;
        } else {
            result.unwrap();
        }
    }
    assert!(failed);
    assert_eq!(h.s.peers[0].store.states, before);
    assert_eq!(h.s.peers[0].session.cursor(), AuthorityCursor::new(3, 0));
    assert_eq!(
        h.p.ct.backend_mut().sent.len(),
        1,
        "only offer acceptance; no install ACK"
    );
}

#[test]
fn exact_offer_metadata_and_local_scope_changes_fail_without_install() {
    let mut h = Harness::new(true, CatchUpLimits::default());
    h.host_to_client().unwrap();
    h.client_to_host().unwrap();
    h.host_to_client().unwrap();
    h.client_to_host().unwrap();
    let transfer = h.host.transfer_binding(HA).unwrap();
    assert!(matches!(
        h.send_host(WireMessage::BulkTransfer(BulkTransferMessage::Start {
            transfer_id: transfer.transfer_id,
            kind: transfer.kind,
            total_size: transfer.total_size + 1,
            sha256: transfer.sha256
        })),
        Err(SyncError::TransferMismatch)
    ));
    assert_eq!(h.client.declared_in_flight_bytes(), 0);
    h.host
        .pump(&h.p.host, HA, &mut h.p.ht, &authority(&h.s), h.p.now)
        .unwrap();
    h.host_to_client().unwrap();
    assert!(h.client.declared_in_flight_bytes() > 0);
    h.s.peers[0].store.epoch += 1;
    h.host
        .pump(&h.p.host, HA, &mut h.p.ht, &authority(&h.s), h.p.now)
        .unwrap();
    assert!(matches!(h.host_to_client(), Err(SyncError::WrongPhase)));
    assert_eq!(h.client.phase(), SyncPhase::RestartRequired);
    assert_eq!(h.client.declared_in_flight_bytes(), 0);
}

#[test]
fn missed_retention_hook_preserves_applied_outcome_for_ready_publication() {
    let mut h = Harness::new(true, CatchUpLimits::default());
    h.host_to_client().unwrap();
    h.client_to_host().unwrap();
    h.apply_grab();
    // Deliberately omit the cancellation hook. The next Reliable command must
    // expose the missed cursor and retain its own already-applied publication.
    h.s.contexts
        .cancel_replicated(&mut h.s.host.session, &mut h.s.host.store, B)
        .unwrap()
        .unwrap();
    let mut request = grab();
    request.player = B;
    request.sequence = ClientCommandSequence::Control(1);
    let routed =
        h.s.host_router()
            .route_with_sync(
                &message_event(HB, &WireMessage::ClientCommand(request)),
                &mut h.host,
            )
            .unwrap();
    assert_eq!(
        routed.retention,
        Err(CatchUpError::RestartRequired(
            CatchUpRestartReason::MissedAuthorityEvent
        ))
    );
    let HostRouteOutcome::Applied(outcome) = routed.gameplay else {
        unreachable!()
    };
    assert_eq!(
        outcome.authority_event.as_ref().unwrap().cursor,
        AuthorityCursor::new(3, 3)
    );
    let mut transport = FakeTransport::default();
    h.s.host_router()
        .publish(&mut transport, Some(HB), &outcome)
        .unwrap();
    assert_eq!(transport.sent.len(), 1);
    assert_eq!(h.s.host.session.cursor(), AuthorityCursor::new(3, 3));
    assert_eq!(h.host.phase(HA), Some(SyncPhase::RestartRequired));
}
