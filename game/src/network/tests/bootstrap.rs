use super::*;
use crate::network::{
    auth::{ClientHandshake, PasswordError, ServerHandshake, SessionPassword},
    bootstrap::*,
    secure::SecureTransport,
    session_control::*,
};
use std::time::{Duration, Instant};

fn password(value: &str) -> SessionPassword {
    SessionPassword::new(value.to_owned()).unwrap()
}
fn metadata() -> SessionMetadata {
    SessionMetadata {
        definition: SESSION,
        cursor: AuthorityCursor::new(3, 0),
        host: HOST,
    }
}
fn hello() -> (ServerHandshake, ServerHello) {
    ServerHandshake::start(&password("correct password"), metadata(), A).unwrap()
}
fn remap(mut event: TransportEvent, id: ConnectionId) -> TransportEvent {
    let TransportEvent::Message { connection, .. } = &mut event else {
        panic!("message expected")
    };
    *connection = id;
    event
}
struct Pair {
    host: HostBootstrap,
    client: ClientBootstrap,
    host_connections: SessionConnections,
    client_connections: SessionConnections,
    ht: SecureTransport<FakeTransport>,
    ct: SecureTransport<FakeTransport>,
    now: Instant,
}
const CLIENT_HOST: ConnectionId = ConnectionId::new(200);
impl Pair {
    fn new(client_password: &str) -> Self {
        let now = Instant::now();
        Self {
            host: HostBootstrap::new(password("correct password"), metadata(), [], now),
            client: ClientBootstrap::new(password(client_password), CLIENT_HOST),
            host_connections: SessionConnections::default(),
            client_connections: SessionConnections::default(),
            ht: SecureTransport::new(FakeTransport::default()),
            ct: SecureTransport::new(FakeTransport::default()),
            now,
        }
    }
    fn connect(&mut self) {
        self.host
            .process(
                &TransportEvent::Connected { connection: HA },
                &mut self.ht,
                &mut self.host_connections,
                self.now,
            )
            .unwrap();
        self.client
            .process(
                &TransportEvent::Connected {
                    connection: CLIENT_HOST,
                },
                &mut self.ct,
                &mut self.client_connections,
                self.now,
            )
            .unwrap();
    }
    fn client_proof(&mut self) -> TransportEvent {
        let event = remap(self.ht.backend_mut().sent.pop().unwrap(), CLIENT_HOST);
        self.client
            .process(&event, &mut self.ct, &mut self.client_connections, self.now)
            .unwrap();
        remap(self.ct.backend_mut().sent.pop().unwrap(), HA)
    }
    fn authenticate(&mut self) {
        self.connect();
        let event = self.client_proof();
        self.host
            .process(&event, &mut self.ht, &mut self.host_connections, self.now)
            .unwrap();
        assert_eq!(self.host.state(HA), Some(ConnectionState::Securing));
        assert!(self.ht.has_channel(HA));
        assert_eq!(self.host.assigned_player(HA), None);
        assert!(self.host.begin_sync(HA).is_err());
        assert_eq!(self.client.state(), Some(ConnectionState::Authenticating));
        assert_eq!(self.client.assigned_player(), None);
        let event = remap(self.ht.backend_mut().sent.pop().unwrap(), CLIENT_HOST);
        let TransportEvent::Message { payload, .. } = &event else {
            unreachable!()
        };
        assert!(matches!(
            wire::decode(payload),
            Ok(WireMessage::SessionControl(
                SessionControlMessage::AuthAccepted(_)
            ))
        ));
        self.client
            .process(&event, &mut self.ct, &mut self.client_connections, self.now)
            .unwrap();
        assert!(self.ct.has_channel(CLIENT_HOST));
        assert!(!self.client.retains_password());
        let event = remap(self.ct.backend_mut().sent.pop().unwrap(), HA);
        let TransportEvent::Message { payload, .. } = &event else {
            unreachable!()
        };
        assert!(
            wire::decode(payload).is_err(),
            "SecureChannelReady is encrypted"
        );
        self.ht.backend_mut().inbox.push(event);
        let mut decrypted = Vec::new();
        self.ht.poll(&mut decrypted).unwrap();
        assert_eq!(decrypted.len(), 1);
        self.host
            .process(
                &decrypted[0],
                &mut self.ht,
                &mut self.host_connections,
                self.now,
            )
            .unwrap();
    }
}

#[test]
fn password_validates_utf8_bytes_and_redacts_debug() {
    for value in ["", "1234567", "ああ"] {
        assert_eq!(
            SessionPassword::new(value.to_owned()).unwrap_err(),
            PasswordError::TooShort
        );
    }
    assert!(SessionPassword::new("あああ".to_owned()).is_ok());
    assert!(SessionPassword::new("x".repeat(128)).is_ok());
    assert_eq!(
        SessionPassword::new("x".repeat(129)).unwrap_err(),
        PasswordError::TooLong
    );
    assert_eq!(
        format!("{:?}", password("secret-password")),
        "SessionPassword([REDACTED])"
    );
}
#[test]
fn correct_password_mutual_confirmation_and_explicit_ready_registration() {
    let mut p = Pair::new("correct password");
    p.authenticate();
    assert_eq!(p.host.state(HA), Some(ConnectionState::Authenticated));
    assert_eq!(p.client.state(), Some(ConnectionState::Authenticated));
    let player = p.host.assigned_player(HA).unwrap();
    assert_ne!(player, HOST);
    assert_eq!(p.client.assigned_player(), Some(player));
    assert_eq!(p.client.metadata(), Some(metadata()));
    assert_eq!(p.host_connections.player(HA), None);
    assert_eq!(p.client_connections.player(CLIENT_HOST), None);
    assert_eq!(
        p.host.promote_ready(HA, &mut p.host_connections),
        Err(BootstrapError::InvalidTransition)
    );
    assert_eq!(
        p.client.promote_ready(&mut p.client_connections),
        Err(BootstrapError::InvalidTransition)
    );
    p.host.begin_sync(HA).unwrap();
    p.client.begin_sync().unwrap();
    assert_eq!(p.host.state(HA), Some(ConnectionState::Syncing));
    assert_eq!(p.client.state(), Some(ConnectionState::Syncing));
    p.host.promote_ready(HA, &mut p.host_connections).unwrap();
    p.client.promote_ready(&mut p.client_connections).unwrap();
    assert_eq!(p.host_connections.player(HA), Some(player));
    assert_eq!(p.client_connections.player(CLIENT_HOST), Some(HOST));
    assert_eq!(
        p.host.promote_ready(HA, &mut p.host_connections),
        Err(BootstrapError::InvalidTransition)
    );
    assert_eq!(
        p.client.promote_ready(&mut p.client_connections),
        Err(BootstrapError::InvalidTransition)
    );
    assert_eq!(p.host_connections.peers().count(), 1);
    assert_eq!(
        p.host.process(
            &message_event(HA, &WireMessage::ClientCommand(grab())),
            &mut p.ht,
            &mut p.host_connections,
            p.now
        ),
        Ok(BootstrapOutcome::Gameplay)
    );
}
#[test]
fn wrong_password_closes_without_registering_or_mutating_gameplay() {
    let mut p = Pair::new("incorrect password");
    p.connect();
    let event = p.client_proof();
    assert_eq!(
        p.host
            .process(&event, &mut p.ht, &mut p.host_connections, p.now),
        Err(BootstrapError::Rejected(
            DisconnectReason::AuthenticationFailed
        ))
    );
    assert_eq!(p.host_connections.player(HA), None);
    assert!(p.ht.backend_mut().sent.is_empty());
    let mut events = Vec::new();
    p.ht.poll(&mut events).unwrap();
    assert!(events.contains(&TransportEvent::Disconnected {
        connection: HA,
        reason: DisconnectReason::AuthenticationFailed
    }));
    assert!(!p.ht.has_channel(HA));
    assert!(!p.ct.has_channel(CLIENT_HOST));
    assert!(p.ct.backend_mut().sent.is_empty());
    let mut s = Scenario::new();
    let before = s.host.store.states.clone();
    assert_eq!(
        s.host_router()
            .route(&message_event(HA, &WireMessage::ClientCommand(grab())))
            .unwrap_err(),
        HostRouteError::Unauthenticated
    );
    assert_eq!(s.host.store.states, before);
}
#[test]
fn wrong_client_and_server_confirmations_and_altered_assignment_fail() {
    let (server, h) = hello();
    let (_, mut proof) = ClientHandshake::start(&password("correct password"), &h).unwrap();
    proof.confirmation[0] ^= 1;
    assert!(server.finish(proof).is_err());
    for alter_player in [false, true] {
        let (server, h) = hello();
        let (client, proof) = ClientHandshake::start(&password("correct password"), &h).unwrap();
        let mut accepted = AuthAccepted {
            player: A,
            confirmation: server.finish(proof).unwrap().0,
        };
        if alter_player {
            accepted.player = B;
        } else {
            accepted.confirmation[0] ^= 1;
        }
        assert!(client.finish(accepted).is_err());
    }
    let mut p = Pair::new("correct password");
    p.connect();
    let event = p.client_proof();
    p.host
        .process(&event, &mut p.ht, &mut p.host_connections, p.now)
        .unwrap();
    let wrong = message_event(
        CLIENT_HOST,
        &WireMessage::SessionControl(SessionControlMessage::AuthAccepted(AuthAccepted {
            player: PlayerId(0),
            confirmation: [0; 32],
        })),
    );
    assert!(p
        .client
        .process(&wrong, &mut p.ct, &mut p.client_connections, p.now)
        .is_err());
    assert_eq!(p.client.state(), None);
    assert_eq!(p.client.assigned_player(), None);
}
#[test]
fn replay_on_fresh_connection_nonce_or_session_fails() {
    let (_, original) = hello();
    let (_, proof) = ClientHandshake::start(&password("correct password"), &original).unwrap();
    let (fresh, fresh_hello) = hello();
    assert_ne!(original.nonce, fresh_hello.nonce);
    assert!(fresh.finish(proof).is_err());
    // Alter only the hello seen by the client: these tests isolate each binding
    // while keeping the server's ephemeral share unchanged.
    for field in 0..7 {
        let (server, mut h) = hello();
        match field {
            0 => h.nonce[0] ^= 1,
            1 => h.metadata.definition.id.0 += 1,
            2 => h.metadata.definition.image_hash.0[0] ^= 1,
            3 => h.metadata.host.0 += 100,
            4 => h.metadata.cursor.sequence.0 += 1,
            5 => h.reserved_player.0 += 1,
            _ => h.metadata.cursor.epoch.0 += 1,
        }
        let (_, proof) = ClientHandshake::start(&password("correct password"), &h).unwrap();
        assert!(server.finish(proof).is_err(), "binding {field}");
    }
    let mut p = Pair::new("correct password");
    p.authenticate();
    assert!(p
        .host
        .process(
            &message_event(
                HA,
                &WireMessage::SessionControl(SessionControlMessage::ClientProof(proof))
            ),
            &mut p.ht,
            &mut p.host_connections,
            p.now
        )
        .is_err());
    assert_eq!(p.host.state(HA), None);
}
#[test]
fn authentication_timeout_is_nonblocking_and_exact() {
    let mut p = Pair::new("correct password");
    p.connect();
    assert!(p
        .host
        .expire(
            &mut p.ht,
            &mut p.host_connections,
            p.now + AUTH_TIMEOUT - Duration::from_nanos(1)
        )
        .is_empty());
    assert_eq!(
        p.host
            .expire(&mut p.ht, &mut p.host_connections, p.now + AUTH_TIMEOUT),
        vec![(
            HA,
            BootstrapError::Rejected(DisconnectReason::AuthenticationTimeout)
        )]
    );
    assert_eq!(
        p.client
            .expire(&mut p.ct, &mut p.client_connections, p.now + AUTH_TIMEOUT),
        Err(BootstrapError::Rejected(
            DisconnectReason::AuthenticationTimeout
        ))
    );
    assert_eq!(p.host_connections.peers().count(), 0);
    let mut p = Pair::new("correct password");
    p.authenticate();
    assert!(p
        .host
        .expire(&mut p.ht, &mut p.host_connections, p.now + AUTH_TIMEOUT)
        .is_empty());
    assert!(p
        .client
        .expire(&mut p.ct, &mut p.client_connections, p.now + AUTH_TIMEOUT)
        .is_ok());
}
#[test]
fn pending_capacity_is_separate_from_global_attempt_rate() {
    let mut p = Pair::new("correct password");
    for n in 0..MAX_PENDING_AUTH {
        p.host
            .process(
                &TransportEvent::Connected {
                    connection: ConnectionId::new(n as u64),
                },
                &mut p.ht,
                &mut p.host_connections,
                p.now + Duration::from_millis(n as u64 * 250),
            )
            .unwrap();
    }
    let rejected = ConnectionId::new(999);
    assert_eq!(
        p.host.process(
            &TransportEvent::Connected {
                connection: rejected
            },
            &mut p.ht,
            &mut p.host_connections,
            p.now + Duration::from_secs(8)
        ),
        Err(BootstrapError::Rejected(DisconnectReason::RateLimited))
    );
    assert_eq!(p.host_connections.peers().count(), MAX_PENDING_AUTH);
    p.host
        .process(
            &TransportEvent::Disconnected {
                connection: ConnectionId::new(0),
                reason: DisconnectReason::RemoteClosed,
            },
            &mut p.ht,
            &mut p.host_connections,
            p.now,
        )
        .unwrap();
    p.host
        .process(
            &TransportEvent::Connected {
                connection: rejected,
            },
            &mut p.ht,
            &mut p.host_connections,
            p.now + Duration::from_secs(8),
        )
        .unwrap();
}
#[test]
fn host_global_start_and_failure_buckets_refill() {
    for fail in [false, true] {
        let mut p = Pair::new("correct password");
        let mut proofs = Vec::new();
        for n in 0..AUTH_ATTEMPT_BURST {
            let id = ConnectionId::new(n as u64);
            p.host
                .process(
                    &TransportEvent::Connected { connection: id },
                    &mut p.ht,
                    &mut p.host_connections,
                    p.now,
                )
                .unwrap();
            if fail {
                // Valid point but invalid confirmation: consumes a failed attempt.
                let WireMessage::SessionControl(SessionControlMessage::ServerHello(h)) =
                    wire::decode(match p.ht.backend_mut().sent.last().unwrap() {
                        TransportEvent::Message { payload, .. } => payload,
                        _ => unreachable!(),
                    })
                    .unwrap()
                else {
                    unreachable!()
                };
                let (_, mut proof) =
                    ClientHandshake::start(&password("incorrect password"), &h).unwrap();
                proof.confirmation = [0; 32];
                proofs.push((id, proof));
            }
        }
        // Let the start bucket refill before draining the failure bucket,
        // proving that failures block starts independently of start credit.
        let now = p.now
            + if fail {
                Duration::from_secs(2)
            } else {
                Duration::ZERO
            };
        for (id, proof) in proofs {
            assert!(p
                .host
                .process(
                    &message_event(
                        id,
                        &WireMessage::SessionControl(SessionControlMessage::ClientProof(proof))
                    ),
                    &mut p.ht,
                    &mut p.host_connections,
                    now
                )
                .is_err());
        }
        let id = ConnectionId::new(999);
        assert_eq!(
            p.host.process(
                &TransportEvent::Connected { connection: id },
                &mut p.ht,
                &mut p.host_connections,
                now
            ),
            Err(BootstrapError::Rejected(DisconnectReason::RateLimited))
        );
        assert!(p
            .host
            .process(
                &TransportEvent::Connected { connection: id },
                &mut p.ht,
                &mut p.host_connections,
                now + Duration::from_millis(249)
            )
            .is_err());
        p.host
            .process(
                &TransportEvent::Connected { connection: id },
                &mut p.ht,
                &mut p.host_connections,
                now + Duration::from_millis(250),
            )
            .unwrap();
    }
}
#[test]
fn malformed_and_oversized_session_control_rejected_before_deserialize() {
    let (_, h) = hello();
    let valid = wire::encode(&WireMessage::SessionControl(
        SessionControlMessage::ServerHello(h),
    ))
    .unwrap();
    let mut bytes = valid.clone();
    bytes[wire::HEADER_SIZE] = 255;
    assert_eq!(wire::decode(&bytes), Err(WireError::MalformedPayload));
    bytes = valid.clone();
    bytes.truncate(wire::HEADER_SIZE + 1);
    bytes[8..12].copy_from_slice(&1u32.to_le_bytes());
    bytes[wire::HEADER_SIZE] = 0;
    assert_eq!(wire::decode(&bytes), Err(WireError::MalformedPayload));
    bytes = valid.clone();
    bytes.resize(
        wire::HEADER_SIZE + wire::MAX_SESSION_CONTROL_PAYLOAD + 1,
        255,
    );
    bytes[8..12].copy_from_slice(&((wire::MAX_SESSION_CONTROL_PAYLOAD + 1) as u32).to_le_bytes());
    assert_eq!(wire::decode(&bytes), Err(WireError::Oversized));
    bytes.truncate(wire::HEADER_SIZE);
    assert_eq!(wire::decode(&bytes), Err(WireError::Oversized));
    let mut p = Pair::new("correct password");
    p.connect();
    assert!(p
        .host
        .process(
            &TransportEvent::Message {
                connection: HA,
                class: MessageClass::Control,
                payload: bytes
            },
            &mut p.ht,
            &mut p.host_connections,
            p.now
        )
        .is_err());
    assert_eq!(p.host.state(HA), None);
    assert_eq!(
        wire::decode_for_class(&valid, MessageClass::Bulk),
        Err(WireError::WrongClass)
    );
}
#[test]
fn all_gameplay_is_rejected_before_ready_on_both_endpoints() {
    let gameplay = [
        WireMessage::ClientCommand(grab()),
        WireMessage::ClientCommand(update()),
        bulk_chunk(vec![0]),
        WireMessage::AuthorityEvent(ProtocolAuthorityEventEnvelope {
            session: SESSION.id,
            host: HOST,
            cursor: AuthorityCursor::new(3, 1),
            event: ProtocolAuthorityEvent::GrabAccepted(GrabAccepted {
                player: A,
                grab_sequence: 0,
                accepted: PieceTarget::Components(vec![]),
                rejected: vec![],
            }),
        }),
        WireMessage::DragUpdate(RemoteDragUpdate {
            session: SESSION.id,
            authority_epoch: AuthorityEpoch(3),
            player: A,
            grab_sequence: 0,
            basis_sequence: 0,
            tick: 0,
            delta: Vec2::ZERO,
        }),
    ];
    for message in &gameplay {
        for stage in 0..3 {
            for host_side in [true, false] {
                let mut p = Pair::new("correct password");
                if stage == 0 {
                    p.connect();
                } else {
                    p.authenticate();
                    if stage == 2 {
                        p.host.begin_sync(HA).unwrap();
                        p.client.begin_sync().unwrap();
                    }
                }
                let result = if host_side {
                    p.host.process(
                        &message_event(HA, message),
                        &mut p.ht,
                        &mut p.host_connections,
                        p.now,
                    )
                } else {
                    p.client.process(
                        &message_event(CLIENT_HOST, message),
                        &mut p.ct,
                        &mut p.client_connections,
                        p.now,
                    )
                };
                assert_eq!(
                    result,
                    Err(BootstrapError::Rejected(
                        DisconnectReason::ProtocolViolation
                    ))
                );
                assert_eq!(p.host_connections.player(HA), None);
                assert_eq!(p.client_connections.player(CLIENT_HOST), None);
            }
        }
    }
}
#[test]
fn nonzero_host_reserved_ids_and_reused_connection_never_reuse_stale_mapping() {
    let mut p = Pair::new("correct password");
    p.host = HostBootstrap::new(
        password("correct password"),
        metadata(),
        [PlayerId(0), PlayerId(1)],
        p.now,
    );
    p.authenticate();
    let old = p.client.assigned_player().unwrap();
    assert_eq!(old, PlayerId(2));
    p.host.begin_sync(HA).unwrap();
    p.host.promote_ready(HA, &mut p.host_connections).unwrap();
    p.host
        .process(
            &TransportEvent::Disconnected {
                connection: HA,
                reason: DisconnectReason::RemoteClosed,
            },
            &mut p.ht,
            &mut p.host_connections,
            p.now,
        )
        .unwrap();
    p.client = ClientBootstrap::new(password("correct password"), CLIENT_HOST);
    p.authenticate();
    assert_ne!(p.host.assigned_player(HA), Some(old));
    assert_eq!(p.host_connections.player(HA), None);
}

#[test]
fn authentication_and_syncing_peers_receive_no_gameplay_broadcasts() {
    let mut p = Pair::new("correct password");
    p.authenticate();
    let mut s = Scenario::new();
    let outcome = s
        .contexts
        .apply_replicated(
            &mut s.host.session,
            &mut s.host.store,
            A,
            &grab(),
            Some(&s.definition),
            HOST,
        )
        .unwrap();
    s.host.connections = std::mem::take(&mut p.host_connections);
    let mut published = FakeTransport::default();
    assert!(s
        .host_router()
        .publish(&mut published, None, &outcome)
        .unwrap()
        .is_empty());
    assert!(published.sent.is_empty());
    p.host.begin_sync(HA).unwrap();
    assert!(s
        .host_router()
        .publish(&mut published, None, &outcome)
        .unwrap()
        .is_empty());
    assert!(published.sent.is_empty());
    p.host.promote_ready(HA, &mut s.host.connections).unwrap();
    s.host_router()
        .publish(&mut published, None, &outcome)
        .unwrap();
    assert_eq!(published.sent.len(), 1);
}

#[test]
fn failed_handshake_sends_clear_state_and_cannot_promote() {
    let mut p = Pair::new("correct password");
    p.ht.backend_mut().fail = Some(HA);
    assert_eq!(
        p.host.process(
            &TransportEvent::Connected { connection: HA },
            &mut p.ht,
            &mut p.host_connections,
            p.now
        ),
        Err(BootstrapError::Transport(TransportError::NotConnected))
    );
    assert_eq!(p.host.state(HA), None);
    assert_eq!(p.host_connections.player(HA), None);
    let mut p = Pair::new("correct password");
    p.connect();
    let proof = p.client_proof();
    p.ht.backend_mut().fail = Some(HA);
    assert_eq!(
        p.host
            .process(&proof, &mut p.ht, &mut p.host_connections, p.now),
        Err(BootstrapError::Transport(TransportError::NotConnected))
    );
    assert_eq!(p.host.assigned_player(HA), None);
    assert!(p.host.begin_sync(HA).is_err());
    let mut p = Pair::new("correct password");
    p.connect();
    p.ct.backend_mut().fail = Some(CLIENT_HOST);
    let hello = remap(p.ht.backend_mut().sent.pop().unwrap(), CLIENT_HOST);
    assert_eq!(
        p.client
            .process(&hello, &mut p.ct, &mut p.client_connections, p.now),
        Err(BootstrapError::Transport(TransportError::NotConnected))
    );
    assert_eq!(p.client.state(), None);
    assert!(p.client.begin_sync().is_err());
}

#[test]
fn gameplay_is_gated_before_large_or_malformed_payload_decoding() {
    let mut p = Pair::new("correct password");
    p.connect();
    // Valid gameplay header with an undecodable payload: gating needs only the header.
    let mut payload = wire::encode(&WireMessage::ClientCommand(grab())).unwrap();
    payload.truncate(wire::HEADER_SIZE);
    payload.push(255);
    payload[8..12].copy_from_slice(&1u32.to_le_bytes());
    assert_eq!(
        wire::is_session_control_for_class(&payload, MessageClass::Control),
        Ok(false)
    );
    assert_eq!(wire::decode(&payload), Err(WireError::MalformedPayload));
    assert_eq!(
        p.host.process(
            &TransportEvent::Message {
                connection: HA,
                class: MessageClass::Control,
                payload
            },
            &mut p.ht,
            &mut p.host_connections,
            p.now
        ),
        Err(BootstrapError::Rejected(
            DisconnectReason::ProtocolViolation
        ))
    );
}

#[test]
fn malformed_curve_points_and_stale_registration_are_rejected() {
    let (server, mut h) = hello();
    let invalid = PakePublicMessage {
        x: [0; 32],
        y: [0; 32],
    };
    assert!(server
        .finish(ClientProof {
            public_message: invalid,
            confirmation: [0; 32]
        })
        .is_err());
    h.public_message = invalid;
    assert!(ClientHandshake::start(&password("correct password"), &h).is_err());
    let mut p = Pair::new("correct password");
    // Clear stale gameplay registration even when the previous disconnect
    // was not observed by this registry.
    connected(&mut p.host_connections, HA, A);
    connected(&mut p.client_connections, CLIENT_HOST, HOST);
    p.connect();
    assert_eq!(p.host_connections.player(HA), None);
    assert_eq!(p.client_connections.player(CLIENT_HOST), None);
    assert!(p.host.begin_sync(HA).is_err());
    assert!(p.client.begin_sync().is_err());
}

#[test]
fn securing_timeout_and_plaintext_ready_cannot_complete_authentication() {
    for timeout in [false, true] {
        let mut p = Pair::new("correct password");
        p.connect();
        let proof = p.client_proof();
        p.host
            .process(&proof, &mut p.ht, &mut p.host_connections, p.now)
            .unwrap();
        assert_eq!(p.host.state(HA), Some(ConnectionState::Securing));
        assert!(p.ht.has_channel(HA));
        if timeout {
            assert_eq!(
                p.host
                    .expire(&mut p.ht, &mut p.host_connections, p.now + AUTH_TIMEOUT)
                    .len(),
                1
            );
        } else {
            p.ht.backend_mut().inbox.push(message_event(
                HA,
                &WireMessage::SessionControl(SessionControlMessage::SecureChannelReady),
            ));
            let mut events = Vec::new();
            p.ht.poll(&mut events).unwrap();
            assert_eq!(
                events,
                vec![TransportEvent::Disconnected {
                    connection: HA,
                    reason: DisconnectReason::ProtocolViolation
                }]
            );
            p.host
                .process(&events[0], &mut p.ht, &mut p.host_connections, p.now)
                .unwrap();
        }
        assert!(!p.ht.has_channel(HA));
        assert_eq!(p.host.state(HA), None);
        assert_eq!(p.host.assigned_player(HA), None);
        assert_eq!(p.host_connections.player(HA), None);
    }
}

#[test]
fn securing_connections_still_count_towards_pending_capacity() {
    let mut p = Pair::new("correct password");
    for n in 0..MAX_PENDING_AUTH {
        let id = ConnectionId::new(n as u64);
        let now = p.now + Duration::from_millis(n as u64 * 250);
        p.host
            .process(
                &TransportEvent::Connected { connection: id },
                &mut p.ht,
                &mut p.host_connections,
                now,
            )
            .unwrap();
        let TransportEvent::Message { payload, .. } = p.ht.backend_mut().sent.pop().unwrap() else {
            unreachable!()
        };
        let WireMessage::SessionControl(SessionControlMessage::ServerHello(hello)) =
            wire::decode(&payload).unwrap()
        else {
            unreachable!()
        };
        let (_, proof) = ClientHandshake::start(&password("correct password"), &hello).unwrap();
        p.host
            .process(
                &message_event(
                    id,
                    &WireMessage::SessionControl(SessionControlMessage::ClientProof(proof)),
                ),
                &mut p.ht,
                &mut p.host_connections,
                now,
            )
            .unwrap();
        assert_eq!(p.host.state(id), Some(ConnectionState::Securing));
    }
    assert_eq!(
        p.host.process(
            &TransportEvent::Connected {
                connection: ConnectionId::new(999)
            },
            &mut p.ht,
            &mut p.host_connections,
            p.now + Duration::from_secs(8)
        ),
        Err(BootstrapError::Rejected(DisconnectReason::RateLimited))
    );
}

#[test]
fn encrypted_ready_send_failure_destroys_client_channel_and_assignment() {
    let mut p = Pair::new("correct password");
    p.connect();
    let proof = p.client_proof();
    p.host
        .process(&proof, &mut p.ht, &mut p.host_connections, p.now)
        .unwrap();
    let accepted = remap(p.ht.backend_mut().sent.pop().unwrap(), CLIENT_HOST);
    p.ct.backend_mut().fail = Some(CLIENT_HOST);
    assert!(p
        .client
        .process(&accepted, &mut p.ct, &mut p.client_connections, p.now)
        .is_err());
    assert!(!p.ct.has_channel(CLIENT_HOST));
    assert_eq!(p.client.assigned_player(), None);
    assert_eq!(p.client.state(), None);
    assert!(p.ct.backend_mut().sent.is_empty());
    assert!(p.client.begin_sync().is_err());
}

#[test]
fn tampered_channel_close_routed_through_bootstrap_notifies_exactly_once() {
    let mut p = Pair::new("correct password");
    p.authenticate();
    p.ht.backend_mut().inbox.push(TransportEvent::Message {
        connection: HA,
        class: MessageClass::Control,
        payload: vec![0],
    });
    let mut events = Vec::new();
    p.ht.poll(&mut events).unwrap();
    assert_eq!(events.len(), 1);
    p.host
        .process(&events[0], &mut p.ht, &mut p.host_connections, p.now)
        .unwrap();
    events.clear();
    p.ht.poll(&mut events).unwrap();
    assert!(events.is_empty());
    assert!(!p.ht.has_channel(HA));
    assert_eq!(p.host.state(HA), None);
}
