use super::*;
use crate::network::transport::Origin;
use base64::{engine::general_purpose::STANDARD, Engine};
use futures_util::{SinkExt, StreamExt};
use protocol::{parse_client, Frame};
use std::{future::Future, time::Duration};
use tokio::{
    net::{TcpListener, TcpStream},
    time::timeout,
};
use tokio_tungstenite::{accept_async, tungstenite::Message, WebSocketStream};
type Socket = WebSocketStream<TcpStream>;
fn id(n: u8) -> [u8; 16] {
    [n; 16]
}
fn welcome() -> ServerMessage {
    ServerMessage::Welcome {
        authority_id: AuthorityId(id(1)),
    }
}
fn created() -> ServerMessage {
    ServerMessage::RoomCreated {
        room_id: RoomId(id(2)),
        room_code: "ABCDEFGHJK".parse().unwrap(),
        self_member_id: MemberId(id(3)),
    }
}
fn authorize() -> ServerMessage {
    ServerMessage::AuthorizePeer {
        join_id: JoinId(id(4)),
        peer_id: protocol::PeerId(id(5)),
        member_id: MemberId(id(6)),
    }
}
async fn emit(socket: &mut Socket, message: ServerMessage) {
    socket
        .send(Message::Text(
            serde_json::to_string(&Frame::new(message)).unwrap().into(),
        ))
        .await
        .unwrap();
}
async fn receive(socket: &mut Socket) -> ClientMessage {
    timeout(Duration::from_secs(3), async {
        loop {
            match socket.next().await.unwrap().unwrap() {
                Message::Text(text) => break parse_client(&text).unwrap(),
                Message::Ping(_) => socket.flush().await.unwrap(),
                m => panic!("unexpected {m:?}"),
            }
        }
    })
    .await
    .unwrap()
}
async fn fixture<F, Fut>(script: F) -> (EndpointUrl, tokio::task::JoinHandle<()>)
where
    F: FnOnce(Socket) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url =
        EndpointUrl::loopback_for_test(&format!("ws://{}/v1/ws", listener.local_addr().unwrap()))
            .unwrap();
    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        script(accept_async(stream).await.unwrap()).await;
    });
    (url, task)
}
fn adapter(url: EndpointUrl) -> RendezvousAdapter {
    RendezvousAdapter::from_routed(url, PeerId::from_bytes(id(9)), SignalingEndpoint::routed())
        .unwrap()
}
async fn until(
    adapter: &mut RendezvousAdapter,
    predicate: impl Fn(&RendezvousEvent) -> bool,
) -> RendezvousEvent {
    timeout(Duration::from_secs(5), async {
        loop {
            for event in adapter.poll() {
                if predicate(&event) {
                    return event;
                }
                assert!(
                    !matches!(event, RendezvousEvent::Disconnected(_)),
                    "unexpected {event:?}"
                );
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap()
}
async fn finish(adapter: &mut RendezvousAdapter) {
    adapter.shutdown();
    timeout(Duration::from_secs(3), async {
        while !adapter.worker_finished() {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
}
#[test]
fn endpoint_tls_and_explicit_loopback_policy() {
    assert!(worker::tls_config().is_ok());
    for url in [
        "ws://example.com/v1/ws",
        "ws://127.0.0.1/v1/ws",
        "wss://user:pass@example.com/v1/ws",
        "wss://example.com/v2/ws",
        "wss://example.com/v1/ws?token=x",
    ] {
        assert!(EndpointUrl::production(url).is_err());
    }
    assert!(EndpointUrl::production("wss://example.com/v1/ws").is_ok());
    assert!(EndpointUrl::loopback_for_test("ws://127.0.0.1:1234/v1/ws").is_ok());
    assert!(EndpointUrl::loopback_for_test("ws://[::1]:1234/v1/ws").is_ok());
    for url in [
        "ws://example.com/v1/ws",
        "ws://localhost/v1/ws",
        "ws://192.168.1.1/v1/ws",
    ] {
        assert!(EndpointUrl::loopback_for_test(url).is_err());
    }
    assert_eq!(MAX_SIGNAL_BYTES, protocol::MAX_SIGNAL_BYTES);
}

#[test]
fn production_tls_rejects_untrusted_certificate_with_explicit_provider() {
    use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName};
    use std::io::Cursor;
    // Public, test-only self-signed certificate/key. Neither is used by the
    // production trust store, and no custom verifier/validation bypass exists.
    let cert = STANDARD
        .decode(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/localhost-cert.der.b64"
        )))
        .unwrap();
    let key = STANDARD
        .decode(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/localhost-key.pkcs8.b64"
        )))
        .unwrap();
    let server_config = rustls::ServerConfig::builder_with_provider(std::sync::Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![CertificateDer::from(cert)],
        PrivatePkcs8KeyDer::from(key).into(),
    )
    .unwrap();
    let mut server = rustls::ServerConnection::new(std::sync::Arc::new(server_config)).unwrap();
    let name = ServerName::try_from("127.0.0.1").unwrap();
    let mut client =
        rustls::ClientConnection::new(std::sync::Arc::new(worker::tls_config().unwrap()), name)
            .unwrap();
    for _ in 0..16 {
        let mut outbound = Vec::new();
        client.write_tls(&mut outbound).unwrap();
        server.read_tls(&mut Cursor::new(outbound)).unwrap();
        server.process_new_packets().unwrap();
        let mut inbound = Vec::new();
        server.write_tls(&mut inbound).unwrap();
        client.read_tls(&mut Cursor::new(inbound)).unwrap();
        if let Err(error) = client.process_new_packets() {
            assert!(matches!(error, rustls::Error::InvalidCertificate(_)));
            return;
        }
    }
    panic!("self-signed TLS certificate must be rejected");
}
#[tokio::test]
async fn host_installs_route_before_ack_bridges_opaque_bytes_and_preserves_active_route() {
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    let (continue_tx, continue_rx) = tokio::sync::oneshot::channel();
    let (url, task) = fixture(move |mut socket| async move {
        emit(&mut socket, welcome()).await;
        assert_eq!(
            receive(&mut socket).await,
            ClientMessage::CreateRoom {
                peer_id: protocol::PeerId(id(9))
            }
        );
        emit(&mut socket, created()).await;
        emit(&mut socket, authorize()).await;
        assert_eq!(
            receive(&mut socket).await,
            ClientMessage::AuthorizeAck {
                join_id: JoinId(id(4))
            }
        );
        ack_tx.send(()).unwrap();
        emit(
            &mut socket,
            ServerMessage::PeerJoined {
                peer_id: protocol::PeerId(id(5)),
                member_id: MemberId(id(6)),
            },
        )
        .await;
        emit(
            &mut socket,
            ServerMessage::Signal {
                from_peer_id: protocol::PeerId(id(5)),
                payload_base64: "AP8H".into(),
            },
        )
        .await;
        assert_eq!(
            receive(&mut socket).await,
            ClientMessage::Signal {
                to_peer_id: protocol::PeerId(id(5)),
                payload_base64: "AP8H".into()
            }
        );
        let _ = continue_rx.await;
        emit(
            &mut socket,
            ServerMessage::PeerUnavailable {
                peer_id: protocol::PeerId(id(5)),
            },
        )
        .await;
        emit(&mut socket, ServerMessage::RoomClosed {}).await;
        tokio::time::sleep(Duration::from_millis(30)).await;
    })
    .await;
    let mut a = adapter(url);
    let mailbox = a.signaling.clone();
    let remote = PeerId::from_bytes(id(5));
    until(&mut a, |e| matches!(e, RendezvousEvent::Welcome { .. })).await;
    a.create_room().unwrap();
    until(&mut a, |e| matches!(e, RendezvousEvent::PeerJoined { .. })).await;
    ack_rx.await.unwrap();
    let origin = Some(Origin::Route(RouteOrigin::from_authenticated_route(
        id(1),
        id(2),
        id(6),
    )));
    assert_eq!(mailbox.origin(remote).unwrap(), origin);
    timeout(Duration::from_secs(2), async {
        loop {
            a.poll();
            if let Some(signal) = mailbox.pop_inbound() {
                assert_eq!(signal.peer, remote);
                assert_eq!(signal.payload, [0, 255, 7]);
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    assert!(mailbox.send(remote, &[0, 255, 7]));
    a.poll();
    continue_tx.send(()).unwrap();
    until(&mut a, |e| matches!(e, RendezvousEvent::RoomClosed)).await;
    assert_eq!(mailbox.origin(remote).unwrap(), origin);
    assert!(!a.routes[&remote].available);
    a.release_route(remote);
    assert!(mailbox.origin(remote).is_err());
    finish(&mut a).await;
    task.await.unwrap();
}
#[tokio::test]
async fn join_host_ready_requires_route_binding_and_does_not_bootstrap() {
    let (url, task) = fixture(|mut socket| async move {
        emit(&mut socket, welcome()).await;
        assert!(matches!(
            receive(&mut socket).await,
            ClientMessage::JoinRoom { .. }
        ));
        emit(
            &mut socket,
            ServerMessage::RoomJoined {
                room_id: RoomId(id(2)),
                self_member_id: MemberId(id(3)),
                host_peer_id: protocol::PeerId(id(5)),
                host_member_id: MemberId(id(6)),
            },
        )
        .await;
        assert!(matches!(
            receive(&mut socket).await,
            ClientMessage::LeaveRoom {}
        ));
        emit(&mut socket, ServerMessage::RoomClosed {}).await;
    })
    .await;
    let mut a = adapter(url);
    until(&mut a, |e| matches!(e, RendezvousEvent::Welcome { .. })).await;
    a.join_room("abcdefghjk".parse().unwrap()).unwrap();
    let event = until(&mut a, |e| matches!(e, RendezvousEvent::HostReady { .. })).await;
    assert!(
        matches!(event, RendezvousEvent::HostReady { peer_id, .. } if peer_id == PeerId::from_bytes(id(5)))
    );
    assert_eq!(a.room_id(), Some(RoomId(id(2))));
    assert_eq!(
        a.signaling.origin(PeerId::from_bytes(id(5))).unwrap(),
        Some(Origin::Route(RouteOrigin::from_authenticated_route(
            id(1),
            id(2),
            id(6)
        )))
    );
    a.leave_room().unwrap();
    until(&mut a, |e| matches!(e, RendezvousEvent::RoomClosed)).await;
    finish(&mut a).await;
    task.await.unwrap();
}
#[tokio::test]
async fn unknown_pending_senders_and_mismatched_membership_fail_closed() {
    for bad in [
        // Server-authenticated but still pending: violates v1 activation order.
        ServerMessage::Signal {
            from_peer_id: protocol::PeerId(id(5)),
            payload_base64: "AP8H".into(),
        },
        ServerMessage::Signal {
            from_peer_id: protocol::PeerId(id(8)),
            payload_base64: "AP8H".into(),
        },
        ServerMessage::PeerJoined {
            peer_id: protocol::PeerId(id(5)),
            member_id: MemberId(id(7)),
        },
    ] {
        let (url, task) = fixture(move |mut socket| async move {
            emit(&mut socket, welcome()).await;
            receive(&mut socket).await;
            emit(&mut socket, created()).await;
            emit(&mut socket, authorize()).await;
            receive(&mut socket).await;
            emit(&mut socket, bad).await;
            tokio::time::sleep(Duration::from_millis(30)).await;
        })
        .await;
        let mut a = adapter(url);
        until(&mut a, |e| matches!(e, RendezvousEvent::Welcome { .. })).await;
        a.create_room().unwrap();
        assert_eq!(
            until(&mut a, |e| matches!(e, RendezvousEvent::Disconnected(_))).await,
            RendezvousEvent::Disconnected(RendezvousError::ProtocolViolation)
        );
        assert!(a.signaling.origin(PeerId::from_bytes(id(5))).is_err());
        finish(&mut a).await;
        task.await.unwrap();
    }
}
#[tokio::test]
async fn conflicting_route_origin_is_rejected_and_ack_is_not_sent() {
    let (url, task) = fixture(|mut socket| async move {
        emit(&mut socket, welcome()).await;
        receive(&mut socket).await;
        emit(&mut socket, created()).await;
        emit(&mut socket, authorize()).await;
        assert!(!matches!(socket.next().await, Some(Ok(Message::Text(_)))));
    })
    .await;
    let mut a = adapter(url);
    a.signaling
        .authorize_peer(
            PeerId::from_bytes(id(5)),
            RouteOrigin::from_authenticated_route(id(1), id(2), id(7)),
        )
        .unwrap();
    until(&mut a, |e| matches!(e, RendezvousEvent::Welcome { .. })).await;
    a.create_room().unwrap();
    assert_eq!(
        until(&mut a, |e| matches!(e, RendezvousEvent::Disconnected(_))).await,
        RendezvousEvent::Disconnected(RendezvousError::Transport(
            TransportError::ProtocolViolation
        ))
    );
    finish(&mut a).await;
    task.await.unwrap();
}
#[tokio::test]
async fn bounded_commands_retain_ack_and_shutdown_cancels_handshake() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url =
        EndpointUrl::loopback_for_test(&format!("ws://{}/v1/ws", listener.local_addr().unwrap()))
            .unwrap();
    let mut a = adapter(url);
    for _ in 0..CHANNEL_CAPACITY {
        a.io.send(ClientMessage::LeaveRoom {}).unwrap();
    }
    assert_eq!(
        a.io.send(ClientMessage::LeaveRoom {}),
        Err(RendezvousError::Backpressure)
    );
    a.authority = Some(AuthorityId(id(1)));
    a.phase = Phase::Host(RoomBinding {
        room: RoomId(id(2)),
        member: MemberId(id(3)),
    });
    a.handle(authorize(), &mut Vec::new()).unwrap();
    assert!(!a.flush_deferred().unwrap());
    assert!(matches!(
        a.deferred,
        Some(ClientMessage::AuthorizeAck { .. })
    ));
    assert!(a.signaling.origin(PeerId::from_bytes(id(5))).is_ok());
    finish(&mut a).await;
}
#[tokio::test]
async fn worker_inbound_queue_is_bounded_and_reports_overflow_without_blocking() {
    let (url, task) = fixture(|mut socket| async move {
        for _ in 0..CHANNEL_CAPACITY + 1 {
            emit(&mut socket, welcome()).await;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    })
    .await;
    let mut worker = worker::Worker::start(url).unwrap();
    timeout(Duration::from_secs(3), async {
        while !worker.finished() {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    let mut count = 0;
    while worker.pop().is_some() {
        count += 1;
    }
    assert_eq!(count, CHANNEL_CAPACITY);
    assert_eq!(worker.terminal(), Some(RendezvousError::Backpressure));
    task.await.unwrap();
}
#[tokio::test]
async fn pending_unavailable_revokes_only_unestablished_route() {
    let (url, task) = fixture(|mut socket| async move {
        emit(&mut socket, welcome()).await;
        receive(&mut socket).await;
        emit(&mut socket, created()).await;
        emit(&mut socket, authorize()).await;
        receive(&mut socket).await;
        emit(
            &mut socket,
            ServerMessage::PeerUnavailable {
                peer_id: protocol::PeerId(id(5)),
            },
        )
        .await;
        tokio::time::sleep(Duration::from_millis(50)).await;
    })
    .await;
    let mut a = adapter(url);
    until(&mut a, |e| matches!(e, RendezvousEvent::Welcome { .. })).await;
    a.create_room().unwrap();
    until(&mut a, |e| {
        matches!(e, RendezvousEvent::PeerUnavailable { .. })
    })
    .await;
    assert!(a.signaling.origin(PeerId::from_bytes(id(5))).is_err());
    finish(&mut a).await;
    task.await.unwrap();
}

#[tokio::test]
async fn per_route_mailbox_backpressure_preserves_other_peers_and_control() {
    // Real local worker exists, but keep its handshake pending while exercising
    // the caller-owned bridge without native GNS allocation/consumption.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url =
        EndpointUrl::loopback_for_test(&format!("ws://{}/v1/ws", listener.local_addr().unwrap()))
            .unwrap();
    let mut a = adapter(url);
    a.authority = Some(AuthorityId(id(1)));
    a.phase = Phase::Host(RoomBinding {
        room: RoomId(id(2)),
        member: MemberId(id(3)),
    });
    a.handle(authorize(), &mut Vec::new()).unwrap();
    a.handle(
        ServerMessage::PeerJoined {
            peer_id: protocol::PeerId(id(5)),
            member_id: MemberId(id(6)),
        },
        &mut Vec::new(),
    )
    .unwrap();
    let room = RoomBinding {
        room: RoomId(id(2)),
        member: MemberId(id(3)),
    };
    a.bind(PeerId::from_bytes(id(7)), MemberId(id(8)), room, None)
        .unwrap();
    let mut events = Vec::new();
    for _ in 0..super::super::signaling::MAX_SIGNALS_PER_ROUTE + 1 {
        a.handle(
            ServerMessage::Signal {
                from_peer_id: protocol::PeerId(id(5)),
                payload_base64: "AP8H".into(),
            },
            &mut events,
        )
        .unwrap();
    }
    assert_eq!(
        events,
        [RendezvousEvent::SignalBackpressure {
            peer_id: PeerId::from_bytes(id(5))
        }]
    );
    a.handle(
        ServerMessage::Signal {
            from_peer_id: protocol::PeerId(id(7)),
            payload_base64: "AP8H".into(),
        },
        &mut events,
    )
    .unwrap();
    assert!(matches!(a.phase, Phase::Host(_)));
    let mut healthy = false;
    while let Some(signal) = a.signaling.pop_inbound() {
        healthy |= signal.peer == PeerId::from_bytes(id(7));
    }
    assert!(healthy);
    finish(&mut a).await;
}

#[tokio::test]
async fn adapter_drop_cleans_pending_authorization_and_preserves_active_binding() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url =
        EndpointUrl::loopback_for_test(&format!("ws://{}/v1/ws", listener.local_addr().unwrap()))
            .unwrap();
    let mut a = adapter(url);
    a.authority = Some(AuthorityId(id(1)));
    let room = RoomBinding {
        room: RoomId(id(2)),
        member: MemberId(id(3)),
    };
    a.phase = Phase::Host(room);
    a.bind(
        PeerId::from_bytes(id(5)),
        MemberId(id(6)),
        room,
        Some(JoinId(id(4))),
    )
    .unwrap();
    a.bind(PeerId::from_bytes(id(7)), MemberId(id(8)), room, None)
        .unwrap();
    let mailbox = a.signaling.clone();
    drop(a);
    assert!(mailbox.origin(PeerId::from_bytes(id(5))).is_err());
    assert!(mailbox.origin(PeerId::from_bytes(id(7))).is_ok());
}

#[tokio::test]
async fn owner_reclaims_join_churn_within_a_nearly_full_control_batch() {
    let (url, task) = fixture(|mut socket| async move {
        for n in 80..86 {
            emit(
                &mut socket,
                ServerMessage::AuthorizePeer {
                    join_id: JoinId(id(n + 100)),
                    peer_id: protocol::PeerId(id(n)),
                    member_id: MemberId(id(n + 70)),
                },
            )
            .await;
            emit(
                &mut socket,
                ServerMessage::PeerJoined {
                    peer_id: protocol::PeerId(id(n)),
                    member_id: MemberId(id(n + 70)),
                },
            )
            .await;
            emit(
                &mut socket,
                ServerMessage::PeerUnavailable {
                    peer_id: protocol::PeerId(id(n)),
                },
            )
            .await;
        }
        for n in 80..86 {
            assert_eq!(
                receive(&mut socket).await,
                ClientMessage::AuthorizeAck {
                    join_id: JoinId(id(n + 100)),
                }
            );
        }
        let _ = socket.next().await;
    })
    .await;
    let mut a = adapter(url);
    a.authority = Some(AuthorityId(id(1)));
    let room = RoomBinding {
        room: RoomId(id(2)),
        member: MemberId(id(3)),
    };
    a.phase = Phase::Host(room);
    for n in 10..70 {
        a.bind(PeerId::from_bytes(id(n)), MemberId(id(n)), room, None)
            .unwrap();
    }
    timeout(Duration::from_secs(3), async {
        while a.io.queued_events() != 18 {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    let events = a.poll_with_peer_connections(|p| (10..70).contains(&p.to_bytes()[0]));
    assert_eq!(events.len(), 12);
    assert!(!events
        .iter()
        .any(|e| matches!(e, RendezvousEvent::Disconnected(_))));
    assert_eq!(a.routes.len(), 60);
    a.bind(PeerId::from_bytes(id(90)), MemberId(id(160)), room, None)
        .unwrap();
    assert!(a.signaling.origin(PeerId::from_bytes(id(90))).is_ok());
    finish(&mut a).await;
    task.await.unwrap();
}

#[tokio::test]
async fn owner_reconciliation_preserves_native_and_queued_inbound_until_they_are_gone() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url =
        EndpointUrl::loopback_for_test(&format!("ws://{}/v1/ws", listener.local_addr().unwrap()))
            .unwrap();
    let mut a = adapter(url);
    a.authority = Some(AuthorityId(id(1)));
    let room = RoomBinding {
        room: RoomId(id(2)),
        member: MemberId(id(3)),
    };
    a.phase = Phase::Host(room);
    for n in 5..8 {
        a.bind(PeerId::from_bytes(id(n)), MemberId(id(n + 10)), room, None)
            .unwrap();
    }
    let queued = PeerId::from_bytes(id(6));
    let native = PeerId::from_bytes(id(7));
    a.signaling.receive(queued, &[0, 255, 7]).unwrap();
    a.control_lost();
    a.reclaim_unavailable_routes(|p| p == native);
    assert!(a.signaling.origin(PeerId::from_bytes(id(5))).is_err());
    assert!(a.has_pending_signal(queued));
    assert!(a.signaling.origin(native).is_ok());
    assert_eq!(a.signaling.pop_inbound().unwrap().peer, queued);
    a.reclaim_unavailable_routes(|p| p == native);
    assert!(a.signaling.origin(queued).is_err());
    assert!(a.signaling.origin(native).is_ok());
    a.reclaim_unavailable_routes(|_| false);
    assert!(a.routes.is_empty());
    finish(&mut a).await;
}

#[tokio::test]
async fn host_ready_route_survives_control_loss_in_its_delivery_batch() {
    let (url, task) = fixture(|mut socket| async move {
        emit(
            &mut socket,
            ServerMessage::RoomJoined {
                room_id: RoomId(id(2)),
                self_member_id: MemberId(id(3)),
                host_peer_id: protocol::PeerId(id(5)),
                host_member_id: MemberId(id(6)),
            },
        )
        .await;
        emit(&mut socket, ServerMessage::RoomClosed {}).await;
        let _ = socket.next().await;
    })
    .await;
    let mut a = adapter(url);
    a.authority = Some(AuthorityId(id(1)));
    a.phase = Phase::Joining;
    timeout(Duration::from_secs(3), async {
        while a.io.queued_events() != 2 {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    let events = a.poll_with_peer_connections(|_| false);
    assert!(matches!(
        events.first(),
        Some(RendezvousEvent::HostReady { .. })
    ));
    assert!(events.contains(&RendezvousEvent::RoomClosed));
    let host = PeerId::from_bytes(id(5));
    assert!(a.signaling.origin(host).is_ok());
    a.reclaim_unavailable_routes(|p| p == host);
    assert!(a.signaling.origin(host).is_ok());
    a.reclaim_unavailable_routes(|_| false);
    assert!(a.signaling.origin(host).is_err());
    finish(&mut a).await;
    task.await.unwrap();
}
