//! Opt-in cross-repository, two-process native runtime entrypoint test.
use super::*;
use serde::{Deserialize, Serialize};
use std::{
    io::{BufRead, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
};
#[derive(Debug, Serialize, Deserialize)]
enum Frame {
    Code(String),
    Join(String),
    Ready,
    Grab,
    Held,
    Release,
    Released,
    Leave,
    Left,
    Finish,
}
fn emit(frame: Frame) {
    let mut out = std::io::stdout().lock();
    writeln!(out, "RUNTIME:{}", serde_json::to_string(&frame).unwrap()).unwrap();
    out.flush().unwrap();
}
struct Process {
    child: Child,
    input: ChildStdin,
}
impl Process {
    fn send(&mut self, frame: Frame) {
        writeln!(self.input, "{}", serde_json::to_string(&frame).unwrap()).unwrap();
        self.input.flush().unwrap();
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}
#[test]
#[ignore = "requires the real puzzella-rendezvous binary on loopback"]
fn gns_localhost_real_rendezvous_runtime_ready_command_roundtrip() {
    let url = std::env::var("PUZZELLA_RENDEZVOUS_SMOKE_URL").expect("set loopback rendezvous URL");
    EndpointUrl::loopback_for_test(&url).unwrap();
    let (tx, rx) = mpsc::sync_channel(64);
    let mut processes = Vec::new();
    for (id, role) in ["host", "client"].into_iter().enumerate() {
        let mut child=Command::new(std::env::current_exe().unwrap()).args(["--exact","network::runtime::rendezvous::tests::smoke::gns_localhost_rendezvous_runtime_child","--nocapture"])
            .env("PUZZELLA_RUNTIME_SMOKE_ROLE",role).env("PUZZELLA_RENDEZVOUS_SMOKE_URL",&url)
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn().unwrap();
        let input = child.stdin.take().unwrap();
        let output = child.stdout.take().unwrap();
        let sender = tx.clone();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(output).lines() {
                let Ok(line) = line else { break };
                if let Some(json) = line.strip_prefix("RUNTIME:") {
                    if sender
                        .send((id, serde_json::from_str::<Frame>(json).unwrap()))
                        .is_err()
                    {
                        break;
                    }
                }
            }
        });
        processes.push(Process { child, input });
    }
    let deadline = Instant::now() + std::time::Duration::from_secs(45);
    let mut ready = [false; 2];
    let mut held = [false; 2];
    let mut released = [false; 2];
    let mut left = [false; 2];
    let mut grabbed = false;
    let mut release = false;
    let mut leave = false;
    while !left.into_iter().all(|x| x) {
        assert!(Instant::now() < deadline, "runtime smoke timed out");
        for p in &mut processes {
            assert!(
                p.child.try_wait().unwrap().is_none(),
                "runtime child exited early"
            );
        }
        if let Ok((id, frame)) = rx.recv_timeout(std::time::Duration::from_millis(5)) {
            match frame {
                Frame::Code(code) => {
                    assert_eq!(id, 0);
                    processes[1].send(Frame::Join(code));
                }
                Frame::Ready => ready[id] = true,
                Frame::Held => held[id] = true,
                Frame::Released => released[id] = true,
                Frame::Left => left[id] = true,
                other => panic!("unexpected runtime frame {other:?}"),
            }
        }
        if ready.into_iter().all(|x| x) && !grabbed {
            processes[1].send(Frame::Grab);
            grabbed = true;
        }
        if held.into_iter().all(|x| x) && !release {
            processes[1].send(Frame::Release);
            release = true;
        }
        if released.into_iter().all(|x| x) && !leave {
            for p in &mut processes {
                p.send(Frame::Leave);
            }
            leave = true;
        }
    }
    for p in &mut processes {
        p.send(Frame::Finish);
    }
    for p in &mut processes {
        assert!(p.child.wait().unwrap().success());
    }
    eprintln!("real rendezvous runtime: Room Code -> native GNS -> SPAKE2 -> image/baseline/catch-up -> Ready -> Grab/Release roundtrip -> teardown passed");
}
#[test]
fn gns_localhost_rendezvous_runtime_child() {
    let Ok(role) = std::env::var("PUZZELLA_RUNTIME_SMOKE_ROLE") else {
        return;
    };
    let (tx, rx) = mpsc::sync_channel(16);
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if tx
                .send(serde_json::from_str::<Frame>(&line).unwrap())
                .is_err()
            {
                break;
            }
        }
    });
    let mut app = fixtures::app();
    app.world_mut().insert_resource(RendezvousRuntimeConfig {
        endpoint: EndpointUrl::loopback_for_test(
            &std::env::var("PUZZELLA_RENDEZVOUS_SMOKE_URL").unwrap(),
        )
        .unwrap(),
        ice: IceConfig::default(),
    });
    if role == "host" {
        fixtures::host_world(&mut app);
        start_rendezvous_host(app.world_mut(), host_options()).unwrap();
    }
    let deadline = Instant::now() + std::time::Duration::from_secs(50);
    let mut code = false;
    let mut ready = false;
    let mut held = false;
    let mut released = false;
    let mut leaving = false;
    loop {
        assert!(Instant::now() < deadline, "runtime child timed out");
        while let Ok(frame) = rx.try_recv() {
            match frame {
                Frame::Join(code) => {
                    let mut options = join_options();
                    options.room_code = code.parse().unwrap();
                    start_rendezvous_join(app.world_mut(), options).unwrap();
                }
                Frame::Grab => {
                    fixtures::send(&mut app, PieceCommand::Grab(puzzella_core::PieceId(0)))
                }
                Frame::Release => {
                    fixtures::send(&mut app, PieceCommand::Release(puzzella_core::PieceId(0)))
                }
                Frame::Leave => {
                    stop_session(app.world_mut());
                    assert!(!app.world().contains_non_send::<NetworkSession>());
                    assert!(app.world().resource::<PieceDataStore>().held_by.is_empty());
                    emit(Frame::Left);
                    leaving = true;
                }
                Frame::Finish => return,
                other => panic!("unexpected child frame {other:?}"),
            }
        }
        if !leaving {
            app.update();
            let status = app.world().resource::<NetworkStatus>();
            assert_ne!(status.phase, RuntimePhase::Failed, "{:?}", status.error);
            if role == "host" && !code {
                if let Some(value) = &status.room_code {
                    emit(Frame::Code(value.clone()));
                    code = true;
                }
            }
            if !ready
                && (status.phase == RuntimePhase::Ready
                    || (role == "host"
                        && status
                            .peers
                            .iter()
                            .any(|p| p.state == Some(ConnectionState::Ready))))
            {
                emit(Frame::Ready);
                ready = true;
            }
            if ready {
                let store = app.world().resource::<PieceDataStore>();
                if !held && !store.held_by.is_empty() {
                    emit(Frame::Held);
                    held = true;
                }
                if held && !released && store.held_by.is_empty() {
                    emit(Frame::Released);
                    released = true;
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}
