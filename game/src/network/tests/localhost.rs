use super::*;
use crate::network::gns::GnsDirectIp;
use crate::network::{
    auth::SessionPassword,
    bootstrap::{BootstrapOutcome, ClientBootstrap, ConnectionState, HostBootstrap},
    session_control::SessionMetadata,
};
use std::{
    net::{Ipv4Addr, SocketAddr},
    time::{Duration, Instant},
};

struct Live {
    host: GnsDirectIp,
    clients: [GnsDirectIp; 2],
    s: Scenario,
    host_peers: [Option<ConnectionId>; 2],
    client_hosts: [Option<ConnectionId>; 2],
    controls: [usize; 2],
    drags: [usize; 2],
    bulk: [usize; 2],
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
                    if outcome == BootstrapOutcome::Consumed {
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
                        if outcome == BootstrapOutcome::Consumed {
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
                            ClientRouteOutcome::BulkChunk(bytes) => {
                                assert_eq!(*class, MessageClass::Bulk);
                                assert_eq!(bytes, b"future chunk");
                                self.bulk[index] += 1;
                            }
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
        host: GnsDirectIp::new().unwrap(),
        clients: [GnsDirectIp::new().unwrap(), GnsDirectIp::new().unwrap()],
        s: Scenario::new(),
        host_peers: [None; 2],
        client_hosts: [None; 2],
        controls: [0; 2],
        drags: [0; 2],
        bulk: [0; 2],
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
            .promote_ready(host_peer, &mut live.s.host.connections)
            .unwrap();
        bootstrap
            .promote_ready(&mut live.s.peers[index].connections)
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
    let bulk = wire::encode(&WireMessage::BulkChunk(b"future chunk".to_vec())).unwrap();
    live.host
        .send(live.host_peers[1].unwrap(), MessageClass::Bulk, &bulk)
        .unwrap();
    live.send(release());
    live.until("ReleaseCommitted and independent Bulk lane", None, |live| {
        live.controls == [2, 2] && live.bulk == [0, 1]
    });
    assert_eq!(live.host_commands, 3);
    live.s.assert_final_equal();
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
        live.until("disconnect on both endpoints", None, |live| {
            live.s.host.connections.player(host_peer).is_none()
                && live.s.peers[index].connections.player(client).is_none()
        });
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
    let mut host = GnsDirectIp::new().unwrap();
    let mut client = GnsDirectIp::new().unwrap();
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
