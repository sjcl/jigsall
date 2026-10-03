use super::*;
use crate::network::gns::GnsDirectIp;
use crate::network::syncing::{
    ClientSyncOutcome, ClientSyncRouter, HostSyncCoordinator, SyncAuthority, SyncPhase, SyncReplica,
};
use crate::network::{
    auth::SessionPassword,
    bootstrap::{BootstrapOutcome, ClientBootstrap, ConnectionState, HostBootstrap},
    secure::SecureTransport,
    session_control::SessionMetadata,
};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::{
    net::{Ipv4Addr, SocketAddr},
    time::{Duration, Instant},
};

#[test]
fn gns_localhost_syncing_image_baseline_and_catch_up_stops_before_ready() {
    let mut s = Scenario::new();
    let image: Arc<[u8]> = vec![0x42; 2 * MAX_BULK_DATA_BYTES + 5].into();
    let session_definition = SessionDefinition {
        id: SESSION.id,
        image_hash: ImageHash(Sha256::digest(&image).into()),
    };
    let cursor = AuthorityCursor::new(3, 0);
    s.host.session = AuthoritySession::new(session_definition, HOST, cursor);
    s.peers[0].session = AuthoritySession::new(session_definition, HOST, cursor);
    let mut host = SecureTransport::new(RecordedGns::new());
    let listener = host
        .listen(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .unwrap();
    let mut client = SecureTransport::new(RecordedGns::new());
    let client_host = client
        .connect(host.listener_address(listener).unwrap())
        .unwrap();
    let password = || SessionPassword::new("correct password".to_owned()).unwrap();
    let mut hb = HostBootstrap::new(
        password(),
        SessionMetadata {
            definition: session_definition,
            cursor,
            host: HOST,
        },
        [],
        Instant::now(),
    );
    let mut cb = ClientBootstrap::new(password(), client_host);
    let mut hs = HostSyncCoordinator::default();
    let mut cs = None;
    let mut host_peer = None;
    let mut generated = false;
    let mut images = 0;
    let deadline = Instant::now() + Duration::from_secs(15);
    while cs
        .as_ref()
        .is_none_or(|sync: &ClientSyncRouter| sync.phase() != SyncPhase::Finalizing)
    {
        assert!(Instant::now() < deadline, "secure localhost join timed out");
        let mut events = Vec::new();
        host.poll(&mut events).unwrap();
        for event in events {
            if let TransportEvent::Connected { connection } = event {
                host_peer = Some(connection);
            }
            let outcome = hb
                .process(&event, &mut host, &mut s.host.connections, Instant::now())
                .unwrap();
            let authority = SyncAuthority {
                session: &s.host.session,
                store: &s.host.store,
                contexts: &s.contexts,
                definition: &s.definition,
            };
            if outcome == BootstrapOutcome::Syncing {
                hs.route(
                    &hb,
                    &event,
                    &mut host,
                    &authority,
                    Some(image.clone()),
                    Instant::now(),
                )
                .unwrap();
            }
            if let Some(connection) = host_peer {
                if hb.state(connection) == Some(ConnectionState::Authenticated) {
                    hs.start(&mut hb, connection, &mut host, &authority, Instant::now())
                        .unwrap();
                }
            }
        }
        if let Some(connection) = host_peer {
            if hs.phase(connection).is_some() {
                let player = hb.assigned_player(connection).unwrap();
                if hs.catch_up().status(player).is_ok() && !generated {
                    // A real authority operation while baseline Bulk is in flight.
                    let mut request = grab();
                    request.player = B;
                    let applied = s
                        .contexts
                        .apply_replicated(
                            &mut s.host.session,
                            &mut s.host.store,
                            B,
                            &request,
                            Some(&s.definition),
                            HOST,
                        )
                        .unwrap();
                    hs.record_command_outcome(&s.host.session, &s.host.store, &applied)
                        .unwrap();
                    generated = true;
                }
                hs.pump(
                    &hb,
                    connection,
                    &mut host,
                    &SyncAuthority {
                        session: &s.host.session,
                        store: &s.host.store,
                        contexts: &s.contexts,
                        definition: &s.definition,
                    },
                    Instant::now(),
                )
                .unwrap();
            }
        }
        let mut events = Vec::new();
        client.poll(&mut events).unwrap();
        for event in events {
            let outcome = cb
                .process(
                    &event,
                    &mut client,
                    &mut s.peers[0].connections,
                    Instant::now(),
                )
                .unwrap();
            if cb.state() == Some(ConnectionState::Authenticated) {
                cs = Some(ClientSyncRouter::start(&mut cb, None, Instant::now()).unwrap());
            }
            if outcome == BootstrapOutcome::Syncing {
                let peer = &mut s.peers[0];
                let routed = cs
                    .as_mut()
                    .unwrap()
                    .route(
                        &cb,
                        &event,
                        &mut client,
                        &mut SyncReplica {
                            replica: &mut peer.replica,
                            session: &mut peer.session,
                            store: &mut peer.store,
                        },
                        Instant::now(),
                    )
                    .unwrap();
                if let ClientSyncOutcome::ImageReady(completed) = routed {
                    assert_eq!(completed.into_bytes().unwrap(), image.as_ref());
                    images += 1;
                }
            }
        }
        std::thread::park_timeout(Duration::from_millis(1));
    }
    assert_eq!(images, 1);
    assert!(generated);
    assert_eq!(s.peers[0].store.states, s.host.store.states);
    assert_eq!(s.peers[0].session.cursor(), s.host.session.cursor());
    assert_eq!(s.host.session.cursor().sequence.0, 1);
    assert!(s.peers[0]
        .replica
        .remote_drag(&s.peers[0].session, &s.peers[0].store, B)
        .is_some());
    assert_eq!(hb.state(host_peer.unwrap()), Some(ConnectionState::Syncing));
    assert_eq!(cb.state(), Some(ConnectionState::Syncing));
    assert_eq!(s.host.connections.player(host_peer.unwrap()), None);
    assert_eq!(s.peers[0].connections.player(client_host), None);
    host.backend_mut().assert_protected_after_handshake(2);
    client.backend_mut().assert_protected_after_handshake(1);
    client
        .close(client_host, DisconnectReason::Requested)
        .unwrap();
    host.close_listener(listener).unwrap();
}

/// Inspect/tamper the bytes immediately below SecureTransport, before real GNS
/// sends them. Test-only access cannot bypass protection in production.
struct RecordedGns {
    inner: GnsDirectIp,
    sent: Vec<(ConnectionId, MessageClass, Vec<u8>)>,
    tamper_next: bool,
}
impl RecordedGns {
    fn new() -> Self {
        Self {
            inner: GnsDirectIp::new().unwrap(),
            sent: Vec::new(),
            tamper_next: false,
        }
    }
    fn assert_protected_after_handshake(&self, plaintext_count: usize) {
        assert_eq!(
            self.sent
                .iter()
                .filter(|(_, _, bytes)| wire::decode(bytes).is_ok())
                .count(),
            plaintext_count
        );
        assert!(self.sent.len() > plaintext_count);
        assert!(self
            .sent
            .iter()
            .filter(|(_, class, _)| *class != MessageClass::Control)
            .all(|(_, _, bytes)| wire::decode(bytes).is_err()));
    }
}
impl Transport for RecordedGns {
    fn poll(&mut self, events: &mut Vec<TransportEvent>) -> Result<(), TransportError> {
        self.inner.poll(events)
    }
    fn send(
        &mut self,
        connection: ConnectionId,
        class: MessageClass,
        payload: &[u8],
    ) -> Result<(), TransportError> {
        let mut bytes = payload.to_vec();
        if self.tamper_next {
            self.tamper_next = false;
            assert!(
                wire::decode(&bytes).is_err(),
                "tamper ciphertext, never a plaintext handshake"
            );
            bytes[8] ^= 1;
        }
        self.sent.push((connection, class, bytes.clone()));
        self.inner.send(connection, class, &bytes)
    }
    fn close(
        &mut self,
        connection: ConnectionId,
        reason: DisconnectReason,
    ) -> Result<(), TransportError> {
        self.inner.close(connection, reason)
    }
}
impl DirectIpTransport for RecordedGns {
    fn listen(&mut self, address: SocketAddr) -> Result<ListenerId, TransportError> {
        self.inner.listen(address)
    }
    fn listener_address(&self, listener: ListenerId) -> Result<SocketAddr, TransportError> {
        self.inner.listener_address(listener)
    }
    fn close_listener(&mut self, listener: ListenerId) -> Result<(), TransportError> {
        self.inner.close_listener(listener)
    }
    fn connect(&mut self, address: SocketAddr) -> Result<ConnectionId, TransportError> {
        self.inner.connect(address)
    }
}

struct Live {
    host: SecureTransport<RecordedGns>,
    clients: [SecureTransport<RecordedGns>; 2],
    s: Scenario,
    host_peers: [Option<ConnectionId>; 2],
    client_hosts: [Option<ConnectionId>; 2],
    controls: [usize; 2],
    drags: [usize; 2],
    host_commands: usize,
    host_bootstrap: HostBootstrap,
    client_bootstraps: [Option<ClientBootstrap>; 2],
}
impl Live {
    fn pump(&mut self, assigning: Option<usize>) {
        let mut events = Vec::new();
        self.host.poll(&mut events).unwrap();
        for event in events {
            let outcome = self
                .host_bootstrap
                .process(
                    &event,
                    &mut self.host,
                    &mut self.s.host.connections,
                    Instant::now(),
                )
                .unwrap();
            match &event {
                TransportEvent::Connected { connection } => {
                    let index = assigning.expect("unexpected connection");
                    self.host_peers[index] = Some(*connection);
                }
                TransportEvent::Message { connection, .. } => {
                    if outcome != BootstrapOutcome::Gameplay {
                        continue;
                    }
                    let mut router = self.s.host_router();
                    let HostRouteOutcome::Applied(outcome) = router.route(&event).unwrap() else {
                        panic!("unexpected host bulk")
                    };
                    assert!(router
                        .publish(&mut self.host, Some(*connection), &outcome)
                        .unwrap()
                        .is_empty());
                    self.host_commands += 1;
                }
                TransportEvent::ConnectionFailed { .. } => {
                    panic!("localhost connection failed: {event:?}")
                }
                TransportEvent::Disconnected { .. } => {}
            }
        }
        for index in 0..2 {
            let mut events = Vec::new();
            self.clients[index].poll(&mut events).unwrap();
            for event in events {
                let outcome = self.client_bootstraps[index]
                    .as_mut()
                    .unwrap()
                    .process(
                        &event,
                        &mut self.clients[index],
                        &mut self.s.peers[index].connections,
                        Instant::now(),
                    )
                    .unwrap();
                match &event {
                    TransportEvent::Connected { connection } => {
                        assert_eq!(Some(*connection), self.client_hosts[index]);
                    }
                    TransportEvent::Message { class, .. } => {
                        if outcome != BootstrapOutcome::Gameplay {
                            continue;
                        }
                        let local_player = self.client_bootstraps[index]
                            .as_ref()
                            .unwrap()
                            .assigned_player()
                            .unwrap();
                        let mut router = self
                            .s
                            .client_router(index, self.client_hosts[index].unwrap());
                        router.local_player = local_player;
                        match router.route(&event).unwrap() {
                            ClientRouteOutcome::Authority(_) => {
                                assert_eq!(*class, MessageClass::Control);
                                self.controls[index] += 1;
                            }
                            ClientRouteOutcome::Drag(_) => {
                                assert_eq!(*class, MessageClass::Transient);
                                self.drags[index] += 1;
                            }
                            ClientRouteOutcome::DroppedTransient(_) => {}
                        }
                    }
                    TransportEvent::ConnectionFailed { .. } => {
                        panic!("localhost connection failed: {event:?}")
                    }
                    TransportEvent::Disconnected { .. } => {}
                }
            }
        }
    }
    fn until(&mut self, label: &str, assigning: Option<usize>, done: impl Fn(&Self) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !done(self) {
            assert!(Instant::now() < deadline, "timeout waiting for {label}");
            self.pump(assigning);
            // Deadline-driven polling backoff, never an assumed delivery delay.
            std::thread::park_timeout(Duration::from_millis(1));
        }
    }
    fn send(&mut self, mut command: ProtocolCommandEnvelope) {
        command.player = self.client_bootstraps[0]
            .as_ref()
            .unwrap()
            .assigned_player()
            .unwrap();
        let mut router = self.s.client_router(0, self.client_hosts[0].unwrap());
        router.local_player = command.player;
        router.send_command(&mut self.clients[0], &command).unwrap();
    }
}

/// Actual UDP loopback listen/connect (not GNS's in-process socket_pair).
#[test]
fn gns_localhost_host_two_clients_grab_drag_release_disconnect() {
    let mut live = Live {
        host: SecureTransport::new(RecordedGns::new()),
        clients: [
            SecureTransport::new(RecordedGns::new()),
            SecureTransport::new(RecordedGns::new()),
        ],
        s: Scenario::new(),
        host_peers: [None; 2],
        client_hosts: [None; 2],
        controls: [0; 2],
        drags: [0; 2],
        host_commands: 0,
        host_bootstrap: HostBootstrap::new(
            SessionPassword::new("localhost password".to_owned()).unwrap(),
            SessionMetadata {
                definition: SESSION,
                cursor: AuthorityCursor::new(3, 0),
                host: HOST,
            },
            [],
            Instant::now(),
        ),
        client_bootstraps: [None, None],
    };
    let listener = live
        .host
        .listen(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .unwrap();
    let address = live.host.listener_address(listener).unwrap();
    assert!(address.ip().is_loopback());
    assert_ne!(address.port(), 0);
    for index in 0..2 {
        let connection = live.clients[index].connect(address).unwrap();
        live.client_hosts[index] = Some(connection);
        live.client_bootstraps[index] = Some(ClientBootstrap::new(
            SessionPassword::new("localhost password".to_owned()).unwrap(),
            connection,
        ));
        live.until("mutual password authentication", Some(index), |live| {
            live.host_peers[index].is_some()
                && live.client_bootstraps[index].as_ref().unwrap().state()
                    == Some(ConnectionState::Authenticated)
                && live.host_bootstrap.state(live.host_peers[index].unwrap())
                    == Some(ConnectionState::Authenticated)
        });
        let host_peer = live.host_peers[index].unwrap();
        assert_eq!(
            live.host_bootstrap.state(host_peer),
            Some(ConnectionState::Authenticated)
        );
        assert_eq!(live.s.host.connections.player(host_peer), None);
        assert_eq!(live.s.peers[index].connections.player(connection), None);
        let bootstrap = live.client_bootstraps[index].as_mut().unwrap();
        assert_eq!(
            bootstrap.assigned_player(),
            live.host_bootstrap.assigned_player(host_peer)
        );
        assert_ne!(bootstrap.assigned_player(), Some(HOST));
        live.host_bootstrap.begin_sync(host_peer).unwrap();
        bootstrap.begin_sync().unwrap();
        live.host_bootstrap
            .promote_ready_for_test(host_peer, &mut live.s.host.connections)
            .unwrap();
        bootstrap
            .promote_ready_for_test(&mut live.s.peers[index].connections)
            .unwrap();
        assert_ne!(Some(connection), live.host_peers[index]); // tokens are not mirrored native handles
    }
    assert_eq!(live.s.host.connections.peers().count(), 2);
    assert_ne!(live.host_peers[0], live.host_peers[1]);
    assert_ne!(
        live.client_bootstraps[0]
            .as_ref()
            .unwrap()
            .assigned_player(),
        live.client_bootstraps[1]
            .as_ref()
            .unwrap()
            .assigned_player()
    );
    live.send(grab());
    live.until("GrabAccepted on both replicas", None, |live| {
        live.controls == [1, 1]
    });
    assert_eq!(live.s.host.session.cursor().sequence.0, 1);
    live.send(update());
    live.until("Transient RemoteDragUpdate on B", None, |live| {
        live.drags[1] == 1
    });
    assert_eq!(live.drags, [0, 1]);
    let b = &live.s.peers[1];
    assert_eq!(
        b.replica
            .remote_drag(
                &b.session,
                &b.store,
                live.client_bootstraps[0]
                    .as_ref()
                    .unwrap()
                    .assigned_player()
                    .unwrap()
            )
            .unwrap()
            .delta,
        Vec2::splat(-50.0)
    );
    assert_eq!(live.s.host.session.cursor().sequence.0, 1);
    let bulk = wire::encode(&bulk_chunk(b"future chunk".to_vec())).unwrap();
    live.send(release());
    live.until("ReleaseCommitted", None, |live| live.controls == [2, 2]);
    assert_eq!(live.host_commands, 3);
    live.s.assert_final_equal();
    live.host.backend_mut().assert_protected_after_handshake(4);
    for client in &mut live.clients {
        client.backend_mut().assert_protected_after_handshake(1);
    }
    assert_eq!(
        live.host
            .send(ConnectionId::new(u64::MAX), MessageClass::Control, &bulk),
        Err(TransportError::UnknownConnection)
    );
    assert_eq!(
        live.host.send(
            live.host_peers[0].unwrap(),
            MessageClass::Transient,
            &vec![0; wire::frame_limit(MessageClass::Transient) + 1]
        ),
        Err(TransportError::PayloadTooLarge)
    );
    for index in 0..2 {
        let client = live.client_hosts[index].unwrap();
        let host_peer = live.host_peers[index].unwrap();
        live.clients[index]
            .close(client, DisconnectReason::Requested)
            .unwrap();
        assert!(!live.clients[index].has_channel(client));
        live.until("disconnect on both endpoints", None, |live| {
            live.s.host.connections.player(host_peer).is_none()
                && live.s.peers[index].connections.player(client).is_none()
        });
        assert!(!live.host.has_channel(host_peer));
    }
    live.host.close_listener(listener).unwrap();
    assert_eq!(
        live.host.listener_address(listener),
        Err(TransportError::UnknownListener)
    );
    let pending = live.clients[0].connect(address).unwrap();
    assert_ne!(Some(pending), live.client_hosts[0]);
    live.clients[0]
        .close(pending, DisconnectReason::Requested)
        .unwrap();
    let mut events = Vec::new();
    live.clients[0].poll(&mut events).unwrap();
    assert!(events.contains(&TransportEvent::ConnectionFailed {
        connection: pending,
        reason: DisconnectReason::Requested
    }));
    // RAII closes every listener/connection on assertion failure too.
}

#[test]
fn gns_localhost_wrong_password_never_registers_or_mutates() {
    use crate::network::bootstrap::BootstrapError;
    let mut host = SecureTransport::new(GnsDirectIp::new().unwrap());
    let mut client = SecureTransport::new(GnsDirectIp::new().unwrap());
    let listener = host
        .listen(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .unwrap();
    let outgoing = client
        .connect(host.listener_address(listener).unwrap())
        .unwrap();
    let mut scenario = Scenario::new();
    let original = scenario.host.store.states.clone();
    let mut hb = HostBootstrap::new(
        SessionPassword::new("correct password".to_owned()).unwrap(),
        SessionMetadata {
            definition: SESSION,
            cursor: AuthorityCursor::new(3, 0),
            host: HOST,
        },
        [],
        Instant::now(),
    );
    let mut cb = ClientBootstrap::new(
        SessionPassword::new("incorrect password".to_owned()).unwrap(),
        outgoing,
    );
    let mut incoming = None;
    let mut rejected = false;
    let mut closed = false;
    let deadline = Instant::now() + Duration::from_secs(15);
    while !closed {
        assert!(Instant::now() < deadline, "wrong-password close timeout");
        let mut events = Vec::new();
        host.poll(&mut events).unwrap();
        for event in events {
            if let TransportEvent::Connected { connection } = event {
                incoming = Some(connection);
            }
            let result = hb.process(
                &event,
                &mut host,
                &mut scenario.host.connections,
                Instant::now(),
            );
            match result {
                Err(BootstrapError::Rejected(DisconnectReason::AuthenticationFailed)) => {
                    rejected = true
                }
                Ok(BootstrapOutcome::Consumed) => {}
                other => panic!("unexpected bootstrap result: {other:?}"),
            }
            assert!(scenario
                .host
                .connections
                .peers()
                .all(|peer| peer.player.is_none()));
        }
        let mut events = Vec::new();
        client.poll(&mut events).unwrap();
        for event in events {
            if matches!(event, TransportEvent::Disconnected { .. }) {
                closed = true;
            }
            cb.process(
                &event,
                &mut client,
                &mut scenario.peers[0].connections,
                Instant::now(),
            )
            .unwrap();
            assert_eq!(cb.assigned_player(), None);
            assert_ne!(cb.state(), Some(ConnectionState::Authenticated));
        }
        std::thread::park_timeout(Duration::from_millis(1));
    }
    assert!(rejected);
    assert!(!host.has_channel(incoming.unwrap()));
    assert!(!client.has_channel(outgoing));
    let incoming = incoming.unwrap();
    assert_eq!(scenario.host.connections.player(incoming), None);
    assert_eq!(scenario.peers[0].connections.player(outgoing), None);
    assert_eq!(
        scenario
            .host_router()
            .route(&message_event(
                incoming,
                &WireMessage::ClientCommand(grab())
            ))
            .unwrap_err(),
        HostRouteError::Unauthenticated
    );
    assert_eq!(scenario.host.store.states, original);
    host.close_listener(listener).unwrap();
}

#[test]
fn gns_localhost_tampered_secure_grab_closes_without_gameplay_mutation() {
    let mut host = SecureTransport::new(RecordedGns::new());
    let mut client = SecureTransport::new(RecordedGns::new());
    let listener = host
        .listen(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .unwrap();
    let outgoing = client
        .connect(host.listener_address(listener).unwrap())
        .unwrap();
    let mut scenario = Scenario::new();
    let before = scenario.host.store.states.clone();
    let before_cursor = scenario.host.session.cursor();
    let mut hb = HostBootstrap::new(
        SessionPassword::new("correct password".into()).unwrap(),
        SessionMetadata {
            definition: SESSION,
            cursor: AuthorityCursor::new(3, 0),
            host: HOST,
        },
        [],
        Instant::now(),
    );
    let mut cb = ClientBootstrap::new(
        SessionPassword::new("correct password".into()).unwrap(),
        outgoing,
    );
    let mut incoming = None;
    let deadline = Instant::now() + Duration::from_secs(15);
    while cb.state() != Some(ConnectionState::Authenticated)
        || incoming.is_none_or(|id| hb.state(id) != Some(ConnectionState::Authenticated))
    {
        assert!(Instant::now() < deadline, "secure authentication timeout");
        let mut events = Vec::new();
        host.poll(&mut events).unwrap();
        for event in events {
            if let TransportEvent::Connected { connection } = event {
                incoming = Some(connection);
            }
            assert_eq!(
                hb.process(
                    &event,
                    &mut host,
                    &mut scenario.host.connections,
                    Instant::now()
                )
                .unwrap(),
                BootstrapOutcome::Consumed
            );
        }
        let mut events = Vec::new();
        client.poll(&mut events).unwrap();
        for event in events {
            cb.process(
                &event,
                &mut client,
                &mut scenario.peers[0].connections,
                Instant::now(),
            )
            .unwrap();
        }
        std::thread::park_timeout(Duration::from_millis(1));
    }
    let incoming = incoming.unwrap();
    assert!(host.has_channel(incoming));
    assert!(client.has_channel(outgoing));
    hb.begin_sync(incoming).unwrap();
    cb.begin_sync().unwrap();
    hb.promote_ready_for_test(incoming, &mut scenario.host.connections)
        .unwrap();
    cb.promote_ready_for_test(&mut scenario.peers[0].connections)
        .unwrap();
    client.backend_mut().tamper_next = true;
    let mut command = grab();
    command.player = cb.assigned_player().unwrap();
    let mut router = scenario.client_router(0, outgoing);
    router.local_player = command.player;
    router.send_command(&mut client, &command).unwrap();
    let mut rejected = false;
    let mut remote_closed = false;
    while !rejected || !remote_closed {
        assert!(Instant::now() < deadline, "tampered channel close timeout");
        let mut events = Vec::new();
        host.poll(&mut events).unwrap();
        for event in events {
            if event
                == (TransportEvent::Disconnected {
                    connection: incoming,
                    reason: DisconnectReason::ProtocolViolation,
                })
            {
                rejected = true;
            }
            assert_eq!(
                hb.process(
                    &event,
                    &mut host,
                    &mut scenario.host.connections,
                    Instant::now()
                )
                .unwrap(),
                BootstrapOutcome::Consumed
            );
        }
        let mut events = Vec::new();
        client.poll(&mut events).unwrap();
        for event in events {
            if matches!(event, TransportEvent::Disconnected { .. }) {
                remote_closed = true;
            }
            cb.process(
                &event,
                &mut client,
                &mut scenario.peers[0].connections,
                Instant::now(),
            )
            .unwrap();
        }
        std::thread::park_timeout(Duration::from_millis(1));
    }
    assert_eq!(scenario.host.store.states, before);
    assert_eq!(scenario.host.session.cursor(), before_cursor);
    assert_eq!(scenario.host.connections.player(incoming), None);
    assert!(!host.has_channel(incoming));
    assert!(!client.has_channel(outgoing));
    let mut after_close = Vec::new();
    host.poll(&mut after_close).unwrap();
    assert!(
        after_close.is_empty(),
        "bootstrap observes one local close event"
    );
    host.close_listener(listener).unwrap();
}
