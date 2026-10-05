use super::*;
struct Fixture {
    child: Child,
    input: ChildStdin,
    address: String,
    stats: mpsc::Receiver<serde_json::Value>,
}
impl Fixture {
    fn start() -> Self {
        let python = if cfg!(windows) { "python" } else { "python3" };
        let mut child = Command::new(python)
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/turn_server.py"
            ))
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
fn gns_localhost_turn_live_rotation_stale_nonce_and_reallocation() {
    let mut fixture = Fixture::start();
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
    let mut rotated = [false; 2];
    let mut recovery_started = false;
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
        if rotation_started
            && !rotated.iter().any(|x| *x)
            && stage_time.elapsed() > Duration::from_millis(2200)
        {
            fixture.send("stale");
            for peer in &mut peers {
                peer.send(Frame::Rotate);
            }
            rotated = [true; 2]; // commands sent; retain the connection IDs in children
            received = [false; 2];
            stage_time = Instant::now();
        }
        if rotated.into_iter().all(|x| x)
            && !recovery_started
            && stage_time.elapsed() > Duration::from_secs(2)
        {
            fixture.send("expire_a");
            fixture.send("fail");
            recovery_started = true;
            stage_time = Instant::now();
        }
        if recovery_started && !second_exchange && stage_time.elapsed() > Duration::from_secs(4) {
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
    for metric in [
        "allocate_a",
        "allocate_b",
        "refresh_b",
        "stale",
        "refresh_fail",
        "relayed",
        "held",
    ] {
        assert!(stats[metric].as_u64().unwrap() > 0, "{metric}: {stats}");
    }
    assert!(second_exchange);
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
    let initial = if std::env::var_os("JIGSALL_TURN_INITIAL_B").is_some() {
        "B"
    } else {
        "A"
    };
    let credentials = |version| {
        vec![TurnServer {
            address: address.clone(),
            username: format!("user-{version}"),
            password: format!("password-{version}"),
        }]
    };
    let mut version = initial;
    let mut initial_credentials = credentials(initial);
    if std::env::var_os("JIGSALL_TURN_WRONG_PASSWORD").is_some() {
        initial_credentials[0].password = "wrong".into();
    }
    backend.install_turn(&initial_credentials).unwrap();
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
                    backend.install_turn(&credentials("B")).unwrap();
                    version = "B";
                    for connection in backend.connections.values() {
                        connection.native.assert_turn_user("user-B");
                    }
                    actor_emit(ActorFrame::Rotated);
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
                    backend.connections[&connection]
                        .native
                        .assert_turn_user(&format!("user-{version}"));
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
        initial_b: bool,
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
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        if relay_only {
            command.env("JIGSALL_TURN_TEST_ONLY", "1");
        }
        if initial_b {
            command.env("JIGSALL_TURN_INITIAL_B", "1");
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
        Actor::spawn(0, &fixture.address, false, false, false, tx.clone()),
        Actor::spawn(1, &fixture.address, false, false, false, tx.clone()),
        Actor::spawn(2, &fixture.address, true, false, false, tx.clone()),
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
                actor.send(ActorFrame::Rotate);
            }
            stage = 1;
            stage_time = Instant::now();
        }
        if stage == 1 && stage_time.elapsed() > Duration::from_secs(1) {
            fixture.send("expire_a");
            actors.push(Actor::spawn(
                3,
                &fixture.address,
                true,
                true,
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
                true,
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
    for actor in &mut actors {
        actor.send(ActorFrame::Stop);
    }
    for actor in &mut actors {
        assert!(actor.process.child.wait().unwrap().success());
    }
}
