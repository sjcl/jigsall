use super::*;

#[test]
fn dns_address_parser_accepts_names_and_preserves_literal_endpoints() {
    for value in [
        "example.com:27015",
        " localhost:27015 ",
        "game.example.com.:42",
        "my-pc:65535",
    ] {
        let address: ServerAddress = value.parse().unwrap();
        assert_eq!(address.socket_addr(), None, "{value}");
    }
    for value in ["192.168.1.10:27015", "[2001:db8::1]:27015", " [::1]:27015 "] {
        let address: ServerAddress = value.parse().unwrap();
        assert_eq!(address.socket_addr(), Some(value.trim().parse().unwrap()));
    }
    let name = format!("{}.example:27015", "a".repeat(64));
    let too_long = format!("{}:27015", vec!["a".repeat(63); 4].join("."));
    for value in [
        "",
        "example.com",
        "example.com:0",
        "example.com:65536",
        "example.com:abc",
        ":27015",
        "example.com:+27015",
        "https://example.com:27015",
        "example.com:27015/path",
        "exa mple.com:27015",
        "-example.com:27015",
        "example-.com:27015",
        "example..com:27015",
        "2001:db8::1:27015",
        "[localhost]:27015",
        "999.999.999.999:27015",
        "127.0.0.1",
        "0.0.0.0:27015",
        "[::]:27015",
        "224.0.0.1:27015",
        "[ff02::1]:27015",
        "[::ffff:0.0.0.0]:27015",
        "[::ffff:224.0.0.1]:27015",
        &name,
        &too_long,
    ] {
        assert_eq!(
            value.parse::<ServerAddress>(),
            Err(InvalidAddress),
            "{value}"
        );
    }
}

#[test]
fn dns_answers_prefer_ipv4_preserve_family_order_and_filter_invalid_endpoints() {
    let v4: SocketAddr = "192.0.2.1:27015".parse().unwrap();
    let v6: SocketAddr = "[2001:db8::1]:27015".parse().unwrap();
    assert_eq!(
        choose_address([v6, v4, "192.0.2.2:27015".parse().unwrap()]),
        Ok(v4)
    );
    assert_eq!(
        choose_address([v6, "[2001:db8::2]:27015".parse().unwrap()]),
        Ok(v6)
    );
    let invalid: Vec<SocketAddr> = [
        "0.0.0.0:27015",
        "[::]:27015",
        "224.0.0.1:27015",
        "[ff02::1]:27015",
        "127.0.0.1:0",
    ]
    .map(|value| value.parse().unwrap())
    .into();
    assert_eq!(
        choose_address(invalid.clone()),
        Err(ResolutionError::NoUsableAddress)
    );
    assert_eq!(choose_address(invalid.into_iter().chain([v6])), Ok(v6));
    assert_eq!(choose_address([]), Err(ResolutionError::NoUsableAddress));
}

fn pending() -> (
    crossbeam::channel::Sender<Result<SocketAddr, ResolutionError>>,
    AddressResolution,
) {
    let (sender, receiver) = bounded(1);
    (
        sender,
        AddressResolution {
            receiver: Some(receiver),
            deadline: Instant::now() + RESOLUTION_TIMEOUT,
        },
    )
}

#[test]
fn dns_poll_is_nonblocking_and_delivers_success_or_failure_only_once() {
    for result in [
        Ok("127.0.0.1:27015".parse().unwrap()),
        Err(ResolutionError::Lookup("not found".into())),
        Err(ResolutionError::NoUsableAddress),
    ] {
        let (sender, mut resolution) = pending();
        assert_eq!(resolution.poll(Instant::now()), None);
        sender.send(result.clone()).unwrap();
        assert_eq!(resolution.poll(Instant::now()), Some(result));
        assert_eq!(resolution.poll(Instant::now()), None);
    }
    let (sender, mut resolution) = pending();
    drop(sender);
    assert!(matches!(
        resolution.poll(Instant::now()),
        Some(Err(ResolutionError::Lookup(_)))
    ));
}

#[test]
fn dns_timeout_and_cancel_discard_late_results_without_affecting_new_requests() {
    let (sender, mut resolution) = pending();
    assert_eq!(
        resolution.poll(resolution.deadline),
        Some(Err(ResolutionError::Timeout))
    );
    assert!(sender.send(Ok("127.0.0.1:27015".parse().unwrap())).is_err());
    assert_eq!(resolution.poll(Instant::now()), None);
    let (old_sender, old) = pending();
    drop(old);
    let (new_sender, mut new) = pending();
    assert!(old_sender
        .send(Ok("127.0.0.1:27015".parse().unwrap()))
        .is_err());
    assert_eq!(new.poll(Instant::now()), None);
    let address = "127.0.0.2:27015".parse().unwrap();
    new_sender.send(Ok(address)).unwrap();
    assert_eq!(new.poll(Instant::now()), Some(Ok(address)));
}

#[test]
fn dns_literal_resolution_returns_immediately_without_a_worker() {
    let address = "[::1]:27015".parse::<ServerAddress>().unwrap();
    let socket = address.socket_addr().unwrap();
    let mut resolution = address.resolve().unwrap();
    assert_eq!(resolution.poll(Instant::now()), Some(Ok(socket)));
    assert_eq!(resolution.poll(Instant::now()), None);
}

#[test]
fn dns_worker_can_wait_and_report_failure_without_blocking_frame_polling() {
    let (release, wait) = bounded(1);
    let mut resolution = AddressResolution::spawn(move || {
        wait.recv().unwrap();
        Err(ResolutionError::Lookup("injected lookup failure".into()))
    })
    .unwrap();
    assert_eq!(resolution.poll(Instant::now()), None);
    release.send(()).unwrap();
    let result = loop {
        if let Some(result) = resolution.poll(Instant::now()) {
            break result;
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    assert_eq!(
        result,
        Err(ResolutionError::Lookup("injected lookup failure".into()))
    );
    assert_eq!(resolution.poll(Instant::now()), None);
}

#[test]
fn dns_os_worker_resolves_localhost_with_the_requested_port() {
    let mut resolution = "localhost:27015"
        .parse::<ServerAddress>()
        .unwrap()
        .resolve()
        .unwrap();
    let address = loop {
        if let Some(result) = resolution.poll(Instant::now()) {
            break result.unwrap();
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    assert!(address.ip().is_loopback());
    assert_eq!(address.port(), 27015);
}
