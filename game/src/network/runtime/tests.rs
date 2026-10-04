use super::*;
mod pending_release_tests;
mod presence_tests;
mod presentation_tests;
use crate::{persistence::runtime::OriginalPuzzleImage, resources::*};
use bevy::state::app::StatesPlugin;
use puzzella_core::{PieceBitSet, PieceCommand, PieceId, GENERATOR_VERSION};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::Mutex,
};

#[derive(Default)]
struct Bus {
    bulk_sent: BTreeMap<ConnectionId, u64>,
    inbox: BTreeMap<u64, VecDeque<TransportEvent>>,
    routes: BTreeMap<ConnectionId, (u64, ConnectionId)>,
    next: u64,
    drop_transient: bool,
    hold_authority_control: bool,
    delayed: Vec<(u64, TransportEvent)>,
    fail: BTreeSet<ConnectionId>,
    sent: Vec<(ConnectionId, MessageClass, Vec<u8>)>,
    closed: Vec<ConnectionId>,
}
struct Fake {
    id: u64,
    bus: Arc<Mutex<Bus>>,
}

#[test]
fn runtime_session_nonresponse_expires_and_cleans_all_layers_with_explicit_time() {
    let now = Instant::now();
    let bus = Arc::new(Mutex::new(Bus::default()));
    let bytes = encoded();
    let session = SessionDefinition {
        id: SessionId(271),
        image_hash: crate::persistence::image_hash(&bytes),
    };
    let mut host = Runtime::host(
        Fake {
            id: 0,
            bus: bus.clone(),
        },
        HostOptions {
            display_name: None,
            address: "127.0.0.1:0".parse().unwrap(),
            session,
            host: PlayerId(0),
            password: password(),
        },
        definition(),
        Some(bytes),
        now,
    )
    .unwrap();
    let mut client = Runtime::client(
        Fake {
            id: 1,
            bus: bus.clone(),
        },
        JoinOptions {
            display_name: None,
            address: "127.0.0.1:10000".parse().unwrap(),
            password: password(),
            cached_image: None,
        },
        ImageDecodeLimits {
            max_texture_dimension: 8192,
        },
    )
    .unwrap();
    let mut store = PieceDataStore::default();
    store.initialize(vec![Vec2::splat(100.0); 4]);
    let mut client_store = PieceDataStore::default();
    let mut interaction = PieceInteraction::default();
    let mut client_interaction = PieceInteraction::default();
    let id = loop {
        host.poll(&mut store, &mut interaction, now).unwrap();
        if let Role::Host(h) = &host.role {
            if let Some(&id) = h.joining.first() {
                assert_eq!(h.bootstrap.state(id), Some(ConnectionState::Syncing));
                break id;
            }
        }
        client
            .poll(&mut client_store, &mut client_interaction, now)
            .unwrap();
    };
    assert_eq!(host.connections.player(id), None);
    host.poll(&mut store, &mut interaction, now + Duration::from_secs(12))
        .unwrap();
    let Role::Host(h) = &host.role else {
        unreachable!()
    };
    assert!(h.joining.is_empty());
    assert_eq!(h.bootstrap.state(id), None);
    assert_eq!(h.sync.resources().peers, 0);
    assert!(host.live.is_empty());
    assert!(!host.transport.has_channel(id));
    assert_eq!(host.connections.peers().count(), 0);
    assert_eq!(
        host.session.as_ref().unwrap().cursor(),
        AuthorityCursor::new(0, 0)
    );
    assert!(host.active);
    assert!(host
        .status
        .error
        .as_ref()
        .unwrap()
        .contains("ImageAvailability"));
    assert!(bus.lock().unwrap().routes.is_empty());
}
impl Transport for Fake {
    fn reliable_egress(&self, connection: ConnectionId) -> Result<ReliableEgress, TransportError> {
        Ok(ReliableEgress {
            bulk_delivered_bytes: self
                .bus
                .lock()
                .unwrap()
                .bulk_sent
                .get(&connection)
                .copied()
                .unwrap_or(0),
            ..Default::default()
        })
    }
    fn activate_secure_channel(&mut self, connection: ConnectionId) -> Result<(), TransportError> {
        if self.bus.lock().unwrap().routes.contains_key(&connection) {
            Ok(())
        } else {
            Err(TransportError::NotConnected)
        }
    }
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
        if class == MessageClass::Bulk {
            *bus.bulk_sent.entry(connection).or_default() += payload.len() as u64;
        }
        if !(class == MessageClass::Transient && bus.drop_transient) {
            let event = TransportEvent::Message {
                connection: peer,
                class,
                payload: payload.to_vec(),
            };
            if self.id == 0 && class == MessageClass::Control && bus.hold_authority_control {
                bus.delayed.push((id, event));
            } else {
                bus.inbox.entry(id).or_default().push_back(event);
            }
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
        image_size: UVec2::splat(128),
        snap_distance: 0.01,
    }
}
fn encoded() -> Arc<[u8]> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        128,
        128,
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
        .init_resource::<remote_drag::RemoteDragUpload>()
        .add_systems(Last, remote_drag::prepare_remote_drag_upload)
        .init_resource::<PieceInteraction>()
        .init_resource::<LocalPlayerId>()
        .init_resource::<SessionHostId>()
        .init_resource::<PerformanceMonitor>()
        .init_resource::<InputState>()
        .init_resource::<crate::selection::PuzzleSelection>()
        .init_resource::<Assets<Image>>()
        .insert_resource(PuzzleImageLimits {
            device_max_dimension: 8192,
            gpu_memory_bytes: Some(8 * 1024 * 1024 * 1024),
        })
        .insert_resource(crate::image_settings::ImageSettingsState::load(None))
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
        image_lease: None,
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
fn cursor(app: &App) -> AuthorityCursor {
    app.world()
        .get_non_send::<NetworkSession>()
        .unwrap()
        .authority()
        .unwrap()
        .cursor()
}
fn pointer(app: &mut App, position: Vec2, pressed: bool, just_pressed: bool) {
    let player = app.world().resource::<LocalPlayerId>().0;
    app.world_mut().resource_mut::<InputState>().mouse_position = Some(position);
    let mut interaction = app
        .world_mut()
        .remove_resource::<PieceInteraction>()
        .unwrap();
    interaction.set_play_area(app.world().get_resource::<PuzzleDefinition>());
    let mut store = app.world_mut().remove_resource::<PieceDataStore>().unwrap();
    let mut selection = app
        .world_mut()
        .remove_resource::<crate::selection::PuzzleSelection>()
        .unwrap();
    let commands = interaction.update(
        crate::interaction::PointerFrame {
            position: Some(position),
            screen_position: Some(position),
            pressed,
            just_pressed,
            ctrl: false,
            over_ui: false,
            focused: true,
        },
        &mut store,
        &mut selection,
        player,
    );
    app.world_mut().insert_resource(interaction);
    app.world_mut().insert_resource(store);
    app.world_mut().insert_resource(selection);
    for command in commands {
        send(app, command);
    }
}
fn begin_gesture(app: &mut App) {
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .selected_pieces = members();
    pointer(app, Vec2::ZERO, true, true);
    let request = app
        .world()
        .resource::<crate::selection::PuzzleSelection>()
        .latest
        .unwrap();
    app.world_mut()
        .resource_mut::<crate::selection::PuzzleSelection>()
        .completed = Some(crate::selection::SelectionResult {
        request_id: request.request_id,
        mode: crate::selection::SelectionMode::Point,
        payload: crate::selection::SelectionPayload::Point(Some(PieceId(0))),
        error: None,
    });
    pointer(app, Vec2::ZERO, true, false);
}
struct Pair {
    host: App,
    client: App,
    bus: Arc<Mutex<Bus>>,
}
impl Pair {
    fn new() -> Self {
        Self::with_image_options(8192, None)
    }
    fn with_image_options(cap: u32, cached_image: Option<Arc<[u8]>>) -> Self {
        Self::with_image_bytes(cap, cached_image, encoded())
    }
    fn with_image_bytes(cap: u32, cached_image: Option<Arc<[u8]>>, bytes: Arc<[u8]>) -> Self {
        let bus = Arc::new(Mutex::new(Bus::default()));
        let mut host = app();
        let mut client = app();
        client
            .world_mut()
            .resource_mut::<PuzzleImageLimits>()
            .device_max_dimension = cap;
        let mut session = host_world(&mut host);
        session.image_hash = crate::persistence::image_hash(&bytes);
        host.world_mut().insert_resource(OriginalPuzzleImage {
            hash: session.image_hash,
            encoded: Some(bytes),
            image_lease: None,
        });
        host_with_transport(
            host.world_mut(),
            Fake {
                id: 0,
                bus: bus.clone(),
            },
            HostOptions {
                display_name: None,
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
                display_name: None,
                address: "127.0.0.1:10000".parse().unwrap(),
                password: password(),
                cached_image,
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
fn joined_image_rejects_oversized_sources_from_transfer_and_cache() {
    let bytes: Arc<[u8]> = crate::asset_reader::tests::image_with_claimed_dimensions(
        image::ImageFormat::Gif,
        24000,
        16000,
    )
    .into();
    for cached in [false, true] {
        let mut pair = Pair::with_image_bytes(128, cached.then(|| bytes.clone()), bytes.clone());
        let deadline = Instant::now() + std::time::Duration::from_secs(10);
        loop {
            pair.frame();
            let status = pair.client.world().resource::<NetworkStatus>();
            if status.phase == RuntimePhase::Failed {
                assert!(
                    status
                        .error
                        .as_ref()
                        .unwrap()
                        .contains("Image size exceeds limit"),
                    "{:?}",
                    status.error
                );
                break;
            }
            assert!(
                Instant::now() < deadline,
                "oversized image was not rejected"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(!pair.client.world().contains_resource::<PuzzleImage>());
        assert!(!pair
            .client
            .world()
            .contains_resource::<OriginalPuzzleImage>());
    }
}

#[test]
fn joined_image_uses_client_limit_and_preserves_logical_dimensions_and_original_bytes() {
    for cached in [false, true] {
        let mut pair = Pair::with_image_options(2, cached.then(encoded));
        // Like file selection and save restore, a join captures its decode cap.
        pair.client
            .world_mut()
            .resource_mut::<PuzzleImageLimits>()
            .device_max_dimension = 8192;
        pair.ready();
        let world = pair.client.world();
        let image = world.resource::<PuzzleImage>();
        assert_eq!(image.logical_size, definition().image_size);
        assert_eq!(image.texture_size, UVec2::splat(2));
        assert_eq!(
            world
                .resource::<Assets<Image>>()
                .get(&image.handle)
                .unwrap()
                .size(),
            image.texture_size
        );
        assert_eq!(world.resource::<PuzzleDefinition>(), &definition());
        let original = world.resource::<OriginalPuzzleImage>();
        assert_eq!(original.hash, crate::persistence::image_hash(&encoded()));
        assert_eq!(original.encoded.as_deref().unwrap(), encoded().as_ref());
    }
}

#[test]
fn hosting_uses_logical_dimensions_when_local_texture_is_smaller() {
    let mut host = app();
    let session = host_world(&mut host);
    let decoded = crate::asset_reader::decode_image_bytes(
        &encoded(),
        ImageDecodeLimits {
            max_texture_dimension: 2,
        },
    )
    .unwrap();
    let texture_size = decoded.image.size();
    let handle = host
        .world_mut()
        .resource_mut::<Assets<Image>>()
        .add(decoded.image);
    host.world_mut().insert_resource(PuzzleImage {
        handle,
        logical_size: decoded.logical_size,
        texture_size,
        opaque: true,
    });
    host_with_transport(
        host.world_mut(),
        Fake {
            id: 0,
            bus: Arc::new(Mutex::new(Bus::default())),
        },
        HostOptions {
            display_name: None,
            address: "127.0.0.1:0".parse().unwrap(),
            session,
            host: PlayerId(0),
            password: password(),
        },
    )
    .unwrap();
    assert_eq!(
        host.world().resource::<NetworkStatus>().phase,
        RuntimePhase::Hosting
    );
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
    let connection = reach_baseline(&mut pair);
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
    let state = pair
        .host
        .world()
        .get_non_send::<NetworkSession>()
        .unwrap()
        .host_state()
        .unwrap();
    let player = state.bootstrap.assigned_player(connection).unwrap();
    let catch_up = state.sync.catch_up().status(player).unwrap();
    assert_eq!(catch_up.retained_events, 3);
    assert_eq!(catch_up.last_recorded_cursor, cursor(&pair.host));
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
fn reach_baseline(pair: &mut Pair) -> ConnectionId {
    for _ in 0..100 {
        pair.frame();
        let state = pair
            .host
            .world()
            .get_non_send::<NetworkSession>()
            .unwrap()
            .host_state()
            .unwrap();
        if let Some(&id) = state.joining.first() {
            if matches!(state.sync.phase(id), Some(SyncPhase::BaselineTransfer)) {
                return id;
            }
        }
    }
    panic!("baseline transfer was not started");
}

fn second_client(pair: &mut Pair) -> App {
    let mut client = app();
    join_with_transport(
        client.world_mut(),
        Fake {
            id: 2,
            bus: pair.bus.clone(),
        },
        JoinOptions {
            display_name: None,
            address: "127.0.0.1:10000".parse().unwrap(),
            password: password(),
            cached_image: None,
        },
    )
    .unwrap();
    for _ in 0..2000 {
        pair.frame();
        client.update();
        let status = client.world().resource::<NetworkStatus>();
        assert_ne!(status.phase, RuntimePhase::Failed, "{:?}", status.error);
        if status.phase == RuntimePhase::Ready {
            return client;
        }
        std::thread::yield_now();
    }
    panic!("second client did not reach Ready");
}

#[test]
fn ready_disconnect_publishes_one_cancellation_to_remaining_ready_replica() {
    let mut pair = Pair::new();
    pair.ready();
    send(
        &mut pair.client,
        PieceCommand::GrabGroup { members: members() },
    );
    pair.converge();
    let mut other = second_client(&mut pair);
    let player = pair.client.world().resource::<LocalPlayerId>().0;
    let connection = pair
        .host
        .world()
        .resource::<NetworkStatus>()
        .peers
        .iter()
        .find(|peer| peer.player == Some(player))
        .unwrap()
        .connection;
    let before = cursor(&pair.host);
    pair.bus
        .lock()
        .unwrap()
        .inbox
        .entry(0)
        .or_default()
        .extend([
            TransportEvent::Disconnected {
                connection,
                reason: DisconnectReason::RemoteClosed,
            },
            TransportEvent::Disconnected {
                connection,
                reason: DisconnectReason::RemoteClosed,
            },
        ]);
    for _ in 0..20 {
        pair.host.update();
        other.update();
    }
    assert_eq!(cursor(&pair.host).sequence.0, before.sequence.0 + 1);
    assert_eq!(cursor(&other), cursor(&pair.host));
    assert!(other
        .world()
        .resource::<PieceDataStore>()
        .held_by
        .is_empty());
    assert_eq!(
        pair.host.world().resource::<PieceDataStore>().states,
        other.world().resource::<PieceDataStore>().states
    );
}

#[test]
fn failed_publication_still_reaches_other_ready_peers_and_advances_authority_once() {
    let mut pair = Pair::new();
    pair.ready();
    let mut other = second_client(&mut pair);
    let player = pair.client.world().resource::<LocalPlayerId>().0;
    let connection = pair
        .host
        .world()
        .resource::<NetworkStatus>()
        .peers
        .iter()
        .find(|peer| peer.player == Some(player))
        .unwrap()
        .connection;
    pair.bus.lock().unwrap().fail.insert(connection);
    let before = cursor(&pair.host);
    send(
        &mut pair.host,
        PieceCommand::GrabGroup { members: members() },
    );
    for _ in 0..20 {
        pair.host.update();
        other.update();
    }
    assert_eq!(cursor(&pair.host).sequence.0, before.sequence.0 + 1);
    assert_eq!(cursor(&other), cursor(&pair.host));
    assert_eq!(
        pair.host.world().resource::<PieceDataStore>().states,
        other.world().resource::<PieceDataStore>().states
    );
    assert_eq!(pair.host.world().resource::<NetworkStatus>().peers.len(), 1);
}

#[test]
fn failure_before_connected_blocks_offline_commands_then_menu_releases_session() {
    let bus = Arc::new(Mutex::new(Bus::default()));
    let mut client = app();
    join_with_transport(
        client.world_mut(),
        Fake {
            id: 1,
            bus: bus.clone(),
        },
        JoinOptions {
            display_name: None,
            address: "127.0.0.1:10000".parse().unwrap(),
            password: password(),
            cached_image: None,
        },
    )
    .unwrap();
    let mut bus = bus.lock().unwrap();
    bus.inbox.insert(
        1,
        VecDeque::from([TransportEvent::ConnectionFailed {
            connection: ConnectionId::new(0),
            reason: DisconnectReason::ConnectionProblem,
        }]),
    );
    drop(bus);
    send(&mut client, PieceCommand::Grab(PieceId(0)));
    client.update();
    assert_eq!(
        client.world().resource::<NetworkStatus>().phase,
        RuntimePhase::Disconnected
    );
    assert!(!client.world().contains_non_send::<NetworkSession>());
    assert!(client
        .world()
        .resource::<PieceDataStore>()
        .held_by
        .is_empty());
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
    let before = cursor(&pair.host);
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
    assert_eq!(cursor(&pair.host).sequence.0, before.sequence.0 + 1);
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
fn partial_grab_reconciles_the_existing_gesture_and_rejected_members_stop_previewing() {
    let mut pair = Pair::new();
    pair.ready();
    begin_gesture(&mut pair.client);
    // The authority accepts a competing hold after the client's pick.
    send(&mut pair.host, PieceCommand::Grab(PieceId(1)));
    pair.host.update();
    pair.converge();
    let mut accepted = PieceBitSet::new(4);
    accepted.insert(PieceId(0));
    assert_eq!(
        pair.client
            .world()
            .resource::<PieceDataStore>()
            .drag
            .members,
        *accepted.words()
    );
    assert!(pair
        .client
        .world()
        .resource::<PieceInteraction>()
        .is_dragging());
    pointer(&mut pair.client, Vec2::new(30.0, 20.0), false, false);
    pair.converge();
    let store = pair.client.world().resource::<PieceDataStore>();
    assert_eq!(
        store.state(PieceId(0)).unwrap().position,
        Vec2::new(130.0, 120.0)
    );
    assert_eq!(
        store.state(PieceId(1)).unwrap().position,
        Vec2::new(200.0, 100.0)
    );
}

#[test]
fn client_rotation_ack_rebases_pointer_and_release_queued_before_ack_uses_the_new_basis() {
    let mut pair = Pair::new();
    pair.ready();
    begin_gesture(&mut pair.client);
    pair.converge();
    pointer(&mut pair.client, Vec2::new(20.0, 30.0), true, false);
    pair.client.update();
    let rotation = pair
        .client
        .world()
        .resource::<PieceInteraction>()
        .rotation_command(pair.client.world().resource::<PieceDataStore>(), 1)
        .unwrap();
    send(&mut pair.client, rotation);
    pair.client.update();
    // No authority response yet: pointer delta is still in the original basis.
    pointer(&mut pair.client, Vec2::new(30.0, 40.0), true, false);
    assert_eq!(
        pair.client.world().resource::<PieceDataStore>().drag.delta,
        Vec2::new(30.0, 40.0)
    );
    pair.host.update();
    pair.client.update();
    assert_eq!(
        pair.client.world().resource::<PieceDataStore>().drag.delta,
        Vec2::splat(10.0)
    );
    pointer(&mut pair.client, Vec2::new(30.0, 40.0), true, false);
    assert_eq!(
        pair.client.world().resource::<PieceDataStore>().drag.delta,
        Vec2::splat(10.0)
    );
    pair.converge();
    let session = pair
        .client
        .world()
        .get_non_send::<NetworkSession>()
        .unwrap();
    let drag = session
        .replica()
        .unwrap()
        .remote_drag(
            session.authority().unwrap(),
            pair.client.world().resource::<PieceDataStore>(),
            pair.client.world().resource::<LocalPlayerId>().0,
        )
        .unwrap();
    assert_eq!(drag.basis_sequence, 1);
    // Second rotation, with a release sampled before the rebase result returns.
    let rotation = pair
        .client
        .world()
        .resource::<PieceInteraction>()
        .rotation_command(pair.client.world().resource::<PieceDataStore>(), 1)
        .unwrap();
    send(&mut pair.client, rotation);
    pair.client.update();
    pointer(&mut pair.client, Vec2::new(35.0, 47.0), false, false);
    pair.client.update();
    pair.converge();
    assert_eq!(
        pair.host.world().resource::<PieceDataStore>().states,
        pair.client.world().resource::<PieceDataStore>().states
    );
    assert_eq!(
        pair.client.world().resource::<PieceDataStore>().states[0].position,
        Vec2::new(135.0, 147.0)
    );
    assert!(pair
        .client
        .world()
        .resource::<PieceDataStore>()
        .held_by
        .is_empty());
}

#[test]
fn old_rotation_ack_uses_its_members_bounds_and_new_gesture_waits_for_release() {
    let mut pair = Pair::new();
    let area = puzzella_puzzle::placement::LogicalPlayArea::from_definition(&definition()).unwrap();
    pair.host
        .world_mut()
        .resource_mut::<PieceDataStore>()
        .states[2]
        .position
        .x = area.half_extents.x as f32 - 1.0;
    pair.ready();
    begin_gesture(&mut pair.client);
    pair.converge();
    pointer(&mut pair.client, Vec2::new(20.0, 30.0), true, false);
    pair.client.update();
    let rotation = pair
        .client
        .world()
        .resource::<PieceInteraction>()
        .rotation_command(pair.client.world().resource::<PieceDataStore>(), 1)
        .unwrap();
    send(&mut pair.client, rotation);
    pair.client.update();
    pointer(&mut pair.client, Vec2::new(30.0, 40.0), false, false);
    pair.client.update();
    // The old Release waits behind RotateDrag. A new press must preserve it.
    pointer(&mut pair.client, Vec2::ZERO, true, true);
    assert!(pair
        .client
        .world()
        .resource::<crate::selection::PuzzleSelection>()
        .latest
        .is_none());
    assert_eq!(
        pair.client.world().resource::<PieceDataStore>().drag.delta,
        Vec2::new(30.0, 40.0)
    );
    pair.converge();
    // Once the old Release commits, a near-edge component can start a new drag.
    pointer(&mut pair.client, Vec2::ZERO, true, true);
    let request = pair
        .client
        .world()
        .resource::<crate::selection::PuzzleSelection>()
        .latest
        .unwrap();
    pair.client
        .world_mut()
        .resource_mut::<crate::selection::PuzzleSelection>()
        .completed = Some(crate::selection::SelectionResult {
        request_id: request.request_id,
        mode: crate::selection::SelectionMode::Point,
        payload: crate::selection::SelectionPayload::Point(Some(PieceId(2))),
        error: None,
    });
    pointer(&mut pair.client, Vec2::ZERO, true, false);
    pair.client.update();
    pair.converge();
    assert_eq!(
        pair.host.world().resource::<PieceDataStore>().states[0].position,
        Vec2::new(130.0, 140.0)
    );
    assert_eq!(
        pair.host.world().resource::<PieceDataStore>().states[1].position,
        Vec2::new(230.0, 140.0)
    );
    assert_eq!(
        pair.host.world().resource::<PieceDataStore>().states,
        pair.client.world().resource::<PieceDataStore>().states
    );
    assert_eq!(
        pair.client.world().resource::<PieceDataStore>().drag.delta,
        Vec2::ZERO
    );
    assert!(pair
        .client
        .world()
        .resource::<PieceInteraction>()
        .is_dragging());
    pointer(&mut pair.client, Vec2::ZERO, false, false);
    pair.converge();
    assert!(pair
        .host
        .world()
        .resource::<PieceDataStore>()
        .held_by
        .is_empty());
}

#[test]
fn syncing_disconnect_releases_transfer_and_catch_up_without_gameplay_cancellation() {
    let mut pair = Pair::new();
    let mut joining = None;
    for _ in 0..100 {
        pair.frame();
        let state = pair
            .host
            .world()
            .get_non_send::<NetworkSession>()
            .unwrap()
            .host_state()
            .unwrap();
        if let Some(&id) = state.joining.first() {
            if matches!(
                state.sync.phase(id),
                Some(SyncPhase::BaselineTransfer | SyncPhase::CatchingUp)
            ) {
                joining = Some(id);
                break;
            }
        }
    }
    let connection = joining.expect("baseline sync reached");
    let before = cursor(&pair.host);
    pair.bus
        .lock()
        .unwrap()
        .inbox
        .entry(0)
        .or_default()
        .push_back(TransportEvent::Disconnected {
            connection,
            reason: DisconnectReason::RemoteClosed,
        });
    pair.host.update();
    assert_eq!(cursor(&pair.host), before);
    let state = pair
        .host
        .world()
        .get_non_send::<NetworkSession>()
        .unwrap()
        .host_state()
        .unwrap();
    assert!(state.joining.is_empty());
    assert_eq!(state.sync.phase(connection), None);
    assert_eq!(state.sync.transfer_binding(connection), None);
    assert_eq!(state.sync.active_baseline_transfers(), 0);
}

#[test]
fn bounded_catch_up_overflow_restarts_through_the_runtime_and_reaches_ready() {
    let mut pair = Pair::new();
    let connection = reach_baseline(&mut pair);
    let target = PieceTarget::Component(
        ComponentRef::from_member(
            &pair.host.world().resource::<PieceDataStore>().connectivity,
            PieceId(0),
        )
        .unwrap(),
    );
    // Freeze the joining client past the retention bound while local host
    // controls continue. The runtime must schedule the foundation's restart.
    for _ in 0..=crate::multiplayer::catch_up::MAX_CATCH_UP_EVENTS {
        send(
            &mut pair.host,
            PieceCommand::Rotate {
                target: target.clone(),
                quarter_turns: 1,
            },
        );
        pair.host.update();
    }
    pair.host.update();
    let host = pair
        .host
        .world()
        .get_non_send::<NetworkSession>()
        .unwrap()
        .host_state()
        .unwrap();
    let player = host.bootstrap.assigned_player(connection).unwrap();
    assert_eq!(host.sync.catch_up().status(player).unwrap().generation, 1);
    pair.ready();
    pair.converge();
    assert_eq!(
        pair.host.world().resource::<PieceDataStore>().states,
        pair.client.world().resource::<PieceDataStore>().states
    );
    assert_eq!(cursor(&pair.host), cursor(&pair.client));
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
    let session = SessionDefinition {
        id: SessionId(271),
        image_hash: pair.host.world().resource::<OriginalPuzzleImage>().hash,
    };
    assert_eq!(
        host_with_transport(
            pair.host.world_mut(),
            Fake {
                id: 0,
                bus: pair.bus.clone()
            },
            HostOptions {
                display_name: None,
                address: "127.0.0.1:0".parse().unwrap(),
                session,
                host: PlayerId(39),
                password: password()
            }
        ),
        Err(RuntimeStartError::InvalidWorld)
    );
    // Finish the scheduled Menu cleanup before installing a new puzzle/session.
    pair.host.update();
    assert!(!pair.host.world().contains_non_send::<NetworkSession>());
    let session = host_world(&mut pair.host);
    host_with_transport(
        pair.host.world_mut(),
        Fake {
            id: 0,
            bus: pair.bus.clone(),
        },
        HostOptions {
            display_name: None,
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
        pair.host.world().resource::<NetworkStatus>().phase,
        RuntimePhase::Hosting,
        "{:?}",
        pair.host.world().resource::<NetworkStatus>()
    );
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
                display_name: None,
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
            display_name: None,
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

#[test]
fn rejected_host_rotation_preserves_pointer_basis_and_consumes_control_history() {
    let mut pair = Pair::new();
    pair.ready();
    begin_gesture(&mut pair.host);
    pair.converge();
    pointer(&mut pair.host, Vec2::splat(20.0), true, false);
    let before = cursor(&pair.host);
    send(
        &mut pair.host,
        PieceCommand::RotateDrag {
            members: members(),
            delta: Vec2::splat(f32::NAN),
            quarter_turns: 1,
        },
    );
    pair.host.update();
    assert_eq!(cursor(&pair.host), before);
    assert_eq!(
        pair.host.world().resource::<NetworkStatus>().phase,
        RuntimePhase::Hosting
    );
    pointer(&mut pair.host, Vec2::splat(30.0), true, false);
    assert_eq!(
        pair.host.world().resource::<PieceDataStore>().drag.delta,
        Vec2::splat(30.0)
    );
    pointer(&mut pair.host, Vec2::splat(30.0), false, false);
    pair.converge();
    assert_eq!(
        pair.host.world().resource::<PieceDataStore>().states,
        pair.client.world().resource::<PieceDataStore>().states
    );
    assert_eq!(
        pair.host.world().resource::<PieceDataStore>().states[0].position,
        Vec2::splat(130.0)
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
            display_name: None,
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
            display_name: None,
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
    loop {
        host.update();
        client.update();
        if client
            .world()
            .resource::<PieceDataStore>()
            .held_by
            .get(&PieceId(0))
            == Some(&player)
        {
            break;
        }
        assert!(Instant::now() < deadline, "runtime GNS Grab timed out");
        std::thread::sleep(Duration::from_millis(2));
    }
    let canonical = host.world().resource::<PieceDataStore>().states.clone();
    {
        let mut store = client.world_mut().resource_mut::<PieceDataStore>();
        store.drag.members = members().words().clone();
        store.drag.delta = Vec2::new(14.0, 23.0);
    }
    loop {
        host.update();
        client.update();
        if host
            .world()
            .resource::<remote_drag::RemoteDragPresentation>()
            .offset(PieceId(0))
            == Vec2::new(14.0, 23.0)
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "runtime GNS remote presentation timed out"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(host.world().resource::<PieceDataStore>().states, canonical);
    assert_eq!(
        client
            .world()
            .resource::<remote_drag::RemoteDragPresentation>()
            .offset(PieceId(0)),
        Vec2::ZERO
    );
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
    assert_eq!(
        host.world()
            .resource::<remote_drag::RemoteDragPresentation>()
            .offset(PieceId(0)),
        Vec2::ZERO
    );
    stop_session(client.world_mut());
    stop_session(host.world_mut());
}

#[test]
fn joined_game_plugin_uses_installed_world_without_starting_a_generation_worker() {
    use bevy::{asset::AssetPlugin, input::InputPlugin, transform::TransformPlugin};
    let mut pair = Pair::new();
    stop_session(pair.client.world_mut());
    let (service, _requests) =
        crate::persistence::runtime::PersistenceService::with_storage_requests();
    let mut client = App::new();
    client
        .insert_resource(service)
        .add_plugins((
            MinimalPlugins,
            StatesPlugin,
            InputPlugin,
            TransformPlugin,
            AssetPlugin::default(),
            crate::asset_reader::DirectFileAssetPlugin,
            crate::GamePlugin,
        ))
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<ColorMaterial>>()
        .init_resource::<Assets<Image>>()
        .insert_resource(crate::render::RenderReady::waiting_for_test())
        .insert_resource(PuzzleImageLimits {
            device_max_dimension: 8192,
            gpu_memory_bytes: Some(8 * 1024 * 1024 * 1024),
        })
        .insert_resource(crate::image_settings::ImageSettingsState::load(None))
        .init_resource::<bevy_egui::EguiUserTextures>();
    client.update();
    let old_reference = client
        .world_mut()
        .spawn(crate::components::GridReference)
        .id();
    let old_selection = client
        .world_mut()
        .spawn(crate::components::SelectionBox)
        .id();
    {
        use crate::persistence::{GameId, SaveId, SaveMetadata, SaveTitle};
        let mut persistence = client
            .world_mut()
            .resource_mut::<crate::persistence::runtime::PersistenceState>();
        persistence.game_id = GameId(42);
        persistence.generation = 9;
        persistence.title_dialog_open = true;
        persistence.current_save = Some(SaveMetadata {
            id: SaveId(3),
            game_id: GameId(42),
            title: SaveTitle::new("Previous offline puzzle").unwrap(),
            revision: 7,
            created_at: 1,
            updated_at: 2,
            is_autosave: false,
        });
        persistence.current_autosave = persistence.current_save.clone();
    }
    join_with_transport(
        client.world_mut(),
        Fake {
            id: 2,
            bus: pair.bus.clone(),
        },
        JoinOptions {
            display_name: None,
            address: "127.0.0.1:10000".parse().unwrap(),
            password: password(),
            cached_image: None,
        },
    )
    .unwrap();
    assert!(client.world().get_entity(old_reference).is_err());
    assert!(client.world().get_entity(old_selection).is_err());
    let persistence = client
        .world()
        .resource::<crate::persistence::runtime::PersistenceState>();
    assert_eq!(persistence.generation, 10);
    assert_ne!(persistence.game_id, crate::persistence::GameId(42));
    assert!(persistence.current_save.is_none() && persistence.current_autosave.is_none());
    assert!(!persistence.title_dialog_open);
    for _ in 0..2000 {
        pair.host.update();
        client.update();
        if *client.world().resource::<State<AppState>>().get() == AppState::InGame {
            break;
        }
        assert_ne!(
            client.world().resource::<NetworkStatus>().phase,
            RuntimePhase::Failed,
            "{:?}",
            client.world().resource::<NetworkStatus>().error
        );
        std::thread::yield_now();
    }
    assert_eq!(
        *client.world().resource::<State<AppState>>().get(),
        AppState::InGame
    );
    for _ in 0..5 {
        pair.host.update();
        client.update();
    }
    assert_eq!(
        client.world().resource::<PieceDataStore>().states,
        pair.host.world().resource::<PieceDataStore>().states
    );
    let progress = client.world().resource::<PieceGenerationProgress>();
    assert_eq!(progress.generation_phase, GenerationPhase::UploadingGpu);
    assert!(progress.receiver.is_none());
    assert_eq!(client.world().resource::<PuzzleDefinition>(), &definition());
    assert_eq!(
        client.world().resource::<OriginalPuzzleImage>().hash,
        pair.host.world().resource::<OriginalPuzzleImage>().hash
    );
    client
        .world_mut()
        .resource_mut::<NextState<AppState>>()
        .set(AppState::Menu);
    client.update();
    assert!(!client.world().contains_non_send::<NetworkSession>());
    assert_eq!(
        *client.world().resource::<LocalPlayerId>(),
        LocalPlayerId::default()
    );
    assert!(client.world().resource::<PieceDataStore>().is_empty());
}
