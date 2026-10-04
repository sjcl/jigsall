use super::*;
mod smoke;
use crate::network::gns::rendezvous::protocol::{AuthorityId, MemberId, RoomId};
use crate::network::runtime::tests as fixtures;
use crate::{
    persistence::runtime::OriginalPuzzleImage,
    resources::{AppState, LocalGameplayBlocked},
};
use std::{
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
    rc::Rc,
    sync::Mutex,
};

#[derive(Default)]
struct ControlState {
    events: VecDeque<RendezvousEvent>,
    creates: usize,
    joins: Vec<String>,
    stopped: bool,
    released: Vec<PeerId>,
    confirmed: BTreeSet<PeerId>,
    revoked: Vec<PeerId>,
    routes: BTreeMap<PeerId, bool>,
    pending_signals: BTreeSet<PeerId>,
    native_capacity: bool,
}
struct Control(Rc<RefCell<ControlState>>);
impl ControlPlane for Control {
    fn poll(
        &mut self,
        has_peer: &dyn Fn(PeerId) -> bool,
        native_capacity: bool,
    ) -> Vec<RendezvousEvent> {
        self.0.borrow_mut().native_capacity = native_capacity;
        let events: Vec<_> = self.0.borrow_mut().events.drain(..).collect();
        self.reclaim_unavailable_routes(has_peer);
        let mut announced_host = None;
        for event in &events {
            {
                let mut state = self.0.borrow_mut();
                match event {
                    RendezvousEvent::PeerJoined { peer_id, .. }
                    | RendezvousEvent::HostReady { peer_id, .. } => {
                        assert!(state.routes.len() < crate::network::gns::rendezvous::MAX_ROUTES);
                        state.routes.insert(*peer_id, true);
                        if matches!(event, RendezvousEvent::HostReady { .. }) {
                            announced_host = Some(*peer_id);
                        }
                    }
                    RendezvousEvent::PeerUnavailable { peer_id } => {
                        state.routes.insert(*peer_id, false);
                    }
                    RendezvousEvent::RoomClosed | RendezvousEvent::Disconnected(_) => {
                        state
                            .routes
                            .values_mut()
                            .for_each(|available| *available = false);
                    }
                    _ => {}
                }
            }
            self.reclaim_unavailable_routes(&|peer| has_peer(peer) || announced_host == Some(peer));
        }
        events
    }
    fn reclaim_unavailable_routes(&mut self, has_peer: &dyn Fn(PeerId) -> bool) {
        let unused: Vec<_> = self
            .0
            .borrow()
            .routes
            .iter()
            .filter_map(|(&peer, &available)| {
                (!available && !has_peer(peer) && !self.has_pending_signal(peer)).then_some(peer)
            })
            .collect();
        for peer in unused {
            self.release_route(peer);
        }
    }
    fn has_pending_signal(&self, peer: PeerId) -> bool {
        self.0.borrow().pending_signals.contains(&peer)
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
    fn confirm_peer(&mut self, peer: PeerId) {
        self.0.borrow_mut().confirmed.insert(peer);
    }
    fn revoke_peer(&mut self, peer: PeerId) {
        self.0.borrow_mut().revoked.push(peer);
        self.release_route(peer);
    }
    fn release_route(&mut self, peer: PeerId) {
        let mut state = self.0.borrow_mut();
        state.routes.remove(&peer);
        state.pending_signals.remove(&peer);
        state.released.push(peer);
    }
}
// Deliberately implements only Transport, plus the private peer establishment seam.
// There is no fake DirectIpTransport on the common-runtime test backend.
struct PeerOnly {
    inner: fixtures::Fake,
    calls: Rc<RefCell<Vec<PeerId>>>,
    dropped: Rc<RefCell<bool>>,
    handles: Rc<RefCell<BTreeMap<ConnectionId, PeerId>>>,
    retired: BTreeSet<PeerId>,
}
impl Drop for PeerOnly {
    fn drop(&mut self) {
        *self.dropped.borrow_mut() = true;
    }
}
impl Transport for PeerOnly {
    fn poll(&mut self, events: &mut Vec<TransportEvent>) -> Result<(), TransportError> {
        let start = events.len();
        self.inner.poll(events)?;
        for event in &events[start..] {
            match event {
                TransportEvent::Connected { connection } => {
                    self.handles
                        .borrow_mut()
                        .entry(*connection)
                        .or_insert(peer());
                }
                TransportEvent::Disconnected { connection, .. }
                | TransportEvent::ConnectionFailed { connection, .. } => {
                    if let Some(peer) = self.handles.borrow_mut().remove(connection) {
                        self.retired.insert(peer);
                    }
                }
                _ => {}
            }
        }
        Ok(())
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
        if let Some(peer) = self.handles.borrow_mut().remove(&id) {
            self.retired.insert(peer);
        }
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
        let connection = self.inner.connect("127.0.0.1:10000".parse().unwrap())?;
        self.handles.borrow_mut().insert(connection, peer);
        Ok(connection)
    }
    fn remote_peer(&self, connection: ConnectionId) -> Option<PeerId> {
        self.handles.borrow().get(&connection).copied()
    }
    fn has_peer(&self, peer: PeerId) -> bool {
        self.handles.borrow().values().any(|&p| p == peer)
    }
    fn has_connection_capacity(&self) -> bool {
        self.handles.borrow().len() < crate::network::lifecycle::MAX_CONNECTIONS
    }
    fn take_retired_peers(&mut self) -> Vec<PeerId> {
        std::mem::take(&mut self.retired).into_iter().collect()
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
        retired: default(),
        inner: fixtures::Fake {
            id,
            bus: bus.clone(),
        },
        calls: default(),
        dropped: default(),
        handles: default(),
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
fn rendezvous_owner_reports_native_capacity_independently_of_route_capacity() {
    let bus = Arc::new(Mutex::new(fixtures::Bus::default()));
    let backend = backend(0, &bus);
    let handles = backend.handles.clone();
    let (control, state) = control();
    let mut driver = RendezvousRuntimeDriver::host(backend, control, host_options(), prepared());
    driver.poll_control(Instant::now());
    assert!(state.borrow().native_capacity);
    for n in 0..crate::network::lifecycle::MAX_CONNECTIONS {
        handles
            .borrow_mut()
            .insert(ConnectionId::new(n as u64), peer());
    }
    driver.poll_control(Instant::now());
    assert!(!state.borrow().native_capacity);
    assert!(state.borrow().routes.is_empty());
    handles.borrow_mut().clear();
    driver.poll_control(Instant::now());
    assert!(state.borrow().native_capacity);
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
    assert!(driver.status().host_start_failed);
}

#[test]
fn rendezvous_pre_room_host_failure_keeps_prepared_world_and_allows_retry() {
    for welcomed in [false, true] {
        for failure in [
            Some(RendezvousEvent::Disconnected(RendezvousError::Network)),
            Some(RendezvousEvent::ServerError(ErrorCode::Capacity)),
            Some(RendezvousEvent::ServerError(ErrorCode::ProtocolViolation)),
            None, // Establishment deadline, even without a control event.
        ] {
            let bus = Arc::new(Mutex::new(fixtures::Bus::default()));
            let (control, state) = control();
            let backend = backend(0, &bus);
            let dropped = backend.dropped.clone();
            let mut host = fixtures::app();
            fixtures::host_world(&mut host);
            host.world_mut()
                .resource_mut::<NextState<AppState>>()
                .set(AppState::InGame);
            host.update();
            let epoch = host.world().resource::<PieceDataStore>().epoch;
            let states = host.world().resource::<PieceDataStore>().states.clone();
            let original = host
                .world()
                .resource::<OriginalPuzzleImage>()
                .encoded
                .clone()
                .unwrap();
            let handle = host
                .world_mut()
                .resource_mut::<Assets<Image>>()
                .add(Image::default());
            host.world_mut()
                .insert_resource(crate::resources::PuzzleImage {
                    handle: handle.clone(),
                    logical_size: fixtures::definition().image_size,
                    texture_size: UVec2::ONE,
                    opaque: true,
                });
            let mut driver =
                RendezvousRuntimeDriver::host(backend, control, host_options(), prepared());
            if welcomed {
                state.borrow_mut().events.push_back(welcome());
                driver.poll_control(Instant::now());
            }
            if let Some(event) = failure {
                state.borrow_mut().events.push_back(event);
            } else {
                driver.deadline = Instant::now();
            }
            world::prepare_host_world(host.world_mut(), driver.status());
            let status = driver.status().clone();
            let roster = driver.take_roster();
            world::install_driver(host.world_mut(), Box::new(driver), status, roster);
            // A queued command must not fall through to offline authority on failure.
            fixtures::send(&mut host, PieceCommand::Grab(puzzella_core::PieceId(0)));
            host.update();
            host.update();
            let status = host.world().resource::<NetworkStatus>();
            assert_eq!(status.phase, RuntimePhase::Failed);
            assert!(status.host_start_failed);
            assert_eq!(
                *host.world().resource::<State<AppState>>().get(),
                AppState::InGame
            );
            assert!(matches!(
                host.world().resource::<NextState<AppState>>(),
                NextState::Unchanged
            ));
            assert!(!host.world().contains_non_send::<NetworkSession>());
            assert!(state.borrow().stopped && *dropped.borrow());
            let store = host.world().resource::<PieceDataStore>();
            assert_eq!(store.epoch, epoch);
            assert_eq!(store.states.as_ptr(), states.as_ptr());
            assert!(store.held_by.is_empty());
            assert_eq!(
                host.world().resource::<PuzzleDefinition>().seed,
                fixtures::definition().seed
            );
            assert!(Arc::ptr_eq(
                &original,
                host.world()
                    .resource::<OriginalPuzzleImage>()
                    .encoded
                    .as_ref()
                    .unwrap()
            ));
            assert_eq!(
                host.world()
                    .resource::<crate::resources::PuzzleImage>()
                    .handle,
                handle
            );
            let options = HostOptions {
                display_name: None,
                address: "127.0.0.1:27015".parse().unwrap(),
                session: host_options().session,
                host: PlayerId(0),
                password: fixtures::password(),
            };
            host_with_transport(
                host.world_mut(),
                fixtures::Fake {
                    id: 0,
                    bus: bus.clone(),
                },
                options,
            )
            .unwrap();
            assert_eq!(host.world().resource::<PieceDataStore>().epoch, epoch);
            assert!(!host.world().resource::<NetworkStatus>().host_start_failed);
            stop_session(host.world_mut());
        }
    }
}

#[test]
fn rendezvous_join_and_post_room_host_failures_still_return_to_menu() {
    for host_role in [false, true] {
        let bus = Arc::new(Mutex::new(fixtures::Bus::default()));
        let (control, state) = control();
        let mut app = fixtures::app();
        fixtures::host_world(&mut app);
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::InGame);
        app.update();
        let mut driver = if host_role {
            RendezvousRuntimeDriver::host(backend(0, &bus), control, host_options(), prepared())
        } else {
            RendezvousRuntimeDriver::join(backend(1, &bus), control, join_options(), limits())
        };
        if host_role {
            state.borrow_mut().events.extend([welcome(), created()]);
            driver.poll_control(Instant::now());
            // An unexpected transition is terminal after RoomCreated too.
            state.borrow_mut().events.push_back(welcome());
        } else {
            state
                .borrow_mut()
                .events
                .push_back(RendezvousEvent::Disconnected(RendezvousError::Network));
        }
        let status = driver.status().clone();
        world::install_driver(app.world_mut(), Box::new(driver), status, default());
        app.update();
        assert!(!app.world().resource::<NetworkStatus>().host_start_failed);
        assert_eq!(
            *app.world().resource::<State<AppState>>().get(),
            AppState::Menu
        );
        assert!(!app.world().contains_non_send::<NetworkSession>());
        assert!(state.borrow().stopped);
        app.update();
        assert!(matches!(
            app.world().resource::<NextState<AppState>>(),
            NextState::Unchanged
        ));
    }
}
#[test]
fn rendezvous_unconnected_peer_churn_releases_routes() {
    let bus = Arc::new(Mutex::new(fixtures::Bus::default()));
    let (control, state) = control();
    let mut driver =
        RendezvousRuntimeDriver::host(backend(0, &bus), control, host_options(), prepared());
    let mut app = fixtures::app();
    fixtures::host_world(&mut app);
    state.borrow_mut().events.extend([welcome(), created()]);
    for n in 10..110 {
        let peer_id = PeerId::from_bytes([n; 16]);
        state.borrow_mut().events.extend([
            RendezvousEvent::PeerJoined {
                peer_id,
                member_id: MemberId([n; 16]),
            },
            RendezvousEvent::PeerUnavailable { peer_id },
        ]);
        RuntimeDriver::poll(&mut driver, app.world_mut());
        assert!(state.borrow().released.contains(&peer_id));
        assert!(driver.active);
        assert_eq!(driver.status().phase, RuntimePhase::Hosting);
    }
}
#[test]
fn rendezvous_native_failure_without_control_loss_revokes_and_late_errors_keep_room() {
    let bus = Arc::new(Mutex::new(fixtures::Bus::default()));
    let (control, state) = control();
    let backend = backend(0, &bus);
    let pending = ConnectionId::new(901);
    backend.handles.borrow_mut().insert(pending, peer());
    let mut driver = RendezvousRuntimeDriver::host(backend, control, host_options(), prepared());
    let mut app = fixtures::app();
    fixtures::host_world(&mut app);
    state.borrow_mut().events.extend([
        welcome(),
        created(),
        RendezvousEvent::PeerJoined {
            peer_id: peer(),
            member_id: MemberId([9; 16]),
        },
    ]);
    bus.lock()
        .unwrap()
        .inbox
        .entry(0)
        .or_default()
        .push_back(TransportEvent::ConnectionFailed {
            connection: pending,
            reason: DisconnectReason::BackendConnectionTimeout,
        });
    RuntimeDriver::poll(&mut driver, app.world_mut());
    assert_eq!(state.borrow().revoked, [peer()]);
    assert!(state.borrow().confirmed.is_empty());
    assert!(driver.runtime.as_ref().unwrap().live.is_empty());
    for code in [ErrorCode::UnknownTarget, ErrorCode::JoinTimeout] {
        state
            .borrow_mut()
            .events
            .push_back(RendezvousEvent::ServerError(code));
        RuntimeDriver::poll(&mut driver, app.world_mut());
        assert!(driver.active);
        assert!(!state.borrow().stopped);
        assert!(driver.status().room_code.is_some());
    }
}
#[test]
fn rendezvous_wrong_password_revokes_before_ready() {
    let bus = Arc::new(Mutex::new(fixtures::Bus::default()));
    let (hc, hs) = control();
    let (cc, cs) = control();
    let mut host = fixtures::app();
    fixtures::host_world(&mut host);
    let mut client = fixtures::app();
    let mut hd = RendezvousRuntimeDriver::host(backend(0, &bus), hc, host_options(), prepared());
    let mut options = join_options();
    options.password = SessionPassword::new("wrong password".into()).unwrap();
    let mut cd = RendezvousRuntimeDriver::join(backend(1, &bus), cc, options, limits());
    world::prepare_join_world(client.world_mut());
    hs.borrow_mut().events.extend([welcome(), created()]);
    cs.borrow_mut().events.extend([welcome(), ready()]);
    for _ in 0..100 {
        RuntimeDriver::poll(&mut hd, host.world_mut());
        RuntimeDriver::poll(&mut cd, client.world_mut());
        if !hs.borrow().revoked.is_empty() {
            break;
        }
    }
    assert_eq!(hs.borrow().revoked, [peer()]);
    assert!(hs.borrow().confirmed.is_empty());
    assert!(hd.active);
    assert_ne!(cd.status().phase, RuntimePhase::Ready);
}
#[test]
fn rendezvous_host_pending_failure_reclaims_route_without_a_connected_event() {
    for loss in [
        RendezvousEvent::PeerUnavailable { peer_id: peer() },
        RendezvousEvent::RoomClosed,
        RendezvousEvent::Disconnected(RendezvousError::Network),
    ] {
        let bus = Arc::new(Mutex::new(fixtures::Bus::default()));
        let (control, state) = control();
        let backend = backend(0, &bus);
        let handles = backend.handles.clone();
        let pending = ConnectionId::new(901);
        handles.borrow_mut().insert(pending, peer());
        let mut driver =
            RendezvousRuntimeDriver::host(backend, control, host_options(), prepared());
        let mut app = fixtures::app();
        fixtures::host_world(&mut app);
        state.borrow_mut().events.extend([
            welcome(),
            created(),
            RendezvousEvent::PeerJoined {
                peer_id: peer(),
                member_id: MemberId([9; 16]),
            },
            loss,
        ]);
        RuntimeDriver::poll(&mut driver, app.world_mut());
        assert!(driver.runtime.as_ref().unwrap().live.is_empty());
        assert!(state.borrow().released.is_empty());
        assert!(bus.lock().unwrap().closed.is_empty());
        bus.lock().unwrap().inbox.entry(0).or_default().push_back(
            TransportEvent::ConnectionFailed {
                connection: pending,
                reason: DisconnectReason::BackendConnectionTimeout,
            },
        );
        RuntimeDriver::poll(&mut driver, app.world_mut());
        assert_eq!(state.borrow().released, [peer()]);
        assert!(handles.borrow().is_empty());
        assert!(driver.active);
    }
}
#[test]
fn rendezvous_departed_live_connection_preserves_a_pending_sibling() {
    let bus = Arc::new(Mutex::new(fixtures::Bus::default()));
    let (control, state) = control();
    let backend = backend(0, &bus);
    let departed = ConnectionId::new(900);
    let pending = ConnectionId::new(901);
    let handles = backend.handles.clone();
    handles
        .borrow_mut()
        .extend([(departed, peer()), (pending, peer())]);
    let mut driver = RendezvousRuntimeDriver::host(backend, control, host_options(), prepared());
    driver.runtime.as_mut().unwrap().live.insert(departed);
    state.borrow_mut().events.extend([
        welcome(),
        created(),
        RendezvousEvent::PeerUnavailable { peer_id: peer() },
    ]);
    bus.lock()
        .unwrap()
        .inbox
        .entry(0)
        .or_default()
        .push_back(TransportEvent::Disconnected {
            connection: departed,
            reason: DisconnectReason::RemoteClosed,
        });
    let mut app = fixtures::app();
    fixtures::host_world(&mut app);
    RuntimeDriver::poll(&mut driver, app.world_mut());
    assert!(driver.runtime.as_ref().unwrap().live.is_empty());
    assert_eq!(handles.borrow().len(), 1);
    assert!(state.borrow().released.is_empty());
    bus.lock()
        .unwrap()
        .inbox
        .entry(0)
        .or_default()
        .push_back(TransportEvent::ConnectionFailed {
            connection: pending,
            reason: DisconnectReason::ConnectionProblem,
        });
    RuntimeDriver::poll(&mut driver, app.world_mut());
    assert_eq!(state.borrow().released, [peer()]);
}
#[test]
fn rendezvous_transient_server_errors_preserve_control_during_hosting_and_ice() {
    for host in [true, false] {
        let bus = Arc::new(Mutex::new(fixtures::Bus::default()));
        let (control, state) = control();
        let mut driver = if host {
            RendezvousRuntimeDriver::host(backend(0, &bus), control, host_options(), prepared())
        } else {
            RendezvousRuntimeDriver::join(backend(1, &bus), control, join_options(), limits())
        };
        state
            .borrow_mut()
            .events
            .extend([welcome(), if host { created() } else { ready() }]);
        driver.poll_control(Instant::now());
        let phase = driver.status().phase;
        let code = driver.status().room_code.clone();
        for error in [ErrorCode::Backpressure, ErrorCode::RateLimited] {
            state
                .borrow_mut()
                .events
                .push_back(RendezvousEvent::ServerError(error));
            driver.poll_control(Instant::now());
            assert!(!state.borrow().stopped);
            assert_eq!(
                driver.status().rendezvous_control,
                Some(RendezvousControlStatus::Available)
            );
            assert_eq!(driver.status().phase, phase);
            assert_eq!(driver.status().room_code, code);
        }
        state
            .borrow_mut()
            .events
            .push_back(RendezvousEvent::PeerJoined {
                peer_id: PeerId::from_bytes([8; 16]),
                member_id: MemberId([8; 16]),
            });
        driver.poll_control(Instant::now());
        assert!(driver.active && !state.borrow().stopped);
        state
            .borrow_mut()
            .events
            .push_back(RendezvousEvent::Disconnected(RendezvousError::Network));
        driver.poll_control(Instant::now());
        assert_eq!(
            driver.status().rendezvous_control,
            Some(RendezvousControlStatus::Unavailable)
        );
        assert!(driver.active);
    }
}
#[test]
fn rendezvous_protocol_errors_remain_terminal_and_establishment_errors_remain_failures() {
    for established in [false, true] {
        let bus = Arc::new(Mutex::new(fixtures::Bus::default()));
        let (control, state) = control();
        let mut driver =
            RendezvousRuntimeDriver::host(backend(0, &bus), control, host_options(), prepared());
        state.borrow_mut().events.push_back(welcome());
        if established {
            state.borrow_mut().events.push_back(created());
        }
        driver.poll_control(Instant::now());
        state
            .borrow_mut()
            .events
            .push_back(RendezvousEvent::ServerError(if established {
                ErrorCode::ProtocolViolation
            } else {
                ErrorCode::RateLimited
            }));
        driver.poll_control(Instant::now());
        if established {
            assert!(state.borrow().stopped && driver.active);
            assert!(driver.status().room_code.is_none());
            assert_eq!(
                driver.status().rendezvous_control,
                Some(RendezvousControlStatus::Unavailable)
            );
        } else {
            assert!(!driver.active);
            assert_eq!(driver.status().phase, RuntimePhase::Failed);
            assert_eq!(driver.status().failure, Some(NetworkFailureKind::Capacity));
        }
    }
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
fn rendezvous_host_ready_and_control_loss_in_one_batch_still_start_pending_ice() {
    let bus = Arc::new(Mutex::new(fixtures::Bus::default()));
    let (control, state) = control();
    let mut driver =
        RendezvousRuntimeDriver::join(backend(1, &bus), control, join_options(), limits());
    state
        .borrow_mut()
        .events
        .extend([welcome(), ready(), RendezvousEvent::RoomClosed]);
    driver.poll_control(Instant::now());
    assert_eq!(driver.stage, Stage::PeerConnecting);
    assert!(driver.active);
    assert!(driver
        .runtime
        .as_ref()
        .unwrap()
        .transport
        .backend()
        .has_peer(peer()));
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
    let mut confirmed_before_ready = false;
    while client.world().resource::<NetworkStatus>().phase != RuntimePhase::Ready {
        assert!(Instant::now() < deadline);
        host.update();
        if hs.borrow().confirmed.contains(&peer()) {
            confirmed_before_ready = true;
            assert_ne!(
                client.world().resource::<NetworkStatus>().phase,
                RuntimePhase::Ready
            );
        }
        client.update();
        std::thread::yield_now();
    }
    host.update();
    assert!(confirmed_before_ready);
    assert!(cs.borrow().confirmed.is_empty());
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
    let epoch = client.world().resource::<PieceDataStore>().epoch;
    let confirmed = crate::checkpoint::PuzzleCheckpoint::capture(
        client.world().resource::<PieceDataStore>(),
        client.world().resource::<PuzzleDefinition>(),
        client.world().resource::<OriginalPuzzleImage>().hash,
    )
    .unwrap();
    stop_session(host.world_mut());
    client.update();
    client.update();
    // Internet gameplay loss uses the same retained-save path as Direct IP.
    let status = client.world().resource::<NetworkStatus>();
    assert_eq!(status.failure, Some(NetworkFailureKind::ConnectionLost));
    assert!(status.has_disconnected_game());
    assert_eq!(
        *client.world().resource::<State<AppState>>().get(),
        AppState::InGame
    );
    assert_eq!(client.world().resource::<PieceDataStore>().epoch, epoch);
    assert_eq!(
        crate::checkpoint::PuzzleCheckpoint::capture(
            client.world().resource::<PieceDataStore>(),
            client.world().resource::<PuzzleDefinition>(),
            client.world().resource::<OriginalPuzzleImage>().hash,
        )
        .unwrap(),
        confirmed
    );
    assert!(client
        .world()
        .resource::<PieceDataStore>()
        .held_by
        .is_empty());
    assert!(client.world().contains_resource::<OriginalPuzzleImage>());
    assert!(client.world().resource::<LocalGameplayBlocked>().0);
    assert!(client.world().contains_non_send::<NetworkSession>());
    stop_session(client.world_mut());
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
