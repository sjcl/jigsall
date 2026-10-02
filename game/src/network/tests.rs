use super::{
    client::{ClientRouteError, ClientRouteOutcome, ClientRouter},
    host::{HostRouteError, HostRouteOutcome, HostRouter},
    session::{SessionConnectionError, SessionConnections},
    transport::*,
    wire::{self, WireError, WireMessage},
};
use crate::{
    multiplayer::{
        protocol::{ProtocolCommandError, ProtocolDragContexts},
        replication::PeerReplicationState,
        GameSnapshot, SnapshotExpectation,
    },
    resources::PieceDataStore,
};
use bevy::math::{UVec2, Vec2};
use puzzella_core::{
    protocol::*, session::*, PieceBitSet, PieceId, PlayerId, PuzzleDefinition, GENERATOR_VERSION,
    MAX_PIECES,
};

const HOST: PlayerId = PlayerId(9);
const A: PlayerId = PlayerId(10);
const B: PlayerId = PlayerId(20);
const SESSION: SessionDefinition = SessionDefinition {
    id: SessionId(123),
    image_hash: ImageHash([7; 32]),
};
const HA: ConnectionId = ConnectionId::new(101);
const HB: ConnectionId = ConnectionId::new(102);

fn command(
    sequence: ClientCommandSequence,
    command: ProtocolPieceCommand,
) -> ProtocolCommandEnvelope {
    ProtocolCommandEnvelope {
        session: SESSION.id,
        authority_epoch: AuthorityEpoch(3),
        player: A,
        sequence,
        command,
    }
}
fn grab() -> ProtocolCommandEnvelope {
    command(
        ClientCommandSequence::Control(0),
        ProtocolPieceCommand::Grab {
            target: PieceTarget::Component(ComponentRef {
                member: PieceId(0),
                expected_size: 1,
            }),
        },
    )
}
fn update() -> ProtocolCommandEnvelope {
    command(
        ClientCommandSequence::Move {
            after_control_sequence: 0,
            tick: 0,
        },
        ProtocolPieceCommand::DragUpdate {
            delta: Vec2::new(-50.0, -50.0),
        },
    )
}
fn release() -> ProtocolCommandEnvelope {
    command(
        ClientCommandSequence::Control(1),
        ProtocolPieceCommand::Release {
            grab_sequence: 0,
            final_delta: Vec2::new(-100.0, -100.0),
        },
    )
}
fn message_event(connection: ConnectionId, message: &WireMessage) -> TransportEvent {
    TransportEvent::Message {
        connection,
        class: message.class(),
        payload: wire::encode(message).unwrap(),
    }
}
fn connected(connections: &mut SessionConnections, connection: ConnectionId, player: PlayerId) {
    connections.observe(&TransportEvent::Connected { connection });
    connections.assign_player(connection, player).unwrap();
}

#[derive(Default)]
struct FakeTransport {
    inbox: Vec<TransportEvent>,
    sent: Vec<TransportEvent>,
    fail: Option<ConnectionId>,
}
impl Transport for FakeTransport {
    fn poll(&mut self, events: &mut Vec<TransportEvent>) -> Result<(), TransportError> {
        events.append(&mut self.inbox);
        Ok(())
    }
    fn send(
        &mut self,
        connection: ConnectionId,
        class: MessageClass,
        payload: &[u8],
    ) -> Result<(), TransportError> {
        if self.fail == Some(connection) {
            return Err(TransportError::NotConnected);
        }
        self.sent.push(TransportEvent::Message {
            connection,
            class,
            payload: payload.to_vec(),
        });
        Ok(())
    }
    fn close(
        &mut self,
        connection: ConnectionId,
        reason: DisconnectReason,
    ) -> Result<(), TransportError> {
        self.inbox
            .push(TransportEvent::Disconnected { connection, reason });
        Ok(())
    }
}

struct State {
    store: PieceDataStore,
    session: AuthoritySession,
    connections: SessionConnections,
    replica: PeerReplicationState,
}
struct Scenario {
    definition: PuzzleDefinition,
    host: State,
    peers: [State; 2],
    contexts: ProtocolDragContexts,
}
impl Scenario {
    fn new() -> Self {
        let definition = PuzzleDefinition {
            generator_version: GENERATOR_VERSION,
            seed: 42,
            grid_size: UVec2::new(2, 1),
            image_size: UVec2::new(40, 20),
            snap_distance: 5.0,
        };
        let cursor = AuthorityCursor::new(3, 0);
        let mut store = PieceDataStore::default();
        store.initialize(
            (0..2)
                .map(|id| definition.correct_position(PieceId(id)) + Vec2::splat(100.0))
                .collect(),
        );
        let snapshot = GameSnapshot::capture(&store, &definition, SESSION, cursor).unwrap();
        let new_state = || {
            let mut store = PieceDataStore::default();
            snapshot
                .install(
                    &mut store,
                    SnapshotExpectation {
                        session: SESSION.id,
                        image_hash: SESSION.image_hash,
                        cursor,
                        definition: &definition,
                    },
                )
                .unwrap();
            State {
                store,
                session: AuthoritySession::new(SESSION, HOST, cursor),
                connections: SessionConnections::default(),
                replica: PeerReplicationState::default(),
            }
        };
        Self {
            host: new_state(),
            peers: [new_state(), new_state()],
            definition,
            contexts: ProtocolDragContexts::default(),
        }
    }
    fn host_router(&mut self) -> HostRouter<'_> {
        HostRouter {
            local_player: HOST,
            connections: &self.host.connections,
            contexts: &mut self.contexts,
            session: &mut self.host.session,
            store: &mut self.host.store,
            definition: Some(&self.definition),
        }
    }
    fn client_router(&mut self, index: usize, host_connection: ConnectionId) -> ClientRouter<'_> {
        let peer = &mut self.peers[index];
        ClientRouter {
            local_player: [A, B][index],
            host_connection,
            connections: &peer.connections,
            replica: &mut peer.replica,
            session: &mut peer.session,
            store: &mut peer.store,
            definition: Some(&self.definition),
        }
    }
    fn assert_final_equal(&self) {
        assert_ne!(
            self.host.store.states[0].flags & crate::resources::pieces::PLACED,
            0
        );
        for peer in &self.peers {
            assert_eq!(peer.session.cursor(), self.host.session.cursor());
            assert_eq!(peer.store.states, self.host.store.states);
            for id in 0..self.host.store.len() {
                assert_eq!(
                    peer.store.connectivity.minimum_member(PieceId(id as u32)),
                    self.host
                        .store
                        .connectivity
                        .minimum_member(PieceId(id as u32))
                );
            }
            assert_eq!(
                GameSnapshot::capture(
                    &peer.store,
                    &self.definition,
                    SESSION,
                    peer.session.cursor()
                )
                .unwrap(),
                GameSnapshot::capture(
                    &self.host.store,
                    &self.definition,
                    SESSION,
                    self.host.session.cursor()
                )
                .unwrap()
            );
        }
    }
}

#[test]
fn wire_header_and_protocol_roundtrips() {
    let authority = |event| {
        WireMessage::AuthorityEvent(ProtocolAuthorityEventEnvelope {
            session: SESSION.id,
            host: HOST,
            cursor: AuthorityCursor::new(3, 1),
            event,
        })
    };
    let messages = [
        WireMessage::ClientCommand(grab()),
        WireMessage::ClientCommand(update()),
        WireMessage::ClientCommand(release()),
        authority(ProtocolAuthorityEvent::GrabAccepted(GrabAccepted {
            player: A,
            grab_sequence: 0,
            accepted: PieceTarget::Components(vec![]),
            rejected: vec![],
        })),
        authority(ProtocolAuthorityEvent::ReleaseCommitted(ReleaseCommitted {
            player: A,
            grab_sequence: 0,
            final_delta: Vec2::splat(2.0),
            result: ReleaseResultFingerprint(u128::MAX),
        })),
        WireMessage::DragUpdate(RemoteDragUpdate {
            session: SessionId(u128::MAX),
            authority_epoch: AuthorityEpoch(u64::MAX),
            player: A,
            grab_sequence: u64::MAX,
            basis_sequence: u64::MAX,
            tick: u64::MAX,
            delta: Vec2::ONE,
        }),
        WireMessage::BulkChunk(vec![1, 2, 3]),
    ];
    for message in messages {
        let bytes = wire::encode(&message).unwrap();
        assert_eq!(&bytes[..4], b"PZLA");
        assert_eq!(&bytes[4..6], &wire::WIRE_VERSION.to_le_bytes());
        assert_eq!(bytes[7], 0);
        assert_eq!(
            u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize,
            bytes.len() - wire::HEADER_SIZE
        );
        assert_eq!(wire::decode(&bytes).unwrap(), message);
        assert_eq!(
            wire::decode_for_class(&bytes, message.class()).unwrap(),
            message
        );
    }
    let bytes = wire::encode(&WireMessage::ClientCommand(update())).unwrap();
    assert!(bytes.len() <= wire::HEADER_SIZE + wire::MAX_TRANSIENT_PAYLOAD);
}

#[test]
fn wire_rejects_untrusted_headers_and_payloads() {
    let valid = wire::encode(&WireMessage::ClientCommand(grab())).unwrap();
    for len in 0..valid.len() {
        assert!(wire::decode(&valid[..len]).is_err());
    }
    let mut bytes = valid.clone();
    bytes[0] = 0;
    assert_eq!(wire::decode(&bytes), Err(WireError::BadMagic));
    bytes = valid.clone();
    for version in [1u16, 3] {
        bytes[4..6].copy_from_slice(&version.to_le_bytes());
        assert_eq!(
            wire::decode(&bytes),
            Err(WireError::UnsupportedVersion(version))
        );
    }
    bytes = valid.clone();
    bytes[6] = 255;
    assert_eq!(wire::decode(&bytes), Err(WireError::UnknownKind(255)));
    bytes = valid.clone();
    bytes[7] = 1;
    assert_eq!(wire::decode(&bytes), Err(WireError::ReservedBits));
    bytes = valid.clone();
    bytes[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(wire::decode(&bytes), Err(WireError::Oversized));
    assert_eq!(
        wire::decode(&vec![0; wire::MAX_WIRE_MESSAGE + 1]),
        Err(WireError::Oversized)
    );
    bytes = valid.clone();
    bytes.push(0);
    assert_eq!(wire::decode(&bytes), Err(WireError::LengthMismatch));
    let len = (bytes.len() - 12) as u32;
    bytes[8..12].copy_from_slice(&len.to_le_bytes());
    assert_eq!(wire::decode(&bytes), Err(WireError::MalformedPayload));
    bytes = valid.clone();
    bytes[12..].fill(255);
    assert_eq!(wire::decode(&bytes), Err(WireError::MalformedPayload));
    bytes = valid.clone();
    bytes[6] = 4;
    assert_eq!(wire::decode(&bytes), Err(WireError::WrongClass));
    assert_eq!(
        wire::decode_for_class(&valid, MessageClass::Bulk),
        Err(WireError::WrongClass)
    );
    for (kind, limit) in [
        (1, wire::MAX_CONTROL_PAYLOAD),
        (3, wire::MAX_TRANSIENT_PAYLOAD),
        (5, wire::MAX_BULK_CHUNK),
    ] {
        bytes = valid.clone();
        bytes[6] = kind;
        bytes[8..12].copy_from_slice(&((limit + 1) as u32).to_le_bytes());
        assert_eq!(wire::decode(&bytes), Err(WireError::Oversized));
    }
    assert_eq!(
        wire::encode(&WireMessage::BulkChunk(vec![0; wire::MAX_BULK_CHUNK + 1])),
        Err(WireError::Oversized)
    );
    assert!(wire::encode(&WireMessage::BulkChunk(vec![0; wire::MAX_BULK_CHUNK])).is_ok());
}

#[test]
fn wire_bounded_sparse_and_million_piece_dense_decode() {
    let mut dense = PieceBitSet::new(MAX_PIECES);
    dense.fill();
    let target = PieceTarget::Dense(DenseTarget {
        members: dense,
        component_count: MAX_PIECES as u32,
        topology_digest: u128::MAX,
    });
    let message = WireMessage::ClientCommand(command(
        ClientCommandSequence::Control(0),
        ProtocolPieceCommand::Grab { target },
    ));
    let bytes = wire::encode(&message).unwrap();
    assert!(bytes.len() < wire::MAX_WIRE_MESSAGE);
    assert_eq!(wire::decode(&bytes).unwrap(), message);
    let target = PieceTarget::Components(vec![
        ComponentRef {
            member: PieceId(0),
            expected_size: 1
        };
        MAX_COMPONENT_REFS + 1
    ]);
    let message = WireMessage::ClientCommand(command(
        ClientCommandSequence::Control(0),
        ProtocolPieceCommand::Grab { target },
    ));
    assert_eq!(
        wire::decode(&wire::encode(&message).unwrap()),
        Err(WireError::MalformedPayload)
    );
    // Mirror Postcard's field order, but inject an invalid mask BEFORE it can
    // become a PieceBitSet. No change to the core serde representation.
    for (bit_len, words) in [
        (MAX_PIECES + 1, vec![]),
        (1, vec![2u32]),
        (32, vec![]),
        (MAX_PIECES, vec![0; MAX_PIECES.div_ceil(32) + 1]),
    ] {
        let payload = postcard::to_allocvec(&(
            SESSION.id,
            AuthorityEpoch(3),
            A,
            ClientCommandSequence::Control(0),
            0u32,
            2u32,
            bit_len,
            words,
            1u32,
            0u128,
        ))
        .unwrap();
        let mut frame = bytes[..12].to_vec();
        frame[8..12].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        frame.extend(payload);
        assert_eq!(wire::decode(&frame), Err(WireError::MalformedPayload));
    }
}

#[test]
fn malformed_binary_corpus_never_panics() {
    let mut seed = 0x73c1_a209u32;
    for len in 0..512 {
        let mut frame = Vec::from(&b"PZLA\x02\x00\x01\x00"[..]);
        frame.extend_from_slice(&(len as u32).to_le_bytes());
        for _ in 0..len {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            frame.push(seed as u8);
        }
        for kind in 1..=4 {
            frame[6] = kind;
            let _ = wire::decode(&frame);
        }
    }
}

#[test]
fn connection_player_mapping_has_explicit_lifecycle() {
    let mut connections = SessionConnections::default();
    assert_eq!(
        connections.assign_player(HA, A),
        Err(SessionConnectionError::UnknownConnection)
    );
    connections.observe(&TransportEvent::Connected { connection: HA });
    assert_eq!(connections.player(HA), None);
    connections.assign_player(HA, A).unwrap();
    connections.assign_player(HA, A).unwrap();
    connections.observe(&TransportEvent::Connected { connection: HB });
    assert_eq!(
        connections.assign_player(HB, A),
        Err(SessionConnectionError::PlayerAlreadyAssigned)
    );
    assert_eq!(
        connections.assign_player(HA, B),
        Err(SessionConnectionError::ConnectionAlreadyAssigned)
    );
    connections.observe(&TransportEvent::Disconnected {
        connection: HA,
        reason: DisconnectReason::RemoteClosed,
    });
    assert_eq!(connections.player(HA), None);
    connections.assign_player(HB, A).unwrap();
    connections.observe(&TransportEvent::ConnectionFailed {
        connection: HB,
        reason: DisconnectReason::ConnectionProblem,
    });
    assert_eq!(connections.peers().count(), 0);
}

#[test]
fn fake_transport_routes_grab_drag_release_without_host_echo() {
    let mut s = Scenario::new();
    let mut transport = FakeTransport::default();
    connected(&mut s.host.connections, HA, A);
    connected(&mut s.host.connections, HB, B);
    for (index, connection) in [HA, HB].into_iter().enumerate() {
        connected(&mut s.peers[index].connections, connection, HOST);
    }
    for (step, command) in [grab(), update(), release()].into_iter().enumerate() {
        s.client_router(0, HA)
            .send_command(&mut transport, &command)
            .unwrap();
        let request = transport.sent.pop().unwrap();
        let before = s.host.session.cursor();
        let mut router = s.host_router();
        let HostRouteOutcome::Applied(outcome) = router.route(&request).unwrap() else {
            panic!("command outcome")
        };
        assert!(router
            .publish(&mut transport, Some(HA), &outcome)
            .unwrap()
            .is_empty());
        assert_eq!(
            s.host.session.cursor().sequence.0,
            before.sequence.0 + u64::from(step != 1)
        );
        let messages = std::mem::take(&mut transport.sent);
        assert_eq!(messages.len(), if step == 1 { 1 } else { 2 });
        for event in messages {
            let TransportEvent::Message {
                connection, class, ..
            } = &event
            else {
                unreachable!()
            };
            assert_eq!(
                *class,
                if step == 1 {
                    MessageClass::Transient
                } else {
                    MessageClass::Control
                }
            );
            let index = usize::from(*connection == HB);
            let routed = s.client_router(index, *connection).route(&event).unwrap();
            assert!(matches!(
                (step, routed),
                (1, ClientRouteOutcome::Drag(_)) | (0 | 2, ClientRouteOutcome::Authority(_))
            ));
        }
        if step == 1 {
            assert_eq!(
                s.peers[1]
                    .replica
                    .remote_drag(&s.peers[1].session, &s.peers[1].store, A)
                    .unwrap()
                    .delta,
                Vec2::splat(-50.0)
            );
        }
    }
    assert_eq!(s.host.session.cursor().sequence.0, 2);
    s.assert_final_equal();
    let bulk = message_event(HA, &WireMessage::BulkChunk(vec![1, 2]));
    assert!(
        matches!(s.host_router().route(&bulk).unwrap(), HostRouteOutcome::BulkChunk(v) if v == vec![1, 2])
    );
    assert!(
        matches!(s.client_router(0, HA).route(&bulk).unwrap(), ClientRouteOutcome::BulkChunk(v) if v == vec![1, 2])
    );
    transport.close(HA, DisconnectReason::Requested).unwrap();
    let mut events = Vec::new();
    transport.poll(&mut events).unwrap();
    s.host.connections.observe(&events[0]);
    assert_eq!(s.host.connections.player(HA), None);
}

#[test]
fn routing_rejects_spoofed_identities_classes_and_directions() {
    let mut s = Scenario::new();
    let request = message_event(HA, &WireMessage::ClientCommand(grab()));
    assert_eq!(
        s.host_router().route(&request).unwrap_err(),
        HostRouteError::Unauthenticated
    );
    connected(&mut s.host.connections, HA, B);
    assert_eq!(
        s.host_router().route(&request).unwrap_err(),
        HostRouteError::Command(ProtocolCommandError::WrongPlayer)
    );
    connected(&mut s.host.connections, HB, HOST);
    assert_eq!(
        s.host_router()
            .route(&message_event(HB, &WireMessage::ClientCommand(grab())))
            .unwrap_err(),
        HostRouteError::HostConnection
    );
    connected(&mut s.peers[0].connections, HA, B);
    assert_eq!(
        s.client_router(0, HA).route(&request).unwrap_err(),
        ClientRouteError::UnauthenticatedHost
    );
    assert_eq!(
        s.client_router(0, HB).route(&request).unwrap_err(),
        ClientRouteError::WrongConnection
    );
    assert_eq!(s.host.session.cursor(), AuthorityCursor::new(3, 0));
    s.peers[0].connections = SessionConnections::default();
    connected(&mut s.peers[0].connections, HA, HOST);
    assert_eq!(
        s.client_router(0, HA).route(&request).unwrap_err(),
        ClientRouteError::WrongDirection
    );
    let TransportEvent::Message { payload, .. } = request else {
        unreachable!()
    };
    assert_eq!(
        s.host_router()
            .route(&TransportEvent::Message {
                connection: HA,
                class: MessageClass::Bulk,
                payload
            })
            .unwrap_err(),
        HostRouteError::Wire(WireError::WrongClass)
    );
}

#[test]
fn failed_publication_preserves_outcome_and_attempts_other_peers() {
    let mut s = Scenario::new();
    connected(&mut s.host.connections, HA, A);
    connected(&mut s.host.connections, HB, B);
    let unassigned = ConnectionId::new(103);
    s.host.connections.observe(&TransportEvent::Connected {
        connection: unassigned,
    });
    let mut transport = FakeTransport {
        fail: Some(HA),
        ..Default::default()
    };
    let mut router = s.host_router();
    let HostRouteOutcome::Applied(outcome) = router
        .route(&message_event(HA, &WireMessage::ClientCommand(grab())))
        .unwrap()
    else {
        unreachable!()
    };
    assert_eq!(
        router.publish(&mut transport, Some(HA), &outcome).unwrap(),
        vec![(HA, TransportError::NotConnected)]
    );
    assert_eq!(transport.sent.len(), 1);
    assert!(matches!(
        &transport.sent[0],
        TransportEvent::Message { connection: HB, .. }
    ));
    let cursor = router.session.cursor();
    transport.fail = None;
    router.publish(&mut transport, Some(HA), &outcome).unwrap();
    assert_eq!(router.session.cursor(), cursor);
}

mod golden;

#[cfg(feature = "gns")]
mod localhost;

#[test]
fn rotation_commands_and_semantic_events_use_reliable_control_wire() {
    let target = PieceTarget::Component(ComponentRef {
        member: PieceId(5),
        expected_size: 3,
    });
    let messages = [
        WireMessage::ClientCommand(command(
            ClientCommandSequence::Control(7),
            ProtocolPieceCommand::RotateDrag {
                grab_sequence: 5,
                final_delta: Vec2::ONE,
                through_tick: Some(20),
                quarter_turns: -1,
            },
        )),
        WireMessage::AuthorityEvent(ProtocolAuthorityEventEnvelope {
            session: SESSION.id,
            host: HOST,
            cursor: AuthorityCursor::new(3, 8),
            event: ProtocolAuthorityEvent::DragRotationCommitted(DragRotationCommitted {
                player: A,
                grab_sequence: 5,
                basis_sequence: 7,
                through_tick: Some(20),
                final_delta: Vec2::ONE,
                quarter_turns: 3,
                result: ReleaseResultFingerprint(123),
            }),
        }),
        WireMessage::ClientCommand(command(
            ClientCommandSequence::Control(7),
            ProtocolPieceCommand::Rotate {
                target: target.clone(),
                quarter_turns: -1,
            },
        )),
        WireMessage::AuthorityEvent(ProtocolAuthorityEventEnvelope {
            session: SESSION.id,
            host: HOST,
            cursor: AuthorityCursor::new(3, 8),
            event: ProtocolAuthorityEvent::RotationCommitted(RotationCommitted {
                player: A,
                accepted: target,
                quarter_turns: 3,
                result: ReleaseResultFingerprint(123),
            }),
        }),
    ];
    for message in messages {
        assert_eq!(message.class(), MessageClass::Control);
        let bytes = wire::encode(&message).unwrap();
        assert_eq!(
            wire::decode_for_class(&bytes, MessageClass::Control).unwrap(),
            message
        );
        assert_eq!(
            wire::decode_for_class(&bytes, MessageClass::Transient),
            Err(WireError::WrongClass)
        );
    }
}

#[path = "tests/local_identity.rs"]
mod local_identity;
