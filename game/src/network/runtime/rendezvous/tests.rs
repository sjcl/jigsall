use super::*;
mod smoke;
use crate::network::gns::rendezvous::protocol::{AuthorityId, MemberId, RoomId};
use crate::network::runtime::tests as fixtures;
use std::{cell::RefCell, collections::VecDeque, rc::Rc, sync::Mutex};

#[derive(Default)]
struct ControlState {
    events: VecDeque<RendezvousEvent>,
    creates: usize,
    joins: Vec<String>,
    stopped: bool,
    released: Vec<PeerId>,
}
struct Control(Rc<RefCell<ControlState>>);
impl ControlPlane for Control {
    fn poll(&mut self) -> Vec<RendezvousEvent> {
        self.0.borrow_mut().events.drain(..).collect()
    }
    fn create_room(&mut self) -> Result<(), RendezvousError> {
        self.0.borrow_mut().creates += 1;
        Ok(())
    }
    fn join_room(&mut self, code: RoomCode) -> Result<(), RendezvousError> {
        self.0.borrow_mut().joins.push(code.to_string());
        Ok(())
    }
    fn shutdown(&mut self) {
        self.0.borrow_mut().stopped = true;
    }
    fn release_route(&mut self, peer: PeerId) {
        self.0.borrow_mut().released.push(peer);
    }
}
// Deliberately implements only Transport, plus the private peer establishment seam.
// There is no fake DirectIpTransport on the common-runtime test backend.
struct PeerOnly {
    inner: fixtures::Fake,
    calls: Rc<RefCell<Vec<PeerId>>>,
    dropped: Rc<RefCell<bool>>,
}
impl Drop for PeerOnly {
    fn drop(&mut self) {
        *self.dropped.borrow_mut() = true;
    }
}
impl Transport for PeerOnly {
    fn poll(&mut self, events: &mut Vec<TransportEvent>) -> Result<(), TransportError> {
        self.inner.poll(events)
    }
    fn send(
        &mut self,
        id: ConnectionId,
        class: MessageClass,
        payload: &[u8],
    ) -> Result<(), TransportError> {
        self.inner.send(id, class, payload)
    }
    fn close(&mut self, id: ConnectionId, reason: DisconnectReason) -> Result<(), TransportError> {
        self.inner.close(id, reason)
    }
    fn reliable_egress(&self, id: ConnectionId) -> Result<ReliableEgress, TransportError> {
        self.inner.reliable_egress(id)
    }
    fn activate_secure_channel(&mut self, id: ConnectionId) -> Result<(), TransportError> {
        self.inner.activate_secure_channel(id)
    }
}
impl PeerTransport for PeerOnly {
    fn connect_peer(&mut self, peer: PeerId) -> Result<ConnectionId, TransportError> {
        self.calls.borrow_mut().push(peer);
        self.inner.connect("127.0.0.1:10000".parse().unwrap())
    }
    fn remote_peer(&self, _: ConnectionId) -> Option<PeerId> {
        Some(peer())
    }
}
fn peer() -> PeerId {
    PeerId::from_bytes([7; 16])
}
fn welcome() -> RendezvousEvent {
    RendezvousEvent::Welcome {
        authority_id: AuthorityId([1; 16]),
    }
}
fn created() -> RendezvousEvent {
    RendezvousEvent::RoomCreated {
        room_id: RoomId([2; 16]),
        room_code: "abcdefGHJK".parse().unwrap(),
        self_member_id: MemberId([3; 16]),
    }
}
fn ready() -> RendezvousEvent {
    RendezvousEvent::HostReady {
        peer_id: peer(),
        room_id: RoomId([2; 16]),
        self_member_id: MemberId([4; 16]),
    }
}
fn host_options() -> RendezvousHostOptions {
    RendezvousHostOptions {
        display_name: None,
        session: SessionDefinition {
            id: SessionId(271),
            image_hash: crate::persistence::image_hash(&fixtures::encoded()),
        },
        host: PlayerId(0),
        password: fixtures::password(),
    }
}
fn join_options() -> RendezvousJoinOptions {
    RendezvousJoinOptions {
        display_name: None,
        room_code: "ABCDEFGHJK".parse().unwrap(),
        password: fixtures::password(),
        cached_image: None,
    }
}
fn backend(id: u64, bus: &Arc<Mutex<fixtures::Bus>>) -> PeerOnly {
    PeerOnly {
        inner: fixtures::Fake {
            id,
            bus: bus.clone(),
        },
        calls: default(),
        dropped: default(),
    }
}
fn control() -> (Control, Rc<RefCell<ControlState>>) {
    let state = Rc::new(RefCell::new(ControlState::default()));
    (Control(state.clone()), state)
}
fn prepared() -> PreparedHost {
    PreparedHost::new(
        fixtures::definition(),
        Some(fixtures::encoded()),
        host_options().session,
    )
    .unwrap()
}
fn limits() -> ImageDecodeLimits {
    ImageDecodeLimits {
        max_texture_dimension: 8192,
    }
}

#[test]
fn rendezvous_host_creates_once_and_exposes_code_only_after_room_created() {
    let bus = Arc::new(Mutex::new(fixtures::Bus::default()));
    let (control, state) = control();
    let mut driver =
        RendezvousRuntimeDriver::host(backend(0, &bus), control, host_options(), prepared());
    assert_eq!(driver.status().phase, RuntimePhase::Connecting);
    assert!(driver.status().room_code.is_none());
    state.borrow_mut().events.push_back(welcome());
    driver.poll_control(Instant::now());
    assert_eq!(driver.stage, Stage::CreatingRoom);
    assert_eq!(state.borrow().creates, 1);
    assert!(driver.runtime.as_ref().unwrap().live.is_empty());
    state.borrow_mut().events.push_back(created());
    driver.poll_control(Instant::now());
    assert_eq!(driver.status().role, Some(RuntimeRole::Host));
    assert_eq!(
        driver.status().connection_method,
        Some(RuntimeConnectionMethod::Internet)
    );
    assert_eq!(driver.status().phase, RuntimePhase::Hosting);
    assert_eq!(driver.status().room_code.as_deref(), Some("ABCDEFGHJK"));
    assert!(driver.status().address.is_none());
    assert_eq!(state.borrow().creates, 1);
}
#[test]
fn rendezvous_join_constructs_bootstrap_only_after_host_ready_and_connect_peer() {
    let bus = Arc::new(Mutex::new(fixtures::Bus::default()));
    let (control, state) = control();
    let backend = backend(1, &bus);
    let calls = backend.calls.clone();
    let mut driver = RendezvousRuntimeDriver::join(backend, control, join_options(), limits());
    assert!(driver.runtime.is_none());
    state.borrow_mut().events.push_back(welcome());
    driver.poll_control(Instant::now());
    assert_eq!(state.borrow().joins, vec!["ABCDEFGHJK"]);
    assert!(driver.runtime.is_none());
    assert!(calls.borrow().is_empty());
    state.borrow_mut().events.push_back(ready());
    driver.poll_control(Instant::now());
    assert_eq!(*calls.borrow(), vec![peer()]);
    let runtime = driver.runtime.as_ref().unwrap();
    let Role::Client(client) = &runtime.role else {
        panic!()
    };
    assert_eq!(client.bootstrap.state(), None);
    assert_eq!(runtime.status.phase, RuntimePhase::Connecting);
    assert!(runtime.connections.peers().next().is_none());
    assert!(!runtime
        .transport
        .has_channel(client.bootstrap.host_connection()));
}
#[test]
fn rendezvous_control_loss_before_establishment_fails_and_cancel_drops_pending_secret_backend() {
    for event in [
        RendezvousEvent::RoomClosed,
        RendezvousEvent::Disconnected(RendezvousError::Network),
    ] {
        let bus = Arc::new(Mutex::new(fixtures::Bus::default()));
        let (control, state) = control();
        let backend = backend(1, &bus);
        let dropped = backend.dropped.clone();
        let mut driver = RendezvousRuntimeDriver::join(backend, control, join_options(), limits());
        state.borrow_mut().events.push_back(event);
        driver.poll_control(Instant::now());
        assert!(!driver.active);
        assert_eq!(driver.status().phase, RuntimePhase::Failed);
        let mut app = fixtures::app();
        RuntimeDriver::teardown(&mut driver, app.world_mut());
        assert!(state.borrow().stopped && *dropped.borrow());
        assert!(driver.join.is_none());
        assert!(bus.lock().unwrap().closed.is_empty());
    }
    let bus = Arc::new(Mutex::new(fixtures::Bus::default()));
    let (control, state) = control();
    let backend = backend(1, &bus);
    let dropped = backend.dropped.clone();
    let driver = RendezvousRuntimeDriver::join(backend, control, join_options(), limits());
    let mut app = fixtures::app();
    world::prepare_join_world(app.world_mut());
    let status = driver.status().clone();
    world::install_driver(app.world_mut(), Box::new(driver), status, default());
    stop_session(app.world_mut());
    assert!(!app.world().contains_non_send::<NetworkSession>());
    assert!(state.borrow().stopped && *dropped.borrow());
}
#[test]
fn rendezvous_host_control_loss_before_room_created_is_failure() {
    let bus = Arc::new(Mutex::new(fixtures::Bus::default()));
    let (control, state) = control();
    let mut driver =
        RendezvousRuntimeDriver::host(backend(0, &bus), control, host_options(), prepared());
    state.borrow_mut().events.push_back(welcome());
    driver.poll_control(Instant::now());
    state
        .borrow_mut()
        .events
        .push_back(RendezvousEvent::Disconnected(RendezvousError::Timeout));
    driver.poll_control(Instant::now());
    assert_eq!(driver.status().phase, RuntimePhase::Failed);
    assert_eq!(driver.status().failure, Some(NetworkFailureKind::Timeout));
}
#[test]
fn rendezvous_control_loss_during_ice_does_not_close_or_revoke_game_connection() {
    let bus = Arc::new(Mutex::new(fixtures::Bus::default()));
    let (control, state) = control();
    let mut driver =
        RendezvousRuntimeDriver::join(backend(1, &bus), control, join_options(), limits());
    state.borrow_mut().events.extend([welcome(), ready()]);
    driver.poll_control(Instant::now());
    state.borrow_mut().events.extend([
        RendezvousEvent::PeerUnavailable { peer_id: peer() },
        RendezvousEvent::RoomClosed,
        RendezvousEvent::Disconnected(RendezvousError::Network),
    ]);
    driver.poll_control(Instant::now());
    assert!(driver.active);
    assert_eq!(driver.stage, Stage::PeerConnecting);
    assert_eq!(driver.status().phase, RuntimePhase::Connecting);
    assert_eq!(
        driver.status().rendezvous_control,
        Some(RendezvousControlStatus::Unavailable)
    );
    assert!(state.borrow().released.is_empty());
    assert!(bus.lock().unwrap().closed.is_empty());
}
#[test]
fn rendezvous_runtime_ready_command_roundtrip_survives_control_loss_then_tears_down() {
    let bus = Arc::new(Mutex::new(fixtures::Bus::default()));
    let (hc, hs) = control();
    let (cc, cs) = control();
    let mut host = fixtures::app();
    fixtures::host_world(&mut host);
    let mut client = fixtures::app();
    let mut hd = RendezvousRuntimeDriver::host(backend(0, &bus), hc, host_options(), prepared());
    world::prepare_host_world(host.world_mut(), hd.status());
    let status = hd.status().clone();
    let roster = hd.take_roster();
    world::install_driver(host.world_mut(), Box::new(hd), status, roster);
    let cd = RendezvousRuntimeDriver::join(backend(1, &bus), cc, join_options(), limits());
    world::prepare_join_world(client.world_mut());
    let status = cd.status().clone();
    world::install_driver(client.world_mut(), Box::new(cd), status, default());
    hs.borrow_mut().events.extend([welcome(), created()]);
    cs.borrow_mut().events.extend([welcome(), ready()]);
    let deadline = Instant::now() + std::time::Duration::from_secs(5);
    while client.world().resource::<NetworkStatus>().phase != RuntimePhase::Ready {
        assert!(Instant::now() < deadline);
        host.update();
        client.update();
        std::thread::yield_now();
    }
    host.update();
    for state in [&hs, &cs] {
        state.borrow_mut().events.extend([
            RendezvousEvent::PeerUnavailable { peer_id: peer() },
            RendezvousEvent::RoomClosed,
            RendezvousEvent::Disconnected(RendezvousError::Network),
        ]);
    }
    host.update();
    client.update();
    assert_eq!(
        host.world().resource::<NetworkStatus>().phase,
        RuntimePhase::Hosting
    );
    assert!(host.world().resource::<NetworkStatus>().room_code.is_none());
    assert_eq!(
        client.world().resource::<NetworkStatus>().phase,
        RuntimePhase::Ready
    );
    assert_eq!(
        client
            .world()
            .resource::<NetworkStatus>()
            .rendezvous_control,
        Some(RendezvousControlStatus::Unavailable)
    );
    assert!(bus.lock().unwrap().closed.is_empty());
    let mut members = puzzella_core::PieceBitSet::new(4);
    members.insert(puzzella_core::PieceId(0));
    fixtures::send(&mut client, PieceCommand::GrabGroup { members });
    for _ in 0..10 {
        client.update();
        host.update();
    }
    assert_eq!(host.world().resource::<PieceDataStore>().held_by.len(), 1);
    assert_eq!(client.world().resource::<PieceDataStore>().held_by.len(), 1);
    stop_session(client.world_mut());
    stop_session(host.world_mut());
    assert!(hs.borrow().stopped && cs.borrow().stopped);
    assert!(!bus.lock().unwrap().closed.is_empty());
    assert!(host.world().resource::<PieceDataStore>().held_by.is_empty());
}
#[test]
fn rendezvous_errors_are_typed_and_config_absence_leaves_world_unchanged() {
    for (code, kind) in [
        (ErrorCode::UnknownRoom, NetworkFailureKind::RoomNotFound),
        (ErrorCode::RoomFull, NetworkFailureKind::Capacity),
        (ErrorCode::JoinTimeout, NetworkFailureKind::Timeout),
        (ErrorCode::ProtocolViolation, NetworkFailureKind::Protocol),
    ] {
        assert_eq!(server_error_kind(code), kind);
    }
    let mut app = fixtures::app();
    fixtures::host_world(&mut app);
    let epoch = app.world().resource::<PieceDataStore>().epoch;
    assert_eq!(
        start_rendezvous_host(app.world_mut(), host_options()),
        Err(RuntimeStartError::InternetUnavailable)
    );
    assert_eq!(
        start_rendezvous_join(app.world_mut(), join_options()),
        Err(RuntimeStartError::InternetUnavailable)
    );
    assert_eq!(app.world().resource::<PieceDataStore>().epoch, epoch);
    assert!(app.world().contains_resource::<PuzzleDefinition>());
    assert!(!app.world().contains_non_send::<NetworkSession>());
}
