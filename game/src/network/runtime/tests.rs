use super::*;
use crate::{persistence::runtime::OriginalPuzzleImage, resources::*};
use bevy::state::app::StatesPlugin;
use puzzella_core::{PieceBitSet, PieceCommand, PieceId, GENERATOR_VERSION};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::Mutex,
};

#[derive(Default)]
struct Bus {
    inbox: BTreeMap<u64, VecDeque<TransportEvent>>,
    routes: BTreeMap<ConnectionId, (u64, ConnectionId)>,
    next: u64,
    drop_transient: bool,
    fail: BTreeSet<ConnectionId>,
    sent: Vec<(ConnectionId, MessageClass, Vec<u8>)>,
    closed: Vec<ConnectionId>,
}
struct Fake {
    id: u64,
    bus: Arc<Mutex<Bus>>,
}
impl Transport for Fake {
    fn poll(&mut self, events: &mut Vec<TransportEvent>) -> Result<(), TransportError> {
        events.extend(
            self.bus
                .lock()
                .unwrap()
                .inbox
                .entry(self.id)
                .or_default()
                .drain(..),
        );
        Ok(())
    }
    fn send(
        &mut self,
        connection: ConnectionId,
        class: MessageClass,
        payload: &[u8],
    ) -> Result<(), TransportError> {
        let mut bus = self.bus.lock().unwrap();
        if bus.fail.contains(&connection) {
            return Err(TransportError::Backend("injected send failure".into()));
        }
        let (id, peer) = *bus
            .routes
            .get(&connection)
            .ok_or(TransportError::NotConnected)?;
        bus.sent.push((connection, class, payload.to_vec()));
        if !(class == MessageClass::Transient && bus.drop_transient) {
            bus.inbox
                .entry(id)
                .or_default()
                .push_back(TransportEvent::Message {
                    connection: peer,
                    class,
                    payload: payload.to_vec(),
                });
        }
        Ok(())
    }
    fn close(
        &mut self,
        connection: ConnectionId,
        reason: DisconnectReason,
    ) -> Result<(), TransportError> {
        let mut bus = self.bus.lock().unwrap();
        bus.closed.push(connection);
        if let Some((id, peer)) = bus.routes.remove(&connection) {
            bus.routes.remove(&peer);
            bus.inbox
                .entry(id)
                .or_default()
                .push_back(TransportEvent::Disconnected {
                    connection: peer,
                    reason,
                });
        }
        Ok(())
    }
}
impl DirectIpTransport for Fake {
    fn listen(&mut self, _: SocketAddr) -> Result<ListenerId, TransportError> {
        Ok(ListenerId::new(1))
    }
    fn listener_address(&self, _: ListenerId) -> Result<SocketAddr, TransportError> {
        Ok("127.0.0.1:10000".parse().unwrap())
    }
    fn close_listener(&mut self, _: ListenerId) -> Result<(), TransportError> {
        Ok(())
    }
    fn connect(&mut self, _: SocketAddr) -> Result<ConnectionId, TransportError> {
        let mut bus = self.bus.lock().unwrap();
        let client = ConnectionId::new(bus.next);
        bus.next += 1;
        let host = ConnectionId::new(bus.next);
        bus.next += 1;
        bus.routes.insert(client, (0, host));
        bus.routes.insert(host, (self.id, client));
        bus.inbox
            .entry(self.id)
            .or_default()
            .push_back(TransportEvent::Connected { connection: client });
        bus.inbox
            .entry(0)
            .or_default()
            .push_back(TransportEvent::Connected { connection: host });
        Ok(client)
    }
}
fn password() -> SessionPassword {
    SessionPassword::new("runtime test password".into()).unwrap()
}
fn definition() -> PuzzleDefinition {
    PuzzleDefinition {
        generator_version: GENERATOR_VERSION,
        seed: 37,
        grid_size: UVec2::splat(2),
        image_size: UVec2::splat(8),
        snap_distance: 0.01,
    }
}
fn encoded() -> Arc<[u8]> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        8,
        8,
        image::Rgba([60, 170, 20, 255]),
    ))
    .write_to(&mut bytes, image::ImageFormat::Png)
    .unwrap();
    bytes.into_inner().into()
}
fn app() -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, StatesPlugin))
        .init_state::<AppState>()
        .add_sub_state::<GameSubState>()
        .add_message::<ClientCommand>()
        .init_resource::<PieceDataStore>()
        .init_resource::<PieceInteraction>()
        .init_resource::<LocalPlayerId>()
        .init_resource::<SessionHostId>()
        .init_resource::<PerformanceMonitor>()
        .init_resource::<InputState>()
        .init_resource::<Assets<Image>>()
        .add_plugins(NetworkRuntimePlugin)
        .add_systems(
            PostUpdate,
            crate::systems::apply_piece_commands.run_if(world_offline),
        );
    app.update();
    app
}
fn host_world(app: &mut App) -> SessionDefinition {
    let image = encoded();
    let session = SessionDefinition {
        id: SessionId(271),
        image_hash: crate::persistence::image_hash(&image),
    };
    app.world_mut().insert_resource(definition());
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .initialize(vec![
            Vec2::new(100.0, 100.0),
            Vec2::new(200.0, 100.0),
            Vec2::new(100.0, 200.0),
            Vec2::new(200.0, 200.0),
        ]);
    app.world_mut().insert_resource(OriginalPuzzleImage {
        hash: session.image_hash,
        encoded: Some(image),
    });
    session
}
fn send(app: &mut App, command: PieceCommand) {
    let player = app.world().resource::<LocalPlayerId>().0;
    app.world_mut()
        .resource_mut::<Messages<ClientCommand>>()
        .write(ClientCommand { player, command });
}
fn members() -> PieceBitSet {
    let mut mask = PieceBitSet::new(4);
    mask.insert(PieceId(0));
    mask.insert(PieceId(1));
    mask
}
struct Pair {
    host: App,
    client: App,
    bus: Arc<Mutex<Bus>>,
}
impl Pair {
    fn new() -> Self {
        let bus = Arc::new(Mutex::new(Bus::default()));
        let mut host = app();
        let mut client = app();
        let session = host_world(&mut host);
        host_with_transport(
            host.world_mut(),
            Fake {
                id: 0,
                bus: bus.clone(),
            },
            HostOptions {
                address: "127.0.0.1:0".parse().unwrap(),
                session,
                host: PlayerId(0),
                password: password(),
            },
        )
        .unwrap();
        join_with_transport(
            client.world_mut(),
            Fake {
                id: 1,
                bus: bus.clone(),
            },
            JoinOptions {
                address: "127.0.0.1:10000".parse().unwrap(),
                password: password(),
                cached_image: None,
            },
        )
        .unwrap();
        Self { host, client, bus }
    }
    fn frame(&mut self) {
        self.host.update();
        self.client.update();
    }
    fn ready(&mut self) {
        for _ in 0..2000 {
            self.frame();
            let status = self.client.world().resource::<NetworkStatus>();
            assert_ne!(status.phase, RuntimePhase::Failed, "{:?}", status.error);
            if status.phase == RuntimePhase::Ready
                && self.client.world().contains_resource::<PuzzleImage>()
            {
                return;
            }
            std::thread::yield_now();
        }
        panic!(
            "runtime join did not reach Ready: {:?}",
            self.client.world().resource::<NetworkStatus>()
        );
    }
    fn converge(&mut self) {
        for _ in 0..20 {
            self.frame();
        }
    }
}

#[test]
fn offline_commands_keep_the_direct_authority_path() {
    let mut app = app();
    host_world(&mut app);
    send(&mut app, PieceCommand::Grab(PieceId(0)));
    app.update();
    assert_eq!(
        app.world()
            .resource::<PieceDataStore>()
            .held_by
            .get(&PieceId(0)),
        Some(&PlayerId(0))
    );
    send(
        &mut app,
        PieceCommand::Move {
            id: PieceId(0),
            position: Vec2::splat(350.0),
        },
    );
    app.update();
    assert_eq!(
        app.world()
            .resource::<PieceDataStore>()
            .state(PieceId(0))
            .unwrap()
            .position,
        Vec2::splat(350.0)
    );
    assert!(!app.world().contains_non_send::<NetworkSession>());
}

#[test]
fn client_roundtrip_uses_assigned_identity_and_reliable_release_survives_transient_loss() {
    let mut pair = Pair::new();
    pair.ready();
    let player = pair.client.world().resource::<LocalPlayerId>().0;
    assert_ne!(player, PlayerId(0));
    assert_eq!(
        pair.client.world().resource::<SessionHostId>().0,
        PlayerId(0)
    );
    let before = pair
        .client
        .world()
        .resource::<PieceDataStore>()
        .states
        .clone();
    send(
        &mut pair.client,
        PieceCommand::GrabGroup { members: members() },
    );
    pair.client.update();
    assert_eq!(
        pair.client.world().resource::<PieceDataStore>().states,
        before
    );
    pair.converge();
    assert_eq!(
        pair.host
            .world()
            .resource::<PieceDataStore>()
            .held_by
            .get(&PieceId(0)),
        Some(&player)
    );
    pair.bus.lock().unwrap().drop_transient = true;
    let delta = Vec2::new(31.0, -13.0);
    pair.client
        .world_mut()
        .resource_mut::<PieceDataStore>()
        .drag
        .members = members().words().clone();
    pair.client
        .world_mut()
        .resource_mut::<PieceDataStore>()
        .drag
        .delta = delta;
    pair.converge();
    send(
        &mut pair.client,
        PieceCommand::ReleaseGroup {
            members: members(),
            delta,
        },
    );
    pair.converge();
    let host = pair.host.world().resource::<PieceDataStore>();
    let client = pair.client.world().resource::<PieceDataStore>();
    assert_eq!(host.states, client.states);
    assert_eq!(
        client.state(PieceId(0)).unwrap().position,
        Vec2::new(131.0, 87.0)
    );
    assert!(host.held_by.is_empty());
    assert!(client.held_by.is_empty());
}

#[test]
fn host_local_commands_are_retained_during_join_then_replicated() {
    let mut pair = Pair::new();
    send(
        &mut pair.host,
        PieceCommand::GrabGroup { members: members() },
    );
    pair.host.update();
    send(
        &mut pair.host,
        PieceCommand::RotateDrag {
            members: members(),
            delta: Vec2::splat(11.0),
            quarter_turns: 1,
        },
    );
    pair.host.update();
    send(
        &mut pair.host,
        PieceCommand::ReleaseGroup {
            members: members(),
            delta: Vec2::splat(19.0),
        },
    );
    pair.host.update();
    pair.ready();
    pair.converge();
    assert_eq!(
        pair.host.world().resource::<PieceDataStore>().states,
        pair.client.world().resource::<PieceDataStore>().states
    );
    assert_eq!(
        puzzella_core::decode_rotation(
            pair.client.world().resource::<PieceDataStore>().states[0].flags
        ),
        1
    );
}

#[test]
fn ready_disconnect_cancels_once_and_late_same_connection_message_cannot_apply() {
    let mut pair = Pair::new();
    pair.ready();
    send(
        &mut pair.client,
        PieceCommand::GrabGroup { members: members() },
    );
    pair.converge();
    let connection = pair.host.world().resource::<NetworkStatus>().peers[0].connection;
    let mut bus = pair.bus.lock().unwrap();
    let inbox = bus.inbox.entry(0).or_default();
    inbox.push_back(TransportEvent::Disconnected {
        connection,
        reason: DisconnectReason::RemoteClosed,
    });
    inbox.push_back(TransportEvent::Disconnected {
        connection,
        reason: DisconnectReason::RemoteClosed,
    });
    inbox.push_back(TransportEvent::Message {
        connection,
        class: MessageClass::Control,
        payload: vec![1, 2, 3],
    });
    drop(bus);
    pair.host.update();
    let store = pair.host.world().resource::<PieceDataStore>();
    assert!(store.held_by.is_empty());
    assert_eq!(
        store.state(PieceId(0)).unwrap().position,
        Vec2::splat(100.0)
    );
    assert!(pair
        .host
        .world()
        .resource::<NetworkStatus>()
        .peers
        .is_empty());
}

#[test]
fn publication_failure_does_not_reapply_host_control_and_session_teardown_resets_sender() {
    let mut pair = Pair::new();
    pair.ready();
    let connection = pair.host.world().resource::<NetworkStatus>().peers[0].connection;
    pair.bus.lock().unwrap().fail.insert(connection);
    send(
        &mut pair.host,
        PieceCommand::GrabGroup { members: members() },
    );
    pair.host.update();
    let z = pair.host.world().resource::<PieceDataStore>().next_z_order;
    pair.host.update();
    assert_eq!(
        pair.host.world().resource::<PieceDataStore>().next_z_order,
        z
    );
    assert!(pair
        .host
        .world()
        .resource::<NetworkStatus>()
        .peers
        .is_empty());
    stop_session(pair.host.world_mut());
    assert!(!pair.host.world().contains_non_send::<NetworkSession>());
    assert!(pair
        .host
        .world()
        .resource::<PieceDataStore>()
        .held_by
        .is_empty());
    assert_eq!(
        *pair.host.world().resource::<LocalPlayerId>(),
        LocalPlayerId::default()
    );
    let session = host_world(&mut pair.host);
    host_with_transport(
        pair.host.world_mut(),
        Fake {
            id: 0,
            bus: pair.bus.clone(),
        },
        HostOptions {
            address: "127.0.0.1:0".parse().unwrap(),
            session,
            host: PlayerId(39),
            password: password(),
        },
    )
    .unwrap();
    send(&mut pair.host, PieceCommand::Grab(PieceId(0)));
    pair.host.update();
    assert_eq!(
        pair.host
            .world()
            .resource::<PieceDataStore>()
            .held_by
            .get(&PieceId(0)),
        Some(&PlayerId(39))
    );
}

#[test]
fn image_identity_is_checked_before_host_listener_and_missing_bytes_remain_explicit() {
    let mut host = app();
    let session = host_world(&mut host);
    let bus = Arc::new(Mutex::new(Bus::default()));
    host.world_mut()
        .resource_mut::<OriginalPuzzleImage>()
        .encoded = Some(vec![7u8; 3].into());
    assert_eq!(
        host_with_transport(
            host.world_mut(),
            Fake {
                id: 0,
                bus: bus.clone()
            },
            HostOptions {
                address: "127.0.0.1:0".parse().unwrap(),
                session,
                host: PlayerId(9),
                password: password()
            }
        ),
        Err(RuntimeStartError::ImageHashMismatch)
    );
    host.world_mut()
        .resource_mut::<OriginalPuzzleImage>()
        .encoded = None;
    assert!(host_with_transport(
        host.world_mut(),
        Fake { id: 0, bus },
        HostOptions {
            address: "127.0.0.1:0".parse().unwrap(),
            session,
            host: PlayerId(9),
            password: password()
        }
    )
    .is_ok());
    assert_eq!(
        host.world().resource::<NetworkStatus>().image,
        ImageReadiness::Unavailable
    );
}

#[cfg(feature = "gns")]
#[test]
fn gns_localhost_runtime_entrypoints_join_ready_and_command_roundtrip() {
    let mut host = app();
    let mut client = app();
    let session = host_world(&mut host);
    let address = start_host(
        host.world_mut(),
        HostOptions {
            address: "127.0.0.1:0".parse().unwrap(),
            session,
            host: PlayerId(0),
            password: password(),
        },
    )
    .unwrap();
    start_join(
        client.world_mut(),
        JoinOptions {
            address,
            password: password(),
            cached_image: None,
        },
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        host.update();
        client.update();
        let status = client.world().resource::<NetworkStatus>();
        assert_ne!(status.phase, RuntimePhase::Failed, "{:?}", status.error);
        if status.phase == RuntimePhase::Ready && client.world().contains_resource::<PuzzleImage>()
        {
            break;
        }
        assert!(Instant::now() < deadline, "runtime GNS join: {status:?}");
        std::thread::sleep(Duration::from_millis(2));
    }
    let player = client.world().resource::<LocalPlayerId>().0;
    assert_ne!(player, PlayerId(0));
    send(&mut client, PieceCommand::GrabGroup { members: members() });
    send(
        &mut client,
        PieceCommand::ReleaseGroup {
            members: members(),
            delta: Vec2::new(14.0, 23.0),
        },
    );
    loop {
        host.update();
        client.update();
        if client
            .world()
            .resource::<PieceDataStore>()
            .state(PieceId(0))
            .unwrap()
            .position
            == Vec2::new(114.0, 123.0)
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "runtime GNS command roundtrip timed out"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        host.world().resource::<PieceDataStore>().states,
        client.world().resource::<PieceDataStore>().states
    );
    stop_session(client.world_mut());
    stop_session(host.world_mut());
}
