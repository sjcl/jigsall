use super::*;
/// Cross-repository test. Start the actual server binary separately, then set
/// JIGSALL_RENDEZVOUS_SMOKE_URL=ws://127.0.0.1:8080/v1/ws. CI stays standalone.
#[test]
#[ignore = "requires the real puzzella-rendezvous binary on loopback"]
fn gns_localhost_real_rendezvous_native_ice_password_secure_lanes() {
    let url = std::env::var("JIGSALL_RENDEZVOUS_SMOKE_URL").expect("set loopback rendezvous URL");
    super::super::super::rendezvous::EndpointUrl::loopback_for_test(&url).unwrap();
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
            .env("JIGSALL_P2P_RENDEZVOUS", &url)
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
                    if tx
                        .send((index, serde_json::from_str::<Frame>(json).unwrap()))
                        .is_err()
                    {
                        break;
                    }
                } else if !line.is_empty() {
                    eprintln!("rendezvous child {index}: {line}");
                }
            }
        });
        peers.push(Process { child, input });
    }
    let deadline = Instant::now() + Duration::from_secs(40);
    let mut connected = [0; 2];
    let mut authenticated = [false; 2];
    let mut received = [false; 2];
    let mut control_gone = [false; 2];
    let mut closed = [false; 2];
    let mut exchanging = false;
    let mut stopping = false;
    let mut second_exchange = false;
    let mut closing = false;
    while !closed.into_iter().all(|x| x) {
        assert!(Instant::now() < deadline, "rendezvous smoke timed out");
        for peer in &mut peers {
            assert!(
                peer.child.try_wait().unwrap().is_none(),
                "smoke child exited early"
            );
        }
        if let Ok((index, frame)) = rx.recv_timeout(Duration::from_millis(5)) {
            match frame {
                Frame::Peer(_) => {}
                Frame::RoomCode(code) => {
                    assert_eq!(index, 1);
                    peers[0].send(Frame::StartCode(code));
                }
                Frame::Connected(details) => {
                    connected[index] += 1;
                    assert!(details.contains("ICE"), "native ICE required: {details}");
                    eprintln!("real rendezvous P2P {index}: {details}");
                }
                Frame::Authenticated => authenticated[index] = true,
                Frame::Received => received[index] = true,
                Frame::ControlGone => control_gone[index] = true,
                Frame::Closed => closed[index] = true,
                other => panic!("unexpected smoke frame: {other:?}"),
            }
        }
        if !exchanging && authenticated.into_iter().all(|x| x) {
            for peer in &mut peers {
                peer.send(Frame::Exchange);
            }
            exchanging = true;
        }
        if !stopping && received.into_iter().all(|x| x) {
            for peer in &mut peers {
                peer.send(Frame::ControlStop);
            }
            stopping = true;
            received = [false; 2];
        }
        if stopping && !second_exchange && control_gone.into_iter().all(|x| x) {
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
    assert!(authenticated.into_iter().all(|x| x));
    assert!(second_exchange);
    for peer in &mut peers {
        peer.send(Frame::Finish);
    }
    for peer in &mut peers {
        assert!(peer.child.wait().unwrap().success());
    }
    eprintln!("real server: native ICE + SPAKE2 + encrypted Control/Transient/Bulk passed before and after WebSocket shutdown");
}
