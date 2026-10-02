use super::*;
use crate::network::gns::GnsDirectIp;
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
}
impl Live {
    fn pump(&mut self, assigning: Option<usize>) {
        let mut events = Vec::new();
        self.host.poll(&mut events).unwrap();
        for event in events {
            self.s.host.connections.observe(&event);
            match &event {
                TransportEvent::Connected { connection } => {
                    let index = assigning.expect("unexpected connection");
                    self.host_peers[index] = Some(*connection);
                    self.s
                        .host
                        .connections
                        .assign_player(*connection, [A, B][index])
                        .unwrap();
                }
                TransportEvent::Message { connection, .. } => {
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
                self.s.peers[index].connections.observe(&event);
                match &event {
                    TransportEvent::Connected { connection } => {
                        assert_eq!(Some(*connection), self.client_hosts[index]);
                        self.s.peers[index]
                            .connections
                            .assign_player(*connection, HOST)
                            .unwrap();
                    }
                    TransportEvent::Message { class, .. } => {
                        match self
                            .s
                            .client_router(index, self.client_hosts[index].unwrap())
                            .route(&event)
                            .unwrap()
                        {
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
    fn send(&mut self, command: ProtocolCommandEnvelope) {
        self.s
            .client_router(0, self.client_hosts[0].unwrap())
            .send_command(&mut self.clients[0], &command)
            .unwrap();
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
        live.until("connect", Some(index), |live| {
            live.host_peers[index].is_some()
                && live.s.peers[index].connections.player(connection) == Some(HOST)
        });
        assert_ne!(Some(connection), live.host_peers[index]); // tokens are not mirrored native handles
    }
    assert_eq!(live.s.host.connections.peers().count(), 2);
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
            .remote_drag(&b.session, &b.store, A)
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
