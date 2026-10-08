use super::*;
use crate::network::gns::P2P_VIRTUAL_PORT;
use crate::network::{
    auth::SessionPassword,
    bootstrap::{ClientBootstrap, ConnectionState, HostBootstrap},
    rate_limit::{InboundRateLimiter, PREAUTH_INBOUND_POLICY},
    secure::SecureTransport,
    session::SessionConnections,
    session_control::{SessionControlMessage, SessionMetadata},
    wire,
};
use jigsall_core::{
    session::{AuthorityCursor, ImageHash, SessionDefinition, SessionId},
    PlayerId,
};
use serde::{Deserialize, Serialize};
use signaling::{InMemorySignaling, MAX_QUEUED_SIGNALS};
use std::{
    io::{BufRead, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{mpsc, Arc, Mutex},
    time::Duration,
};

#[derive(Serialize, Deserialize, Debug)]
enum Frame {
    Peer([u8; 16]),
    Start([u8; 16]),
    RoomCode(String),
    StartCode(String),
    ControlStop,
    ControlGone,
    Signal([u8; 16], Vec<u8>),
    Connected(String),
    Authenticated,
    Exchange,
    Rotate,
    Rotated,
    Received,
    Close,
    Closed,
    Finish,
}
fn emit(frame: Frame) {
    let mut out = std::io::stdout().lock();
    writeln!(out, "P2P:{}", serde_json::to_string(&frame).unwrap()).unwrap();
    out.flush().unwrap();
}
fn test_messages() -> [(MessageClass, Vec<u8>); 3] {
    let control = wire::encode(&wire::WireMessage::SessionControl(
        SessionControlMessage::SecureChannelReady,
    ))
    .unwrap();
    let transient = wire::encode(&wire::WireMessage::DragUpdate(
        jigsall_core::protocol::RemoteDragUpdate {
            session: SessionId(7788),
            authority_epoch: jigsall_core::session::AuthorityEpoch(1),
            player: PlayerId(10),
            grab_sequence: 0,
            basis_sequence: 0,
            tick: 1,
            delta: bevy::math::Vec2::ONE,
        },
    ))
    .unwrap();
    let bulk = wire::encode(&wire::WireMessage::BulkTransfer(
        crate::network::bulk::BulkTransferMessage::Chunk {
            transfer_id: crate::network::bulk::TransferId(7),
            offset: 0,
            data: vec![42; 1024],
        },
    ))
    .unwrap();
    [
        (MessageClass::Control, control),
        (MessageClass::Transient, transient),
        (MessageClass::Bulk, bulk),
    ]
}
type SentRecords = Arc<Mutex<Vec<(MessageClass, Vec<u8>)>>>;
struct Observed {
    inner: GnsP2p,
    records: SentRecords,
}
impl Transport for Observed {
    fn origin(&self, id: ConnectionId) -> Option<Origin> {
        self.inner.origin(id)
    }
    fn reliable_egress(&self, id: ConnectionId) -> Result<ReliableEgress, TransportError> {
        self.inner.reliable_egress(id)
    }
    fn activate_secure_channel(&mut self, id: ConnectionId) -> Result<(), TransportError> {
        self.inner.activate_secure_channel(id)
    }
    fn mark_ready(&mut self, id: ConnectionId) -> Result<(), TransportError> {
        self.inner.mark_ready(id)
    }
    fn poll(&mut self, events: &mut Vec<TransportEvent>) -> Result<(), TransportError> {
        let start = events.len();
        self.inner.poll(events)?;
        for event in &events[start..] {
            if let TransportEvent::Connected { connection } = event {
                self.inner.connections[connection]
                    .native
                    .assert_send_rate(256 * 1024);
                if std::env::var_os("JIGSALL_TURN_INITIAL_UNAVAILABLE").is_some() {
                    self.inner.connections[connection]
                        .native
                        .assert_turn_user("");
                    assert!(!self.inner.connections[connection].native.is_relay());
                }
                emit(Frame::Connected(
                    self.inner.connections[connection].native.details(),
                ));
            }
        }
        Ok(())
    }
    fn send(
        &mut self,
        id: ConnectionId,
        class: MessageClass,
        bytes: &[u8],
    ) -> Result<(), TransportError> {
        self.records.lock().unwrap().push((class, bytes.to_vec()));
        self.inner.send(id, class, bytes)
    }
    fn close(&mut self, id: ConnectionId, reason: DisconnectReason) -> Result<(), TransportError> {
        self.inner.close(id, reason)
    }
}

// Re-entered only by the parent test. Separate processes provide independent
// native singleton identities and exercise the real inter-process UDP route.
#[test]
fn gns_p2p_child() {
    let Ok(role) = std::env::var("JIGSALL_P2P_TEST_ROLE") else {
        return;
    };
    let host = role == "host";
    #[cfg(feature = "rendezvous")]
    let mut rendezvous = None;
    #[cfg(feature = "rendezvous")]
    let mut backend = if let Ok(url) = std::env::var("JIGSALL_P2P_RENDEZVOUS") {
        let endpoint = super::super::rendezvous::EndpointUrl::loopback_for_test(&url).unwrap();
        let (backend, adapter) = super::super::rendezvous::RendezvousAdapter::new(
            endpoint,
            P2P_VIRTUAL_PORT,
            IceConfig::default(),
        )
        .unwrap();
        rendezvous = Some(adapter);
        backend
    } else {
        GnsP2p::new_routed(P2P_VIRTUAL_PORT, IceConfig::default()).unwrap()
    };
    #[cfg(not(feature = "rendezvous"))]
    let mut backend = GnsP2p::new_routed(P2P_VIRTUAL_PORT, IceConfig::default()).unwrap();
    if let Ok(address) = std::env::var("JIGSALL_TURN_TEST_ADDRESS") {
        backend
            .install_turn(&[TurnServer {
                address,
                username: "user-A".into(),
                password: "password-A".into(),
            }])
            .unwrap();
    }
    let mailbox = backend.signaling();
    emit(Frame::Peer(backend.peer_id().to_bytes()));
    let (tx, rx) = mpsc::sync_channel(256);
    // Test-only pipe reader. The application backend creates no worker threads.
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else {
                break;
            };
            let frame: Frame = serde_json::from_str(&line).unwrap();
            if tx.send(frame).is_err() {
                break;
            }
        }
    });
    let fixture_peer = || {
        let Frame::Start(remote) = rx.recv_timeout(Duration::from_secs(10)).unwrap() else {
            panic!("expected start");
        };
        // Local foundation fixture binding, never from a peer's signal bytes.
        let origin = RouteOrigin::from_authenticated_route(
            [1; 16],
            [2; 16],
            [if host { 12 } else { 11 }; 16],
        );
        mailbox
            .authorize_peer(PeerId::from_bytes(remote), origin)
            .unwrap();
        remote
    };
    #[cfg(feature = "rendezvous")]
    let remote = if let Some(adapter) = &mut rendezvous {
        use super::super::rendezvous::RendezvousEvent;
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut welcome = false;
        let mut code = None;
        let mut joining = false;
        'ready: loop {
            assert!(
                Instant::now() < deadline,
                "room handshake timed out ({role})"
            );
            while let Ok(frame) = rx.try_recv() {
                let Frame::StartCode(value) = frame else {
                    panic!("expected room code");
                };
                code = Some(value);
            }
            for event in adapter.poll() {
                match event {
                    RendezvousEvent::Welcome { .. } => {
                        welcome = true;
                        if host {
                            adapter.create_room().unwrap();
                        }
                    }
                    RendezvousEvent::RoomCreated { room_code, .. } => {
                        emit(Frame::RoomCode(room_code.to_string()))
                    }
                    RendezvousEvent::HostReady { peer_id, .. }
                    | RendezvousEvent::PeerJoined { peer_id, .. } => {
                        break 'ready peer_id.to_bytes()
                    }
                    other => panic!("room handshake: {other:?}"),
                }
            }
            if !host && welcome && !joining {
                if let Some(code) = code.take() {
                    adapter.join_room(code.parse().unwrap()).unwrap();
                    joining = true;
                }
            }
            std::thread::park_timeout(Duration::from_millis(1));
        }
    } else {
        fixture_peer()
    };
    #[cfg(not(feature = "rendezvous"))]
    let remote = fixture_peer();
    let Some(Origin::Route(origin)) = mailbox.origin(PeerId::from_bytes(remote)).unwrap() else {
        panic!("route required");
    };
    let mut connection = if host {
        None
    } else {
        Some(
            backend
                .connect_peer(PeerId::from_bytes(remote), P2P_VIRTUAL_PORT)
                .unwrap(),
        )
    };
    let records = Arc::new(Mutex::new(Vec::new()));
    let mut transport = SecureTransport::new(Observed {
        inner: backend,
        records: records.clone(),
    });
    let password = || SessionPassword::new("p2p test password".into()).unwrap();
    let mut hb = HostBootstrap::new(
        password(),
        SessionMetadata {
            definition: SessionDefinition {
                id: SessionId(7788),
                image_hash: ImageHash([42; 32]),
            },
            cursor: AuthorityCursor::new(1, 0),
            host: PlayerId(9),
        },
        [],
        Instant::now(),
    );
    let mut cb = connection.map(|id| ClientBootstrap::new(password(), id));
    let mut connections = SessionConnections::default();
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut connected = 0;
    let mut closed = 0;
    let mut authenticated = false;
    let mut exchange = false;
    let mut sent = false;
    let mut received = [false; 3];
    let mut reported_receive = false;
    #[cfg(feature = "rendezvous")]
    let mut control_stopping = false;
    #[cfg(feature = "rendezvous")]
    let mut control_reported = false;
    loop {
        assert!(Instant::now() < deadline, "P2P child timed out ({role})");
        while let Ok(frame) = rx.try_recv() {
            match frame {
                Frame::Signal(peer, bytes) => {
                    mailbox.receive(PeerId::from_bytes(peer), &bytes).unwrap();
                }
                Frame::Rotate => {
                    let address = std::env::var("JIGSALL_TURN_TEST_ADDRESS").unwrap();
                    transport
                        .backend_mut()
                        .inner
                        .install_turn(&[TurnServer {
                            address,
                            username: "user-B".into(),
                            password: "password-B".into(),
                        }])
                        .unwrap();
                    // Only future connection defaults changed; this connection stays A.
                    for connection in transport.backend_mut().inner.connections.values() {
                        connection.native.assert_turn_user("user-A");
                    }
                    emit(Frame::Rotated);
                    exchange = false;
                    sent = false;
                    received = [false; 3];
                    reported_receive = false;
                    records.lock().unwrap().clear();
                }
                Frame::Exchange => {
                    assert!(authenticated);
                    exchange = true;
                }
                #[cfg(feature = "rendezvous")]
                Frame::ControlStop => {
                    assert!(authenticated && received.into_iter().all(|x| x));
                    rendezvous.as_mut().unwrap().shutdown();
                    control_stopping = true;
                    exchange = false;
                    sent = false;
                    received = [false; 3];
                    reported_receive = false;
                    records.lock().unwrap().clear();
                }
                Frame::Close => transport
                    .close(connection.unwrap(), DisconnectReason::Requested)
                    .unwrap(),
                Frame::Finish => {
                    assert_eq!((connected, closed), (1, 1));
                    assert!(authenticated && received.into_iter().all(|x| x));
                    assert!(!transport.has_channel(connection.unwrap()));
                    assert_eq!(
                        mailbox.receive(PeerId::from_bytes([8; 16]), &[1]),
                        Err(TransportError::ProtocolViolation)
                    );
                    assert_eq!(mailbox.receive(PeerId::from_bytes(remote), &[1]), Ok(()));
                    drop(transport);
                    assert_eq!(
                        mailbox.receive(PeerId::from_bytes([8; 16]), &[1]),
                        Err(TransportError::NotConnected)
                    );
                    return;
                }
                _ => panic!("unexpected parent frame"),
            }
        }
        #[cfg(feature = "rendezvous")]
        if let Some(adapter) = &mut rendezvous {
            for event in adapter.poll() {
                assert!(
                    control_stopping
                        || !matches!(
                            event,
                            super::super::rendezvous::RendezvousEvent::Disconnected(_)
                                | super::super::rendezvous::RendezvousEvent::RoomClosed
                        ),
                    "unexpected control loss {event:?}"
                );
            }
            if control_stopping && !control_reported && adapter.worker_finished() {
                assert_eq!(
                    mailbox.origin(PeerId::from_bytes(remote)).unwrap(),
                    Some(Origin::Route(origin))
                );
                assert!(transport.has_channel(connection.unwrap()));
                control_reported = true;
                emit(Frame::ControlGone);
            }
        }
        let mut events = Vec::new();
        transport.poll(&mut events).unwrap();
        for event in events {
            match &event {
                TransportEvent::Connected { connection: id } => {
                    connected += 1;
                    connection = Some(*id);
                    assert!(!transport.has_channel(*id));
                    assert_eq!(transport.origin(*id), Some(Origin::Route(origin)));
                    assert!(connections.player(*id).is_none());
                }
                TransportEvent::ConnectionFailed { .. } => panic!("P2P failed: {event:?}"),
                TransportEvent::Disconnected { .. } => {
                    closed += 1;
                    assert_eq!(closed, 1);
                    emit(Frame::Closed);
                    continue;
                }
                TransportEvent::Message { class, payload, .. } if exchange => {
                    let index = test_messages()
                        .iter()
                        .position(|(c, bytes)| c == class && bytes == payload)
                        .expect("exact secured payload");
                    received[index] = true;
                    continue;
                }
                _ => {}
            }
            if host {
                hb.process(&event, &mut transport, &mut connections, Instant::now())
                    .unwrap();
            } else if let Some(cb) = &mut cb {
                cb.process(&event, &mut transport, &mut connections, Instant::now())
                    .unwrap();
            }
        }
        if let Some(id) = connection {
            let state = if host {
                hb.state(id)
            } else {
                cb.as_ref().and_then(ClientBootstrap::state)
            };
            if !authenticated && state == Some(ConnectionState::Authenticated) {
                assert!(transport.has_channel(id));
                assert!(connections.player(id).is_none()); // PAKE success != Ready
                transport.backend_mut().inner.connections[&id]
                    .native
                    .assert_send_rate(crate::network::lifecycle::BULK_BYTES_PER_SECOND as i32);
                authenticated = true;
                #[cfg(feature = "rendezvous")]
                if host {
                    if let Some(adapter) = &mut rendezvous {
                        adapter.confirm_peer(PeerId::from_bytes(remote));
                    }
                }
                records.lock().unwrap().clear();
                emit(Frame::Authenticated);
            }
            if exchange && !sent {
                for (class, payload) in test_messages() {
                    transport.send(id, class, &payload).unwrap();
                }
                let actual = records.lock().unwrap();
                assert_eq!(actual.len(), 3);
                for ((class, plaintext), (observed_class, ciphertext)) in
                    test_messages().iter().zip(actual.iter())
                {
                    assert_eq!(class, observed_class);
                    assert_ne!(plaintext, ciphertext);
                    assert_eq!(
                        ciphertext.len(),
                        plaintext.len() + crate::network::secure::RECORD_OVERHEAD
                    );
                }
                assert!(transport.reliable_egress(id).is_ok());
                sent = true;
            }
        }
        if !reported_receive && received.into_iter().all(|x| x) {
            reported_receive = true;
            emit(Frame::Received);
        }
        #[cfg(feature = "rendezvous")]
        let external_control = rendezvous.is_some();
        #[cfg(not(feature = "rendezvous"))]
        let external_control = false;
        for _ in 0..if external_control {
            0
        } else {
            MAX_QUEUED_SIGNALS
        } {
            let Some(signal) = mailbox.pop_outbound() else {
                break;
            };
            emit(Frame::Signal(signal.peer.to_bytes(), signal.payload));
        }
        std::thread::park_timeout(Duration::from_millis(1));
    }
}

#[cfg(feature = "rendezvous")]
mod rendezvous_smoke;
pub(in crate::network::gns) mod turn_rotation;

struct Process {
    child: Child,
    input: ChildStdin,
}
impl Process {
    fn send(&mut self, frame: Frame) {
        writeln!(self.input, "{}", serde_json::to_string(&frame).unwrap()).unwrap();
        self.input.flush().unwrap();
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn gns_localhost_p2p_native_ice_password_secure_lanes_and_close() {
    let (tx, rx) = mpsc::sync_channel(256);
    let mut peers = Vec::new();
    for (index, role) in ["client", "host"].into_iter().enumerate() {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "network::gns::p2p::tests::gns_p2p_child",
                "--nocapture",
            ])
            .env("JIGSALL_P2P_TEST_ROLE", role)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = child.stdout.take().unwrap();
        let tx = tx.clone();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(output).lines() {
                let Ok(line) = line else {
                    break;
                };
                if let Some(json) = line.strip_prefix("P2P:") {
                    let frame: Frame = serde_json::from_str(json).unwrap();
                    if tx.send((index, frame)).is_err() {
                        break;
                    }
                } else if !line.is_empty() {
                    eprintln!("P2P child {index}: {line}");
                }
            }
        });
        peers.push(Process { child, input });
    }
    let deadline = Instant::now() + Duration::from_secs(35);
    let mailboxes = [SignalingEndpoint::default(), SignalingEndpoint::default()];
    let mut ids = [None; 2];
    let mut relay = InMemorySignaling::default();
    let mut started = false;
    let mut auth = [false; 2];
    let mut exchanged = false;
    let mut received = [false; 2];
    let mut closing = false;
    let mut closed = [false; 2];
    let mut connected = [0; 2];
    while !closed.into_iter().all(|x| x) {
        assert!(Instant::now() < deadline, "P2P parent timed out");
        for peer in &mut peers {
            assert!(
                peer.child.try_wait().unwrap().is_none(),
                "child exited early"
            );
        }
        if let Ok((index, frame)) = rx.recv_timeout(Duration::from_millis(5)) {
            match frame {
                Frame::Peer(bytes) => {
                    let id = PeerId::from_bytes(bytes);
                    ids[index] = Some(id);
                    relay.register(id, mailboxes[index].clone()).unwrap();
                }
                Frame::Signal(to, payload) => {
                    assert!(mailboxes[index].send(PeerId::from_bytes(to), &payload));
                    // Duplicate valid GNS signals must not create duplicate
                    // connections, Connected events or application records.
                    assert!(mailboxes[index].send(PeerId::from_bytes(to), &payload));
                }
                Frame::Connected(details) => {
                    connected[index] += 1;
                    eprintln!("P2P {index}: {details}");
                    assert!(details.contains("ICE"), "must use actual ICE transport");
                }
                Frame::Authenticated => auth[index] = true,
                Frame::Received => received[index] = true,
                Frame::Closed => {
                    assert!(!closed[index]);
                    closed[index] = true;
                }
                _ => panic!("unexpected child frame"),
            }
        }
        if !started && ids.iter().all(Option::is_some) {
            assert_ne!(ids[0], ids[1]);
            peers[1].send(Frame::Start(ids[0].unwrap().to_bytes()));
            peers[0].send(Frame::Start(ids[1].unwrap().to_bytes()));
            started = true;
        }
        relay.poll();
        for index in 0..2 {
            while let Some(signal) = mailboxes[index].pop_inbound() {
                peers[index].send(Frame::Signal(signal.peer.to_bytes(), signal.payload));
            }
        }
        if !exchanged && auth.into_iter().all(|x| x) {
            for peer in &mut peers {
                peer.send(Frame::Exchange);
            }
            exchanged = true;
        }
        if !closing && received.into_iter().all(|x| x) {
            peers[0].send(Frame::Close);
            closing = true;
        }
    }
    assert_eq!(connected, [1, 1]);
    for peer in &mut peers {
        peer.send(Frame::Finish);
    }
    for peer in &mut peers {
        assert!(peer.child.wait().unwrap().success());
    }
}

#[test]
fn gns_p2p_pending_timeout_duplicate_signals_and_drop_cleanup() {
    let _guard = BACKEND_TEST_LOCK.lock().unwrap();
    assert!(matches!(
        GnsP2p::new_unverified_for_test(
            0,
            IceConfig {
                allow_public_candidates: true,
                stun_servers: vec!["invalid\0server".into()]
            }
        ),
        Err(TransportError::ProtocolViolation)
    ));
    let mut backend = GnsP2p::new_unverified_for_test(0, IceConfig::default()).unwrap();
    let mailbox = backend.signaling();
    assert!(matches!(
        GnsP2p::new_unverified_for_test(1, IceConfig::default()),
        Err(TransportError::Capacity)
    ));
    let id = backend
        .connect_peer(PeerId::from_bytes([42; 16]), 0)
        .unwrap();
    assert_eq!(backend.remote_peer(id), Some(PeerId::from_bytes([42; 16])));
    for _ in 0..10 {
        mailbox
            .receive(PeerId::from_bytes([42; 16]), &[0, 255, 7])
            .unwrap();
    }
    let mut events = Vec::new();
    backend.poll(&mut events).unwrap();
    backend.maintain(Instant::now() + CONNECTING_TIMEOUT, &mut events);
    assert_eq!(
        events,
        vec![TransportEvent::ConnectionFailed {
            connection: id,
            reason: DisconnectReason::BackendConnectionTimeout
        }]
    );
    assert!(backend.connections.is_empty());
    assert_eq!(
        backend.close(id, DisconnectReason::Requested),
        Err(TransportError::UnknownConnection)
    );
    events.clear();
    backend.poll(&mut events).unwrap();
    assert!(events.is_empty());
    drop(backend);
    assert_eq!(
        mailbox.receive(PeerId::from_bytes([42; 16]), &[1]),
        Err(TransportError::NotConnected)
    );
    let mut replacement = GnsP2p::new_unverified_for_test(0, IceConfig::default()).unwrap();
    let fresh = replacement
        .connect_peer(PeerId::from_bytes([43; 16]), 0)
        .unwrap();
    assert_ne!(id, fresh);
}

static BACKEND_TEST_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn gns_p2p_connection_capacity_matches_direct_ip_and_rejects_full_starts() {
    let _guard = BACKEND_TEST_LOCK.lock().unwrap();
    for (total, connecting, pending, expected) in super::super::tests::CAPACITY_CASES {
        let mut backend = GnsP2p::new_unverified_for_test(0, IceConfig::default()).unwrap();
        for state in super::super::tests::states(total, connecting, pending) {
            let [_, receiver] = native::Connection::test_pair();
            let id = ConnectionId::new(token().unwrap());
            backend.insert(id, receiver, PeerId::from_bytes([42; 16]), None);
            backend.connections.get_mut(&id).unwrap().state = state;
        }
        assert_eq!(backend.has_connection_capacity(), expected);
        assert_eq!(
            backend.has_connection_capacity(),
            super::super::direct_ip::tests::capacity_for_test(total, connecting, pending)
        );
        if !expected {
            assert_eq!(
                backend.connect_peer(PeerId::from_bytes([43; 16]), 0),
                Err(TransportError::Capacity)
            );
            assert_eq!(backend.connections.len(), total);
        }
    }
}

#[test]
fn gns_p2p_authentication_and_ready_preserve_admission_occupancy() {
    let _guard = BACKEND_TEST_LOCK.lock().unwrap();
    let mut backend = GnsP2p::new_unverified_for_test(0, IceConfig::default()).unwrap();
    let origin = Some(Origin::Route(RouteOrigin::from_authenticated_route(
        [1; 16], [2; 16], [3; 16],
    )));
    let mut senders = Vec::new();
    let mut ids = Vec::new();
    for _ in 0..lifecycle::MAX_PENDING_PER_ORIGIN {
        let [sender, receiver] = native::Connection::test_pair();
        senders.push(sender);
        let id = ConnectionId::new(token().unwrap());
        backend.insert(id, receiver, PeerId::from_bytes([42; 16]), origin);
        ids.push(id);
    }
    let id = ids[0];
    assert_eq!(
        backend.activate_secure_channel(id),
        Err(TransportError::NotConnected)
    );
    for c in backend.connections.values_mut() {
        c.state.connected = true;
    }
    let pending = backend.origin_pending(origin);
    assert_eq!(pending, lifecycle::MAX_PENDING_PER_ORIGIN);
    assert_eq!(
        backend.admission.admit(origin, pending, Instant::now()),
        Err(DisconnectReason::JoinCapacity)
    );
    backend.activate_secure_channel(id).unwrap();
    backend.connections[&id]
        .native
        .assert_send_rate(lifecycle::BULK_BYTES_PER_SECOND as i32);
    assert_eq!(backend.origin_pending(origin), pending);
    assert_eq!(
        backend.activate_secure_channel(id),
        Err(TransportError::ProtocolViolation)
    );
    backend.mark_ready(id).unwrap();
    backend.mark_ready(id).unwrap();
    let pending = backend.origin_pending(origin);
    assert_eq!(pending, lifecycle::MAX_PENDING_PER_ORIGIN - 1);
    backend
        .admission
        .admit(origin, pending, Instant::now())
        .unwrap();
    let failed = ids[1];
    backend
        .connections
        .get_mut(&failed)
        .unwrap()
        .native
        .close(1000);
    assert!(matches!(
        backend.activate_secure_channel(failed),
        Err(TransportError::Backend(_))
    ));
    assert!(!backend.connections[&failed].state.authenticated);
    assert!(!backend.connections[&failed].state.ready);
    assert_eq!(backend.origin_pending(origin), pending);
    assert_eq!(
        backend.activate_secure_channel(ConnectionId::new(0)),
        Err(TransportError::UnknownConnection)
    );
    assert_eq!(
        backend.mark_ready(ConnectionId::new(0)),
        Err(TransportError::UnknownConnection)
    );
}

fn flush_and_wait_for_reliable_ack(sender: &native::Connection) {
    sender.flush_for_test().unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let (queued, _) = sender
            .egress()
            .expect("query native reliable delivery status");
        if queued == 0 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "timeout waiting for P2P native reliable ACKs: {queued} bytes pending or unacked"
        );
        std::thread::park_timeout(Duration::from_millis(1));
    }
}

/// Exercise P2P's actual connection receive loop with an already queued native
/// backlog. ICE establishment is covered by the separate inter-process test.
pub(in crate::network::gns) fn inbound_events(
    id: ConnectionId,
    authenticated: bool,
    policy: &'static crate::network::rate_limit::InboundRatePolicy,
    native_lane: u16,
    payload: &[u8],
    count: usize,
) -> Vec<TransportEvent> {
    let _guard = BACKEND_TEST_LOCK.lock().unwrap();
    let mut backend = GnsP2p::new_unverified_for_test(0, IceConfig::default()).unwrap();
    let [sender, receiver] = native::Connection::test_pair();
    backend.insert(id, receiver, PeerId::from_bytes([42; 16]), None);
    backend.connections.get_mut(&id).unwrap().state.connected = true;
    if authenticated {
        backend.activate_secure_channel(id).unwrap();
    }
    backend.connections.get_mut(&id).unwrap().state.limiter =
        InboundRateLimiter::with_policy(policy, Instant::now());
    for _ in 0..count {
        // Reliable even on lane 0, so the comparison is independent of UDP loss.
        sender
            .send(native_lane, MessageClass::Control, payload)
            .unwrap();
    }
    // Native service threads deliver/ACK without dequeuing the receiver. Wait
    // before poll can disconnect on invalid/reliable excess, discarding its tail.
    flush_and_wait_for_reliable_ack(&sender);
    let mut events = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        assert!(
            Instant::now() < deadline,
            "timeout draining P2P backlog: authenticated={authenticated}, lane={native_lane}, \
             count={count}, events={events:?}"
        );
        let start = events.len();
        backend.poll(&mut events).unwrap();
        if !authenticated {
            assert!(events.len() - start <= 1, "pre-auth handshake barrier");
        }
        if !backend.connections.contains_key(&id) || events.len() == start {
            break;
        }
    }
    // Include a subsequent poll after termination too, so duplicate disconnect
    // events cannot be hidden by stopping as soon as the connection is removed.
    backend.poll(&mut events).unwrap();
    if let Some(connection) = backend.connections.get(&id) {
        // Transient excess is consumed without an event. An empty poll after
        // the ACK barrier must leave no native backlog in these fixtures.
        assert!(
            connection.native.receive().unwrap().is_none(),
            "undrained P2P backlog: authenticated={authenticated}, lane={native_lane}, count={count}"
        );
    }
    events
}

#[test]
fn gns_p2p_inbound_backlog_preserves_handshake_barrier_and_drop_budget() {
    use crate::network::rate_limit::{BucketPolicy, InboundRatePolicy};
    const POLICY: InboundRatePolicy = InboundRatePolicy {
        transient: BucketPolicy {
            bytes_per_second: 0,
            burst_bytes: 128,
            minimum_charge: 64,
        },
        ..PREAUTH_INBOUND_POLICY
    };
    let _guard = BACKEND_TEST_LOCK.lock().unwrap();
    let payload = test_messages()[0].1.clone();
    for authenticated in [false, true] {
        let mut backend = GnsP2p::new_unverified_for_test(0, IceConfig::default()).unwrap();
        let [sender, receiver] = native::Connection::test_pair();
        let id = ConnectionId::new(token().unwrap());
        backend.insert(id, receiver, PeerId::from_bytes([42; 16]), None);
        backend.connections.get_mut(&id).unwrap().state.connected = true;
        if authenticated {
            backend.activate_secure_channel(id).unwrap();
        }
        backend.connections.get_mut(&id).unwrap().state.limiter =
            InboundRateLimiter::with_policy(&POLICY, Instant::now());
        let class = if authenticated {
            MessageClass::Transient
        } else {
            MessageClass::Control
        };
        for _ in 0..513 {
            sender
                .send(lane(class), MessageClass::Control, &payload)
                .unwrap();
        }
        flush_and_wait_for_reliable_ack(&sender);

        let mut events = Vec::new();
        backend.poll(&mut events).unwrap();
        assert_eq!(
            events,
            vec![
                TransportEvent::Message {
                    connection: id,
                    class,
                    payload: payload.clone(),
                };
                if authenticated { 2 } else { 1 }
            ]
        );
        let connection = &backend.connections[&id];
        assert!(connection.state.connected);
        // Pre-auth stops after one record. Authenticated polling consumes 512,
        // including 510 intentional Transient drops, leaving one native-owned.
        let remaining = if authenticated { 1 } else { 512 };
        for index in 0..remaining {
            let message = connection.native.receive().unwrap().unwrap_or_else(|| {
                panic!(
                    "missing native tail record {index}/{remaining}: authenticated={authenticated}"
                )
            });
            assert_eq!(message.lane(), lane(class));
            assert_eq!(message.payload().unwrap(), payload);
        }
        assert!(connection.native.receive().unwrap().is_none());
        events.clear();
        backend.poll(&mut events).unwrap();
        assert!(events.is_empty());
    }
}

#[test]
fn gns_p2p_verified_route_pending_limits_cooldown_and_revocation() {
    let _guard = BACKEND_TEST_LOCK.lock().unwrap();
    let mut backend = GnsP2p::new_routed(P2P_VIRTUAL_PORT, IceConfig::default()).unwrap();
    let mailbox = backend.signaling();
    let bad = RouteOrigin::from_authenticated_route([1; 16], [2; 16], [3; 16]);
    let other = RouteOrigin::from_authenticated_route([1; 16], [2; 16], [4; 16]);
    assert_eq!(
        backend.connect_peer(PeerId::from_bytes([10; 16]), 0),
        Err(TransportError::ProtocolViolation)
    );
    for n in 10..16 {
        mailbox
            .authorize_peer(PeerId::from_bytes([n; 16]), bad)
            .unwrap();
    }
    mailbox
        .authorize_peer(PeerId::from_bytes([20; 16]), other)
        .unwrap();
    let mut ids = Vec::new();
    for n in 10..14 {
        assert!(!backend.has_peer(PeerId::from_bytes([n; 16])));
        let id = backend
            .connect_peer(PeerId::from_bytes([n; 16]), 0)
            .unwrap();
        assert_eq!(backend.origin(id), Some(Origin::Route(bad)));
        assert!(backend.has_peer(PeerId::from_bytes([n; 16])));
        assert!(!backend.connections[&id].state.connected);
        ids.push(id);
    }
    assert_eq!(
        backend.connect_peer(PeerId::from_bytes([14; 16]), 0),
        Err(TransportError::Capacity)
    );
    let healthy = backend
        .connect_peer(PeerId::from_bytes([20; 16]), 0)
        .unwrap();
    backend.close(ids[0], DisconnectReason::Requested).unwrap();
    assert!(!backend.has_peer(PeerId::from_bytes([10; 16])));
    backend
        .connect_peer(PeerId::from_bytes([14; 16]), 0)
        .unwrap();
    let mut events = Vec::new();
    backend.maintain(Instant::now() + CONNECTING_TIMEOUT, &mut events);
    assert!(backend.connections.is_empty());
    assert!(!backend.has_peer(PeerId::from_bytes([20; 16])));
    assert_eq!(backend.origin(healthy), None);
    // Rotating peer IDs or closing sockets does not replenish account history.
    assert_eq!(
        backend.connect_peer(PeerId::from_bytes([15; 16]), 0),
        Err(TransportError::Capacity)
    );
    let healthy = backend
        .connect_peer(PeerId::from_bytes([20; 16]), 0)
        .unwrap();
    mailbox.revoke_peer(PeerId::from_bytes([20; 16]));
    events.clear();
    backend.poll(&mut events).unwrap();
    assert!(!backend.has_peer(PeerId::from_bytes([20; 16])));
    assert!(events.iter().any(|e| matches!(e, TransportEvent::ConnectionFailed { connection, reason: DisconnectReason::Requested } if *connection == healthy)));
    assert_eq!(
        backend.connect_peer(PeerId::from_bytes([20; 16]), 0),
        Err(TransportError::ProtocolViolation)
    );
}
