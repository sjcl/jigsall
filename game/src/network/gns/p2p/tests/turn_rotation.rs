use super::*;
struct Fixture {
    child: Child,
    input: ChildStdin,
    address: String,
    stats: mpsc::Receiver<serde_json::Value>,
}
impl Fixture {
    fn start() -> Self {
        Self::start_with_relay_pairs(false)
    }
    fn start_with_relay_pairs(relay_pairs_only: bool) -> Self {
        let python = if cfg!(windows) { "python" } else { "python3" };
        let mut command = Command::new(python);
        command.arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/turn_server.py"
        ));
        if relay_pairs_only {
            command.arg("--relay-pairs-only");
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let mut reader = std::io::BufReader::new(child.stdout.take().unwrap());
        let mut address = String::new();
        reader.read_line(&mut address).unwrap();
        assert!(
            !address.trim().is_empty(),
            "fixture failed to bind a LAN interface"
        );
        let (tx, stats) = mpsc::channel();
        std::thread::spawn(move || {
            for line in reader.lines().map_while(Result::ok) {
                if let Some(json) = line.strip_prefix("STATS:") {
                    let _ = tx.send(serde_json::from_str(json).unwrap());
                }
            }
        });
        Self {
            child,
            input,
            address: address.trim().into(),
            stats,
        }
    }
    fn send(&mut self, value: &str) {
        writeln!(self.input, "{value}").unwrap();
        self.input.flush().unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
#[test]
fn gns_localhost_turn_fixed_credentials_stale_nonce() {
    let mut fixture = Fixture::start_with_relay_pairs(true);
    let (tx, rx) = mpsc::sync_channel(256);
    let mut peers = Vec::new();
    for (index, role) in ["client", "host"].into_iter().enumerate() {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "network::gns::p2p::tests::gns_p2p_child",
                "--nocapture",
            ])
            .env("JIGSALL_P2P_TEST_ROLE", role)
            .env("JIGSALL_TURN_TEST_ONLY", "1")
            .env("JIGSALL_TURN_TEST_ADDRESS", &fixture.address)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = child.stdout.take().unwrap();
        let tx = tx.clone();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(output).lines() {
                let Ok(line) = line else {
                    break;
                };
                if let Some(json) = line.strip_prefix("P2P:") {
                    let frame: Frame = serde_json::from_str(json).unwrap();
                    if tx.send((index, frame)).is_err() {
                        break;
                    }
                } else if !line.is_empty() {
                    eprintln!("P2P child {index}: {line}");
                }
            }
        });
        peers.push(Process { child, input });
    }
    let deadline = Instant::now() + Duration::from_secs(45);
    let mailboxes = [SignalingEndpoint::default(), SignalingEndpoint::default()];
    let mut ids = [None; 2];
    let mut relay = InMemorySignaling::default();
    let mut started = false;
    let mut auth = [false; 2];
    let mut exchanged = false;
    let mut received = [false; 2];
    let mut closing = false;
    let mut rotation_started = false;
    let mut rotation_sent = false;
    let mut rotated = [false; 2];
    let mut second_exchange = false;
    let mut stage_time = Instant::now();
    let mut closed = [false; 2];
    let mut connected = [0; 2];
    while !closed.into_iter().all(|x| x) {
        assert!(Instant::now() < deadline, "P2P parent timed out");
        for peer in &mut peers {
            assert!(
                peer.child.try_wait().unwrap().is_none(),
                "child exited early"
            );
        }
        if let Ok((index, frame)) = rx.recv_timeout(Duration::from_millis(5)) {
            match frame {
                Frame::Peer(bytes) => {
                    let id = PeerId::from_bytes(bytes);
                    ids[index] = Some(id);
                    relay.register(id, mailboxes[index].clone()).unwrap();
                }
                Frame::Signal(to, payload) => {
                    assert!(mailboxes[index].send(PeerId::from_bytes(to), &payload));
                    // Duplicate valid GNS signals must not create duplicate
                    // connections, Connected events or application records.
                    assert!(mailboxes[index].send(PeerId::from_bytes(to), &payload));
                }
                Frame::Connected(details) => {
                    connected[index] += 1;
                    eprintln!("P2P {index}: {details}");
                    assert!(details.contains("ICE"), "must use actual ICE transport");
                }
                Frame::Rotated => rotated[index] = true,
                Frame::Authenticated => auth[index] = true,
                Frame::Received => received[index] = true,
                Frame::Closed => {
                    assert!(!closed[index]);
                    closed[index] = true;
                }
                _ => panic!("unexpected child frame"),
            }
        }
        if !started && ids.iter().all(Option::is_some) {
            assert_ne!(ids[0], ids[1]);
            peers[1].send(Frame::Start(ids[0].unwrap().to_bytes()));
            peers[0].send(Frame::Start(ids[1].unwrap().to_bytes()));
            started = true;
        }
        relay.poll();
        for index in 0..2 {
            while let Some(signal) = mailboxes[index].pop_inbound() {
                peers[index].send(Frame::Signal(signal.peer.to_bytes(), signal.payload));
            }
        }
        if !exchanged && auth.into_iter().all(|x| x) {
            for peer in &mut peers {
                peer.send(Frame::Exchange);
            }
            exchanged = true;
        }
        if !rotation_started && received.into_iter().all(|x| x) {
            fixture.send("hold");
            stage_time = Instant::now();
            rotation_started = true;
        }
        if rotation_started && !rotation_sent && stage_time.elapsed() > Duration::from_millis(2200)
        {
            fixture.send("stale");
            for peer in &mut peers {
                peer.send(Frame::Rotate);
            }
            rotation_sent = true; // Wait for both acknowledgments before continuing.
            received = [false; 2];
            stage_time = Instant::now();
        }
        if rotated.into_iter().all(|x| x)
            && !second_exchange
            && stage_time.elapsed() > Duration::from_secs(3)
        {
            for peer in &mut peers {
                peer.send(Frame::Exchange);
            }
            second_exchange = true;
        }
        if second_exchange && !closing && received.into_iter().all(|x| x) {
            peers[0].send(Frame::Close);
            closing = true;
        }
    }
    assert_eq!(connected, [1, 1]);
    fixture.send("stats");
    let stats = fixture.stats.recv_timeout(Duration::from_secs(2)).unwrap();
    for metric in ["allocate_a", "refresh_a", "stale", "relayed", "held"] {
        assert!(stats[metric].as_u64().unwrap() > 0, "{metric}: {stats}");
    }
    for metric in [
        "allocate_b",
        "refresh_b",
        "permission_b",
        "wrong_credentials",
        "refresh_fail",
    ] {
        assert_eq!(stats[metric].as_u64().unwrap(), 0, "{metric}: {stats}");
    }
    assert!(second_exchange && closing && received.into_iter().all(|x| x));
    for peer in &mut peers {
        peer.send(Frame::Finish);
    }
    for peer in &mut peers {
        assert!(peer.child.wait().unwrap().success());
    }
}

#[derive(Debug, Serialize, Deserialize)]
enum ActorFrame {
    Peer([u8; 16]),
    Connect([u8; 16]),
    Signal([u8; 16], Vec<u8>),
    Connected([u8; 16], bool),
    Rotate,
    RejectEndpointChange,
    Rotated,
    Exchange,
    Received([u8; 16]),
    Stop,
}
fn actor_emit(frame: ActorFrame) {
    let mut out = std::io::stdout().lock();
    writeln!(out, "ACTOR:{}", serde_json::to_string(&frame).unwrap()).unwrap();
    out.flush().unwrap();
}
#[test]
fn gns_turn_actor_child() {
    if std::env::var_os("JIGSALL_TURN_ACTOR").is_none() {
        return;
    }
    let mut backend =
        GnsP2p::new_unverified_for_test(P2P_VIRTUAL_PORT, IceConfig::default()).unwrap();
    let mailbox = backend.signaling();
    let address = std::env::var("JIGSALL_TURN_TEST_ADDRESS").unwrap();
    let initial = std::env::var("JIGSALL_TURN_INITIAL").unwrap();
    let credentials = |version: &str| {
        vec![TurnServer {
            address: address.clone(),
            username: format!("user-{version}"),
            password: format!("password-{version}"),
        }]
    };
    let no_turn = initial.is_empty();
    let mut version = initial.as_str();
    let mut initial_credentials = credentials(&initial);
    if std::env::var_os("JIGSALL_TURN_WRONG_PASSWORD").is_some() {
        initial_credentials[0].password = "wrong".into();
    }
    if !no_turn {
        backend.install_turn(&initial_credentials).unwrap();
    }
    actor_emit(ActorFrame::Peer(backend.peer_id().to_bytes()));
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if tx
                .send(serde_json::from_str::<ActorFrame>(&line).unwrap())
                .is_err()
            {
                break;
            }
        }
    });
    let mut connections = BTreeMap::new();
    let mut connection_versions: BTreeMap<ConnectionId, String> = BTreeMap::new();
    let deadline = Instant::now() + Duration::from_secs(40);
    while Instant::now() < deadline {
        while let Ok(frame) = rx.try_recv() {
            match frame {
                ActorFrame::Connect(peer) => {
                    backend
                        .connect_peer(PeerId::from_bytes(peer), P2P_VIRTUAL_PORT)
                        .unwrap();
                }
                ActorFrame::Signal(peer, bytes) => {
                    mailbox.receive(PeerId::from_bytes(peer), &bytes).unwrap();
                }
                ActorFrame::Rotate => {
                    if no_turn {
                        assert_eq!(
                            backend.install_turn(&credentials("B")),
                            Err(TransportError::ProtocolViolation)
                        );
                    } else {
                        backend.install_turn(&credentials("B")).unwrap();
                        version = "B";
                    }
                    for (id, connection) in &backend.connections {
                        connection.native.assert_turn_user(&connection_versions[id]);
                    }
                    actor_emit(ActorFrame::Rotated);
                }
                ActorFrame::RejectEndpointChange => {
                    let mut changed = credentials("B");
                    let mut extra = changed[0].clone();
                    extra.address = "unknown.invalid:3478".into();
                    changed.push(extra);
                    assert_eq!(
                        backend.install_turn(&changed),
                        Err(TransportError::ProtocolViolation)
                    );
                    for (id, connection) in &backend.connections {
                        connection.native.assert_turn_user(&connection_versions[id]);
                    }
                }
                ActorFrame::Exchange => {
                    for connection in connections.values().copied().collect::<Vec<_>>() {
                        backend
                            .send(connection, MessageClass::Control, &test_messages()[0].1)
                            .unwrap();
                    }
                }
                ActorFrame::Stop => return,
                _ => panic!("invalid actor command"),
            }
        }
        let mut events = Vec::new();
        backend.poll(&mut events).unwrap();
        for event in events {
            match event {
                TransportEvent::Connected { connection } => {
                    let peer = backend.remote_peer(connection).unwrap();
                    assert!(
                        connections.insert(peer, connection).is_none(),
                        "connection must stay stable across rotation"
                    );
                    let username = if no_turn {
                        String::new()
                    } else {
                        format!("user-{version}")
                    };
                    backend.connections[&connection]
                        .native
                        .assert_turn_user(&username);
                    connection_versions.insert(connection, username);
                    let relay = backend.connections[&connection].native.is_relay();
                    backend.activate_secure_channel(connection).unwrap();
                    backend.mark_ready(connection).unwrap();
                    actor_emit(ActorFrame::Connected(peer.to_bytes(), relay));
                }
                TransportEvent::Message {
                    connection,
                    payload,
                    ..
                } => {
                    assert_eq!(payload, test_messages()[0].1);
                    actor_emit(ActorFrame::Received(
                        backend.remote_peer(connection).unwrap().to_bytes(),
                    ));
                }
                TransportEvent::ConnectionFailed { .. }
                    if std::env::var_os("JIGSALL_TURN_WRONG_PASSWORD").is_some() => {}
                other => panic!("actor connection failed: {other:?}"),
            }
        }
        while let Some(signal) = mailbox.pop_outbound() {
            actor_emit(ActorFrame::Signal(signal.peer.to_bytes(), signal.payload));
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("actor timed out");
}
struct Actor {
    process: Process,
    peer: [u8; 16],
}
impl Actor {
    fn spawn(
        index: usize,
        address: &str,
        relay_only: bool,
        initial: &str,
        wrong: bool,
        output: mpsc::Sender<(usize, ActorFrame)>,
    ) -> Self {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "network::gns::p2p::tests::turn_rotation::gns_turn_actor_child",
                "--nocapture",
            ])
            .env("JIGSALL_TURN_ACTOR", "1")
            .env("JIGSALL_TURN_TEST_ADDRESS", address)
            .env("JIGSALL_TURN_INITIAL", initial)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        if relay_only {
            command.env("JIGSALL_TURN_TEST_ONLY", "1");
        }
        if wrong {
            command.env("JIGSALL_TURN_WRONG_PASSWORD", "1");
        }
        let mut child = command.spawn().unwrap();
        let input = child.stdin.take().unwrap();
        let reader = std::io::BufReader::new(child.stdout.take().unwrap());
        let (peer_tx, peer_rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in reader.lines().map_while(Result::ok) {
                if let Some(json) = line.strip_prefix("ACTOR:") {
                    let frame: ActorFrame = serde_json::from_str(json).unwrap();
                    if let ActorFrame::Peer(peer) = frame {
                        let _ = peer_tx.send(peer);
                    } else if output.send((index, frame)).is_err() {
                        break;
                    }
                }
            }
        });
        let peer = peer_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        Self {
            process: Process { child, input },
            peer,
        }
    }
    fn send(&mut self, frame: ActorFrame) {
        writeln!(
            self.process.input,
            "{}",
            serde_json::to_string(&frame).unwrap()
        )
        .unwrap();
        self.process.input.flush().unwrap();
    }
}
#[test]
fn gns_localhost_turn_host_mixed_peers_future_incoming_and_wrong_credentials() {
    let mut fixture = Fixture::start();
    let (tx, rx) = mpsc::channel();
    let mut actors = vec![
        Actor::spawn(0, &fixture.address, false, "A", false, tx.clone()),
        Actor::spawn(1, &fixture.address, false, "A", false, tx.clone()),
        Actor::spawn(2, &fixture.address, true, "A", false, tx.clone()),
    ];
    let host = actors[0].peer;
    actors[1].send(ActorFrame::Connect(host));
    actors[2].send(ActorFrame::Connect(host));
    let mut connected = Vec::new();
    let mut received = std::collections::BTreeSet::new();
    let mut stage = 0;
    let mut stage_time = Instant::now();
    let deadline = Instant::now() + Duration::from_secs(30);
    while stage < 6 {
        assert!(
            Instant::now() < deadline,
            "mixed peers timed out: {connected:?}"
        );
        if let Ok((index, frame)) = rx.recv_timeout(Duration::from_millis(5)) {
            match frame {
                ActorFrame::Signal(to, bytes) => {
                    let from = actors[index].peer;
                    if let Some(target) = actors.iter_mut().find(|a| a.peer == to) {
                        target.send(ActorFrame::Signal(from, bytes));
                    }
                }
                ActorFrame::Connected(peer, relay) => {
                    assert!(!connected.iter().any(|&(i, p, _)| i == index && p == peer));
                    connected.push((index, peer, relay));
                }
                ActorFrame::Received(peer) => {
                    received.insert((index, peer));
                }
                ActorFrame::Rotated => {}
                other => panic!("unexpected {other:?}"),
            }
        }
        if stage == 0 && connected.len() == 4 {
            assert!(
                !connected.iter().find(|&&(i, _, _)| i == 1).unwrap().2,
                "direct route must win with TURN configured"
            );
            assert!(connected.iter().find(|&&(i, _, _)| i == 2).unwrap().2);
            for actor in &mut actors {
                actor.send(ActorFrame::RejectEndpointChange);
                actor.send(ActorFrame::Rotate);
            }
            stage = 1;
            stage_time = Instant::now();
        }
        if stage == 1 && stage_time.elapsed() > Duration::from_secs(1) {
            actors.push(Actor::spawn(
                3,
                &fixture.address,
                true,
                "B",
                false,
                tx.clone(),
            ));
            actors[3].send(ActorFrame::Connect(host));
            stage = 2;
        }
        if stage == 2 && connected.len() == 6 {
            for actor in &mut actors {
                actor.send(ActorFrame::Exchange);
            }
            stage = 3;
        }
        if stage == 3 && received.len() >= 6 {
            actors.push(Actor::spawn(
                4,
                &fixture.address,
                true,
                "B",
                true,
                tx.clone(),
            ));
            actors[4].send(ActorFrame::Connect(host));
            stage = 4;
            stage_time = Instant::now();
        }
        if stage == 4 && stage_time.elapsed() > Duration::from_secs(2) {
            received.clear();
            for actor in &mut actors[..4] {
                actor.send(ActorFrame::Exchange);
            }
            stage = 5;
        }
        if stage == 5
            && received
                .iter()
                .filter(|&&(i, p)| i < 4 && p != actors[4].peer)
                .count()
                >= 6
        {
            stage = 6;
        }
    }
    assert_eq!(
        connected
            .iter()
            .filter(|&&(i, p, _)| i < 4 && p != actors[4].peer)
            .count(),
        6
    );
    fixture.send("stats");
    let stats = fixture.stats.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(stats["allocate_b"].as_u64().unwrap() > 0);
    assert!(stats["bad_auth"].as_u64().unwrap() > 0);
    assert_eq!(stats["wrong_allocations"].as_u64().unwrap(), 0);
    assert!(stats["refresh_a"].as_u64().unwrap() > 0);
    assert_eq!(stats["wrong_credentials"].as_u64().unwrap(), 0);
    for actor in &mut actors {
        actor.send(ActorFrame::Stop);
    }
    for actor in &mut actors {
        assert!(actor.process.child.wait().unwrap().success());
    }
}

#[test]
fn gns_localhost_turn_absent_initially_rejects_late_install_and_preserves_direct_ice() {
    let mut fixture = Fixture::start();
    let (tx, rx) = mpsc::channel();
    let mut actors = vec![
        Actor::spawn(0, &fixture.address, false, "", false, tx.clone()),
        Actor::spawn(1, &fixture.address, false, "", false, tx.clone()),
    ];
    let host = actors[0].peer;
    actors[1].send(ActorFrame::Connect(host));
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut connected = 0;
    let mut rotated = 0;
    let mut received = std::collections::BTreeSet::new();
    let mut stage = 0;
    while stage < 3 {
        assert!(
            Instant::now() < deadline,
            "direct-only regression timed out"
        );
        if let Ok((index, frame)) = rx.recv_timeout(Duration::from_millis(5)) {
            match frame {
                ActorFrame::Signal(to, bytes) => {
                    let from = actors[index].peer;
                    actors
                        .iter_mut()
                        .find(|a| a.peer == to)
                        .unwrap()
                        .send(ActorFrame::Signal(from, bytes));
                }
                ActorFrame::Connected(_, relay) => {
                    assert!(!relay);
                    connected += 1;
                    assert!(connected <= 2);
                }
                ActorFrame::Rotated => rotated += 1,
                ActorFrame::Received(peer) => {
                    received.insert((index, peer));
                }
                other => panic!("unexpected {other:?}"),
            }
        }
        if stage == 0 && connected == 2 {
            for actor in &mut actors {
                actor.send(ActorFrame::Rotate);
            }
            stage = 1;
        }
        if stage == 1 && rotated == 2 {
            for actor in &mut actors {
                actor.send(ActorFrame::Exchange);
            }
            stage = 2;
        }
        if stage == 2 && received.len() == 2 {
            stage = 3;
        }
    }
    fixture.send("stats");
    let stats = fixture.stats.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(
        stats["allocate_a"].as_u64().unwrap() + stats["allocate_b"].as_u64().unwrap(),
        0
    );
    for actor in &mut actors {
        actor.send(ActorFrame::Stop);
    }
    for actor in &mut actors {
        assert!(actor.process.child.wait().unwrap().success());
    }
}

#[test]
fn gns_localhost_turn_fixture_rejects_allocation_credential_changes() {
    let mut fixture = Fixture::start();
    let python = if cfg!(windows) { "python" } else { "python3" };
    let status = Command::new(python)
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/check_turn_credentials.py"
        ))
        .arg(&fixture.address)
        .status()
        .unwrap();
    assert!(
        status.success(),
        "TURN allocation credential binding failed"
    );
    fixture.send("stats");
    let stats = fixture.stats.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(stats["wrong_credentials"].as_u64().unwrap(), 2);
    assert_eq!(stats["allocate_b"].as_u64().unwrap(), 0);
    assert_eq!(stats["refresh_a"].as_u64().unwrap(), 2);
    assert_eq!(stats["permission_a"].as_u64().unwrap(), 1);
}

/// Drives real native ICE with TURN control messages supplied by the adapter test.
#[cfg(feature = "rendezvous")]
pub(in crate::network::gns) fn exercise_turn_default_expiry(
    mailbox: SignalingEndpoint,
    mut control: impl FnMut(&str, &str) -> u64,
) {
    // Both endpoints must use allocations for A/C: a one-sided TURN path can
    // let native ICE discover and select a direct peer-reflexive route. The
    // direct-only peer still connects outside the fixture while TURN is disabled.
    let mut fixture = Fixture::start_with_relay_pairs(true);
    let mut host =
        GnsP2p::with_signaling(P2P_VIRTUAL_PORT, IceConfig::default(), mailbox.clone()).unwrap();
    let host_peer = host.peer_id().to_bytes();
    control(&fixture.address, "A");
    host.apply_turn_update().unwrap();
    let (tx, rx) = mpsc::channel();
    let mut actors = vec![Actor::spawn(
        0,
        &fixture.address,
        true,
        "A",
        false,
        tx.clone(),
    )];
    let authorize = |actor: &Actor| {
        mailbox
            .authorize_peer(
                PeerId::from_bytes(actor.peer),
                RouteOrigin::from_authenticated_route([1; 16], [2; 16], actor.peer),
            )
            .unwrap();
    };
    authorize(&actors[0]);
    actors[0].send(ActorFrame::Connect(host_peer));
    let mut connected = std::collections::BTreeSet::new();
    let mut host_received = std::collections::BTreeSet::new();
    let mut actor_received = std::collections::BTreeSet::new();
    let mut stage = 0;
    let mut expiry = 0;
    let mut allocation_requests = 0;
    let mut exchanged = false;
    let deadline = Instant::now() + Duration::from_secs(30);
    while stage < 4 {
        assert!(
            Instant::now() < deadline,
            "TURN default expiry/recovery timed out at {stage}"
        );
        if let Ok((index, frame)) = rx.recv_timeout(Duration::from_millis(5)) {
            match frame {
                ActorFrame::Signal(to, bytes) => {
                    assert_eq!(to, host_peer);
                    mailbox
                        .receive(PeerId::from_bytes(actors[index].peer), &bytes)
                        .unwrap();
                }
                ActorFrame::Connected(peer, relay) => {
                    assert_eq!(peer, host_peer);
                    assert!(connected.insert(index), "existing ICE handle was replaced");
                    assert_eq!(
                        relay,
                        index != 1,
                        "actor {index} at expiry stage {stage} selected an unexpected ICE route"
                    );
                }
                ActorFrame::Received(peer) => {
                    assert_eq!(peer, host_peer);
                    actor_received.insert(index);
                }
                other => panic!("unexpected {other:?}"),
            }
        }
        let mut events = Vec::new();
        host.poll(&mut events).unwrap();
        for event in events {
            match event {
                TransportEvent::Connected { connection } => {
                    let peer = host.remote_peer(connection).unwrap().to_bytes();
                    let index = actors.iter().position(|a| a.peer == peer).unwrap();
                    host.connections[&connection]
                        .native
                        .assert_turn_user(["user-A", "", "user-C"][index]);
                    host.activate_secure_channel(connection).unwrap();
                    host.mark_ready(connection).unwrap();
                }
                TransportEvent::Message {
                    connection,
                    payload,
                    ..
                } => {
                    assert_eq!(payload, test_messages()[0].1);
                    host_received.insert(host.remote_peer(connection).unwrap().to_bytes());
                }
                other => panic!("host connection failed: {other:?}"),
            }
        }
        while let Some(signal) = mailbox.pop_outbound() {
            actors
                .iter_mut()
                .find(|a| a.peer == signal.peer.to_bytes())
                .unwrap()
                .send(ActorFrame::Signal(host_peer, signal.payload));
        }
        // Incoming A and direct-only handles must retain their snapshots at every stage.
        for connection in host.connections.values().filter(|c| c.connected) {
            let index = actors
                .iter()
                .position(|a| a.peer == connection.peer.to_bytes())
                .unwrap();
            connection
                .native
                .assert_turn_user(["user-A", "", "user-C"][index]);
        }
        if stage == 0
            && connected.len() == 1
            && host.connections.len() == 1
            && host.connections.values().all(|c| c.ready)
        {
            expiry = control(&fixture.address, "B");
            host.apply_turn_update().unwrap();
            actors[0].send(ActorFrame::Exchange);
            stage = 1;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        if stage == 1 && now >= expiry && host_received.len() == 1 {
            control(&fixture.address, "Disable");
            host.apply_turn_update().unwrap();
            assert_eq!(host.turn_addresses, [fixture.address.clone()]);
            fixture.send("stats");
            allocation_requests = fixture.stats.recv_timeout(Duration::from_secs(2)).unwrap()
                ["allocate_requests"]
                .as_u64()
                .unwrap();
            actors.push(Actor::spawn(
                1,
                &fixture.address,
                false,
                "",
                false,
                tx.clone(),
            ));
            authorize(&actors[1]);
            actors[1].send(ActorFrame::Connect(host_peer));
            stage = 2;
        }
        if stage == 2
            && connected.len() == 2
            && host.connections.len() == 2
            && host.connections.values().all(|c| c.ready)
        {
            fixture.send("stats");
            let stats = fixture.stats.recv_timeout(Duration::from_secs(2)).unwrap();
            assert_eq!(
                stats["allocate_requests"].as_u64().unwrap(),
                allocation_requests,
                "direct-only peer2 must not send even an unauthenticated TURN Allocate"
            );
            assert_eq!(stats["allocate_b"].as_u64().unwrap(), 0);
            control(&fixture.address, "C");
            host.apply_turn_update().unwrap();
            assert_eq!(host.turn_addresses, [fixture.address.clone()]);
            actors.push(Actor::spawn(
                2,
                &fixture.address,
                true,
                "C",
                false,
                tx.clone(),
            ));
            authorize(&actors[2]);
            actors[2].send(ActorFrame::Connect(host_peer));
            stage = 3;
        }
        if stage == 3
            && connected.len() == 3
            && host.connections.len() == 3
            && host.connections.values().all(|c| c.ready)
        {
            // Exchange on every stable handle after recovery, including peer1(A)/peer2(direct).
            if !exchanged {
                host_received.clear();
                for actor in &mut actors {
                    actor.send(ActorFrame::Exchange);
                }
                for connection in host.connections.keys().copied().collect::<Vec<_>>() {
                    host.send(connection, MessageClass::Control, &test_messages()[0].1)
                        .unwrap();
                }
                exchanged = true;
            }
            if actor_received.len() == 3 && host_received.len() == 3 {
                stage = 4;
            }
        }
    }
    assert_eq!(host.connections.len(), 3);
    fixture.send("stats");
    let stats = fixture.stats.recv_timeout(Duration::from_secs(2)).unwrap();
    for metric in [
        "allocate_a",
        "refresh_a",
        "allocate_c",
        "permission_c",
        "relayed",
    ] {
        assert!(stats[metric].as_u64().unwrap() > 0, "{metric}: {stats}");
    }
    for metric in [
        "allocate_b",
        "refresh_b",
        "permission_b",
        "wrong_credentials",
    ] {
        assert_eq!(stats[metric].as_u64().unwrap(), 0, "{metric}: {stats}");
    }
    for actor in &mut actors {
        actor.send(ActorFrame::Stop);
    }
    for actor in &mut actors {
        assert!(actor.process.child.wait().unwrap().success());
    }
}

#[test]
fn gns_localhost_turn_queued_initial_expiry_retains_topology_for_recovery() {
    let mut backend =
        GnsP2p::new_unverified_for_test(P2P_VIRTUAL_PORT, IceConfig::default()).unwrap();
    let mailbox = backend.signaling();
    let servers = |version| {
        vec![TurnServer {
            address: "127.0.0.1:9".into(),
            username: format!("user-{version}"),
            password: format!("password-{version}"),
        }]
    };
    mailbox.install_turn(servers("A"));
    mailbox.disable_turn(vec!["127.0.0.1:9".into()]);
    backend.poll(&mut Vec::new()).unwrap();
    assert_eq!(backend.turn_addresses, ["127.0.0.1:9"]);
    let connection = backend
        .connect_peer(PeerId::from_bytes([7; 16]), P2P_VIRTUAL_PORT)
        .unwrap();
    backend.connections[&connection].native.assert_turn_user("");
    // The direct-only handle must not prevent re-enabling future defaults.
    mailbox.install_turn(servers("C"));
    backend.poll(&mut Vec::new()).unwrap();
    assert!(backend.pending_turn.is_none());
    backend.connections[&connection].native.assert_turn_user("");
    // An invalid Set remains pending, deferring new handles without changing this one.
    let mut changed = servers("D");
    changed[0].address = "127.0.0.1:10".into();
    mailbox.install_turn(changed);
    backend.poll(&mut Vec::new()).unwrap();
    assert_eq!(
        backend.connect_peer(PeerId::from_bytes([8; 16]), P2P_VIRTUAL_PORT),
        Err(TransportError::Backpressure)
    );
    assert_eq!(backend.connections.len(), 1);
    mailbox.disable_turn(vec!["127.0.0.1:9".into()]);
    backend.poll(&mut Vec::new()).unwrap();
    assert!(backend.pending_turn.is_none());
    backend.connections[&connection].native.assert_turn_user("");
}
