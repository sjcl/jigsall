use super::*;
use crate::network::cursor::{
    CursorEntry, CursorSnapshot, CursorUpdate, CURSOR_INTERVAL, CURSOR_TIMEOUT,
};
fn cursor(endpoint: &Endpoint, player: PlayerId) -> Option<Vec2> {
    endpoint
        .runtime
        .cursors
        .presentation
        .cursors()
        .find(|(id, _)| *id == player)
        .map(|(_, c)| c.target_world_position)
}
fn poll_at(endpoint: &mut Endpoint, now: Instant) {
    endpoint
        .runtime
        .poll(&mut endpoint.store, &mut endpoint.gesture, now)
        .unwrap();
}
fn send_update(client: &mut Endpoint, tick: u64, position: Option<Vec2>) {
    let session = client.runtime.runtime.session.as_ref().unwrap();
    let msg = WireMessage::CursorUpdate(CursorUpdate {
        session: session.session_definition().id,
        authority_epoch: session.cursor().epoch,
        tick,
        position,
    });
    let Role::Client(c) = &client.runtime.runtime.role else {
        unreachable!()
    };
    client
        .runtime
        .runtime
        .transport
        .send(
            c.bootstrap.host_connection(),
            MessageClass::Transient,
            &wire::encode(&msg).unwrap(),
        )
        .unwrap();
}

#[test]
fn cursor_runtime_authenticated_identity_ordering_full_batch_loss_and_cleanup() {
    let bus = Arc::new(Mutex::new(Bus::default()));
    let mut host = Endpoint::host(&bus);
    let mut a = Endpoint::client(&bus, 1);
    let mut b = Endpoint::client(&bus, 2);
    join(&mut host, &mut [&mut a, &mut b]);
    a.poll();
    b.poll();
    let aid = a.runtime.runtime.status.local_player.unwrap();
    let bid = b.runtime.runtime.status.local_player.unwrap();
    let now = Instant::now();
    for (tick, pos, expected) in [
        (10, 10., 10.),
        (10, 100., 10.),
        (9, 9., 10.),
        (11, 11., 11.),
    ] {
        send_update(&mut a, tick, Some(Vec2::splat(pos)));
        poll_at(&mut host, now);
        assert_eq!(cursor(&host, aid), Some(Vec2::splat(expected)));
    }
    b.runtime.cursor_frame(Some(Vec2::splat(30.)), now);
    poll_at(&mut host, now);
    let states = host.store.states.clone();
    let authority = host.runtime.runtime.session.as_ref().unwrap().cursor();
    bus.lock().unwrap().sent.clear();
    crate::resources::pieces::without_piece_state_access(|| {
        host.runtime.cursor_frame(Some(Vec2::new(20., -20.)), now);
    });
    assert_eq!(
        bus.lock()
            .unwrap()
            .sent
            .iter()
            .filter(|(_, class, _)| *class == MessageClass::Transient)
            .count(),
        2
    );
    a.poll();
    b.poll();
    assert_eq!(cursor(&a, PlayerId(0)), Some(Vec2::new(20., -20.)));
    assert_eq!(cursor(&a, bid), Some(Vec2::splat(30.)));
    assert!(cursor(&a, aid).is_none());
    assert!(cursor(&host, PlayerId(0)).is_none());
    // Both directions lose multiple samples. Stationary heartbeat and one full snapshot heal.
    bus.lock().unwrap().drop_transient = true;
    for tick in 1..=3 {
        b.runtime
            .cursor_frame(Some(Vec2::splat(60.)), now + CURSOR_INTERVAL * tick);
        host.runtime
            .cursor_frame(Some(Vec2::new(20., -20.)), now + CURSOR_INTERVAL * tick);
    }
    assert_eq!(cursor(&a, bid), Some(Vec2::splat(30.)));
    bus.lock().unwrap().drop_transient = false;
    b.runtime
        .cursor_frame(Some(Vec2::splat(60.)), now + CURSOR_INTERVAL * 4);
    poll_at(&mut host, now + CURSOR_INTERVAL * 4);
    host.runtime
        .cursor_frame(Some(Vec2::new(20., -20.)), now + CURSOR_INTERVAL * 4);
    a.poll();
    assert_eq!(cursor(&a, bid), Some(Vec2::splat(60.)));
    // Hide packet is lost; stale timeout removes it without reliable resend.
    bus.lock().unwrap().drop_transient = true;
    b.runtime
        .cursor_frame(None, now + CURSOR_INTERVAL * 4 + Duration::from_millis(1));
    bus.lock().unwrap().drop_transient = false;
    host.runtime.cursor_frame(
        Some(Vec2::new(20., -20.)),
        now + CURSOR_INTERVAL * 4 + CURSOR_TIMEOUT,
    );
    a.poll();
    assert!(cursor(&host, bid).is_none());
    assert!(cursor(&a, bid).is_none());
    send_update(&mut b, 20, Some(Vec2::ONE));
    host.poll();
    assert!(cursor(&host, bid).is_some());
    let connection = host.connection(bid);
    host.runtime
        .disconnect(
            connection,
            DisconnectReason::RemoteClosed,
            None,
            &mut host.store,
        )
        .unwrap();
    a.poll();
    assert!(cursor(&host, bid).is_none());
    assert!(cursor(&a, bid).is_none());
    assert_eq!(host.store.states, states);
    assert_eq!(
        host.runtime.runtime.session.as_ref().unwrap().cursor(),
        authority
    );
    host.runtime.teardown(&mut host.store);
    assert_eq!(host.runtime.cursors.presentation.cursors().count(), 0);
}

#[test]
fn cursor_runtime_idle_stops_after_lost_empty_and_resumes_for_late_ready_peer() {
    let bus = Arc::new(Mutex::new(Bus::default()));
    let mut host = Endpoint::host(&bus);
    let mut a = Endpoint::client(&bus, 1);
    join(&mut host, &mut [&mut a]);
    let now = Instant::now();
    host.runtime.cursor_frame(Some(Vec2::ONE), now);
    poll_at(&mut a, now);
    assert_eq!(cursor(&a, PlayerId(0)), Some(Vec2::ONE));

    bus.lock().unwrap().sent.clear();
    bus.lock().unwrap().drop_transient = true;
    let hidden_at = now + Duration::from_millis(1);
    host.runtime.cursor_frame(None, hidden_at);
    assert_eq!(
        bus.lock()
            .unwrap()
            .sent
            .iter()
            .filter(|(_, class, _)| *class == MessageClass::Transient)
            .count(),
        1,
        "send the final empty batch once"
    );
    bus.lock().unwrap().drop_transient = false;

    let mut b = Endpoint::client(&bus, 2);
    join(&mut host, &mut [&mut a, &mut b]);
    assert!(cursor(&b, PlayerId(0)).is_none());
    a.runtime
        .cursor_frame(None, now + CURSOR_TIMEOUT - Duration::from_millis(1));
    assert!(cursor(&a, PlayerId(0)).is_some());
    a.runtime.cursor_frame(None, now + CURSOR_TIMEOUT);
    assert!(cursor(&a, PlayerId(0)).is_none());

    bus.lock().unwrap().sent.clear();
    for frame in 0..360 {
        host.runtime.cursor_frame(
            None,
            hidden_at + Duration::from_secs_f64(frame as f64 / 360.),
        );
    }
    assert!(bus
        .lock()
        .unwrap()
        .sent
        .iter()
        .all(|(_, class, _)| *class != MessageClass::Transient));

    let resumed_at = now + Duration::from_secs(2);
    host.runtime
        .cursor_frame(Some(Vec2::splat(42.)), resumed_at);
    poll_at(&mut a, resumed_at);
    poll_at(&mut b, resumed_at);
    for client in [&a, &b] {
        assert_eq!(cursor(client, PlayerId(0)), Some(Vec2::splat(42.)));
    }
    host.runtime
        .cursor_frame(Some(Vec2::splat(42.)), resumed_at + CURSOR_INTERVAL);
    assert_eq!(
        bus.lock()
            .unwrap()
            .sent
            .iter()
            .filter(|(_, class, _)| *class == MessageClass::Transient)
            .count(),
        4,
        "resume stationary heartbeats to both Ready peers"
    );
}

#[test]
fn cursor_runtime_ready_gating_batch_recipients_and_best_effort_failure() {
    let bus = Arc::new(Mutex::new(Bus::default()));
    let mut host = Endpoint::host(&bus);
    let mut client = Endpoint::client(&bus, 1);
    let now = Instant::now();
    for phase in [
        RuntimePhase::Connecting,
        RuntimePhase::Authenticating,
        RuntimePhase::Syncing(SyncPhase::BaselineTransfer),
    ] {
        client.runtime.runtime.status.phase = phase;
        client.runtime.cursor_frame(Some(Vec2::ONE), now);
    }
    assert!(bus.lock().unwrap().sent.is_empty());
    client.runtime.runtime.status.phase = RuntimePhase::Connecting;
    host.poll();
    host.runtime.cursor_frame(Some(Vec2::ONE), now);
    assert_eq!(
        bus.lock()
            .unwrap()
            .sent
            .iter()
            .filter(|(_, class, _)| *class == MessageClass::Transient)
            .count(),
        0
    );
    join(&mut host, &mut [&mut client]);
    let id = host.connection(client.runtime.runtime.status.local_player.unwrap());
    // A Syncing connection can coexist with Ready connections and never receives cursors.
    let mut syncing = Endpoint::client(&bus, 2);
    host.poll();
    syncing.poll();
    bus.lock().unwrap().sent.clear();
    host.runtime
        .cursor_frame(Some(Vec2::ONE), now + CURSOR_INTERVAL);
    let sent: Vec<_> = bus
        .lock()
        .unwrap()
        .sent
        .iter()
        .filter(|(_, class, _)| *class == MessageClass::Transient)
        .map(|(id, _, _)| *id)
        .collect();
    assert_eq!(sent, vec![id]);
    let Role::Client(c) = &client.runtime.runtime.role else {
        unreachable!()
    };
    let outgoing = c.bootstrap.host_connection();
    bus.lock().unwrap().fail.extend([id, outgoing]);
    client.runtime.cursor_frame(Some(Vec2::ONE), now);
    host.runtime.cursor_frame(None, now + CURSOR_INTERVAL * 2);
    assert!(client.runtime.active && host.runtime.active);
    assert!(bus.lock().unwrap().closed.is_empty());
    bus.lock().unwrap().fail.clear();
    client.poll();
    host.poll();
    assert!(client.runtime.active && host.runtime.active);
    assert!(client.runtime.runtime.transport.has_channel(outgoing));
    assert!(host.runtime.runtime.transport.has_channel(id));
}

#[test]
fn cursor_snapshot_overtakes_ready_commit_then_next_periodic_snapshot_recovers() {
    let bus = Arc::new(Mutex::new(Bus::default()));
    let mut host = Endpoint::host(&bus);
    let mut client = Endpoint::client(&bus, 1);
    for _ in 0..200 {
        host.poll();
        if host.runtime.runtime.roster.len() == 2 {
            break;
        }
        client.poll();
    }
    assert_eq!(host.runtime.runtime.roster.len(), 2);
    assert!(!client.ready());
    // ReadyCommit is already encrypted and queued. Deliver Transient before it.
    let commit = bus
        .lock()
        .unwrap()
        .inbox
        .get_mut(&1)
        .unwrap()
        .pop_back()
        .unwrap();
    let now = Instant::now();
    host.runtime.cursor_frame(Some(Vec2::splat(42.)), now);
    client.poll();
    assert!(client.runtime.active);
    assert_eq!(client.runtime.cursors.presentation.cursors().count(), 0);
    let Role::Client(c) = &client.runtime.runtime.role else {
        unreachable!()
    };
    assert_eq!(c.bootstrap.state(), Some(ConnectionState::Syncing));
    assert!(matches!(
        client.runtime.runtime.status.phase,
        RuntimePhase::Syncing(_)
    ));
    bus.lock()
        .unwrap()
        .inbox
        .entry(1)
        .or_default()
        .push_back(commit);
    client.poll();
    assert!(client.ready());
    host.runtime
        .cursor_frame(Some(Vec2::splat(42.)), now + CURSOR_INTERVAL);
    client.poll();
    assert_eq!(cursor(&client, PlayerId(0)), Some(Vec2::splat(42.)));
    assert!(bus.lock().unwrap().closed.is_empty());
}

#[test]
fn cursor_runtime_unknown_player_before_presence_is_benign_then_left_stays_hidden() {
    let bus = Arc::new(Mutex::new(Bus::default()));
    let mut host = Endpoint::host(&bus);
    let mut a = Endpoint::client(&bus, 1);
    join(&mut host, &mut [&mut a]);
    let aid = a.runtime.runtime.status.local_player.unwrap();
    let connection = host.connection(aid);
    let session = host.runtime.runtime.session.as_ref().unwrap();
    let msg = |sequence| {
        WireMessage::CursorSnapshot(CursorSnapshot {
            session: session.session_definition().id,
            authority_epoch: session.cursor().epoch,
            sequence,
            entries: vec![CursorEntry {
                player: PlayerId(999),
                position: Vec2::ONE,
            }],
        })
    };
    host.runtime
        .runtime
        .transport
        .send(
            connection,
            MessageClass::Transient,
            &wire::encode(&msg(20)).unwrap(),
        )
        .unwrap();
    a.poll();
    assert!(a.runtime.active);
    assert!(cursor(&a, PlayerId(999)).is_none());
    let joined = PresenceMessage::PlayerJoined {
        revision: 2,
        player: RosterPlayer {
            player: PlayerId(999),
            display_name: None,
        },
    };
    host.runtime
        .runtime
        .transport
        .send(
            connection,
            MessageClass::Control,
            &wire::encode(&WireMessage::Presence(joined)).unwrap(),
        )
        .unwrap();
    host.runtime
        .runtime
        .transport
        .send(
            connection,
            MessageClass::Transient,
            &wire::encode(&msg(21)).unwrap(),
        )
        .unwrap();
    a.poll();
    assert_eq!(cursor(&a, PlayerId(999)), Some(Vec2::ONE));
    host.runtime
        .runtime
        .transport
        .send(
            connection,
            MessageClass::Control,
            &wire::encode(&WireMessage::Presence(PresenceMessage::PlayerLeft {
                revision: 3,
                player: PlayerId(999),
            }))
            .unwrap(),
        )
        .unwrap();
    a.poll();
    assert!(cursor(&a, PlayerId(999)).is_none());
    host.runtime
        .runtime
        .transport
        .send(
            connection,
            MessageClass::Transient,
            &wire::encode(&msg(22)).unwrap(),
        )
        .unwrap();
    a.poll();
    assert!(cursor(&a, PlayerId(999)).is_none());
    assert!(a.runtime.active);
}

#[test]
fn cursor_world_pause_pending_focus_outside_and_session_cleanup_ignore_stale_input() {
    let mut pair = Pair::new();
    pair.ready();
    let world = pair.client.world_mut();
    world.resource_mut::<NextState<AppState>>().reset();
    world.insert_resource(State::new(GameSubState::Playing));
    world.resource_mut::<InputState>().mouse_position = Some(Vec2::new(12., 34.));
    world.resource_mut::<InputState>().window_focused = true;
    assert_eq!(cursors::local_cursor(world), Some(Vec2::new(12., 34.)));
    world.insert_resource(LocalGameplayBlocked(true));
    assert!(cursors::local_cursor(world).is_none());
    world.resource_mut::<LocalGameplayBlocked>().0 = false;
    assert!(cursors::local_cursor(world).is_some());
    world.insert_resource(NextState::Pending(GameSubState::Paused));
    assert!(cursors::local_cursor(world).is_none());
    world.resource_mut::<NextState<GameSubState>>().reset();
    world.insert_resource(State::new(GameSubState::Paused));
    assert!(cursors::local_cursor(world).is_none());
    world.insert_resource(State::new(GameSubState::Playing));
    world.resource_mut::<InputState>().window_focused = false;
    assert!(cursors::local_cursor(world).is_none());
    world.resource_mut::<InputState>().window_focused = true;
    let window = world
        .spawn((
            Window {
                focused: true,
                ..default()
            },
            bevy::window::PrimaryWindow,
        ))
        .id();
    assert!(cursors::local_cursor(world).is_none());
    world
        .entity_mut(window)
        .get_mut::<Window>()
        .unwrap()
        .set_cursor_position(Some(Vec2::ONE));
    assert!(cursors::local_cursor(world).is_some());
    world
        .resource_mut::<NextState<AppState>>()
        .set(AppState::Menu);
    assert!(cursors::local_cursor(world).is_none());
    stop_session(world);
    assert_eq!(
        world
            .resource::<remote_cursor::RemoteCursorPresentation>()
            .cursors()
            .count(),
        0
    );
}

#[test]
fn cursor_runtime_stationary_world_resource_stays_unchanged_and_host_broadcast_rate_is_bounded() {
    let mut pair = Pair::new();
    pair.ready();
    pair.converge();
    let before = pair
        .client
        .world()
        .get_resource_ref::<remote_cursor::RemoteCursorPresentation>()
        .unwrap()
        .last_changed();
    pair.frame();
    assert_eq!(
        pair.client
            .world()
            .get_resource_ref::<remote_cursor::RemoteCursorPresentation>()
            .unwrap()
            .last_changed(),
        before
    );
    let bus = Arc::new(Mutex::new(Bus::default()));
    let mut host = Endpoint::host(&bus);
    let mut a = Endpoint::client(&bus, 1);
    let mut b = Endpoint::client(&bus, 2);
    join(&mut host, &mut [&mut a, &mut b]);
    a.poll();
    b.poll();
    let now = Instant::now();
    bus.lock().unwrap().sent.clear();
    for frame in 0..360 {
        let at = now + Duration::from_nanos(frame * 1_000_000_000 / 360);
        // Accepting incoming samples every frame must never trigger per-update fan-out.
        send_update(&mut a, frame, Some(Vec2::ONE));
        poll_at(&mut host, at);
        host.runtime.cursor_frame(Some(Vec2::ONE), at);
    }
    let host_ids: BTreeSet<_> = host
        .runtime
        .runtime
        .connections
        .peers()
        .filter(|p| p.player.is_some())
        .map(|p| p.connection)
        .collect();
    let sent = &bus.lock().unwrap().sent;
    assert_eq!(
        sent.iter()
            .filter(|(id, class, _)| host_ids.contains(id) && *class == MessageClass::Transient)
            .count(),
        40
    );
}
