use super::*;
use tokio::sync::mpsc;

fn host() -> (
    RendezvousAdapter,
    mpsc::Receiver<ClientMessage>,
    mpsc::Sender<ServerMessage>,
) {
    let (io, commands, events) = worker::Worker::channels_for_test();
    (
        RendezvousAdapter {
            io,
            signaling: SignalingEndpoint::routed(),
            local: PeerId::from_bytes(id(9)),
            authority: Some(AuthorityId(id(1))),
            phase: Phase::Host(room()),
            routes: BTreeMap::new(),
            rejected: BTreeMap::new(),
            deferred: None,
            deferred_signal: None,
            terminal_reported: false,
            turn_expiry: 0,
            turn_addresses: Vec::new(),
        },
        commands,
        events,
    )
}
fn room() -> RoomBinding {
    RoomBinding {
        room: RoomId(id(2)),
        member: MemberId(id(3)),
    }
}
fn auth(p: u8, m: u8, j: u8) -> ServerMessage {
    ServerMessage::AuthorizePeer {
        peer_id: protocol::PeerId(id(p)),
        member_id: MemberId(id(m)),
        abuse_key: AbuseKey(id(m)),
        join_id: JoinId(id(j)),
    }
}
fn unavailable(p: u8) -> ServerMessage {
    ServerMessage::PeerUnavailable {
        peer_id: protocol::PeerId(id(p)),
    }
}
fn bind(a: &mut RendezvousAdapter, p: u8, m: u8) {
    a.bind(
        PeerId::from_bytes(id(p)),
        MemberId(id(m)),
        room(),
        None,
        AbuseKey(id(m)),
    )
    .unwrap();
}
fn signal(p: u8) -> ClientMessage {
    ClientMessage::Signal {
        to_peer_id: protocol::PeerId(id(p)),
        payload_base64: "AP8H".into(),
    }
}

#[test]
fn rejected_rejoin_completion_preserves_old_native_origin_even_after_retirement() {
    for retire in [false, true] {
        for revoking in [false, true] {
            let (mut a, mut commands, events) = host();
            bind(&mut a, 5, 6);
            let p = PeerId::from_bytes(id(5));
            let origin = a.signaling.origin(p).unwrap();
            a.routes.get_mut(&p).unwrap().available = false;
            a.routes.get_mut(&p).unwrap().revoking = revoking;
            events.try_send(auth(5, 8, 4)).unwrap();
            assert!(a.poll_with_admission(|_| true, true).is_empty());
            assert_eq!(
                commands.try_recv().unwrap(),
                ClientMessage::AuthorizeReject {
                    join_id: JoinId(id(4))
                }
            );
            assert_eq!(a.signaling.origin(p).unwrap(), origin);
            if retire {
                a.release_route(p);
            }
            events.try_send(unavailable(5)).unwrap();
            assert!(a.poll_with_admission(|_| !retire, true).is_empty());
            assert!(a.rejected.is_empty());
            assert!(matches!(a.phase, Phase::Host(_)));
            if !retire {
                assert_eq!(a.signaling.origin(p).unwrap(), origin);
            }
        }
    }
}

#[test]
fn route_or_native_capacity_rejection_is_bounded_and_freed_capacity_accepts_join() {
    for native in [false, true] {
        let (mut a, mut commands, events) = host();
        if native {
            for p in 10..74 {
                bind(&mut a, p, p + 80);
                a.routes
                    .get_mut(&PeerId::from_bytes(id(p)))
                    .unwrap()
                    .available = false;
            }
        }
        for _ in 0..130 {
            events.try_send(auth(80, 180, 160)).unwrap();
            assert!(a.poll_with_admission(|_| true, native).is_empty());
            assert_eq!(
                commands.try_recv().unwrap(),
                ClientMessage::AuthorizeReject {
                    join_id: JoinId(id(160))
                }
            );
            assert_eq!(a.rejected.len(), 1);
            events.try_send(unavailable(80)).unwrap();
            assert!(a.poll_with_admission(|_| true, native).is_empty());
            assert!(a.rejected.is_empty());
            assert!(matches!(a.phase, Phase::Host(_)));
        }
        a.release_route(PeerId::from_bytes(id(10)));
        events.try_send(auth(80, 180, 160)).unwrap();
        assert!(a.poll_with_admission(|_| true, true).is_empty());
        assert_eq!(
            commands.try_recv().unwrap(),
            ClientMessage::AuthorizeAck {
                join_id: JoinId(id(160))
            }
        );
        events
            .try_send(ServerMessage::PeerJoined {
                peer_id: protocol::PeerId(id(80)),
                member_id: MemberId(id(180)),
            })
            .unwrap();
        assert!(matches!(
            a.poll_with_admission(|_| true, true).as_slice(),
            [RendezvousEvent::PeerJoined { .. }]
        ));
    }
}

#[test]
fn identity_conflicts_remain_fatal_before_capacity_rejection() {
    for message in [
        auth(9, 8, 4),
        auth(5, 3, 4),
        auth(5, 8, 4),
        auth(7, 6, 4),
        auth(7, 8, 20),
    ] {
        let (mut a, _commands, _events) = host();
        bind(&mut a, 5, 6);
        a.bind(
            PeerId::from_bytes(id(10)),
            MemberId(id(11)),
            room(),
            Some(JoinId(id(20))),
            AbuseKey(id(11)),
        )
        .unwrap();
        assert_eq!(
            a.handle_with_admission(message, &mut Vec::new(), false),
            Err(RendezvousError::ProtocolViolation)
        );
        assert!(a.rejected.is_empty());
    }
    for message in [auth(7, 8, 4), auth(7, 6, 10), auth(5, 8, 10)] {
        let (mut a, _commands, _events) = host();
        a.handle_with_admission(auth(5, 6, 4), &mut Vec::new(), false)
            .unwrap();
        assert_eq!(
            a.handle_with_admission(message, &mut Vec::new(), false),
            Err(RendezvousError::ProtocolViolation)
        );
    }
}

#[test]
fn one_freed_command_slot_prioritizes_required_then_revoke_confirm_signal() {
    let (mut a, mut commands, events) = host();
    bind(&mut a, 5, 6);
    bind(&mut a, 7, 8); // Reverse peer order must not let Confirm beat Revoke.
    bind(&mut a, 10, 11);
    a.confirm_peer(PeerId::from_bytes(id(5)));
    a.revoke_peer(PeerId::from_bytes(id(7)));
    a.deferred_signal = Some(signal(10));
    for _ in 0..CHANNEL_CAPACITY {
        a.io.send(ClientMessage::LeaveRoom {}).unwrap();
    }
    events.try_send(auth(12, 13, 14)).unwrap();
    a.poll_with_admission(|_| true, true);
    assert!(a.deferred.is_some());
    let expected = [
        ClientMessage::AuthorizeAck {
            join_id: JoinId(id(14)),
        },
        ClientMessage::RevokePeer {
            peer_id: protocol::PeerId(id(7)),
            member_id: MemberId(id(8)),
        },
        ClientMessage::ConfirmPeer {
            peer_id: protocol::PeerId(id(5)),
            member_id: MemberId(id(6)),
        },
        signal(10),
    ];
    for _ in &expected {
        assert_eq!(commands.try_recv().unwrap(), ClientMessage::LeaveRoom {});
        assert!(a.poll_with_admission(|_| true, true).is_empty());
    }
    for _ in expected.len()..CHANNEL_CAPACITY {
        assert_eq!(commands.try_recv().unwrap(), ClientMessage::LeaveRoom {});
    }
    for message in expected {
        assert_eq!(commands.try_recv().unwrap(), message);
    }
    assert!(a.deferred.is_none() && a.deferred_signal.is_none());
}

#[test]
fn obsolete_deferred_signal_cannot_cross_peer_rebinding() {
    for transition in 0..3 {
        let (mut a, mut commands, _events) = host();
        bind(&mut a, 5, 6);
        a.deferred_signal = Some(signal(5));
        let p = PeerId::from_bytes(id(5));
        match transition {
            0 => a.handle(unavailable(5), &mut Vec::new()).unwrap(),
            1 => a.revoke_peer(p),
            _ => a.release_route(p),
        }
        assert!(a.deferred_signal.is_none());
        a.release_route(p);
        bind(&mut a, 5, 8);
        assert!(a.flush_signal().unwrap());
        assert!(commands.try_recv().is_err());
    }
}

#[tokio::test]
async fn full_inbound_channel_does_not_block_outgoing_required_control() {
    let (url, task) = fixture(|mut socket| async move {
        for _ in 0..CHANNEL_CAPACITY + 1 {
            emit(&mut socket, welcome()).await;
        }
        assert_eq!(
            receive(&mut socket).await,
            ClientMessage::AuthorizeReject {
                join_id: JoinId(id(4))
            }
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    })
    .await;
    let mut w = worker::Worker::start(url).unwrap();
    timeout(Duration::from_secs(1), async {
        while w.queued_events() < CHANNEL_CAPACITY {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    // Owner deliberately does not drain the inbound queue.
    w.send(ClientMessage::AuthorizeReject {
        join_id: JoinId(id(4)),
    })
    .unwrap();
    task.await.unwrap();
    assert_eq!(w.queued_events(), CHANNEL_CAPACITY);
    w.shutdown();
}
