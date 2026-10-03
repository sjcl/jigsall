use super::*;
use crate::network::tests::FakeTransport;
use zeroize::{Zeroize, ZeroizeOnDrop};

const ID: ConnectionId = ConnectionId::new(501);
const OTHER: ConnectionId = ConnectionId::new(502);
const CLASSES: [MessageClass; 3] = [
    MessageClass::Control,
    MessageClass::Transient,
    MessageClass::Bulk,
];
fn secret() -> AuthenticatedSecret {
    AuthenticatedSecret::fixture(&[7; 16])
}
fn channels() -> (Channel, Channel) {
    (
        Channel::new(secret(), ChannelRole::Client),
        Channel::new(secret(), ChannelRole::Host),
    )
}
fn frame(class: MessageClass) -> Vec<u8> {
    // Channel tests intentionally use byte messages. Codec tests separately
    // validate protocol objects; protection covers every byte of the inner frame.
    let mut bytes = vec![0; wire::HEADER_SIZE + 4];
    bytes[..4].copy_from_slice(b"PZLA");
    bytes[4..6].copy_from_slice(&WIRE_VERSION.to_le_bytes());
    bytes[6] = match class {
        MessageClass::Control => 6,
        MessageClass::Transient => 3,
        MessageClass::Bulk => 5,
    };
    bytes[8..12].copy_from_slice(&4u32.to_le_bytes());
    bytes
}
fn wrapped() -> SecureTransport<FakeTransport> {
    let mut transport = SecureTransport::new(FakeTransport::default());
    transport.start_connection(ID);
    transport.install(ID, secret(), ChannelRole::Host).unwrap();
    transport
}
fn incoming(class: MessageClass, payload: Vec<u8>) -> TransportEvent {
    TransportEvent::Message {
        connection: ID,
        class,
        payload,
    }
}
fn poll(transport: &mut SecureTransport<FakeTransport>) -> Vec<TransportEvent> {
    let mut events = Vec::new();
    transport.poll(&mut events).unwrap();
    events
}
fn rejected(payload: Vec<u8>, class: MessageClass) {
    let mut transport = wrapped();
    transport.backend_mut().inbox.push(incoming(class, payload));
    assert_eq!(
        poll(&mut transport),
        vec![TransportEvent::Disconnected {
            connection: ID,
            reason: DisconnectReason::ProtocolViolation
        }]
    );
    assert!(!transport.has_channel(ID));
    assert!(poll(&mut transport).is_empty());
}

fn session_control_frame() -> Vec<u8> {
    use crate::network::session_control::{AuthAccepted, SessionControlMessage};
    wire::encode(&wire::WireMessage::SessionControl(
        SessionControlMessage::AuthAccepted(AuthAccepted {
            player: puzzella_core::PlayerId(43),
            confirmation: [8; 32],
        }),
    ))
    .unwrap()
}
fn forbidden_plaintext_frames() -> Vec<(MessageClass, Vec<u8>)> {
    let mut frames = Vec::new();
    // All gameplay header kinds must be rejected before body deserialization.
    for (kind, class) in [
        (1, MessageClass::Control),
        (2, MessageClass::Control),
        (3, MessageClass::Transient),
        (4, MessageClass::Transient),
    ] {
        let mut payload = frame(class);
        payload[6] = kind;
        frames.push((class, payload));
    }
    frames.push((
        MessageClass::Bulk,
        wire::encode(&wire::WireMessage::BulkChunk(
            b"private image bytes".to_vec(),
        ))
        .unwrap(),
    ));
    frames.push((MessageClass::Control, vec![0]));
    for class in [MessageClass::Transient, MessageClass::Bulk] {
        frames.push((class, session_control_frame()));
    }
    for (offset, value) in [(0, 0), (4, 0), (6, 7), (7, 1)] {
        let mut payload = session_control_frame();
        payload[offset] = value;
        frames.push((MessageClass::Control, payload));
    }
    let mut truncated = session_control_frame();
    truncated.pop();
    frames.push((MessageClass::Control, truncated));
    for (class, length) in CLASSES
        .map(|class| (class, wire::frame_limit(class) + 1))
        .into_iter()
        .chain([(
            MessageClass::Control,
            wire::HEADER_SIZE + wire::MAX_SESSION_CONTROL_PAYLOAD + 1,
        )])
    {
        let mut payload = frame(class);
        payload.resize(length, 0);
        payload[8..12].copy_from_slice(&((length - wire::HEADER_SIZE) as u32).to_le_bytes());
        frames.push((class, payload));
    }
    frames
}

#[test]
fn plaintext_send_blocks_gameplay_and_invalid_frames_before_backend_send() {
    for (class, payload) in forbidden_plaintext_frames() {
        let mut transport = SecureTransport::new(FakeTransport::default());
        transport.start_connection(ID);
        transport.start_connection(OTHER);
        assert_eq!(
            transport.send(ID, class, &payload),
            Err(TransportError::ProtocolViolation)
        );
        assert!(transport.backend_mut().sent.is_empty());
        assert_eq!(
            poll(&mut transport),
            vec![TransportEvent::Disconnected {
                connection: ID,
                reason: DisconnectReason::ProtocolViolation,
            }]
        );
        assert!(poll(&mut transport).is_empty());
        assert_eq!(
            transport.send(ID, MessageClass::Control, &session_control_frame()),
            Err(TransportError::UnknownConnection)
        );
        transport
            .send(OTHER, MessageClass::Control, &session_control_frame())
            .unwrap();
    }
}

#[test]
fn plaintext_receive_blocks_gameplay_and_invalid_frames_before_bootstrap() {
    for (class, payload) in forbidden_plaintext_frames() {
        let mut transport = SecureTransport::new(FakeTransport::default());
        transport.start_connection(ID);
        transport.start_connection(OTHER);
        transport.backend_mut().inbox.push(incoming(class, payload));
        assert_eq!(
            poll(&mut transport),
            vec![TransportEvent::Disconnected {
                connection: ID,
                reason: DisconnectReason::ProtocolViolation,
            }]
        );
        assert!(poll(&mut transport).is_empty());
        transport
            .send(OTHER, MessageClass::Control, &session_control_frame())
            .unwrap();
    }
}

#[test]
fn plaintext_session_control_passes_and_installed_channel_encrypts_bulk() {
    let mut transport = SecureTransport::new(FakeTransport::default());
    transport.start_connection(ID);
    let handshake = session_control_frame();
    transport
        .send(ID, MessageClass::Control, &handshake)
        .unwrap();
    assert_eq!(
        transport.backend_mut().sent,
        vec![incoming(MessageClass::Control, handshake.clone())]
    );
    transport
        .backend_mut()
        .inbox
        .push(incoming(MessageClass::Control, handshake.clone()));
    assert_eq!(
        poll(&mut transport),
        vec![incoming(MessageClass::Control, handshake)]
    );

    transport.install(ID, secret(), ChannelRole::Host).unwrap();
    let message = wire::WireMessage::BulkChunk(b"private image bytes".to_vec());
    let plaintext = wire::encode(&message).unwrap();
    transport.send(ID, MessageClass::Bulk, &plaintext).unwrap();
    let TransportEvent::Message { payload, .. } = transport.backend_mut().sent.pop().unwrap()
    else {
        panic!("message expected");
    };
    assert_ne!(payload, plaintext);
    let (mut client, _) = channels();
    assert_eq!(
        client.open(MessageClass::Bulk, payload).unwrap(),
        Some(plaintext.clone())
    );
    transport.backend_mut().inbox.push(incoming(
        MessageClass::Bulk,
        client.seal(MessageClass::Bulk, &plaintext).unwrap(),
    ));
    assert_eq!(
        poll(&mut transport),
        vec![incoming(MessageClass::Bulk, plaintext)]
    );
    assert!(transport.has_channel(ID));
}

#[test]
fn opposite_roles_roundtrip_every_direction_and_class_with_independent_keys() {
    let (mut client, mut host) = channels();
    for class in CLASSES {
        let plaintext = frame(class);
        let record = client.seal(class, &plaintext).unwrap();
        assert_eq!(record.len(), plaintext.len() + RECORD_OVERHEAD);
        assert_eq!(&record[..8], &0u64.to_le_bytes());
        assert_ne!(&record[8..8 + plaintext.len()], plaintext);
        assert_eq!(host.open(class, record).unwrap(), Some(plaintext.clone()));
        let record = host.seal(class, &plaintext).unwrap();
        assert_eq!(client.open(class, record).unwrap(), Some(plaintext));
    }
    for (index, key) in client.keys.iter().enumerate() {
        assert!(client.keys.iter().skip(index + 1).all(|other| key != other));
    }
}

#[test]
fn application_keys_also_bind_the_confirmation_verified_session_context() {
    let key = [7; 16];
    let first = Channel::new(
        AuthenticatedSecret::fixture_binding(&key, b"session context one"),
        ChannelRole::Host,
    );
    let second = Channel::new(
        AuthenticatedSecret::fixture_binding(&key, b"session context two"),
        ChannelRole::Host,
    );
    assert!(first
        .keys
        .iter()
        .zip(second.keys.iter())
        .all(|(a, b)| a != b));
}
#[test]
fn direction_reflection_and_wrong_lane_fail() {
    let (mut client, _) = channels();
    let control = client
        .seal(MessageClass::Control, &frame(MessageClass::Control))
        .unwrap();
    assert!(client.open(MessageClass::Control, control.clone()).is_err());
    let (_, mut host) = channels();
    assert!(host.open(MessageClass::Bulk, control).is_err());
    for source in CLASSES {
        for destination in CLASSES {
            if source == destination {
                continue;
            }
            let (mut client, _) = channels();
            rejected(client.seal(source, &frame(source)).unwrap(), destination);
        }
    }
}
#[test]
fn ciphertext_tag_and_sequence_tampering_fail_without_advancing_receive_state() {
    let (mut client, _) = channels();
    let record = client
        .seal(MessageClass::Control, &frame(MessageClass::Control))
        .unwrap();
    for offset in [8, record.len() - 1] {
        let (_, mut host) = channels();
        let mut tampered = record.clone();
        tampered[offset] ^= 1;
        assert!(host.open(MessageClass::Control, tampered.clone()).is_err());
        assert_eq!(host.recv_next[0], 0);
        assert!(host.open(MessageClass::Control, record.clone()).is_ok());
        rejected(tampered, MessageClass::Control);
    }
    // A forged Transient sequence passes the gap policy but fails nonce/AAD
    // authentication; the forged high number cannot poison highest_received.
    let (mut client, mut host) = channels();
    let record = client
        .seal(MessageClass::Transient, &frame(MessageClass::Transient))
        .unwrap();
    let mut tampered = record.clone();
    tampered[..8].copy_from_slice(&1000u64.to_le_bytes());
    assert!(host
        .open(MessageClass::Transient, tampered.clone())
        .is_err());
    assert_eq!(host.transient_highest, None);
    assert!(host.open(MessageClass::Transient, record).is_ok());
    rejected(tampered, MessageClass::Transient);
}
#[test]
fn reliable_duplicates_stale_and_gaps_are_connection_local_failures() {
    for class in [MessageClass::Control, MessageClass::Bulk] {
        let (mut client, mut host) = channels();
        let zero = client.seal(class, &frame(class)).unwrap();
        let one = client.seal(class, &frame(class)).unwrap();
        assert!(host.open(class, zero.clone()).is_ok());
        assert!(host.open(class, zero.clone()).is_err());
        assert!(host.open(class, one).is_ok());
        assert!(host.open(class, zero.clone()).is_err());
        let (_, mut host) = channels();
        let gap = client.seal(class, &frame(class)).unwrap();
        assert!(host.open(class, gap.clone()).is_err());
        rejected(gap, class);
        let mut transport = wrapped();
        transport.start_connection(OTHER);
        transport
            .install(OTHER, secret(), ChannelRole::Host)
            .unwrap();
        transport
            .backend_mut()
            .inbox
            .extend([incoming(class, zero.clone()), incoming(class, zero)]);
        let events = poll(&mut transport);
        assert_eq!(events.len(), 2);
        assert!(!transport.has_channel(ID));
        assert!(transport.has_channel(OTHER));
    }
}
#[test]
fn transient_gap_accepted_duplicate_and_reordered_old_packets_dropped() {
    let (mut client, _) = channels();
    let old = client
        .seal(MessageClass::Transient, &frame(MessageClass::Transient))
        .unwrap();
    let skipped = client
        .seal(MessageClass::Transient, &frame(MessageClass::Transient))
        .unwrap();
    let newest = client
        .seal(MessageClass::Transient, &frame(MessageClass::Transient))
        .unwrap();
    let mut transport = wrapped();
    transport.backend_mut().inbox.extend(
        [newest.clone(), newest, old, skipped]
            .map(|record| incoming(MessageClass::Transient, record)),
    );
    assert_eq!(
        poll(&mut transport),
        vec![incoming(
            MessageClass::Transient,
            frame(MessageClass::Transient)
        )]
    );
    assert!(transport.has_channel(ID));
}
#[test]
fn exhaustion_never_wraps_and_closes() {
    for class in CLASSES {
        let (mut client, mut host) = channels();
        client.send_next[lane(class)] = u64::MAX - 1;
        host.recv_next[lane(class)] = u64::MAX - 1;
        let last = client.seal(class, &frame(class)).unwrap();
        assert!(host.open(class, last).is_ok());
        assert!(client.seal(class, &frame(class)).is_err());
        assert_eq!(client.send_next[lane(class)], u64::MAX);
        let mut transport = wrapped();
        if let ConnectionChannel::Secure(channel) = transport.connections.get_mut(&ID).unwrap() {
            channel.send_next[lane(class)] = u64::MAX;
        }
        assert_eq!(
            transport.send(ID, class, &frame(class)),
            Err(TransportError::ProtocolViolation)
        );
        assert!(!transport.has_channel(ID));
        assert_eq!(poll(&mut transport).len(), 1);
        let mut record = vec![0; RECORD_OVERHEAD + wire::HEADER_SIZE];
        record[..8].copy_from_slice(&u64::MAX.to_le_bytes());
        rejected(record, class);
    }
}
#[test]
fn owned_keys_zeroize_and_channel_cannot_be_reinstalled() {
    fn zeroizes_on_drop<T: ZeroizeOnDrop>() {}
    zeroizes_on_drop::<Zeroizing<[[u8; 32]; 6]>>();
    zeroizes_on_drop::<ChaCha20Poly1305>();
    let (mut channel, _) = channels();
    assert!(channel.keys.iter().any(|key| *key != [0; 32]));
    channel.keys.zeroize();
    assert!(channel.keys.iter().all(|key| *key == [0; 32]));
    let mut transport = wrapped();
    assert_eq!(
        transport.install(ID, secret(), ChannelRole::Host),
        Err(TransportError::ProtocolViolation)
    );
    transport.close(ID, DisconnectReason::Requested).unwrap();
    assert!(!transport.has_channel(ID));
    assert_eq!(
        transport.install(ID, secret(), ChannelRole::Host),
        Err(TransportError::ProtocolViolation)
    );
    assert_eq!(poll(&mut transport).len(), 1);
    assert!(poll(&mut transport).is_empty());
}
#[test]
fn new_pake_produces_unrelated_keys_and_old_record_cannot_cross_sessions() {
    use crate::network::{
        auth::{ClientHandshake, ServerHandshake, SessionPassword},
        session_control::*,
    };
    use puzzella_core::{
        session::{AuthorityCursor, ImageHash, SessionDefinition, SessionId},
        PlayerId,
    };
    fn authenticate() -> (AuthenticatedSecret, AuthenticatedSecret) {
        let password = SessionPassword::new("correct password".into()).unwrap();
        let (server, hello) = ServerHandshake::start(
            &password,
            SessionMetadata {
                definition: SessionDefinition {
                    id: SessionId(7),
                    image_hash: ImageHash([8; 32]),
                },
                cursor: AuthorityCursor::new(1, 2),
                host: PlayerId(42),
            },
            PlayerId(43),
        )
        .unwrap();
        let (client, proof) = ClientHandshake::start(&password, &hello).unwrap();
        let (confirmation, server_secret) = server.finish(proof).unwrap();
        let (_, client_secret) = client
            .finish(AuthAccepted {
                player: PlayerId(43),
                confirmation,
            })
            .unwrap();
        assert_eq!(server_secret.as_bytes().len(), 16);
        assert_eq!(server_secret.as_bytes(), client_secret.as_bytes());
        (client_secret, server_secret)
    }
    let (cs, hs) = authenticate();
    let mut client = Channel::new(cs, ChannelRole::Client);
    let host = Channel::new(hs, ChannelRole::Host);
    assert_eq!(*client.keys, *host.keys);
    let (_, hs) = authenticate();
    let mut replacement = Channel::new(hs, ChannelRole::Host);
    assert!(client
        .keys
        .iter()
        .zip(replacement.keys.iter())
        .all(|(a, b)| a != b));
    assert!(replacement
        .open(
            MessageClass::Control,
            client
                .seal(MessageClass::Control, &frame(MessageClass::Control))
                .unwrap()
        )
        .is_err());
}
#[test]
fn plaintext_and_malformed_outer_records_rejected_after_installation() {
    for class in CLASSES {
        rejected(frame(class), class);
        for length in [
            0,
            7,
            23,
            RECORD_OVERHEAD + wire::HEADER_SIZE - 1,
            record_limit(class) + 1,
        ] {
            rejected(vec![0; length], class);
        }
    }
}
#[test]
fn outer_limit_preserves_maximum_plaintext_and_inner_codec_limits() {
    for class in CLASSES {
        let (mut client, mut host) = channels();
        let plaintext = vec![1; wire::frame_limit(class)];
        let record = client.seal(class, &plaintext).unwrap();
        assert_eq!(record.len(), record_limit(class));
        assert_eq!(host.open(class, record).unwrap(), Some(plaintext));
        let mut transport = wrapped();
        assert_eq!(
            transport.send(ID, class, &vec![0; wire::frame_limit(class) + 1]),
            Err(TransportError::PayloadTooLarge)
        );
        assert!(transport.has_channel(ID));
    }
    let mut oversized = frame(MessageClass::Control);
    oversized.resize(wire::HEADER_SIZE + wire::MAX_SESSION_CONTROL_PAYLOAD + 1, 0);
    oversized[8..12]
        .copy_from_slice(&((wire::MAX_SESSION_CONTROL_PAYLOAD + 1) as u32).to_le_bytes());
    let (mut client, mut host) = channels();
    let opened = host
        .open(
            MessageClass::Control,
            client.seal(MessageClass::Control, &oversized).unwrap(),
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        wire::decode_for_class(&opened, MessageClass::Control),
        Err(wire::WireError::Oversized)
    );
}
#[test]
fn disconnect_failure_replacement_and_buffered_tail_destroy_old_channel() {
    for event in [
        TransportEvent::Disconnected {
            connection: ID,
            reason: DisconnectReason::RemoteClosed,
        },
        TransportEvent::ConnectionFailed {
            connection: ID,
            reason: DisconnectReason::ConnectionProblem,
        },
        TransportEvent::Connected { connection: ID },
    ] {
        let mut transport = wrapped();
        transport.backend_mut().inbox.push(event.clone());
        assert_eq!(poll(&mut transport), vec![event]);
        assert!(!transport.has_channel(ID));
    }
    let mut transport = wrapped();
    let (mut client, _) = channels();
    transport.backend_mut().inbox.extend([
        incoming(MessageClass::Control, vec![0]),
        incoming(
            MessageClass::Control,
            client
                .seal(MessageClass::Control, &frame(MessageClass::Control))
                .unwrap(),
        ),
    ]);
    assert_eq!(poll(&mut transport).len(), 1);
    assert!(!transport.has_channel(ID));
    transport.start_connection(ID); // Faulty backend reuse still never keeps old keys.
    assert!(!transport.has_channel(ID));
}
#[test]
fn failed_encrypted_send_never_reuses_a_nonce_or_falls_back_to_plaintext() {
    let mut transport = wrapped();
    transport.backend_mut().fail = Some(ID);
    assert_eq!(
        transport.send(ID, MessageClass::Control, &frame(MessageClass::Control)),
        Err(TransportError::NotConnected)
    );
    assert!(!transport.has_channel(ID));
    transport.backend_mut().fail = None;
    assert_eq!(
        transport.send(ID, MessageClass::Control, &frame(MessageClass::Control)),
        Err(TransportError::NotConnected)
    );
    assert!(transport.backend_mut().sent.is_empty());
}
#[test]
fn encrypted_packet_before_activation_rejected_and_plaintext_batch_tail_rechecked() {
    let mut transport = SecureTransport::new(FakeTransport::default());
    transport.start_connection(ID);
    let (mut client, _) = channels();
    transport.backend_mut().inbox.push(incoming(
        MessageClass::Control,
        client
            .seal(MessageClass::Control, &frame(MessageClass::Control))
            .unwrap(),
    ));
    assert!(matches!(
        poll(&mut transport).as_slice(),
        [TransportEvent::Disconnected {
            reason: DisconnectReason::ProtocolViolation,
            ..
        }]
    ));
    let mut transport = SecureTransport::new(FakeTransport::default());
    transport.start_connection(ID);
    transport.backend_mut().inbox.extend([
        incoming(MessageClass::Control, frame(MessageClass::Control)),
        incoming(MessageClass::Control, frame(MessageClass::Control)),
    ]);
    assert_eq!(poll(&mut transport).len(), 1);
    transport.install(ID, secret(), ChannelRole::Host).unwrap();
    assert!(matches!(
        poll(&mut transport).as_slice(),
        [TransportEvent::Disconnected {
            reason: DisconnectReason::ProtocolViolation,
            ..
        }]
    ));
}

#[test]
fn same_native_batch_waits_for_installation_then_decrypts_the_encrypted_tail() {
    let mut transport = SecureTransport::new(FakeTransport::default());
    transport.start_connection(ID);
    let (mut client, _) = channels();
    let plaintext = frame(MessageClass::Control);
    let encrypted = client.seal(MessageClass::Control, &plaintext).unwrap();
    transport.backend_mut().inbox.extend([
        incoming(MessageClass::Control, plaintext.clone()),
        incoming(MessageClass::Control, encrypted),
    ]);
    assert_eq!(
        poll(&mut transport),
        vec![incoming(MessageClass::Control, plaintext.clone())]
    );
    assert_eq!(transport.raw.len(), 1);
    transport.install(ID, secret(), ChannelRole::Host).unwrap();
    assert_eq!(
        poll(&mut transport),
        vec![incoming(MessageClass::Control, plaintext)]
    );
    assert!(transport.has_channel(ID));
}

#[test]
fn backend_close_failure_still_destroys_keys_and_blocks_buffered_gameplay() {
    let mut transport = wrapped();
    transport.backend_mut().fail_close = true;
    transport
        .close(ID, DisconnectReason::Requested)
        .unwrap_err();
    assert!(!transport.has_channel(ID));
    let (mut client, _) = channels();
    transport.backend_mut().inbox.push(incoming(
        MessageClass::Control,
        client
            .seal(MessageClass::Control, &frame(MessageClass::Control))
            .unwrap(),
    ));
    assert_eq!(
        poll(&mut transport),
        vec![TransportEvent::Disconnected {
            connection: ID,
            reason: DisconnectReason::Requested
        }]
    );
    assert_eq!(
        transport.send(ID, MessageClass::Control, &frame(MessageClass::Control)),
        Err(TransportError::NotConnected)
    );
}

#[test]
fn bootstrap_cleanup_does_not_reemit_the_backends_queued_disconnect() {
    let mut transport = wrapped();
    transport
        .backend_mut()
        .inbox
        .push(incoming(MessageClass::Control, vec![0]));
    assert_eq!(poll(&mut transport).len(), 1);
    transport.forget_connection(ID); // Same cleanup used by both bootstraps.
    assert!(poll(&mut transport).is_empty());
    assert!(!transport.has_channel(ID));
}

#[test]
fn listener_close_discards_secure_tail_before_it_can_complete_activation() {
    struct ListenerBackend(FakeTransport, bool);
    impl Transport for ListenerBackend {
        fn poll(&mut self, events: &mut Vec<TransportEvent>) -> Result<(), TransportError> {
            self.0.poll(events)
        }
        fn send(
            &mut self,
            connection: ConnectionId,
            class: MessageClass,
            payload: &[u8],
        ) -> Result<(), TransportError> {
            self.0.send(connection, class, payload)
        }
        fn close(
            &mut self,
            connection: ConnectionId,
            reason: DisconnectReason,
        ) -> Result<(), TransportError> {
            self.0.close(connection, reason)
        }
    }
    impl DirectIpTransport for ListenerBackend {
        fn listen(&mut self, _: SocketAddr) -> Result<ListenerId, TransportError> {
            Ok(ListenerId::new(1))
        }
        fn listener_address(&self, _: ListenerId) -> Result<SocketAddr, TransportError> {
            Ok("127.0.0.1:1".parse().unwrap())
        }
        fn connect(&mut self, _: SocketAddr) -> Result<ConnectionId, TransportError> {
            Ok(ID)
        }
        fn close_listener(&mut self, _: ListenerId) -> Result<(), TransportError> {
            self.0.close(ID, DisconnectReason::Requested)?;
            if self.1 {
                Err(TransportError::NotConnected)
            } else {
                Ok(())
            }
        }
    }
    for fail_after_close in [false, true] {
        let mut transport =
            SecureTransport::new(ListenerBackend(FakeTransport::default(), fail_after_close));
        transport.start_connection(ID);
        let (mut client, _) = channels();
        transport.backend_mut().0.inbox.extend([
            incoming(MessageClass::Control, frame(MessageClass::Control)),
            incoming(
                MessageClass::Control,
                client
                    .seal(MessageClass::Control, &frame(MessageClass::Control))
                    .unwrap(),
            ),
        ]);
        let mut events = Vec::new();
        transport.poll(&mut events).unwrap();
        assert_eq!(events.len(), 1);
        transport.install(ID, secret(), ChannelRole::Host).unwrap();
        transport.start_connection(OTHER);
        transport
            .install(OTHER, secret(), ChannelRole::Host)
            .unwrap();
        assert_eq!(
            transport.close_listener(ListenerId::new(1)).is_err(),
            fail_after_close
        );
        assert!(!transport.has_channel(ID));
        assert!(!transport.connections.contains_key(&ID));
        assert!(transport.has_channel(OTHER));
        events.clear();
        transport.poll(&mut events).unwrap();
        assert_eq!(
            events,
            vec![TransportEvent::Disconnected {
                connection: ID,
                reason: DisconnectReason::Requested
            }]
        );
    }
}
