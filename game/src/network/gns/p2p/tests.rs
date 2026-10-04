use super::*;
use crate::network::{
    auth::SessionPassword,
    bootstrap::{ClientBootstrap, ConnectionState, HostBootstrap},
    secure::SecureTransport,
    session::SessionConnections,
    session_control::{SessionControlMessage, SessionMetadata},
};
use puzzella_core::{
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
    Signal([u8; 16], Vec<u8>),
    Connected(String),
    Authenticated,
    Exchange,
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
        puzzella_core::protocol::RemoteDragUpdate {
            session: SessionId(7788),
            authority_epoch: puzzella_core::session::AuthorityEpoch(1),
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
    let Ok(role) = std::env::var("PUZZELLA_P2P_TEST_ROLE") else {
        return;
    };
    let host = role == "host";
    let mut backend = GnsP2p::new(0, IceConfig::default()).unwrap();
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
    let Frame::Start(remote) = rx.recv_timeout(Duration::from_secs(10)).unwrap() else {
        panic!("expected start");
    };
    let mut connection = if host {
        None
    } else {
        Some(backend.connect_peer(PeerId::from_bytes(remote), 0).unwrap())
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
    loop {
        assert!(Instant::now() < deadline, "P2P child timed out ({role})");
        while let Ok(frame) = rx.try_recv() {
            match frame {
                Frame::Signal(peer, bytes) => {
                    mailbox.receive(PeerId::from_bytes(peer), &bytes).unwrap();
                }
                Frame::Exchange => {
                    assert!(authenticated);
                    exchange = true;
                }
                Frame::Close => transport
                    .close(connection.unwrap(), DisconnectReason::Requested)
                    .unwrap(),
                Frame::Finish => {
                    assert_eq!((connected, closed), (1, 1));
                    assert!(authenticated && received.into_iter().all(|x| x));
                    assert!(!transport.has_channel(connection.unwrap()));
                    assert_eq!(mailbox.receive(PeerId::from_bytes([8; 16]), &[1]), Ok(()));
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
        let mut events = Vec::new();
        transport.poll(&mut events).unwrap();
        for event in events {
            match &event {
                TransportEvent::Connected { connection: id } => {
                    connected += 1;
                    connection = Some(*id);
                    assert!(!transport.has_channel(*id));
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
                authenticated = true;
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
        for _ in 0..MAX_QUEUED_SIGNALS {
            let Some(signal) = mailbox.pop_outbound() else {
                break;
            };
            emit(Frame::Signal(signal.peer.to_bytes(), signal.payload));
        }
        std::thread::park_timeout(Duration::from_millis(1));
    }
}

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
            .env("PUZZELLA_P2P_TEST_ROLE", role)
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
    assert!(matches!(
        GnsP2p::new(
            0,
            IceConfig {
                allow_public_candidates: true,
                stun_servers: vec!["invalid\0server".into()]
            }
        ),
        Err(TransportError::ProtocolViolation)
    ));
    let mut backend = GnsP2p::new(0, IceConfig::default()).unwrap();
    let mailbox = backend.signaling();
    assert!(matches!(
        GnsP2p::new(1, IceConfig::default()),
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
    let mut replacement = GnsP2p::new(0, IceConfig::default()).unwrap();
    let fresh = replacement
        .connect_peer(PeerId::from_bytes([43; 16]), 0)
        .unwrap();
    assert_ne!(id, fresh);
}
