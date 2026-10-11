use super::*;

#[test]
fn concurrent_join_notifications_follow_ready_and_snapshots_are_silent() {
    let bus = Arc::new(Mutex::new(Bus::default()));
    let mut host = Endpoint::host(&bus);
    let mut a = Endpoint::client(&bus, 1);
    let mut b = Endpoint::client(&bus, 2);
    join(&mut host, &mut [&mut a, &mut b]);
    a.poll();
    b.poll();
    let events = &host.runtime.runtime.events;
    let starts: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            GameEvent::PlayerJoining { connection } => Some(*connection),
            _ => None,
        })
        .collect();
    assert_eq!(starts.len(), 2);
    assert_ne!(starts[0], starts[1]);
    for connection in starts {
        let start = events.iter().position(|event| matches!(event, GameEvent::PlayerJoining { connection: id } if *id == connection)).unwrap();
        let ready = events.iter().position(|event| matches!(event, GameEvent::PlayerJoined { connection: Some(id), .. } if *id == connection)).unwrap();
        assert!(start < ready);
        assert_eq!(events.iter().filter(|event| matches!(event, GameEvent::PlayerJoined { connection: Some(id), .. } if *id == connection)).count(), 1);
    }
    // Each client's snapshot contains the host, itself and possibly the other
    // ready peer. It must never generate notifications for those entries.
    for peer in [&a, &b] {
        let events = &peer.runtime.runtime.events;
        let local = peer.runtime.runtime.status.local_player.unwrap();
        assert!(!events
            .iter()
            .any(|event| matches!(event, GameEvent::PlayerJoining { .. })));
        assert!(events.iter().all(|event| matches!(event,
            GameEvent::PlayerJoined { player, connection: None }
            if player.player != PlayerId(0) && player.player != local)));
    }
    let before = host.runtime.runtime.events.len();
    for _ in 0..3 {
        host.poll();
        a.poll();
        b.poll();
    }
    assert_eq!(host.runtime.runtime.events.len(), before);
}

#[test]
fn leave_keeps_name_after_roster_removal_and_duplicate_disconnect_is_silent() {
    let bus = Arc::new(Mutex::new(Bus::default()));
    let mut host = Endpoint::host(&bus);
    let mut a = Endpoint::client(&bus, 1);
    join(&mut host, &mut [&mut a]);
    let mut b = Endpoint::client(&bus, 2);
    join(&mut host, &mut [&mut a, &mut b]);
    a.poll();
    b.poll();
    assert!(
        b.runtime.runtime.events.is_empty(),
        "initial roster is not a join event"
    );
    let player = a.runtime.runtime.status.local_player.unwrap();
    let connection = host.connection(player);
    host.runtime.runtime.events.clear();
    host.runtime
        .disconnect(
            connection,
            DisconnectReason::RemoteClosed,
            None,
            &mut host.store,
        )
        .unwrap();
    b.poll();
    for peer in [&host, &b] {
        assert!(peer.runtime.runtime.roster.get(player).is_none());
        assert!(
            matches!(peer.runtime.runtime.events.as_slice(), [GameEvent::PlayerLeft { player: info }]
            if info.player == player && info.display_name.as_ref().unwrap().as_ref() == "Alice")
        );
    }
    host.runtime
        .disconnect(
            connection,
            DisconnectReason::RemoteClosed,
            Some(player),
            &mut host.store,
        )
        .unwrap();
    b.poll();
    assert_eq!(host.runtime.runtime.events.len(), 1);
    assert_eq!(b.runtime.runtime.events.len(), 1);
}

#[test]
fn sync_disconnect_or_failure_ends_only_its_join_notification_once() {
    for fail_send in [false, true] {
        let bus = Arc::new(Mutex::new(Bus::default()));
        let mut host = Endpoint::host(&bus);
        let mut a = Endpoint::client(&bus, 1);
        let mut b = Endpoint::client(&bus, 2);
        // Stop polling clients after their authenticated sync begins.
        for _ in 0..100 {
            host.poll();
            if host
                .runtime
                .runtime
                .events
                .iter()
                .filter(|event| matches!(event, GameEvent::PlayerJoining { .. }))
                .count()
                == 2
            {
                break;
            }
            a.poll();
            b.poll();
        }
        let starts: Vec<_> = host
            .runtime
            .runtime
            .events
            .iter()
            .filter_map(|event| match event {
                GameEvent::PlayerJoining { connection } => Some(*connection),
                _ => None,
            })
            .collect();
        assert_eq!(starts.len(), 2);
        let attempt = starts[0];
        let Role::Host(host_state) = &host.runtime.runtime.role else {
            unreachable!()
        };
        let id = *host_state
            .joining
            .iter()
            .find(|id| crate::events::JoinAttemptId::from(**id) == attempt)
            .unwrap();
        if fail_send {
            bus.lock().unwrap().fail.insert(id);
            // Complete a sync step that requires a reply to the affected peer.
            for _ in 0..30 {
                a.poll();
                b.poll();
                host.poll();
            }
        } else {
            host.runtime
                .disconnect(id, DisconnectReason::RemoteClosed, None, &mut host.store)
                .unwrap();
        }
        assert_eq!(host.runtime.runtime.events.iter().filter(|event| matches!(event, GameEvent::PlayerJoinCancelled { connection, .. } if *connection == attempt)).count(), 1);
        assert!(!host.runtime.runtime.events.iter().any(|event| matches!(event, GameEvent::PlayerJoined { connection: Some(connection), .. } if *connection == attempt)));
        host.runtime
            .disconnect(id, DisconnectReason::RemoteClosed, None, &mut host.store)
            .unwrap();
        assert_eq!(host.runtime.runtime.events.iter().filter(|event| matches!(event, GameEvent::PlayerJoinCancelled { connection, .. } if *connection == attempt)).count(), 1);
    }
}

#[test]
fn unauthenticated_connection_and_authentication_failure_emit_no_notifications() {
    let bus = Arc::new(Mutex::new(Bus::default()));
    let mut host = Endpoint::host(&bus);
    let mut backend = Fake {
        id: 1,
        bus: bus.clone(),
    };
    let client_connection = backend.connect("127.0.0.1:10000".parse().unwrap()).unwrap();
    host.poll();
    assert!(host.runtime.runtime.events.is_empty());
    let mut events = Vec::new();
    backend.poll(&mut events).unwrap();
    backend
        .close(client_connection, DisconnectReason::AuthenticationFailed)
        .unwrap();
    host.poll();
    assert!(host.runtime.runtime.events.is_empty());
}

#[test]
fn whole_runtime_failure_finishes_pending_notifications_once() {
    let bus = Arc::new(Mutex::new(Bus::default()));
    let mut host = Endpoint::host(&bus);
    let mut a = Endpoint::client(&bus, 1);
    for _ in 0..100 {
        host.poll();
        if host
            .runtime
            .runtime
            .events
            .iter()
            .any(|e| matches!(e, GameEvent::PlayerJoining { .. }))
        {
            break;
        }
        a.poll();
    }
    assert!(matches!(
        host.runtime.runtime.events.as_slice(),
        [GameEvent::PlayerJoining { .. }]
    ));
    host.runtime
        .runtime
        .fail(RuntimeFailure::internal("fixture failure"));
    assert!(
        matches!(host.runtime.runtime.events.as_slice(), [GameEvent::PlayerJoining { connection }, GameEvent::PlayerJoinCancelled { connection: cancelled, reason: DisconnectReason::BackendFailure }] if connection == cancelled)
    );
    host.runtime
        .runtime
        .fail(RuntimeFailure::internal("repeat failure"));
    assert_eq!(host.runtime.runtime.events.len(), 2);
}

#[test]
fn world_message_bridge_flushes_notifications_and_resets_on_session_changes() {
    let mut pair = Pair::new();
    let mut reader = bevy::ecs::message::MessageCursor::<GameEvent>::default();
    let mut events = Vec::new();
    for _ in 0..200 {
        pair.frame();
        events.extend(
            reader
                .read(pair.host.world().resource::<Messages<GameEvent>>())
                .cloned(),
        );
        if pair.client.world().resource::<NetworkStatus>().phase == RuntimePhase::Ready {
            break;
        }
    }
    pair.host.update();
    events.extend(
        reader
            .read(pair.host.world().resource::<Messages<GameEvent>>())
            .cloned(),
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, GameEvent::SessionReset))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, GameEvent::PlayerJoining { .. }))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, GameEvent::PlayerJoined { .. }))
            .count(),
        1
    );
    stop_session(pair.host.world_mut());
    assert!(reader
        .read(pair.host.world().resource::<Messages<GameEvent>>())
        .any(|e| matches!(e, GameEvent::SessionReset)));
}
