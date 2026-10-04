//! Runs the join foundation through bootstrap and encrypted Transport messages.
use super::*;
use crate::network::sync_control::SyncTransferBinding;
use crate::{
    multiplayer::catch_up::*,
    network::{bulk::*, sync_control::SyncControlMessage as Control, syncing::*},
};
use crate::{multiplayer::finalization::FinalDragSet, network::sync_control::SyncFinalization};
use sha2::{Digest, Sha256};
use std::sync::Arc;

fn expire_join(h: &mut Harness, now: Instant) -> Vec<(ConnectionId, SyncError)> {
    h.host.expire(
        &mut SyncHost {
            roster: &mut h.p.host_roster,
            bootstrap: &mut h.p.host,
            connections: &mut h.p.host_connections,
        },
        &mut h.p.ht,
        now,
    )
}
fn assert_expired_join(h: &mut Harness, wait: ResponseWait) {
    assert_eq!(h.host.timing(HA).unwrap().waiting_for(), Some(wait));
    let player = h.p.host.assigned_player(HA).unwrap();
    let now = h.p.now + Duration::from_secs(12);
    assert!(expire_join(h, now - Duration::from_nanos(1)).is_empty());
    assert_eq!(
        expire_join(h, now),
        vec![(HA, SyncError::PhaseTimeout(wait))]
    );
    assert_eq!(h.host.phase(HA), None);
    assert_eq!(h.p.host.state(HA), None);
    assert!(!h.p.ht.has_channel(HA));
    assert_eq!(h.p.host_connections.peers().count(), 0);
    assert_eq!(
        h.host.catch_up().status(player),
        Err(CatchUpError::NotJoining)
    );
    let resources = h.host.resources();
    assert_eq!(
        (
            resources.peers,
            resources.image_waiters,
            resources.baseline_waiters,
            resources.image_transfers,
            resources.baseline_transfers
        ),
        (0, 0, 0, 0, 0)
    );
}

#[test]
fn sync_disconnect_reasons_preserve_peer_blame_and_local_failure_provenance() {
    use crate::network::lifecycle::is_abuse;
    use crate::{multiplayer::finalization::FinalDragError, players::RosterError};
    for (error, expected) in [
        (
            SyncError::Transport(TransportError::Backpressure),
            DisconnectReason::ConnectionProblem,
        ),
        (
            SyncError::Transport(TransportError::EgressUnavailable),
            DisconnectReason::BackendFailure,
        ),
        (
            SyncError::Transport(TransportError::Backend("backend failure".into())),
            DisconnectReason::BackendFailure,
        ),
        (
            SyncError::Transport(TransportError::NotConnected),
            DisconnectReason::ConnectionProblem,
        ),
        (
            SyncError::Transport(TransportError::UnknownConnection),
            DisconnectReason::ConnectionProblem,
        ),
        (
            SyncError::Transport(TransportError::UnknownListener),
            DisconnectReason::BackendFailure,
        ),
        (
            SyncError::Transport(TransportError::Capacity),
            DisconnectReason::JoinCapacity,
        ),
        (
            SyncError::Transport(TransportError::PayloadTooLarge),
            DisconnectReason::BackendFailure,
        ),
        (
            SyncError::Transport(TransportError::ProtocolViolation),
            DisconnectReason::BackendFailure,
        ),
        (
            SyncError::Bootstrap(BootstrapError::Transport(TransportError::EgressUnavailable)),
            DisconnectReason::BackendFailure,
        ),
        (
            SyncError::Bootstrap(BootstrapError::InvalidTransition),
            DisconnectReason::BackendFailure,
        ),
        (
            SyncError::Bulk(BulkTransferError::AllocationFailed),
            DisconnectReason::BackendFailure,
        ),
        (
            SyncError::Bulk(BulkTransferError::CounterExhausted),
            DisconnectReason::BackendFailure,
        ),
        (
            SyncError::HostBulk(BulkTransferError::HashMismatch),
            DisconnectReason::BackendFailure,
        ),
        (
            SyncError::HostFinalDrag(FinalDragError::ContextMismatch),
            DisconnectReason::BackendFailure,
        ),
        (
            SyncError::HostRoster(RosterError::TooManyPlayers),
            DisconnectReason::JoinCapacity,
        ),
        (
            SyncError::HostRoster(RosterError::RevisionExhausted),
            DisconnectReason::BackendFailure,
        ),
        (
            SyncError::HostCatchUp(CatchUpError::GenerationExhausted),
            DisconnectReason::BackendFailure,
        ),
        (
            SyncError::HostCatchUp(CatchUpError::TooManyPendingJoins),
            DisconnectReason::JoinCapacity,
        ),
        (
            SyncError::FinalizationExhausted,
            DisconnectReason::BackendFailure,
        ),
        (
            SyncError::BaselineEncoding,
            DisconnectReason::BackendFailure,
        ),
        (
            SyncError::Encoding(WireError::Oversized),
            DisconnectReason::BackendFailure,
        ),
        (
            SyncError::AuthorityChanged,
            DisconnectReason::ConnectionProblem,
        ),
        (
            SyncError::CapacityWaitTimeout,
            DisconnectReason::HostCapacityTimeout,
        ),
    ] {
        assert_eq!(error.disconnect_reason(), expected, "{error:?}");
        assert!(!is_abuse(error.disconnect_reason()), "{error:?}");
    }
    for error in [
        SyncError::WrongPhase,
        SyncError::WrongIdentity,
        SyncError::MalformedBaseline,
        SyncError::Wire(WireError::Oversized),
        SyncError::Bulk(BulkTransferError::HashMismatch),
        SyncError::FinalDrag(FinalDragError::ContextMismatch),
        SyncError::Roster(RosterError::TooManyPlayers),
        SyncError::CatchUp(CatchUpError::FutureAcknowledgement),
    ] {
        assert_eq!(
            error.disconnect_reason(),
            DisconnectReason::ProtocolViolation,
            "{error:?}"
        );
        assert!(is_abuse(error.disconnect_reason()));
    }
    for reason in [
        DisconnectReason::AuthenticationTimeout,
        DisconnectReason::AuthenticatedHandoffTimeout,
    ] {
        let error = SyncError::Bootstrap(BootstrapError::Rejected(reason));
        assert_eq!(error.disconnect_reason(), reason);
        assert!(is_abuse(error.disconnect_reason()));
    }
    for error in [
        SyncError::PhaseTimeout(ResponseWait::CatchUpAck),
        SyncError::LifetimeTimeout,
        SyncError::BulkStalled,
    ] {
        assert!(is_abuse(error.disconnect_reason()));
    }
}

#[test]
fn local_egress_failures_do_not_penalize_an_origin_across_reconnects() {
    for error in [
        TransportError::Backpressure,
        TransportError::EgressUnavailable,
        TransportError::Backend("backend failure".into()),
    ] {
        let origin = Origin::Ip("192.0.2.20".parse().unwrap());
        let mut h = BaselineSlots::with_origin(3, Default::default(), Some(origin));
        for n in 0..3 {
            h.ready(n);
            let id = h.id(n);
            let transfer_id = h.host.transfer_binding(id).unwrap().transfer_id;
            h.control(n, Control::TransferAccepted { transfer_id })
                .unwrap();
            let p = &mut h.pairs[n];
            p.ht.backend_mut().egress_error = Some(error.clone());
            assert_eq!(
                h.host.expire_connection(
                    &mut SyncHost {
                        roster: &mut p.host_roster,
                        bootstrap: &mut p.host,
                        connections: &mut p.host_connections,
                    },
                    id,
                    &mut p.ht,
                    p.now
                ),
                Some(SyncError::Transport(error.clone()))
            );
            assert!(!p.ht.has_channel(id));
        }
        assert_eq!(h.host.resources().peers, 0);
        let mut p = Pair::new("correct password");
        p.now = h.pairs.iter().map(|p| p.now).max().unwrap() + Duration::from_secs(20);
        p.host_connection = ConnectionId::new(9997);
        p.ht.backend_mut().origin = Some(origin);
        p.authenticate();
        // Rate tokens have refilled; three local failures must not impose cooldown.
        h.host
            .start(
                &mut p.host,
                p.host_connection,
                &mut p.ht,
                &authority(&h.s),
                p.now,
            )
            .unwrap();
    }
}

#[test]
fn configured_session_image_accepts_equal_content_from_a_fresh_arc() {
    let mut h = Harness::new(false, Default::default());
    h.host.set_image(
        VerifiedPuzzleImage::verify(h.image.clone(), h.p.client.metadata().unwrap().definition)
            .unwrap(),
    );
    let fresh: Arc<[u8]> = Arc::from(h.image.as_ref());
    assert!(!Arc::ptr_eq(&h.image, &fresh));
    h.image = fresh;
    h.send_client(Control::ClientProfile { display_name: None })
        .unwrap();
    h.send_client(Control::ImageAvailability {
        image_hash: h.p.client.metadata().unwrap().definition.image_hash,
        available: false,
    })
    .unwrap();
    assert_eq!(h.host.phase(HA), Some(SyncPhase::ImageTransfer));
    h.host
        .pump(&h.p.host, HA, &mut h.p.ht, &authority(&h.s), h.p.now)
        .unwrap();
    assert_eq!(h.host.phase(HA), Some(SyncPhase::ImageTransfer));
    assert_eq!(h.host.resources().image_transfers, 1);
}

#[test]
fn session_image_negotiation_and_offers_have_independent_short_deadlines() {
    let mut h = Harness::new(true, Default::default());
    assert_expired_join(&mut h, ResponseWait::ImageAvailability);
    let mut h = Harness::new(true, Default::default());
    h.send_client(Control::ClientProfile { display_name: None })
        .unwrap();
    h.send_client(Control::ImageAvailability {
        image_hash: h.p.client.metadata().unwrap().definition.image_hash,
        available: true,
    })
    .unwrap();
    assert_expired_join(&mut h, ResponseWait::ImageReady);
    let mut h = Harness::new(false, Default::default());
    h.host_to_client().unwrap();
    h.client_to_host().unwrap();
    assert_expired_join(&mut h, ResponseWait::ImageAcceptance);
    let mut h = Harness::new(true, Default::default());
    h.host_to_client().unwrap();
    h.client_to_host().unwrap();
    assert_expired_join(&mut h, ResponseWait::BaselineAcceptance);
}

#[test]
fn completed_transfers_wait_for_application_ack_with_a_short_deadline() {
    for cached in [false, true] {
        let mut h = Harness::new(cached, Default::default());
        h.reach(if cached {
            SyncPhase::BaselineTransfer
        } else {
            SyncPhase::ImageTransfer
        });
        h.client_to_host().unwrap();
        // Transport reports real delivery, but no ImageReady/BaselineInstalled.
        for _ in 0..4 {
            h.host
                .pump(&h.p.host, HA, &mut h.p.ht, &authority(&h.s), h.p.now)
                .unwrap();
        }
        assert_expired_join(
            &mut h,
            if cached {
                ResponseWait::BaselineInstalled
            } else {
                ResponseWait::ImageReady
            },
        );
    }
}

#[test]
fn every_active_sync_phase_disconnect_releases_payloads_retention_and_candidates() {
    for (cached, phase) in [
        (false, SyncPhase::ImageNegotiation),
        (false, SyncPhase::ImageTransfer),
        (true, SyncPhase::BaselineTransfer),
        (true, SyncPhase::CatchingUp),
        (true, SyncPhase::Finalizing),
    ] {
        let mut h = Harness::new(cached, Default::default());
        let player = h.p.host.assigned_player(HA).unwrap();
        h.reach(phase);
        h.host.disconnect(HA);
        h.p.host.reject(
            HA,
            DisconnectReason::RemoteClosed,
            &mut h.p.ht,
            &mut h.p.host_connections,
        );
        assert_eq!(h.host.phase(HA), None);
        assert_eq!(h.host.transfer_binding(HA), None);
        assert_eq!(
            h.host.catch_up().status(player),
            Err(CatchUpError::NotJoining)
        );
        let resources = h.host.resources();
        assert_eq!(
            (
                resources.peers,
                resources.image_waiters,
                resources.baseline_waiters,
                resources.image_transfers,
                resources.baseline_transfers
            ),
            (0, 0, 0, 0, 0)
        );
        assert_eq!(
            Arc::strong_count(&h.image),
            if cached || phase == SyncPhase::ImageNegotiation {
                1
            } else {
                2
            }
        );
        assert!(!h.p.ht.has_channel(HA));
    }
}

#[test]
fn duplicate_catchup_and_obsolete_finalize_acks_cannot_extend_response_deadline() {
    let mut h = Harness::new(true, Default::default());
    h.reach(SyncPhase::CatchingUp);
    let generation = h.client.generation().unwrap();
    let acknowledged = h.s.peers[0].session.cursor();
    h.apply_grab();
    h.host
        .pump(&h.p.host, HA, &mut h.p.ht, &authority(&h.s), h.p.now)
        .unwrap();
    let entered = h.p.now;
    assert_eq!(
        h.host.timing(HA).unwrap().waiting_for(),
        Some(ResponseWait::CatchUpAck)
    );
    for second in [3, 7, 11] {
        h.p.now = entered + Duration::from_secs(second);
        h.send_client(Control::CatchUpAck {
            generation,
            cursor: acknowledged,
        })
        .unwrap();
    }
    h.p.now = entered;
    assert_expired_join(&mut h, ResponseWait::CatchUpAck);

    let mut h = Harness::new(true, Default::default());
    h.reach(SyncPhase::Finalizing);
    h.host
        .pump(&h.p.host, HA, &mut h.p.ht, &authority(&h.s), h.p.now)
        .unwrap();
    let entered = h.p.now;
    for second in [3, 7, 11] {
        h.p.now = entered + Duration::from_secs(second);
        h.send_client(Control::FinalizeAck {
            token: final_token(&h, 0),
        })
        .unwrap();
    }
    h.p.now = entered;
    assert_expired_join(&mut h, ResponseWait::FinalizeAck);
}

#[test]
fn invalidated_finalization_starts_catchup_deadline_only_after_sending() {
    for authority_event in [false, true] {
        let mut h = Harness::new(true, Default::default());
        if authority_event {
            h.apply_grab();
        }
        h.reach(SyncPhase::Finalizing);
        let entered = h.p.now;
        let progress = h.host.timing(HA).unwrap().last_peer_progress;
        assert_eq!(
            h.host.timing(HA).unwrap().waiting_for(),
            Some(ResponseWait::FinalizeAck)
        );
        h.p.now = entered + Duration::from_secs(11);
        // The client has received Finalize, but its ACK is still in flight.
        h.host_to_client().unwrap();
        if authority_event {
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
        } else {
            h.apply_grab();
        }
        assert_eq!(h.host.phase(HA), Some(SyncPhase::CatchingUp));
        assert_eq!(h.host.timing(HA).unwrap().waiting_for(), None);
        assert_eq!(h.host.timing(HA).unwrap().last_peer_progress, progress);
        h.client_to_host().unwrap(); // Obsolete FinalizeAck cannot renew progress.
        assert_eq!(h.host.timing(HA).unwrap().last_peer_progress, progress);
        assert_eq!(h.host.timeout(HA, entered + Duration::from_secs(12)), None);
        // Host scheduling can delay the send past the retired FinalizeAck deadline.
        h.p.now = entered + Duration::from_secs(13);
        h.host
            .pump(&h.p.host, HA, &mut h.p.ht, &authority(&h.s), h.p.now)
            .unwrap();
        assert_eq!(
            h.host.timing(HA).unwrap().waiting_for(),
            Some(ResponseWait::CatchUpAck)
        );
        assert_eq!(
            h.host.timeout(
                HA,
                h.p.now + Duration::from_secs(12) - Duration::from_nanos(1)
            ),
            None
        );
        assert_eq!(
            h.host.timeout(HA, h.p.now + Duration::from_secs(12)),
            Some(SyncError::PhaseTimeout(ResponseWait::CatchUpAck))
        );
        h.host_to_client().unwrap(); // Keep CatchUpAck in flight for 11 seconds.
        h.p.now += Duration::from_secs(11);
        h.reach(SyncPhase::Ready);
        assert_eq!(h.s.peers[0].session.cursor(), h.s.host.session.cursor());
    }
}

#[test]
fn baseline_wait_is_host_capacity_and_stalled_slot_is_immediately_reusable() {
    let mut h = BaselineSlots::new(4, Default::default());
    for n in 0..4 {
        h.ready(n);
    }
    let now = h.pairs[0].now + Duration::from_secs(12);
    assert_eq!(h.host.timeout(h.id(2), now), None);
    assert_eq!(h.host.timeout(h.id(3), now), None);
    let id = h.id(0);
    let player = h.player(0);
    let p = &mut h.pairs[0];
    assert_eq!(
        h.host.expire_connection(
            &mut SyncHost {
                roster: &mut p.host_roster,
                bootstrap: &mut p.host,
                connections: &mut p.host_connections
            },
            id,
            &mut p.ht,
            now
        ),
        Some(SyncError::PhaseTimeout(ResponseWait::BaselineAcceptance))
    );
    assert_eq!(h.host.active_baseline_transfers(), 1);
    assert_eq!(
        h.host.catch_up().status(player),
        Err(CatchUpError::NotJoining)
    );
    h.pump(3).unwrap();
    assert_eq!(h.host.phase(h.id(3)), Some(SyncPhase::AwaitingBaselineSlot));
    h.pump(2).unwrap();
    assert_eq!(h.host.active_baseline_transfers(), 2);
    assert_eq!(h.host.phase(h.id(2)), Some(SyncPhase::BaselineTransfer));
}

#[test]
fn local_sync_prerequisite_failures_preserve_admission_credit() {
    use crate::network::lifecycle::{JOIN_ADMISSION_BURST, ORIGIN_JOIN_BURST};
    for origin in [None, Some(Origin::Ip("192.0.2.21".parse().unwrap()))] {
        for failure in [
            "missing metadata",
            "changed scope",
            "frozen",
            "already syncing",
        ] {
            let mut p = Pair::new("correct password");
            p.ht.backend_mut().origin = origin;
            p.authenticate();
            let mut s = Scenario::new();
            let expected = match failure {
                "missing metadata" => {
                    p.host =
                        HostBootstrap::new(password("correct password"), metadata(), [], p.now);
                    SyncError::NotSyncing
                }
                "changed scope" => {
                    s.host.session = AuthoritySession::new(
                        SessionDefinition {
                            id: SessionId(999),
                            ..SESSION
                        },
                        HOST,
                        metadata().cursor,
                    );
                    SyncError::AuthorityChanged
                }
                "frozen" => {
                    s.host.session.begin_graceful(B).unwrap();
                    SyncError::AuthorityChanged
                }
                "already syncing" => {
                    p.host.begin_sync(p.host_connection).unwrap();
                    SyncError::Bootstrap(BootstrapError::InvalidTransition)
                }
                _ => unreachable!(),
            };
            let mut host =
                HostSyncCoordinator::new(JoinCatchUpCoordinator::new(Default::default()));
            let sent = p.ht.backend_mut().sent.len();
            let bootstrap_state = p.host.state(p.host_connection);
            // Keep the supplied clock fixed so rate-limit refill cannot hide a
            // token consumed by one of these local prerequisite refusals.
            for _ in 0..=JOIN_ADMISSION_BURST {
                assert_eq!(
                    host.start(
                        &mut p.host,
                        p.host_connection,
                        &mut p.ht,
                        &authority(&s),
                        p.now
                    )
                    .as_ref()
                    .err(),
                    Some(&expected),
                    "{failure}, origin {origin:?}"
                );
                assert_eq!(host.resources().peers, 0);
                assert_eq!(p.ht.backend_mut().sent.len(), sent);
                assert_eq!(p.host.state(p.host_connection), bootstrap_state);
            }
            let s = Scenario::new();
            let burst = if origin.is_some() {
                ORIGIN_JOIN_BURST
            } else {
                JOIN_ADMISSION_BURST
            };
            for attempt in 0..=burst {
                let mut healthy = Pair::new("correct password");
                healthy.now = p.now;
                healthy.host_connection = ConnectionId::new(10000 + attempt);
                healthy.ht.backend_mut().origin = origin;
                healthy.authenticate();
                let result = host.start(
                    &mut healthy.host,
                    healthy.host_connection,
                    &mut healthy.ht,
                    &authority(&s),
                    healthy.now,
                );
                if attempt < burst {
                    result.unwrap();
                    assert_eq!(
                        host.phase(healthy.host_connection),
                        Some(SyncPhase::ImageNegotiation)
                    );
                    assert_eq!(
                        healthy.host.state(healthy.host_connection),
                        Some(ConnectionState::Syncing)
                    );
                    host.disconnect(healthy.host_connection);
                } else {
                    assert_eq!(
                        result,
                        Err(SyncError::Admission(DisconnectReason::RateLimited))
                    );
                    assert_eq!(
                        healthy.host.state(healthy.host_connection),
                        Some(ConnectionState::Authenticated)
                    );
                }
            }
        }
    }
}

#[test]
fn authenticated_origin_flood_and_reconnect_cannot_acquire_all_join_slots() {
    let origin = Origin::Ip("192.0.2.17".parse().unwrap());
    let mut h = BaselineSlots::with_origin(
        crate::network::lifecycle::MAX_CONNECTIONS,
        Default::default(),
        Some(origin),
    );
    assert_eq!(
        h.host.resources().peers,
        crate::network::lifecycle::MAX_PENDING_PER_ORIGIN
    );
    assert_eq!(h.host.resources().retained_payload_bytes, 0);
    for n in 0..4 {
        h.host.disconnect(h.id(n));
    }
    let mut p = Pair::new("correct password");
    p.host_connection = ConnectionId::new(9999);
    p.ht.backend_mut().origin = Some(origin);
    p.authenticate();
    assert_eq!(
        h.host.start(
            &mut p.host,
            p.host_connection,
            &mut p.ht,
            &authority(&h.s),
            p.now
        ),
        Err(SyncError::Admission(DisconnectReason::RateLimited))
    );
    assert_eq!(h.host.resources().peers, 0);
    // A distinct origin may use the remaining admission/connection capacity.
    p.ht.backend_mut().origin = Some(Origin::Ip("192.0.2.18".parse().unwrap()));
    h.host
        .start(
            &mut p.host,
            p.host_connection,
            &mut p.ht,
            &authority(&h.s),
            p.now,
        )
        .unwrap();
    assert_eq!(h.host.resources().peers, 1);
}

#[test]
fn repeated_sync_timeouts_keep_origin_cooldown_across_authenticated_reconnects() {
    let origin = Origin::Ip("192.0.2.19".parse().unwrap());
    let mut h = BaselineSlots::with_origin(3, Default::default(), Some(origin));
    let expired_at = h.pairs.iter().map(|p| p.now).max().unwrap() + Duration::from_secs(12);
    for n in 0..3 {
        let id = h.id(n);
        let p = &mut h.pairs[n];
        assert_eq!(
            h.host.expire_connection(
                &mut SyncHost {
                    roster: &mut p.host_roster,
                    bootstrap: &mut p.host,
                    connections: &mut p.host_connections,
                },
                id,
                &mut p.ht,
                expired_at,
            ),
            Some(SyncError::PhaseTimeout(ResponseWait::ImageAvailability))
        );
    }
    assert_eq!(h.host.resources().peers, 0);
    let mut p = Pair::new("correct password");
    p.now = expired_at + Duration::from_secs(20);
    p.host_connection = ConnectionId::new(9998);
    p.ht.backend_mut().origin = Some(origin);
    p.authenticate();
    // Tokens have refilled, but a fresh authenticated connection cannot bypass
    // the retained 30-second cooldown from the preceding timed-out joins.
    assert_eq!(
        h.host.start(
            &mut p.host,
            p.host_connection,
            &mut p.ht,
            &authority(&h.s),
            p.now,
        ),
        Err(SyncError::Admission(DisconnectReason::RateLimited))
    );
    assert_eq!(h.host.resources().retained_payload_bytes, 0);
}

#[test]
fn client_stalled_host_releases_declared_bulk_budget_without_sleep() {
    let mut h = Harness::new(false, Default::default());
    h.host_to_client().unwrap();
    let binding = SyncTransferBinding {
        transfer_id: TransferId(1),
        kind: BulkTransferKind::PuzzleImage,
        total_size: MAX_PUZZLE_IMAGE_TRANSFER_BYTES,
        sha256: h.p.client.metadata().unwrap().definition.image_hash.0,
    };
    h.send_host(WireMessage::SyncControl(Control::ImageOffer(binding)))
        .unwrap();
    h.send_host(WireMessage::BulkTransfer(BulkTransferMessage::Start {
        transfer_id: binding.transfer_id,
        kind: binding.kind,
        total_size: binding.total_size,
        sha256: binding.sha256,
    }))
    .unwrap();
    assert_eq!(
        h.client.declared_in_flight_bytes(),
        MAX_PUZZLE_IMAGE_TRANSFER_BYTES
    );
    let peer = &mut h.s.peers[0];
    assert!(matches!(
        h.client.route(
            &mut h.p.client,
            &mut h.p.client_connections,
            &TransportEvent::Message {
                connection: CLIENT_HOST,
                class: MessageClass::Bulk,
                payload: Vec::new()
            },
            &mut h.p.ct,
            &mut SyncReplica {
                roster: &mut peer.roster,
                replica: &mut peer.replica,
                session: &mut peer.session,
                store: &mut peer.store
            },
            h.p.now + Duration::from_secs(30)
        ),
        Err(SyncError::BulkStalled)
    ));
    assert_eq!(h.client.declared_in_flight_bytes(), 0);
    assert!(!h.p.ct.has_channel(CLIENT_HOST));
}

#[test]
fn pump_observes_native_delivery_before_evaluating_bulk_idle_timeout() {
    let mut h = Harness::new(false, Default::default());
    h.reach(SyncPhase::ImageTransfer);
    h.client_to_host().unwrap();
    let now = h.p.now + Duration::from_secs(30);
    assert_eq!(h.host.timeout(HA, now), Some(SyncError::BulkStalled));
    // ACK/drain arrived since the last frame; the first current sample must win
    // over stale timing, even when callers use pump without a separate sweep.
    h.p.ht.backend_mut().egress = Some(ReliableEgress {
        bulk_delivered_bytes: h.image.len() as u64,
        ..Default::default()
    });
    h.host
        .pump(&h.p.host, HA, &mut h.p.ht, &authority(&h.s), now)
        .unwrap();
    assert_eq!(h.host.timing(HA).unwrap().bulk_progress_at(), Some(now));
}

#[test]
fn outbound_queue_blocks_before_chunk_allocation_and_enqueue_does_not_refresh_stall() {
    use crate::network::lifecycle::MAX_BULK_QUEUE_BYTES;
    let mut h = Harness::new(false, Default::default());
    h.reach(SyncPhase::ImageTransfer);
    h.client_to_host().unwrap();
    let delivered = h.p.ht.backend_mut().bulk_sent;
    let count = h.p.ht.backend_mut().sent.len();
    h.p.ht.backend_mut().egress = Some(ReliableEgress {
        queued_bytes: MAX_BULK_QUEUE_BYTES,
        bulk_queued_bytes: MAX_BULK_QUEUE_BYTES,
        bulk_delivered_bytes: delivered,
    });
    for second in 0..30 {
        h.host
            .pump(
                &h.p.host,
                HA,
                &mut h.p.ht,
                &authority(&h.s),
                h.p.now + Duration::from_secs(second),
            )
            .unwrap();
        assert_eq!(h.p.ht.backend_mut().sent.len(), count);
        assert!(h.p.ht.has_channel(HA));
    }
    let now = h.p.now + Duration::from_secs(30);
    assert_eq!(expire_join(&mut h, now), vec![(HA, SyncError::BulkStalled)]);
}

#[test]
fn image_slots_are_fifo_and_share_one_payload_even_at_connection_capacity() {
    let mut h = BaselineSlots::new(
        crate::network::lifecycle::MAX_CONNECTIONS,
        Default::default(),
    );
    let image: Arc<[u8]> = Arc::from(vec![17; 2 * 1024 * 1024]);
    let session = SessionDefinition {
        image_hash: ImageHash(Sha256::digest(&image).into()),
        ..SESSION
    };
    // The fixture has 64 authenticated channels but only the first 12 were admitted.
    // Reconstruct their immutable identity before sync using a matching fixture.
    h.host = HostSyncCoordinator::default();
    h.s.host.session = AuthoritySession::new(session, HOST, metadata().cursor);
    for (n, p) in h.pairs.iter_mut().enumerate() {
        p.host = HostBootstrap::new(
            password("correct password"),
            SessionMetadata {
                definition: session,
                ..metadata()
            },
            (0..100 + n as u64).map(PlayerId),
            p.now,
        );
        p.client = ClientBootstrap::new(password("correct password"), CLIENT_HOST);
        p.authenticate();
        let result = h.host.start(
            &mut p.host,
            p.host_connection,
            &mut p.ht,
            &authority(&h.s),
            p.now,
        );
        if n < MAX_PENDING_JOIN_SYNCS {
            result.unwrap();
        } else {
            assert_eq!(result, Err(SyncError::TooManyJoins));
        }
    }
    h.host
        .set_image(VerifiedPuzzleImage::verify(image.clone(), session).unwrap());
    for n in 0..MAX_PENDING_JOIN_SYNCS {
        h.control(n, Control::ClientProfile { display_name: None })
            .unwrap();
        h.control(
            n,
            Control::ImageAvailability {
                image_hash: session.image_hash,
                available: n >= MAX_PENDING_JOIN_SYNCS - 2,
            },
        )
        .unwrap();
        if n >= MAX_PENDING_JOIN_SYNCS - 2 {
            h.control(
                n,
                Control::ImageReady {
                    image_hash: session.image_hash,
                },
            )
            .unwrap();
        }
    }
    let resources = h.host.resources();
    assert_eq!(resources.image_transfers, 2);
    assert_eq!(resources.image_waiters, MAX_PENDING_JOIN_SYNCS - 4);
    assert_eq!(resources.baseline_transfers, 2);
    let baseline_bytes: u64 = (MAX_PENDING_JOIN_SYNCS - 2..MAX_PENDING_JOIN_SYNCS)
        .map(|n| h.host.transfer_binding(h.id(n)).unwrap().total_size)
        .sum();
    assert_eq!(
        resources.retained_payload_bytes,
        image.len() as u64 + baseline_bytes
    );
    assert_eq!(Arc::strong_count(&image), 4); // fixture + descriptor + two outbound Arc references.
    for n in 2..MAX_PENDING_JOIN_SYNCS - 2 {
        assert_eq!(h.host.phase(h.id(n)), Some(SyncPhase::AwaitingImageSlot));
        assert_eq!(h.host.transfer_binding(h.id(n)), None);
        assert_eq!(
            h.host.catch_up().status(h.player(n)),
            Err(CatchUpError::NotJoining)
        );
    }
    h.host.disconnect(h.id(0));
    h.pump(3).unwrap();
    assert_eq!(h.host.phase(h.id(3)), Some(SyncPhase::AwaitingImageSlot));
    h.pump(2).unwrap();
    assert_eq!(h.host.active_image_transfers(), 2);
    // Many pump calls at one frame/time cannot exceed the host-wide budget.
    for n in [1, 2] {
        h.control(
            n,
            Control::TransferAccepted {
                transfer_id: h.host.transfer_binding(h.id(n)).unwrap().transfer_id,
            },
        )
        .unwrap();
    }
    let now = h.pairs.iter().map(|p| p.now).max().unwrap();
    for _ in 0..100 {
        for n in [1, 2] {
            let p = &mut h.pairs[n];
            h.host
                .pump(&p.host, p.host_connection, &mut p.ht, &authority(&h.s), now)
                .unwrap();
        }
    }
    let bytes: usize = [1, 2]
        .iter()
        .map(|&n| {
            h.pairs[n]
                .ht
                .backend_mut()
                .sent
                .iter()
                .filter_map(|e| match e {
                    TransportEvent::Message {
                        class: MessageClass::Bulk,
                        payload,
                        ..
                    } => Some(payload.len()),
                    _ => None,
                })
                .sum::<usize>()
        })
        .sum();
    assert!(bytes as u64 <= crate::network::lifecycle::BULK_FRAME_BYTES);
    // A wrong-phase status cannot keep a waiting peer alive.
    h.control(
        4,
        Control::ImageReady {
            image_hash: session.image_hash,
        },
    )
    .unwrap_err();
    for n in 0..MAX_PENDING_JOIN_SYNCS {
        h.host.disconnect(h.id(n));
    }
    assert_eq!(h.host.resources().peers, 0);
    assert_eq!(Arc::strong_count(&image), 2);
}

fn final_token(h: &Harness, revision: u64) -> SyncFinalization {
    SyncFinalization {
        generation: h.client.generation().unwrap(),
        cursor: h.s.peers[0].session.cursor(),
        revision,
    }
}

fn authoritative_update(h: &mut Harness, tick: u64, delta: Vec2) {
    let mut cmd = update();
    cmd.player = B;
    cmd.sequence = ClientCommandSequence::Move {
        after_control_sequence: 0,
        tick,
    };
    cmd.command = ProtocolPieceCommand::DragUpdate { delta };
    // Deliberately omit record_drag_update: final capture must still see this.
    h.s.contexts
        .apply_replicated(
            &mut h.s.host.session,
            &mut h.s.host.store,
            B,
            &cmd,
            Some(&h.s.definition),
            HOST,
        )
        .unwrap();
}

#[test]
fn empty_final_barrier_commits_both_endpoints_and_releases_all_join_state() {
    let mut h = Harness::new(true, CatchUpLimits::default());
    let player = h.p.client.assigned_player().unwrap();
    h.reach(SyncPhase::Finalizing);
    assert_eq!(h.p.host_connections.player(HA), None);
    assert_eq!(h.p.client_connections.player(CLIENT_HOST), None);
    assert!(matches!(
        h.host_to_client().unwrap().pop(),
        Some(ClientSyncOutcome::Finalized)
    ));
    assert_eq!(h.p.client.state(), Some(ConnectionState::Syncing));
    h.client_to_host().unwrap();
    assert_eq!(h.p.host.state(HA), Some(ConnectionState::Ready));
    assert_eq!(h.p.host_connections.player(HA), Some(player));
    assert_eq!(h.p.client.state(), Some(ConnectionState::Syncing));
    assert_eq!(h.host.phase(HA), None);
    assert_eq!(
        h.host.catch_up().status(player),
        Err(CatchUpError::NotJoining)
    );
    assert_eq!(h.host.transfer_binding(HA), None);
    assert_eq!(h.host.active_baseline_transfers(), 0);
    assert!(matches!(
        h.host_to_client().unwrap().pop(),
        Some(ClientSyncOutcome::Ready)
    ));
    assert_eq!(h.p.client.state(), Some(ConnectionState::Ready));
    assert_eq!(h.p.client_connections.player(CLIENT_HOST), Some(HOST));
    assert_ne!(player, HOST);
    assert_eq!(h.client.phase(), SyncPhase::Ready);
    assert_eq!(h.client.baseline_cursor(), None);
    assert_eq!(h.client.generation(), None);
    assert_eq!(h.client.declared_in_flight_bytes(), 0);

    // Normal client Grab/Drag/Release routes using the actual promoted mapping.
    for mut cmd in [grab(), update(), release()] {
        cmd.player = player;
        let event = message_event(HA, &WireMessage::ClientCommand(cmd));
        assert_eq!(
            h.p.host
                .process(&event, &mut h.p.ht, &mut h.p.host_connections, h.p.now)
                .unwrap(),
            BootstrapOutcome::Gameplay
        );
        let mut router = HostRouter {
            local_player: HOST,
            connections: &h.p.host_connections,
            contexts: &mut h.s.contexts,
            session: &mut h.s.host.session,
            store: &mut h.s.host.store,
            definition: Some(&h.s.definition),
        };
        assert!(matches!(
            router.route(&event).unwrap(),
            HostRouteOutcome::Applied(_)
        ));
    }
    assert_eq!(h.s.host.session.cursor().sequence.0, 2);
}

#[test]
fn encrypted_transient_overtaking_ready_commit_drops_then_gameplay_resumes() {
    let mut h = Harness::new(true, CatchUpLimits::default());
    let player = h.p.client.assigned_player().unwrap();
    h.apply_grab();
    let final_delta = Vec2::new(12.0, 34.0);
    authoritative_update(&mut h, 0, final_delta);
    h.reach(SyncPhase::Finalizing);
    assert!(matches!(
        h.host_to_client().unwrap().pop(),
        Some(ClientSyncOutcome::Finalized)
    ));
    h.client_to_host().unwrap();
    assert_eq!(h.p.host.state(HA), Some(ConnectionState::Ready));
    assert_eq!(h.p.host_connections.player(HA), Some(player));
    assert_eq!(h.p.client.state(), Some(ConnectionState::Syncing));
    assert_eq!(h.p.client_connections.player(CLIENT_HOST), None);

    // Preserve the queued encrypted Control frame, but deliver a live Transient
    // first. SecureTransport must accept independent sequence numbers per lane.
    let mut ready_commit = std::mem::take(&mut h.p.ht.backend_mut().sent);
    assert_eq!(ready_commit.len(), 1);
    assert!(matches!(
        &ready_commit[0],
        TransportEvent::Message { class: MessageClass::Control, payload, .. }
            if wire::decode(payload).is_err()
    ));
    let cursor = h.s.peers[0].session.cursor();
    let states = h.s.peers[0].store.states.clone();
    for tick in [1, 2] {
        let delta = Vec2::splat(tick as f32 * 5.0);
        let mut cmd = update();
        cmd.player = B;
        cmd.sequence = ClientCommandSequence::Move {
            after_control_sequence: 0,
            tick,
        };
        cmd.command = ProtocolPieceCommand::DragUpdate { delta };
        let outcome =
            h.s.contexts
                .apply_replicated(
                    &mut h.s.host.session,
                    &mut h.s.host.store,
                    B,
                    &cmd,
                    Some(&h.s.definition),
                    HOST,
                )
                .unwrap();
        let router = HostRouter {
            local_player: HOST,
            connections: &h.p.host_connections,
            contexts: &mut h.s.contexts,
            session: &mut h.s.host.session,
            store: &mut h.s.host.store,
            definition: Some(&h.s.definition),
        };
        assert!(router
            .publish(&mut h.p.ht, Some(HB), &outcome)
            .unwrap()
            .is_empty());
        let packets = std::mem::take(&mut h.p.ht.backend_mut().sent);
        assert_eq!(packets.len(), 1);
        assert!(matches!(
            &packets[0],
            TransportEvent::Message { class: MessageClass::Transient, payload, .. }
                if wire::decode(payload).is_err()
        ));
        h.p.ct
            .backend_mut()
            .inbox
            .extend(packets.into_iter().map(|e| remap(e, CLIENT_HOST)));
        let mut events = Vec::new();
        h.p.ct.poll(&mut events).unwrap();
        assert_eq!(events.len(), 1);
        let event = &events[0];
        assert!(matches!(event,
            TransportEvent::Message { class, payload, .. }
                if matches!(wire::decode_for_class(payload, *class).unwrap(), WireMessage::DragUpdate(update)
                    if update.player == B && update.tick == tick && update.delta == delta)
        ));
        let routed =
            h.p.client
                .process(event, &mut h.p.ct, &mut h.p.client_connections, h.p.now)
                .unwrap();
        assert_eq!(h.p.client.failure(), None);
        assert!(h.p.ct.has_channel(CLIENT_HOST));
        if tick == 1 {
            assert_eq!(routed, BootstrapOutcome::Consumed);
            assert_eq!(h.p.client.state(), Some(ConnectionState::Syncing));
            assert_eq!(h.p.client_connections.player(CLIENT_HOST), None);
            let peer = &h.s.peers[0];
            let drag = peer
                .replica
                .remote_drag(&peer.session, &peer.store, B)
                .unwrap();
            assert_eq!(drag.last_tick, Some(0));
            assert_eq!(drag.delta, final_delta);
            assert_eq!(peer.store.states, states);
            assert_eq!(peer.session.cursor(), cursor);

            h.p.ht.backend_mut().sent.append(&mut ready_commit);
            assert!(matches!(
                h.host_to_client().unwrap().pop(),
                Some(ClientSyncOutcome::Ready)
            ));
            assert_eq!(h.p.client.state(), Some(ConnectionState::Ready));
            assert_eq!(h.p.client_connections.player(CLIENT_HOST), Some(HOST));
        } else {
            assert_eq!(routed, BootstrapOutcome::Gameplay);
            let peer = &mut h.s.peers[0];
            let mut router = ClientRouter {
                roster: &mut peer.roster,
                local_player: player,
                host_connection: CLIENT_HOST,
                connections: &h.p.client_connections,
                replica: &mut peer.replica,
                session: &mut peer.session,
                store: &mut peer.store,
                definition: Some(&h.s.definition),
            };
            assert!(matches!(
                router.route(event).unwrap(),
                ClientRouteOutcome::Drag(_)
            ));
            let drag = peer
                .replica
                .remote_drag(&peer.session, &peer.store, B)
                .unwrap();
            assert_eq!(drag.last_tick, Some(tick));
            assert_eq!(drag.delta, delta);
            assert_eq!(peer.session.cursor(), cursor);
        }
    }
}

#[test]
fn final_drags_reconcile_scalar_rollback_and_multiple_players_without_replaying_targets() {
    let mut h = Harness::new(true, CatchUpLimits::default());
    h.apply_grab();
    let mut cmd = grab();
    cmd.player = HOST;
    cmd.command = ProtocolPieceCommand::Grab {
        target: PieceTarget::Component(ComponentRef {
            member: PieceId(1),
            expected_size: 1,
        }),
    };
    let outcome =
        h.s.contexts
            .apply_replicated(
                &mut h.s.host.session,
                &mut h.s.host.store,
                HOST,
                &cmd,
                Some(&h.s.definition),
                HOST,
            )
            .unwrap();
    h.host
        .record_command_outcome(&h.s.host.session, &h.s.host.store, &outcome)
        .unwrap();
    authoritative_update(&mut h, 3, Vec2::new(12.0, 34.0));
    h.reach(SyncPhase::Finalizing);
    let peer = &mut h.s.peers[0];
    peer.replica
        .apply_drag_update(
            &peer.session,
            &peer.store,
            HOST,
            &RemoteDragUpdate {
                session: peer.session.session_id(),
                authority_epoch: peer.session.cursor().epoch,
                player: B,
                grab_sequence: 0,
                basis_sequence: 0,
                tick: 99,
                delta: Vec2::splat(999.0),
            },
        )
        .unwrap();
    h.reach(SyncPhase::Ready);
    let peer = &h.s.peers[0];
    let drag = peer
        .replica
        .remote_drag(&peer.session, &peer.store, B)
        .unwrap();
    assert_eq!(drag.last_tick, Some(3));
    assert_eq!(drag.delta, Vec2::new(12.0, 34.0));
    assert_eq!(
        peer.replica
            .remote_drags(&peer.session, &peer.store)
            .count(),
        2
    );
    assert_eq!(peer.store.states, h.s.host.store.states);
}

#[test]
fn transient_race_retries_fresh_revision_without_reliable_replay_or_hook() {
    let mut h = Harness::new(true, CatchUpLimits::default());
    h.apply_grab();
    h.reach(SyncPhase::Finalizing);
    let old = final_token(&h, 1);
    h.host_to_client().unwrap();
    authoritative_update(&mut h, 7, Vec2::splat(15.0));
    h.client_to_host().unwrap();
    assert_eq!(h.p.host.state(HA), Some(ConnectionState::Syncing));
    assert_eq!(h.host.phase(HA), Some(SyncPhase::Finalizing));
    assert_eq!(h.s.host.session.cursor(), old.cursor);
    h.host
        .pump(&h.p.host, HA, &mut h.p.ht, &authority(&h.s), h.p.now)
        .unwrap();
    h.send_client(Control::FinalizeAck { token: old }).unwrap();
    assert_eq!(h.p.host_connections.player(HA), None);
    h.reach(SyncPhase::Ready);
    let peer = &h.s.peers[0];
    let drag = peer
        .replica
        .remote_drag(&peer.session, &peer.store, B)
        .unwrap();
    assert_eq!(drag.last_tick, Some(7));
    assert_eq!(drag.delta, Vec2::splat(15.0));
}

#[test]
fn reliable_race_obsoletes_ack_then_catches_up_and_finalizes_again() {
    let mut h = Harness::new(true, CatchUpLimits::default());
    h.reach(SyncPhase::Finalizing);
    let old = final_token(&h, 1);
    h.host_to_client().unwrap();
    h.apply_grab();
    assert_eq!(h.host.phase(HA), Some(SyncPhase::CatchingUp));
    h.client_to_host().unwrap();
    assert_eq!(h.p.host_connections.player(HA), None);
    h.reach(SyncPhase::CatchingUp);
    h.reach(SyncPhase::Finalizing);
    h.send_client(Control::FinalizeAck { token: old }).unwrap();
    assert_eq!(h.p.host.state(HA), Some(ConnectionState::Syncing));
    h.reach(SyncPhase::Ready);
    assert_eq!(h.s.peers[0].session.cursor(), h.s.host.session.cursor());
}

#[test]
fn final_full_set_mismatch_never_acks_or_promotes_and_is_transactional() {
    for fault in 0..7 {
        let mut h = Harness::new(true, CatchUpLimits::default());
        h.apply_grab();
        h.reach(SyncPhase::Finalizing);
        // Keep secure Control sequence continuity, then submit a newer bad set.
        h.host_to_client().unwrap();
        h.p.ct.backend_mut().sent.clear();
        let mut drags =
            FinalDragSet::capture(&h.s.contexts, &h.s.host.session, &h.s.host.store).unwrap();
        drags.entries[0].delta = Vec2::splat(222.0);
        match fault {
            0 => drags.entries.clear(),
            1 => {
                let mut extra = drags.entries[0];
                extra.player = PlayerId(99);
                drags.entries.push(extra);
            }
            2 => {
                drags.entries[0].grab_sequence += 1;
                drags.entries[0].basis_sequence += 1;
            }
            3 => drags.entries[0].basis_sequence += 1,
            4 => drags.entries.push(drags.entries[0]),
            5 => drags.entries[0].delta.x = f32::NAN,
            _ => drags.entries[0].player = HOST,
        }
        let before = h.s.peers[0]
            .replica
            .remote_drag(&h.s.peers[0].session, &h.s.peers[0].store, B)
            .unwrap()
            .delta;
        let token = final_token(&h, 2);
        assert!(matches!(
            h.send_host(WireMessage::SyncControl(Control::Finalize { token, drags })),
            Err(SyncError::FinalDrag(_))
        ));
        assert!(h.p.ct.backend_mut().sent.is_empty());
        assert_eq!(h.p.client_connections.player(CLIENT_HOST), None);
        assert_eq!(h.client.phase(), SyncPhase::RestartRequired);
        assert_eq!(
            h.s.peers[0]
                .replica
                .remote_drag(&h.s.peers[0].session, &h.s.peers[0].store, B)
                .unwrap()
                .delta,
            before
        );
        assert_eq!(h.p.host_connections.player(HA), None);
    }
}

#[test]
fn ready_commit_send_or_host_registration_failure_rolls_back_and_drops_join_state() {
    for registration_failure in [false, true] {
        let mut h = Harness::new(true, CatchUpLimits::default());
        let player = h.p.client.assigned_player().unwrap();
        h.reach(SyncPhase::Finalizing);
        h.host_to_client().unwrap();
        if registration_failure {
            h.p.host_connections.observe(&TransportEvent::Disconnected {
                connection: HA,
                reason: DisconnectReason::Requested,
            });
        } else {
            h.p.ht.backend_mut().fail = Some(HA);
        }
        assert!(h.client_to_host().is_err());
        assert_eq!(h.p.host_connections.player(HA), None);
        assert_eq!(h.p.host.state(HA), None);
        assert_eq!(h.host.phase(HA), None);
        assert_eq!(
            h.host.catch_up().status(player),
            Err(CatchUpError::NotJoining)
        );
        assert_eq!(h.p.client.state(), Some(ConnectionState::Syncing));
        assert!(h.p.ht.backend_mut().sent.is_empty());
        assert_eq!(h.p.host_roster.revision(), 0);
        assert_eq!(h.p.host_roster.len(), 1);
    }
}

#[test]
fn stale_generation_ack_cannot_complete_restarted_finalization() {
    let mut h = Harness::new(true, CatchUpLimits::default());
    h.reach(SyncPhase::Finalizing);
    let old = final_token(&h, 1);
    h.host_to_client().unwrap();
    h.s.host.store.epoch += 1;
    assert!(h
        .host
        .observe_host_state(&h.s.host.session, &h.s.host.store)
        .is_err());
    h.host
        .restart(&h.p.host, HA, &mut h.p.ht, &authority(&h.s), h.p.now)
        .unwrap();
    h.host_to_client().unwrap();
    h.client_to_host().unwrap(); // Includes delayed generation-0 final ACK.
    assert_eq!(h.p.host_connections.player(HA), None);
    h.reach(SyncPhase::Finalizing);
    assert_eq!(h.client.generation(), Some(1));
    h.send_client(Control::FinalizeAck { token: old }).unwrap();
    assert_eq!(h.p.host_connections.player(HA), None);
    assert_eq!(h.p.host_roster.revision(), 0);
    assert_eq!(h.p.host_roster.len(), 1);
    h.reach(SyncPhase::Ready);
}

#[test]
fn client_profile_is_mandatory_once_in_secure_sync_and_binds_reserved_identity() {
    let mut h = Harness::new(true, Default::default());
    let hash = h.p.client.metadata().unwrap().definition.image_hash;
    assert_eq!(
        h.send_client(Control::ImageAvailability {
            image_hash: hash,
            available: true
        }),
        Err(SyncError::WrongPhase)
    );
    assert_eq!(h.p.host_roster.revision(), 0);
    let mut h = Harness::new(true, Default::default());
    h.send_client(Control::ClientProfile { display_name: None })
        .unwrap();
    assert_eq!(h.p.host_roster.len(), 1);
    assert_eq!(
        h.send_client(Control::ClientProfile { display_name: None }),
        Err(SyncError::WrongPhase)
    );
    assert_eq!(h.p.host_roster.revision(), 0);
    let name = jigsall_core::PlayerDisplayName::from_user_input("日本語 🧩").unwrap();
    let mut h = Harness::named(true, Default::default(), Some(name.clone()));
    let player = h.p.host.assigned_player(HA).unwrap();
    assert!(h.p.host_roster.get(player).is_none());
    h.reach(SyncPhase::Ready);
    assert_eq!(h.p.host_roster, h.s.peers[0].roster);
    assert_eq!(
        h.p.host_roster.get(player).unwrap().display_name,
        Some(name)
    );
    assert!(h.p.host_roster.get(HOST).is_some());
    assert!(h
        .p
        .host_roster
        .get(h.p.client.assigned_player().unwrap())
        .is_some());
}

#[test]
fn final_ack_cannot_cross_host_scope_or_freeze_changes() {
    for change in 0..5 {
        let mut h = Harness::new(true, CatchUpLimits::default());
        h.reach(SyncPhase::Finalizing);
        h.host_to_client().unwrap();
        match change {
            0 => h.s.host.store.epoch += 1,
            1 => h.s.host.session.begin_graceful(B).unwrap(),
            _ => {
                let mut definition = h.s.host.session.session_definition();
                let mut cursor = h.s.host.session.cursor();
                let mut host = HOST;
                match change {
                    2 => definition.id = SessionId(999),
                    3 => cursor.epoch = AuthorityEpoch(999),
                    _ => host = B,
                }
                h.s.host.session = AuthoritySession::new(definition, host, cursor);
            }
        }
        let result = h.client_to_host();
        if change < 2 {
            result.unwrap();
            assert_eq!(h.host.phase(HA), Some(SyncPhase::RestartRequired));
        } else {
            assert_eq!(result, Err(SyncError::AuthorityChanged));
        }
        assert_eq!(h.p.host_connections.player(HA), None);
        assert_eq!(h.p.client_connections.player(CLIENT_HOST), None);
    }
}

#[test]
fn final_ack_send_and_client_commit_failures_never_leave_client_registered() {
    for failure in 0..4 {
        let mut h = Harness::new(true, CatchUpLimits::default());
        h.reach(SyncPhase::Finalizing);
        if failure == 0 {
            h.p.ct.backend_mut().fail = Some(CLIENT_HOST);
        } else {
            h.host_to_client().unwrap();
            h.client_to_host().unwrap();
            match failure {
                1 => {
                    h.p.client_connections
                        .observe(&TransportEvent::Disconnected {
                            connection: CLIENT_HOST,
                            reason: DisconnectReason::Requested,
                        });
                }
                2 => h.s.peers[0].store.epoch += 1,
                _ => h.s.peers[0].session.begin_graceful(B).unwrap(),
            }
        }
        assert!(h.host_to_client().is_err());
        assert_eq!(h.p.client.state(), None);
        assert_eq!(h.p.client_connections.player(CLIENT_HOST), None);
        assert_eq!(h.client.phase(), SyncPhase::RestartRequired);
        // Observe the remote close using the same runtime lifecycle contract.
        h.p.host
            .process(
                &TransportEvent::Disconnected {
                    connection: HA,
                    reason: DisconnectReason::RemoteClosed,
                },
                &mut h.p.ht,
                &mut h.p.host_connections,
                h.p.now,
            )
            .unwrap();
        h.host.disconnect(HA);
        assert_eq!(h.p.host_connections.player(HA), None);
    }
}

#[test]
fn actual_ready_handoff_rejects_sync_control_and_bulk_on_both_sides() {
    for host_side in [true, false] {
        for message in [
            bulk_chunk(vec![0]),
            WireMessage::SyncControl(Control::Restart { generation: 0 }),
        ] {
            let mut h = Harness::new(true, CatchUpLimits::default());
            h.reach(SyncPhase::Ready);
            let result = if host_side {
                h.p.host.process(
                    &message_event(HA, &message),
                    &mut h.p.ht,
                    &mut h.p.host_connections,
                    h.p.now,
                )
            } else {
                h.p.client.process(
                    &message_event(CLIENT_HOST, &message),
                    &mut h.p.ct,
                    &mut h.p.client_connections,
                    h.p.now,
                )
            };
            assert_eq!(
                result,
                Err(BootstrapError::Rejected(
                    DisconnectReason::ProtocolViolation
                ))
            );
        }
    }
}

#[test]
fn unissued_or_inconsistent_final_ack_cannot_mint_ready_capability() {
    for fault in 0..3 {
        let mut h = Harness::new(true, CatchUpLimits::default());
        h.reach(SyncPhase::Finalizing);
        let mut token = final_token(&h, 1);
        match fault {
            0 => token.revision = u64::MAX,
            1 => token.cursor.sequence.0 += 1,
            _ => token.generation += 1,
        }
        let result = h.send_client(Control::FinalizeAck { token });
        assert_eq!(
            result,
            Err(if fault == 2 {
                SyncError::WrongGeneration
            } else {
                SyncError::WrongFinalization
            })
        );
        assert_eq!(h.p.host_connections.player(HA), None);
        assert_eq!(h.p.client_connections.player(CLIENT_HOST), None);
        assert_eq!(h.host.phase(HA), None);
        assert_eq!(h.p.host_roster.revision(), 0);
        assert_eq!(h.p.host_roster.len(), 1);
    }
}

#[test]
fn ready_snapshot_is_validated_before_client_registration_and_transactional_replace() {
    use crate::players::RosterPlayer;
    for fault in 0..5 {
        let mut h = Harness::new(true, Default::default());
        h.reach(SyncPhase::Finalizing);
        h.host_to_client().unwrap(); // Finalize reconciled; client has sent its ACK.
        h.client_to_host().unwrap();
        let messages = std::mem::take(&mut h.p.ht.backend_mut().sent);
        h.p.ct
            .backend_mut()
            .inbox
            .extend(messages.into_iter().map(|e| remap(e, CLIENT_HOST)));
        let mut events = Vec::new();
        h.p.ct.poll(&mut events).unwrap();
        // Use the actual accepted finalization token, independent of pump revisions.
        let (token, mut snapshot) = events
            .into_iter()
            .find_map(|event| {
                let TransportEvent::Message { class, payload, .. } = event else {
                    return None;
                };
                match wire::decode_for_class(&payload, class).unwrap() {
                    WireMessage::SyncControl(Control::ReadyCommit { token, roster }) => {
                        Some((token, roster))
                    }
                    _ => None,
                }
            })
            .expect("host enqueued ReadyCommit");
        let self_id = h.p.client.assigned_player().unwrap();
        match fault {
            0 => snapshot.players.reverse(),
            1 => snapshot.players.push(RosterPlayer {
                player: self_id,
                display_name: None,
            }),
            2 => {
                snapshot.players.remove(0);
            }
            3 => {
                snapshot.players.pop();
            }
            _ => snapshot.revision = 0,
        }
        let peer = &mut h.s.peers[0];
        let before = peer.roster.snapshot();
        let result = h.client.route(
            &mut h.p.client,
            &mut h.p.client_connections,
            &message_event(
                CLIENT_HOST,
                &WireMessage::SyncControl(Control::ReadyCommit {
                    token,
                    roster: snapshot,
                }),
            ),
            &mut h.p.ct,
            &mut SyncReplica {
                roster: &mut peer.roster,
                replica: &mut peer.replica,
                session: &mut peer.session,
                store: &mut peer.store,
            },
            h.p.now,
        );
        assert!(
            matches!(result, Err(SyncError::Roster(_))),
            "fault {fault}: {:?}",
            result.err()
        );
        assert_eq!(peer.roster.snapshot(), before);
        assert_eq!(h.p.client_connections.player(CLIENT_HOST), None);
        assert_eq!(h.client.phase(), SyncPhase::RestartRequired);
    }
}

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
        Self::named(cached, limits, None)
    }
    fn named(
        cached: bool,
        limits: CatchUpLimits,
        display_name: Option<jigsall_core::PlayerDisplayName>,
    ) -> Self {
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
        let client = ClientSyncRouter::start(
            &mut p.client,
            cached.then_some(image.as_ref()),
            display_name,
            p.now,
        )
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
                &mut self.p.client,
                &mut self.p.client_connections,
                &event,
                &mut self.p.ct,
                &mut SyncReplica {
                    roster: &mut peer.roster,
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
                &mut SyncHost {
                    roster: &mut self.p.host_roster,
                    bootstrap: &mut self.p.host,
                    connections: &mut self.p.host_connections,
                },
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
        if self.host.phase(HA).is_none() {
            return;
        }
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
    // Image preparation retains zero authority events within its finite deadline.
    h.p.now += Duration::from_secs(2);
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

/// Independent authenticated channels share one authority-wide slot scheduler.
struct BaselineSlots {
    host: HostSyncCoordinator,
    s: Scenario,
    pairs: Vec<Pair>,
}
impl BaselineSlots {
    fn new(count: usize, limits: CatchUpLimits) -> Self {
        Self::with_origin(count, limits, None)
    }
    fn with_origin(count: usize, limits: CatchUpLimits, origin: Option<Origin>) -> Self {
        let s = Scenario::new();
        let mut host = HostSyncCoordinator::new(JoinCatchUpCoordinator::new(limits));
        let mut pairs = Vec::new();
        for index in 0..count {
            let mut p = Pair::new("correct password");
            p.host_connection = ConnectionId::new(1000 + index as u64);
            p.ht.backend_mut().origin = origin;
            let player = 100 + index as u64;
            p.host = HostBootstrap::new(
                password("correct password"),
                metadata(),
                (0..player).map(PlayerId),
                p.now,
            );
            p.authenticate();
            assert_eq!(
                p.host.assigned_player(p.host_connection),
                Some(PlayerId(player))
            );
            let admission = host.start(
                &mut p.host,
                p.host_connection,
                &mut p.ht,
                &authority(&s),
                p.now,
            );
            if index
                < if origin.is_some() {
                    crate::network::lifecycle::MAX_PENDING_PER_ORIGIN
                } else {
                    MAX_PENDING_JOIN_SYNCS
                }
            {
                admission.unwrap();
            } else {
                assert_eq!(
                    admission,
                    Err(if origin.is_some() {
                        SyncError::Admission(DisconnectReason::JoinCapacity)
                    } else {
                        SyncError::TooManyJoins
                    })
                );
            }
            // Host-side scheduling tests drive validated image availability/ACKs
            // through the real encrypted channel; client installation is covered
            // by the full join harness above.
            pairs.push(p);
        }
        Self { host, s, pairs }
    }
    fn id(&self, index: usize) -> ConnectionId {
        self.pairs[index].host_connection
    }
    fn player(&self, index: usize) -> PlayerId {
        self.pairs[index]
            .host
            .assigned_player(self.id(index))
            .unwrap()
    }
    fn control(&mut self, index: usize, control: Control) -> Result<(), SyncError> {
        let p = &mut self.pairs[index];
        let message = WireMessage::SyncControl(control);
        p.ct.send(
            CLIENT_HOST,
            message.class(),
            &wire::encode(&message).unwrap(),
        )
        .unwrap();
        let packets = std::mem::take(&mut p.ct.backend_mut().sent);
        p.ht.backend_mut()
            .inbox
            .extend(packets.into_iter().map(|e| remap(e, p.host_connection)));
        let mut events = Vec::new();
        p.ht.poll(&mut events).unwrap();
        for event in events {
            assert_eq!(
                p.host
                    .process(&event, &mut p.ht, &mut p.host_connections, p.now)
                    .unwrap(),
                BootstrapOutcome::Syncing
            );
            self.host.route(
                &mut SyncHost {
                    roster: &mut p.host_roster,
                    bootstrap: &mut p.host,
                    connections: &mut p.host_connections,
                },
                &event,
                &mut p.ht,
                &authority(&self.s),
                None,
                p.now,
            )?;
        }
        Ok(())
    }
    fn ready(&mut self, index: usize) {
        self.control(index, Control::ClientProfile { display_name: None })
            .unwrap();
        self.control(
            index,
            Control::ImageAvailability {
                image_hash: SESSION.image_hash,
                available: true,
            },
        )
        .unwrap();
        self.control(
            index,
            Control::ImageReady {
                image_hash: SESSION.image_hash,
            },
        )
        .unwrap();
    }
    fn pump(&mut self, index: usize) -> Result<(), SyncError> {
        let p = &mut self.pairs[index];
        self.host.pump(
            &p.host,
            p.host_connection,
            &mut p.ht,
            &authority(&self.s),
            p.now,
        )
    }
    fn finish(&mut self, index: usize) {
        let transfer_id = self
            .host
            .transfer_binding(self.id(index))
            .unwrap()
            .transfer_id;
        self.control(index, Control::TransferAccepted { transfer_id })
            .unwrap();
        // The two-piece baseline fits one chunk.
        for _ in 0..3 {
            self.pump(index).unwrap();
        }
    }
    fn installed(&mut self, index: usize) {
        let status = self.host.catch_up().status(self.player(index)).unwrap();
        let transfer_id = self
            .host
            .transfer_binding(self.id(index))
            .unwrap()
            .transfer_id;
        self.control(
            index,
            Control::BaselineInstalled {
                generation: status.generation,
                cursor: status.baseline_cursor,
                transfer_id,
            },
        )
        .unwrap();
    }
    fn restart(&mut self, index: usize) {
        let p = &mut self.pairs[index];
        self.host
            .restart(
                &p.host,
                p.host_connection,
                &mut p.ht,
                &authority(&self.s),
                p.now,
            )
            .unwrap();
    }
    fn apply_grab(&mut self) {
        let mut request = grab();
        request.player = B;
        let outcome = self
            .s
            .contexts
            .apply_replicated(
                &mut self.s.host.session,
                &mut self.s.host.store,
                B,
                &request,
                Some(&self.s.definition),
                HOST,
            )
            .unwrap();
        self.host
            .record_command_outcome(&self.s.host.session, &self.s.host.store, &outcome)
            .unwrap();
    }
}

#[test]
fn syncing_capacity_captures_only_two_baselines_and_waits_without_history() {
    let mut h = BaselineSlots::new(MAX_PENDING_JOIN_SYNCS + 1, CatchUpLimits::default());
    // The fixture also verifies that the first excess authenticated admission is rejected.
    for index in 0..MAX_PENDING_JOIN_SYNCS {
        h.ready(index);
    }
    assert_eq!(
        h.host.active_baseline_transfers(),
        MAX_CONCURRENT_BASELINE_TRANSFERS
    );
    for index in MAX_CONCURRENT_BASELINE_TRANSFERS..MAX_PENDING_JOIN_SYNCS {
        assert_eq!(
            h.host.phase(h.id(index)),
            Some(SyncPhase::AwaitingBaselineSlot)
        );
        assert!(h.host.transfer_binding(h.id(index)).is_none());
        assert_eq!(
            h.host.catch_up().status(h.player(index)),
            Err(CatchUpError::NotJoining)
        );
        assert!(h
            .host
            .timing(h.id(index))
            .unwrap()
            .bulk_progress_at()
            .is_none());
    }
    h.apply_grab();
    for index in MAX_CONCURRENT_BASELINE_TRANSFERS..MAX_PENDING_JOIN_SYNCS {
        h.pump(index).unwrap();
        assert_eq!(
            h.host.catch_up().status(h.player(index)),
            Err(CatchUpError::NotJoining)
        );
    }
    // Finish releases host bytes, but the slot stays occupied until install ACK.
    h.finish(0);
    h.pump(2).unwrap();
    assert_eq!(h.host.phase(h.id(2)), Some(SyncPhase::AwaitingBaselineSlot));
    h.installed(0);
    // Pumping a later waiter first must not let it bypass FIFO.
    h.pump(3).unwrap();
    assert_eq!(h.host.phase(h.id(3)), Some(SyncPhase::AwaitingBaselineSlot));
    h.pump(2).unwrap();
    let status = h.host.catch_up().status(h.player(2)).unwrap();
    assert_eq!(status.baseline_cursor, h.s.host.session.cursor());
    assert_eq!(status.retained_events, 0);
    assert_eq!(h.host.active_baseline_transfers(), 2);
    // Disconnecting both a queued head and an active sender reuses the slot.
    h.host.disconnect(h.id(3));
    h.host.disconnect(h.id(1));
    h.pump(4).unwrap();
    assert_eq!(h.host.phase(h.id(4)), Some(SyncPhase::BaselineTransfer));
    assert_eq!(h.host.active_baseline_transfers(), 2);
}

#[test]
fn restarted_baselines_requeue_without_retention_or_bypassing_waiters() {
    let mut h = BaselineSlots::new(
        4,
        CatchUpLimits {
            max_events: 0,
            ..CatchUpLimits::default()
        },
    );
    for index in 0..4 {
        h.ready(index);
    }
    let old = h.host.transfer_binding(h.id(0)).unwrap().transfer_id;
    h.apply_grab();
    assert_eq!(h.host.active_baseline_transfers(), 0);
    h.restart(0);
    assert_eq!(h.host.phase(h.id(0)), Some(SyncPhase::AwaitingBaselineSlot));
    assert_eq!(h.host.timing(h.id(0)).unwrap().waiting_for(), None);
    assert_eq!(
        h.host
            .timeout(h.id(0), h.pairs[0].now + Duration::from_secs(20)),
        None
    );
    let status = h.host.catch_up().status(h.player(0)).unwrap();
    assert!(matches!(status.phase, JoinCatchUpPhase::RestartRequired(_)));
    assert_eq!(status.retained_events, 0);
    assert_eq!(status.retained_bytes, 0);
    h.control(0, Control::TransferAccepted { transfer_id: old })
        .unwrap();
    h.control(
        0,
        Control::BaselineInstalled {
            generation: 0,
            cursor: AuthorityCursor::new(3, 0),
            transfer_id: old,
        },
    )
    .unwrap();
    h.pump(0).unwrap();
    assert!(h.host.transfer_binding(h.id(0)).is_none());
    h.pump(2).unwrap();
    h.pump(3).unwrap();
    assert_eq!(h.host.active_baseline_transfers(), 2);
    h.finish(2);
    h.installed(2);
    h.pump(0).unwrap();
    let status = h.host.catch_up().status(h.player(0)).unwrap();
    assert_eq!(status.generation, 1);
    assert_eq!(status.baseline_cursor, h.s.host.session.cursor());
    assert!(h.host.transfer_binding(h.id(0)).unwrap().transfer_id > old);
    assert_eq!(h.host.active_baseline_transfers(), 2);
}

#[test]
fn waiting_baseline_rechecks_scope_and_frozen_authority_before_capture() {
    let mut h = BaselineSlots::new(3, CatchUpLimits::default());
    for index in 0..3 {
        h.ready(index);
    }
    h.host.disconnect(h.id(0));
    h.s.host.session.begin_graceful(B).unwrap();
    assert_eq!(h.pump(2), Err(SyncError::AuthorityChanged));
    assert_eq!(
        h.host.catch_up().status(h.player(2)),
        Err(CatchUpError::NotJoining)
    );
    assert!(h.host.transfer_binding(h.id(2)).is_none());
    assert_eq!(h.host.phase(h.id(2)), Some(SyncPhase::AwaitingBaselineSlot));

    let mut h = BaselineSlots::new(3, CatchUpLimits::default());
    for index in 0..3 {
        h.ready(index);
    }
    h.host.disconnect(h.id(0));
    h.s.host.session = AuthoritySession::new(
        SessionDefinition {
            id: SessionId(999),
            ..SESSION
        },
        HOST,
        AuthorityCursor::new(3, 0),
    );
    assert_eq!(h.pump(2), Err(SyncError::AuthorityChanged));
    assert_eq!(
        h.host.catch_up().status(h.player(2)),
        Err(CatchUpError::NotJoining)
    );
    assert!(h.host.transfer_binding(h.id(2)).is_none());
}

#[test]
fn baseline_offer_failure_keeps_its_slot_bounded_until_teardown() {
    let mut h = BaselineSlots::new(4, CatchUpLimits::default());
    for index in 0..4 {
        h.ready(index);
    }
    h.host.disconnect(h.id(0));
    let id = h.id(2);
    h.pairs[2].ht.backend_mut().fail = Some(id);
    assert!(matches!(h.pump(2), Err(SyncError::Transport(_))));
    assert_eq!(h.host.active_baseline_transfers(), 2);
    h.pump(3).unwrap();
    assert_eq!(h.host.phase(h.id(3)), Some(SyncPhase::AwaitingBaselineSlot));
    h.host.disconnect(id);
    assert_eq!(
        h.host.catch_up().status(h.player(2)),
        Err(CatchUpError::NotJoining)
    );
    h.pump(3).unwrap();
    assert_eq!(h.host.active_baseline_transfers(), 2);
    assert_eq!(h.host.phase(h.id(3)), Some(SyncPhase::BaselineTransfer));
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
fn obsolete_abort_waits_for_queue_capacity_before_restart_or_new_baseline() {
    use crate::network::lifecycle::MAX_BULK_QUEUE_BYTES;
    for via_pump in [false, true] {
        let mut h = Harness::new(
            true,
            CatchUpLimits {
                max_events: 0,
                ..Default::default()
            },
        );
        h.reach(SyncPhase::BaselineTransfer);
        h.client_to_host().unwrap();
        h.host
            .pump(&h.p.host, HA, &mut h.p.ht, &authority(&h.s), h.p.now)
            .unwrap();
        h.host_to_client().unwrap();
        assert!(h.client.declared_in_flight_bytes() > 0);
        h.apply_grab();
        let delivered = h.p.ht.backend_mut().bulk_sent;
        h.p.ht.backend_mut().bulk_queue_limit = Some(MAX_BULK_QUEUE_BYTES);
        for queued in [MAX_BULK_QUEUE_BYTES, MAX_BULK_QUEUE_BYTES - 1] {
            h.p.ht.backend_mut().egress = Some(ReliableEgress {
                queued_bytes: queued,
                bulk_queued_bytes: queued,
                bulk_delivered_bytes: delivered,
            });
            h.p.now += Duration::from_secs(1);
            if via_pump {
                h.host
                    .pump(&h.p.host, HA, &mut h.p.ht, &authority(&h.s), h.p.now)
                    .unwrap();
            } else {
                h.host
                    .restart(&h.p.host, HA, &mut h.p.ht, &authority(&h.s), h.p.now)
                    .unwrap();
            }
            assert_eq!(h.host.phase(HA), Some(SyncPhase::RestartRequired));
            assert!(h.p.ht.backend_mut().sent.is_empty());
            assert!(h.p.ht.has_channel(HA));
            assert_eq!(h.host.transfer_binding(HA), None);
        }
        h.p.ht.backend_mut().egress = None;
        if via_pump {
            h.host
                .pump(&h.p.host, HA, &mut h.p.ht, &authority(&h.s), h.p.now)
                .unwrap();
            assert_eq!(h.p.ht.backend_mut().sent.len(), 2); // Abort, then Restart.
        }
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
            1
        );
        h.host_to_client().unwrap();
        assert_eq!(h.client.generation(), Some(1));
        assert_eq!(h.client.declared_in_flight_bytes(), 0);
        h.reach(SyncPhase::Ready);
    }
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
    assert_eq!(h.p.client.state(), None);
    let mut h = Harness::new(true, CatchUpLimits::default());
    let player = h.p.host.assigned_player(HA).unwrap();
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
        h.host.catch_up().status(player),
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
    assert_eq!(h.p.client.state(), None);
    let mut h = Harness::new(false, CatchUpLimits::default());
    h.host_to_client().unwrap();
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
    assert_eq!(
        timing.timeout(
            h.p.now + Duration::from_secs(11),
            crate::network::lifecycle::SyncPolicy {
                bulk_stall: Duration::from_secs(10),
                ..Default::default()
            }
        ),
        Some(SyncError::BulkStalled)
    );
    assert_eq!(
        timing.timeout(h.p.now + Duration::from_secs(600), Default::default()),
        Some(SyncError::LifetimeTimeout)
    );
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
    assert_eq!(h.p.client.state(), None);
    let mut h = Harness::new(true, CatchUpLimits::default());
    h.host_to_client().unwrap();
    h.client_to_host().unwrap();
    h.host_to_client().unwrap();
    h.client_to_host().unwrap();
    h.host
        .pump(&h.p.host, HA, &mut h.p.ht, &authority(&h.s), h.p.now)
        .unwrap();
    h.host_to_client().unwrap();
    assert!(h.client.declared_in_flight_bytes() > 0);
    h.s.peers[0].store.epoch += 1;
    h.host
        .pump(&h.p.host, HA, &mut h.p.ht, &authority(&h.s), h.p.now)
        .unwrap();
    assert!(matches!(
        h.host_to_client(),
        Err(SyncError::AuthorityChanged)
    ));
    assert_eq!(
        h.p.client.failure(),
        Some(DisconnectReason::ConnectionProblem)
    );
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
