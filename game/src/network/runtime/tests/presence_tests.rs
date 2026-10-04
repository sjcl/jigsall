use super::*;
use crate::players::{PresenceMessage, RosterSnapshot};
use puzzella_core::PlayerDisplayName;

#[path = "cursor_tests.rs"]
mod cursor_tests;

struct Endpoint {
    runtime: DirectIpDriver<Fake>,
    store: PieceDataStore,
    gesture: PieceInteraction,
}
impl Endpoint {
    fn host(bus: &Arc<Mutex<Bus>>) -> Self {
        let bytes = encoded();
        let runtime = DirectIpDriver::host(
            Fake {
                id: 0,
                bus: bus.clone(),
            },
            HostOptions {
                display_name: Some(PlayerDisplayName::from_user_input("Host 🧩").unwrap()),
                address: "127.0.0.1:0".parse().unwrap(),
                session: SessionDefinition {
                    id: SessionId(271),
                    image_hash: crate::persistence::image_hash(&bytes),
                },
                host: PlayerId(0),
                password: password(),
            },
            definition(),
            Some(bytes),
            Instant::now(),
        )
        .unwrap();
        let mut store = PieceDataStore::default();
        store.initialize(vec![Vec2::splat(100.0); 4]);
        Self {
            runtime,
            store,
            gesture: default(),
        }
    }
    fn client(bus: &Arc<Mutex<Bus>>, id: u64) -> Self {
        Self {
            runtime: DirectIpDriver::client(
                Fake {
                    id,
                    bus: bus.clone(),
                },
                JoinOptions {
                    display_name: Some(PlayerDisplayName::from_user_input("Alice").unwrap()),
                    address: "127.0.0.1:10000".parse().unwrap(),
                    password: password(),
                    cached_image: Some(encoded()),
                },
                ImageDecodeLimits {
                    max_texture_dimension: 8192,
                },
            )
            .unwrap(),
            store: default(),
            gesture: default(),
        }
    }
    fn poll(&mut self) {
        self.runtime
            .poll(&mut self.store, &mut self.gesture, Instant::now())
            .unwrap();
    }
    fn roster(&self) -> RosterSnapshot {
        self.runtime.runtime.roster.snapshot()
    }
    fn connection(&self, player: PlayerId) -> ConnectionId {
        self.runtime
            .runtime
            .connections
            .peers()
            .find(|p| p.player == Some(player))
            .unwrap()
            .connection
    }
    fn ready(&self) -> bool {
        self.runtime.runtime.status.phase == RuntimePhase::Ready
    }
}
fn join(host: &mut Endpoint, peers: &mut [&mut Endpoint]) {
    for _ in 0..200 {
        host.poll();
        for peer in peers.iter_mut() {
            peer.poll();
        }
        if peers.iter().all(|p| p.ready()) {
            host.poll();
            return;
        }
    }
    panic!("injected runtime did not reach Ready");
}

#[test]
fn named_direct_ip_host_a_join_b_join_a_leave_converges_with_duplicate_names() {
    let bus = Arc::new(Mutex::new(Bus::default()));
    let mut host = Endpoint::host(&bus);
    let mut a = Endpoint::client(&bus, 1);
    join(&mut host, &mut [&mut a]);
    assert_eq!(host.roster(), a.roster());
    assert_eq!(host.roster().revision, 1);
    let mut b = Endpoint::client(&bus, 2);
    join(&mut host, &mut [&mut a, &mut b]);
    a.poll();
    assert_eq!(host.roster(), a.roster());
    assert_eq!(host.roster(), b.roster());
    assert_eq!(host.roster().revision, 2);
    let player = a.runtime.runtime.status.local_player.unwrap();
    a.runtime.teardown(&mut a.store);
    host.poll();
    b.poll();
    assert_eq!(host.roster(), b.roster());
    assert_eq!(host.roster().revision, 3);
    assert!(host.runtime.runtime.roster.get(player).is_none());
    assert_eq!(
        b.runtime
            .runtime
            .roster
            .players()
            .map(|p| p.display_name.as_ref().unwrap().as_ref())
            .collect::<Vec<_>>(),
        vec!["Host 🧩", "Alice"]
    );
    assert!(a.runtime.runtime.roster.is_empty());
}

#[test]
fn active_drag_disconnect_publishes_cancel_before_left_and_duplicate_is_inert() {
    let bus = Arc::new(Mutex::new(Bus::default()));
    let mut host = Endpoint::host(&bus);
    let mut a = Endpoint::client(&bus, 1);
    let mut b = Endpoint::client(&bus, 2);
    join(&mut host, &mut [&mut a, &mut b]);
    a.poll();
    b.poll();
    let player = a.runtime.runtime.status.local_player.unwrap();
    let id = host.connection(player);
    let Role::Host(state) = &mut host.runtime.runtime.role else {
        unreachable!()
    };
    let session = host.runtime.runtime.session.as_mut().unwrap();
    let applied = state
        .contexts
        .apply_replicated(
            session,
            &mut host.store,
            player,
            &ProtocolCommandEnvelope {
                session: session.session_definition().id,
                authority_epoch: session.cursor().epoch,
                player,
                sequence: ClientCommandSequence::Control(0),
                command: ProtocolPieceCommand::Grab {
                    target: PieceTarget::Component(ComponentRef {
                        member: PieceId(0),
                        expected_size: 1,
                    }),
                },
            },
            host.runtime.runtime.definition.as_ref(),
            PlayerId(0),
        )
        .unwrap();
    state
        .sync
        .record_command_outcome(session, &host.store, &applied)
        .unwrap();
    host.runtime
        .publish(Some(id), &applied, &mut host.store)
        .unwrap();
    b.poll();
    host.runtime
        .disconnect(id, DisconnectReason::RemoteClosed, None, &mut host.store)
        .unwrap();
    let mut events = Vec::new();
    b.runtime.runtime.transport.poll(&mut events).unwrap();
    let messages: Vec<_> = events
        .iter()
        .filter_map(|e| {
            if let TransportEvent::Message { class, payload, .. } = e {
                Some(wire::decode_for_class(payload, *class).unwrap())
            } else {
                None
            }
        })
        .collect();
    assert!(
        matches!(&messages[..], [WireMessage::AuthorityEvent(e), WireMessage::Presence(PresenceMessage::PlayerLeft {revision: 3, player: p})] if matches!(e.event, ProtocolAuthorityEvent::DragCancelled(_)) && *p == player)
    );
    let Role::Client(state) = &mut b.runtime.runtime.role else {
        unreachable!()
    };
    for event in events {
        ClientRouter {
            roster: &mut b.runtime.runtime.roster,
            local_player: b.runtime.runtime.status.local_player.unwrap(),
            host_connection: state.bootstrap.host_connection(),
            connections: &b.runtime.runtime.connections,
            replica: &mut state.replica,
            session: b.runtime.runtime.session.as_mut().unwrap(),
            store: &mut b.store,
            definition: b.runtime.runtime.definition.as_ref(),
        }
        .route(&event)
        .unwrap();
    }
    assert_eq!(host.roster(), b.roster());
    host.runtime
        .disconnect(
            id,
            DisconnectReason::RemoteClosed,
            Some(player),
            &mut host.store,
        )
        .unwrap();
    assert_eq!(host.runtime.runtime.roster.revision(), 3);
    events = Vec::new();
    b.runtime.runtime.transport.poll(&mut events).unwrap();
    assert!(events.is_empty());
}

#[test]
fn presence_failure_cascade_reaches_every_surviving_ready_peer() {
    let bus = Arc::new(Mutex::new(Bus::default()));
    let mut host = Endpoint::host(&bus);
    let mut a = Endpoint::client(&bus, 1);
    let mut b = Endpoint::client(&bus, 2);
    let mut c = Endpoint::client(&bus, 3);
    join(&mut host, &mut [&mut a, &mut b, &mut c]);
    a.poll();
    b.poll();
    c.poll();
    let aid = host.connection(a.runtime.runtime.status.local_player.unwrap());
    let bid = host.connection(b.runtime.runtime.status.local_player.unwrap());
    bus.lock().unwrap().fail.insert(bid);
    host.runtime
        .disconnect(aid, DisconnectReason::RemoteClosed, None, &mut host.store)
        .unwrap();
    c.poll();
    assert_eq!(host.runtime.runtime.roster.len(), 2);
    assert_eq!(host.runtime.runtime.roster.revision(), 5);
    assert_eq!(host.roster(), c.roster());
}

#[test]
fn syncing_disconnect_has_no_presence_or_revision_change() {
    let mut pair = Pair::new();
    let id = reach_baseline(&mut pair);
    assert_eq!(pair.host.world().resource::<PlayerRoster>().len(), 1);
    pair.bus
        .lock()
        .unwrap()
        .inbox
        .entry(0)
        .or_default()
        .push_back(TransportEvent::Disconnected {
            connection: id,
            reason: DisconnectReason::RemoteClosed,
        });
    pair.host.update();
    assert_eq!(pair.host.world().resource::<PlayerRoster>().revision(), 0);
    assert_eq!(pair.host.world().resource::<PlayerRoster>().len(), 1);
}

#[test]
fn ready_commit_then_immediate_leave_and_join_converge_in_control_order() {
    let bus = Arc::new(Mutex::new(Bus::default()));
    let mut host = Endpoint::host(&bus);
    let mut a = Endpoint::client(&bus, 1);
    join(&mut host, &mut [&mut a]);
    let mut b = Endpoint::client(&bus, 2);
    for _ in 0..200 {
        host.poll();
        if host.runtime.runtime.roster.len() == 3 {
            break;
        }
        a.poll();
        b.poll();
    }
    assert_eq!(host.runtime.runtime.roster.len(), 3);
    assert!(!b.ready()); // ReadyCommit revision 2 is queued, not yet consumed.
    let aid = host.connection(a.runtime.runtime.status.local_player.unwrap());
    host.runtime
        .disconnect(aid, DisconnectReason::RemoteClosed, None, &mut host.store)
        .unwrap();
    let mut c = Endpoint::client(&bus, 3);
    join(&mut host, &mut [&mut c]);
    // B now receives ReadyCommit(2), PlayerLeft(3), PlayerJoined(4) in one poll.
    b.poll();
    assert!(b.ready());
    assert_eq!(host.runtime.runtime.roster.revision(), 4);
    assert_eq!(host.roster(), b.roster());
    assert_eq!(host.roster(), c.roster());
}

#[test]
fn world_roster_is_the_session_resource_and_teardown_clears_it() {
    let mut pair = Pair::new();
    pair.ready();
    let mut b = second_client(&mut pair);
    pair.converge();
    b.update();
    assert_eq!(
        pair.host.world().resource::<PlayerRoster>(),
        pair.client.world().resource::<PlayerRoster>()
    );
    assert_eq!(
        pair.host.world().resource::<PlayerRoster>(),
        b.world().resource::<PlayerRoster>()
    );
    assert_eq!(pair.host.world().resource::<PlayerRoster>().len(), 3);
    stop_session(pair.client.world_mut());
    pair.host.update();
    b.update();
    assert!(pair.client.world().resource::<PlayerRoster>().is_empty());
    assert_eq!(
        pair.host.world().resource::<PlayerRoster>(),
        b.world().resource::<PlayerRoster>()
    );
    stop_session(pair.host.world_mut());
    assert!(pair.host.world().resource::<PlayerRoster>().is_empty());
}
