use super::*;
use crate::network::{
    lifecycle::MAX_CONNECTING,
    rate_limit::{InboundRateLimiter, PREAUTH_INBOUND_POLICY},
    secure::record_limit,
    wire,
};
use ::gns::SendFlags;
use std::{
    net::Ipv4Addr,
    time::{Duration, Instant},
};

// Capacity fixtures have no native owners or sockets. Invalid handle zero
// cannot alias a live native connection, and Drop has nothing to close.
fn capacity_fixture(total: usize, connecting: usize, pending: usize) -> GnsDirectIp {
    let mut backend = GnsDirectIp::new().unwrap();
    for state in super::super::tests::states(total, connecting, pending) {
        let id = ConnectionId::new(token().unwrap());
        backend.connections.insert(
            id,
            Connection {
                native: GnsConnection::from_raw(0),
                endpoint: Endpoint::Listener(ListenerId::new(0)),
                state,
                origin: Origin::Ip(Ipv4Addr::LOCALHOST.into()),
            },
        );
    }
    backend
}

pub(in crate::network::gns) fn capacity_for_test(
    total: usize,
    connecting: usize,
    pending: usize,
) -> bool {
    let mut backend = capacity_fixture(total, connecting, pending);
    let capacity = backend.has_connection_capacity();
    if !capacity {
        assert_eq!(
            backend.connect("127.0.0.1:9".parse().unwrap()),
            Err(TransportError::Capacity)
        );
        assert_eq!(backend.connections.len(), total);
        assert!(backend.sockets.is_empty());
    }
    capacity
}

#[test]
fn gns_localhost_incoming_capacity_refusals_preserve_close_reason() {
    for (total, connecting, pending) in [
        (lifecycle::MAX_CONNECTIONS, 0, 0),
        (MAX_CONNECTING, MAX_CONNECTING, MAX_CONNECTING),
        (
            lifecycle::MAX_PENDING_CONNECTIONS,
            0,
            lifecycle::MAX_PENDING_CONNECTIONS,
        ),
    ] {
        let mut host = capacity_fixture(total, connecting, pending);
        let listener = host.listen("127.0.0.1:0".parse().unwrap()).unwrap();
        let mut client = GnsDirectIp::new().unwrap();
        let outgoing = client
            .connect(host.listener_address(listener).unwrap())
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            assert!(
                Instant::now() < deadline,
                "timeout waiting for capacity refusal"
            );
            assert!(poll(&mut host).is_empty());
            client.global.poll_callbacks();
            let connection = &client.connections[&outgoing];
            let Socket::Client(socket) = &client.sockets[&connection.endpoint] else {
                unreachable!()
            };
            let info = socket.get_connection_info(connection.native).unwrap();
            if info.state() == State::k_ESteamNetworkingConnectionState_ClosedByPeer {
                assert_eq!(
                    info.end_reason(),
                    close_code(DisconnectReason::JoinCapacity)
                );
                break;
            }
            std::thread::park_timeout(Duration::from_millis(1));
        }
        assert_eq!(host.connections.len(), total);
        assert!(host.native_ids.is_empty());
    }
}

#[test]
fn gns_localhost_authentication_and_ready_preserve_admission_occupancy() {
    let mut host = GnsDirectIp::with_rate_policy(&TEST_POLICY).unwrap();
    let mut client = GnsDirectIp::new().unwrap();
    let (_, incoming, _) = connect_pair_unauthenticated(&mut host, &mut client);
    let origin = host.origin(incoming).unwrap();
    let extra_pending = lifecycle::MAX_PENDING_PER_ORIGIN - 1;
    let mut extra = capacity_fixture(extra_pending, 0, extra_pending);
    host.connections.append(&mut extra.connections);
    let origin_pending = |backend: &GnsDirectIp| {
        backend
            .connections
            .values()
            .filter(|c| c.origin == origin && !c.state.ready)
            .count()
    };
    let pending = origin_pending(&host);
    assert_eq!(pending, lifecycle::MAX_PENDING_PER_ORIGIN);
    assert_eq!(
        host.admission.admit(Some(origin), pending, Instant::now()),
        Err(DisconnectReason::JoinCapacity)
    );
    let native = host.connections[&incoming].native;
    host.connections.get_mut(&incoming).unwrap().native = GnsConnection::from_raw(0);
    assert!(matches!(
        host.activate_secure_channel(incoming),
        Err(TransportError::Backend(_))
    ));
    assert!(!host.connections[&incoming].state.authenticated);
    host.connections.get_mut(&incoming).unwrap().native = native;
    host.activate_secure_channel(incoming).unwrap();
    assert_eq!(origin_pending(&host), pending);
    assert_eq!(
        host.activate_secure_channel(incoming),
        Err(TransportError::ProtocolViolation)
    );
    let state = &mut host.connections.get_mut(&incoming).unwrap().state;
    let now = Instant::now();
    for _ in 0..2 {
        assert_eq!(
            state.limiter.check(MessageClass::Control, 1, now),
            crate::network::rate_limit::RateDecision::Allow
        );
    }
    assert_eq!(
        state.limiter.check(MessageClass::Control, 1, now),
        crate::network::rate_limit::RateDecision::Disconnect
    );
    host.mark_ready(incoming).unwrap();
    host.mark_ready(incoming).unwrap();
    let pending = origin_pending(&host);
    assert_eq!(pending, extra_pending);
    host.admission
        .admit(Some(origin), pending, Instant::now())
        .unwrap();
    assert_eq!(
        host.mark_ready(ConnectionId::new(0)),
        Err(TransportError::UnknownConnection)
    );
    assert_eq!(
        host.activate_secure_channel(ConnectionId::new(0)),
        Err(TransportError::UnknownConnection)
    );
}

#[test]
fn native_invalid_message_cleanup_preserves_origin_cooldown() {
    let sink = UdpSocket::bind("127.0.0.1:0").unwrap();
    let mut backend = GnsDirectIp::new().unwrap();
    let now = Instant::now();
    let origin = Origin::Ip(Ipv4Addr::LOCALHOST.into());
    let mut events = Vec::new();
    for _ in 0..lifecycle::COOLDOWN_FAILURES {
        backend.admission.admit(Some(origin), 0, now).unwrap();
        let id = backend.connect(sink.local_addr().unwrap()).unwrap();
        backend.terminate_at(id, DisconnectReason::InvalidMessage, &mut events, now);
        assert!(!backend.connections.contains_key(&id));
        // A later native callback must not count the same failure again.
        backend.terminate_at(id, DisconnectReason::InvalidMessage, &mut events, now);
    }
    assert_eq!(events.len(), lifecycle::COOLDOWN_FAILURES as usize);
    assert_eq!(
        backend.admission.admit(Some(origin), 0, now),
        Err(DisconnectReason::RateLimited)
    );
    assert_eq!(
        backend.admission.admit(
            Some(origin),
            0,
            now + lifecycle::ORIGIN_COOLDOWN - Duration::from_nanos(1)
        ),
        Err(DisconnectReason::RateLimited)
    );
    backend
        .admission
        .admit(Some(origin), 0, now + lifecycle::ORIGIN_COOLDOWN)
        .unwrap();
    assert!(backend.native_ids.is_empty());
    assert!(backend.sockets.is_empty());
}
#[test]
fn gns_connecting_slots_expire_with_explicit_time_without_sleep() {
    let sink = UdpSocket::bind("127.0.0.1:0").unwrap();
    let mut backend = GnsDirectIp::new().unwrap();
    let mut ids = Vec::new();
    for _ in 0..MAX_CONNECTING {
        ids.push(backend.connect(sink.local_addr().unwrap()).unwrap());
    }
    assert_eq!(
        backend.activate_secure_channel(ids[0]),
        Err(TransportError::NotConnected)
    );
    assert_eq!(
        backend.connect(sink.local_addr().unwrap()),
        Err(TransportError::Capacity)
    );
    let at = backend
        .connections
        .values()
        .map(|c| c.state.created)
        .max()
        .unwrap()
        + CONNECTING_TIMEOUT;
    let mut events = Vec::new();
    backend.expire_connecting(at, &mut events);
    assert_eq!(events.len(), MAX_CONNECTING);
    assert!(events.iter().all(|e| matches!(
        e,
        TransportEvent::ConnectionFailed {
            reason: DisconnectReason::BackendConnectionTimeout,
            ..
        }
    )));
    assert!(backend.connections.is_empty());
    assert!(backend.native_ids.is_empty());
    assert!(backend.sockets.is_empty());
    backend.connect(sink.local_addr().unwrap()).unwrap();
}

fn native_state(transport: &GnsDirectIp, id: ConnectionId) -> Option<State> {
    let connection = &transport.connections[&id];
    transport.sockets[&connection.endpoint].connection_state(connection.native)
}

#[test]
fn gns_localhost_pending_state_wins_over_connecting_timeout() {
    for process_callbacks in [true, false] {
        for closed in [false, true] {
            let mut host = GnsDirectIp::new().unwrap();
            let mut client = GnsDirectIp::new().unwrap();
            let listener = host
                .listen(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
                .unwrap();
            let outgoing = client
                .connect(host.listener_address(listener).unwrap())
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(15);
            let mut incoming = None;
            // Poll the host/global callbacks, leaving the client's real
            // Connected callback queued and its application state unchanged.
            while incoming.is_none()
                || native_state(&client, outgoing)
                    != Some(State::k_ESteamNetworkingConnectionState_Connected)
            {
                assert!(Instant::now() < deadline, "timeout connecting UDP loopback");
                for event in poll(&mut host) {
                    let TransportEvent::Connected { connection } = event else {
                        panic!("unexpected host event: {event:?}");
                    };
                    incoming = Some(connection);
                }
                std::thread::park_timeout(Duration::from_millis(1));
            }
            if closed {
                host.close(incoming.unwrap(), DisconnectReason::Requested)
                    .unwrap();
                while native_state(&client, outgoing)
                    != Some(State::k_ESteamNetworkingConnectionState_ClosedByPeer)
                {
                    assert!(Instant::now() < deadline, "timeout closing UDP loopback");
                    client.global.poll_callbacks();
                    std::thread::park_timeout(Duration::from_millis(1));
                }
            }
            let now = Instant::now();
            let connection = client.connections.get_mut(&outgoing).unwrap();
            assert!(!connection.state.connected);
            connection.state.created = now - CONNECTING_TIMEOUT - Duration::from_secs(1);
            let origin = connection.origin;
            // A false timeout would be the third failure and trigger cooldown.
            for _ in 0..lifecycle::COOLDOWN_FAILURES - 1 {
                client.admission.penalize(Some(origin), now);
            }
            let mut events = Vec::new();
            if process_callbacks {
                client.poll(&mut events).unwrap();
            } else {
                // Also exercise expiry while callbacks remain queued, as
                // happens beyond the per-socket CALLBACK_BATCH limit.
                client.expire_connecting(now, &mut events);
            }
            assert!(!events.iter().any(|event| matches!(
                event,
                TransportEvent::ConnectionFailed {
                    reason: DisconnectReason::BackendConnectionTimeout,
                    ..
                }
            )));
            if closed {
                assert!(events.iter().any(|event| matches!(
                    event,
                    TransportEvent::ConnectionFailed {
                        connection,
                        reason: DisconnectReason::RemoteClosed,
                    } | TransportEvent::Disconnected {
                        connection,
                        reason: DisconnectReason::RemoteClosed,
                    } if *connection == outgoing
                )));
                assert!(!client.connections.contains_key(&outgoing));
                assert!(client.native_ids.is_empty());
                assert!(client.sockets.is_empty());
            } else {
                assert_eq!(
                    events,
                    vec![TransportEvent::Connected {
                        connection: outgoing,
                    }]
                );
                assert!(client.connections[&outgoing].state.connected);
                assert!(!client.connections[&outgoing].state.authenticated);
                assert!(!client.connections[&outgoing].state.ready);
                assert_eq!(client.native_ids.len(), 1);
                assert_eq!(client.sockets.len(), 1);
                // The queued callback must not emit Connected a second time.
                assert!(poll(&mut client).is_empty());
            }
            client.admission.admit(Some(origin), 0, now).unwrap();
        }
    }
}

#[test]
fn gns_poll_stalled_connecting_still_times_out() {
    let sink = UdpSocket::bind("127.0.0.1:0").unwrap();
    let mut backend = GnsDirectIp::new().unwrap();
    let id = backend.connect(sink.local_addr().unwrap()).unwrap();
    backend.connections.get_mut(&id).unwrap().state.created = Instant::now() - CONNECTING_TIMEOUT;
    assert_eq!(
        poll(&mut backend),
        vec![TransportEvent::ConnectionFailed {
            connection: id,
            reason: DisconnectReason::BackendConnectionTimeout,
        }]
    );
    assert!(backend.connections.is_empty());
    assert!(backend.native_ids.is_empty());
    assert!(backend.sockets.is_empty());
}

fn poll(transport: &mut GnsDirectIp) -> Vec<TransportEvent> {
    let mut events = Vec::new();
    transport.poll(&mut events).unwrap();
    events
}

fn connect_pair(
    host: &mut GnsDirectIp,
    client: &mut GnsDirectIp,
) -> (ListenerId, ConnectionId, ConnectionId) {
    let pair = connect_pair_unauthenticated(host, client);
    // Raw lane/drain fixtures exercise the post-auth transport policy.
    host.activate_secure_channel(pair.1).unwrap();
    client.activate_secure_channel(pair.2).unwrap();
    pair
}

fn connect_pair_unauthenticated(
    host: &mut GnsDirectIp,
    client: &mut GnsDirectIp,
) -> (ListenerId, ConnectionId, ConnectionId) {
    let listener = host
        .listen(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .unwrap();
    let outgoing = client
        .connect(host.listener_address(listener).unwrap())
        .unwrap();
    let mut incoming = None;
    let mut client_connected = false;
    let deadline = Instant::now() + Duration::from_secs(15);
    while incoming.is_none() || !client_connected {
        assert!(Instant::now() < deadline, "timeout connecting UDP loopback");
        for event in poll(host) {
            match event {
                TransportEvent::Connected { connection } => incoming = Some(connection),
                _ => panic!("unexpected host event: {event:?}"),
            }
        }
        for event in poll(client) {
            assert_eq!(
                event,
                TransportEvent::Connected {
                    connection: outgoing
                }
            );
            client_connected = true;
        }
        std::thread::park_timeout(Duration::from_millis(1));
    }
    (listener, incoming.unwrap(), outgoing)
}

#[test]
fn gns_localhost_authenticated_send_rate_matches_bulk_budget() {
    use crate::network::lifecycle::BULK_BYTES_PER_SECOND;
    use ::gns::sys::ESteamNetworkingConfigValue;
    let mut host = GnsDirectIp::new().unwrap();
    let mut client = GnsDirectIp::new().unwrap();
    let (_, incoming, outgoing) = connect_pair(&mut host, &mut client);
    for (transport, id) in [(&host, incoming), (&client, outgoing)] {
        let native = transport.connections[&id].native;
        for option in [
            ESteamNetworkingConfigValue::k_ESteamNetworkingConfig_SendRateMin,
            ESteamNetworkingConfigValue::k_ESteamNetworkingConfig_SendRateMax,
        ] {
            assert_eq!(
                transport
                    .global
                    .utils()
                    .get_connection_config_value(native, option)
                    .unwrap(),
                ::gns::GnsConfigValue::Int32(BULK_BYTES_PER_SECOND as i32)
            );
        }
    }
}

fn send_burst(sender: &mut GnsDirectIp, id: ConnectionId, count: u32) {
    for sequence in 0..count {
        sender
            .send(id, MessageClass::Control, &sequence.to_le_bytes())
            .unwrap();
    }
    flush_and_wait_for_reliable_ack(sender, id);
}

fn flush_and_wait_for_reliable_ack(sender: &GnsDirectIp, id: ConnectionId) {
    let connection = &sender.connections[&id];
    let socket = &sender.sockets[&connection.endpoint];
    match socket {
        Socket::Server(s) => s.flush_messages_on_connection(connection.native),
        Socket::Client(s) => s.flush_messages_on_connection(connection.native),
    }
    .unwrap();
    // Wait for native reliable ACKs without polling/dequeuing the receiver.
    // Native service threads advance delivery. A fixed sleep would make the
    // exact per-poll assertions depend on machine timing instead of backlog.
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let (_, lanes) = match socket {
            Socket::Server(s) => s.get_connection_real_time_status(connection.native, 3),
            Socket::Client(s) => s.get_connection_real_time_status(connection.native, 3),
        }
        .unwrap();
        if lanes.iter().all(|lane| {
            lane.pending_bytes_reliable() == 0 && lane.bytes_sent_unacked_reliable() == 0
        }) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "timeout waiting for native reliable ACKs"
        );
        std::thread::park_timeout(Duration::from_millis(1));
    }
}

// Native RELIABLE is intentional even on lane 0: the drop/budget assertions
// require a deterministic delivered backlog, independent of UDP loss. The
// production Transient send flags remain unreliable (covered below).
fn native_burst(sender: &GnsDirectIp, id: ConnectionId, native_lane: u16, count: u32, size: usize) {
    let connection = &sender.connections[&id];
    let socket = &sender.sockets[&connection.endpoint];
    for sequence in 0..count {
        let mut payload = vec![0; size];
        payload[..4].copy_from_slice(&sequence.to_le_bytes());
        let message = sender
            .global
            .utils()
            .allocate_message(connection.native, SendFlags::RELIABLE, payload)
            .set_lane(native_lane);
        socket.send(message).unwrap();
    }
    flush_and_wait_for_reliable_ack(sender, id);
}

const TEST_BUCKET: crate::network::rate_limit::BucketPolicy =
    crate::network::rate_limit::BucketPolicy {
        bytes_per_second: 0,
        burst_bytes: 128,
        minimum_charge: 64,
    };
const TEST_POLICY: InboundRatePolicy = InboundRatePolicy {
    transient: TEST_BUCKET,
    control: TEST_BUCKET,
    bulk: TEST_BUCKET,
};
// Keep the production burst/minimum charge, but never earn extra handshake
// credit while native delivery or pre-auth barriers delay this fixture.
const TEST_PREAUTH_POLICY: InboundRatePolicy = InboundRatePolicy {
    control: crate::network::rate_limit::BucketPolicy {
        bytes_per_second: 0,
        ..PREAUTH_INBOUND_POLICY.control
    },
    ..PREAUTH_INBOUND_POLICY
};

// These fixtures test draining, not rate limits: keep their 513-message
// backlog below capacity while retaining production minimum charges.
const DRAIN_POLICY: InboundRatePolicy = InboundRatePolicy {
    control: crate::network::rate_limit::BucketPolicy {
        burst_bytes: 64 * 1024 * 1024,
        ..DEFAULT_INBOUND_POLICY.control
    },
    ..DEFAULT_INBOUND_POLICY
};

#[test]
fn gns_localhost_empty_reliable_messages_are_rejected_before_and_after_authentication() {
    use crate::network::{
        auth::AuthenticatedSecret,
        secure::{ChannelRole, SecureTransport},
    };
    for authenticated in [false, true] {
        for outgoing_receive in [false, true] {
            for class in [
                MessageClass::Control,
                MessageClass::Bulk,
                MessageClass::Transient,
            ] {
                let mut host = GnsDirectIp::new().unwrap();
                let mut client = GnsDirectIp::new().unwrap();
                let (_, incoming, outgoing) = connect_pair_unauthenticated(&mut host, &mut client);
                let (receiver, receiver_id, receiver_role, sender, sender_id) = if outgoing_receive
                {
                    (client, outgoing, ChannelRole::Client, host, incoming)
                } else {
                    (host, incoming, ChannelRole::Host, client, outgoing)
                };
                let mut receiver = SecureTransport::new(receiver);
                receiver.start_connection(receiver_id);
                if authenticated {
                    receiver
                        .install(
                            receiver_id,
                            AuthenticatedSecret::fixture(&[7; 16]),
                            receiver_role,
                        )
                        .unwrap();
                }
                // Send raw reliable data even on Transient so every recognized
                // lane exercises native zero-size delivery without UDP loss.
                let connection = &sender.connections[&sender_id];
                let message = sender
                    .global
                    .utils()
                    .allocate_message(connection.native, SendFlags::RELIABLE, Vec::<u8>::new())
                    .set_lane(lane(class));
                sender.sockets[&connection.endpoint].send(message).unwrap();
                flush_and_wait_for_reliable_ack(&sender, sender_id);
                let mut events = Vec::new();
                receiver.poll(&mut events).unwrap();
                assert_eq!(
                    events,
                    vec![TransportEvent::Disconnected {
                        connection: receiver_id,
                        reason: if authenticated {
                            DisconnectReason::ProtocolViolation
                        } else {
                            DisconnectReason::InvalidMessage
                        },
                    }],
                    "authenticated={authenticated}, outgoing={outgoing_receive}, class={class:?}"
                );
                events.clear();
                receiver.poll(&mut events).unwrap();
                assert!(events.is_empty(), "disconnect must be emitted only once");
                assert!(!receiver
                    .backend_mut()
                    .connections
                    .contains_key(&receiver_id));
            }
        }
    }
}

#[test]
fn gns_localhost_reliable_release_preserves_original_payload_metadata() {
    use std::sync::{Arc, Mutex};

    #[repr(C)]
    struct ProbePayload {
        bytes: [u8; 1024],
        len: usize,
        released: Arc<Mutex<Vec<usize>>>,
    }
    // SAFETY: repr(C) puts the byte array at offset zero, so the data
    // pointer also identifies the owned Box. Reconstruct the allocation
    // independently of len, recording the callback argument for assertions.
    unsafe impl ::gns::Payload for ProbePayload {
        fn into_raw(self) -> (*mut u8, usize) {
            let len = self.len;
            (Box::into_raw(Box::new(self)).cast(), len)
        }
        unsafe fn from_raw(ptr: *mut u8, len: usize) -> Self {
            let payload = *unsafe { Box::from_raw(ptr.cast::<Self>()) };
            payload.released.lock().unwrap().push(len);
            payload
        }
    }
    fn released_lengths(released: &Arc<Mutex<Vec<usize>>>) -> Vec<usize> {
        // Native ACK counters can reach zero before deferred release runs.
        // Wait for the payload's Arc to drop, without holding its mutex.
        let deadline = Instant::now() + Duration::from_secs(15);
        while Arc::strong_count(released) != 1 {
            assert!(
                Instant::now() < deadline,
                "timeout waiting for payload release"
            );
            std::thread::park_timeout(Duration::from_millis(1));
        }
        released.lock().unwrap().clone()
    }

    for outgoing_receive in [false, true] {
        let mut host = GnsDirectIp::new().unwrap();
        let mut client = GnsDirectIp::new().unwrap();
        let (_, incoming, outgoing) = connect_pair(&mut host, &mut client);
        let (sender, sender_id, receiver, receiver_id) = if outgoing_receive {
            (&host, incoming, &mut client, outgoing)
        } else {
            (&client, outgoing, &mut host, incoming)
        };
        for size in [0, 1, 31, 32, 255, 1024] {
            let released = Arc::new(Mutex::new(Vec::new()));
            let payload = ProbePayload {
                bytes: [0x42; 1024],
                len: size,
                released: Arc::clone(&released),
            };
            let connection = &sender.connections[&sender_id];
            let message = sender
                .global
                .utils()
                .allocate_message(connection.native, SendFlags::RELIABLE, payload)
                .set_lane(lane(MessageClass::Control))
                .set_user_data(0xDEAD_BEEF);
            assert_eq!(message.payload(), vec![0x42; size]);
            assert_eq!(message.user_data(), 0xDEAD_BEEF);
            sender.sockets[&connection.endpoint].send(message).unwrap();
            flush_and_wait_for_reliable_ack(sender, sender_id);
            assert_eq!(
                released_lengths(&released),
                vec![size],
                "release must receive the original length exactly once"
            );
            assert_eq!(Arc::strong_count(&released), 1);
            assert_eq!(
                poll(receiver),
                vec![TransportEvent::Message {
                    connection: receiver_id,
                    class: MessageClass::Control,
                    payload: vec![0x42; size],
                }]
            );
        }
    }

    // The local pipe transfers the original message to the receiver without
    // adding an SNP header, even though the reliable flag is set.
    let backend = GnsDirectIp::new().unwrap();
    let (sender, receiver) = GnsSocket::new(backend.global).socket_pair(false).unwrap();
    let released = Arc::new(Mutex::new(Vec::new()));
    for size in [0, 32] {
        let message = backend.global.utils().allocate_message(
            sender.connection(),
            SendFlags::RELIABLE,
            ProbePayload {
                bytes: [0x42; 1024],
                len: size,
                released: Arc::clone(&released),
            },
        );
        sender.send_message(message).unwrap();
    }
    let received: Vec<_> = receiver
        .receive_messages::<2>()
        .unwrap()
        .map(|message| message.payload().to_vec())
        .collect();
    assert_eq!(received, vec![vec![], vec![0x42; 32]]);
    assert_eq!(released_lengths(&released), vec![0, 32]);
    assert_eq!(Arc::strong_count(&released), 1);
}

#[test]
fn gns_localhost_preauth_oversize_is_rejected_before_copy() {
    for outgoing_receive in [false, true] {
        for size in [
            crate::network::wire::HEADER_SIZE
                + crate::network::wire::MAX_SESSION_CONTROL_PAYLOAD
                + 1,
            record_limit(MessageClass::Control),
        ] {
            let mut host = GnsDirectIp::new().unwrap();
            let mut client = GnsDirectIp::new().unwrap();
            let (_, incoming, outgoing) = connect_pair_unauthenticated(&mut host, &mut client);
            let (receiver, receiver_id, sender, sender_id) = if outgoing_receive {
                (&mut client, outgoing, &mut host, incoming)
            } else {
                (&mut host, incoming, &mut client, outgoing)
            };
            let payload = plaintext_control(size);
            sender
                .send(sender_id, MessageClass::Control, &payload)
                .unwrap();
            flush_and_wait_for_reliable_ack(sender, sender_id);
            let events = poll(receiver);
            assert!(
                matches!(
                    events.as_slice(),
                    [TransportEvent::Disconnected {
                        connection,
                        reason: DisconnectReason::InvalidMessage,
                    }] if *connection == receiver_id
                ),
                "oversized pre-auth data must be rejected without a Message event"
            );
            assert!(poll(receiver).is_empty());
        }
    }
}

fn plaintext_control(size: usize) -> Vec<u8> {
    let mut payload = vec![0; size];
    payload[..4].copy_from_slice(b"PZLA");
    payload[4..6].copy_from_slice(&wire::WIRE_VERSION.to_le_bytes());
    payload[6] = 6;
    payload[8..12].copy_from_slice(&((size - wire::HEADER_SIZE) as u32).to_le_bytes());
    payload
}

#[test]
fn gns_localhost_preauth_gate_checks_actual_length_header_and_lane() {
    let maximum = plaintext_control(wire::HEADER_SIZE + wire::MAX_SESSION_CONTROL_PAYLOAD);
    for outgoing_receive in [false, true] {
        let mut cases = vec![(MessageClass::Control, maximum.clone(), true)];
        let mut oversized = maximum.clone();
        oversized.push(0); // Keep the declared length within the cap.
        cases.push((MessageClass::Control, oversized, false));
        let mut gameplay = maximum.clone();
        gameplay[6] = 1;
        cases.push((MessageClass::Control, gameplay, false));
        let small = plaintext_control(wire::HEADER_SIZE + 1);
        cases.push((MessageClass::Transient, small.clone(), false));
        cases.push((MessageClass::Bulk, small, false));
        cases.push((MessageClass::Control, vec![0; 4], false));
        for (class, payload, allowed) in cases {
            let mut host = GnsDirectIp::new().unwrap();
            let mut client = GnsDirectIp::new().unwrap();
            let (_, incoming, outgoing) = connect_pair_unauthenticated(&mut host, &mut client);
            let (receiver, receiver_id, sender, sender_id) = if outgoing_receive {
                (&mut client, outgoing, &mut host, incoming)
            } else {
                (&mut host, incoming, &mut client, outgoing)
            };
            // Use a reliable native backlog even on Transient, so delivery
            // of wrong-lane attack frames does not depend on UDP timing/loss.
            let connection = &sender.connections[&sender_id];
            let message = sender
                .global
                .utils()
                .allocate_message(connection.native, SendFlags::RELIABLE, payload.clone())
                .set_lane(lane(class));
            sender.sockets[&connection.endpoint].send(message).unwrap();
            flush_and_wait_for_reliable_ack(sender, sender_id);
            let expected = if allowed {
                TransportEvent::Message {
                    connection: receiver_id,
                    class,
                    payload,
                }
            } else {
                TransportEvent::Disconnected {
                    connection: receiver_id,
                    reason: DisconnectReason::InvalidMessage,
                }
            };
            assert_eq!(poll(receiver), vec![expected]);
        }
    }
}

#[test]
fn gns_localhost_preauth_tail_stays_native_until_verified_installation() {
    use crate::network::{
        auth::AuthenticatedSecret,
        secure::{ChannelRole, SecureTransport},
    };
    for outgoing_receive in [false, true] {
        let mut host = GnsDirectIp::new().unwrap();
        let mut client = GnsDirectIp::new().unwrap();
        let (_, incoming, outgoing) = connect_pair_unauthenticated(&mut host, &mut client);
        let (receiver, receiver_id, receiver_role, sender, sender_id, sender_role) =
            if outgoing_receive {
                (
                    client,
                    outgoing,
                    ChannelRole::Client,
                    host,
                    incoming,
                    ChannelRole::Host,
                )
            } else {
                (
                    host,
                    incoming,
                    ChannelRole::Host,
                    client,
                    outgoing,
                    ChannelRole::Client,
                )
            };
        let mut sender = SecureTransport::new(sender);
        let mut receiver = SecureTransport::new(receiver);
        sender.start_connection(sender_id);
        receiver.start_connection(receiver_id);
        let handshake = plaintext_control(wire::HEADER_SIZE + 1);
        sender
            .send(sender_id, MessageClass::Control, &handshake)
            .unwrap();
        flush_and_wait_for_reliable_ack(sender.backend_mut(), sender_id);
        sender
            .install(
                sender_id,
                AuthenticatedSecret::fixture(&[7; 16]),
                sender_role,
            )
            .unwrap();
        let control = vec![0x42; wire::frame_limit(MessageClass::Control)];
        let bulk = vec![0x24; wire::frame_limit(MessageClass::Bulk)];
        sender
            .send(sender_id, MessageClass::Control, &control)
            .unwrap();
        // The maximum Control record occupies the Bulk preflight budget.
        // Drain its native ACK without polling/dequeuing the receiver tail.
        flush_and_wait_for_reliable_ack(sender.backend_mut(), sender_id);
        sender.send(sender_id, MessageClass::Bulk, &bulk).unwrap();
        flush_and_wait_for_reliable_ack(sender.backend_mut(), sender_id);
        let mut events = Vec::new();
        receiver.poll(&mut events).unwrap();
        assert_eq!(
            events,
            vec![TransportEvent::Message {
                connection: receiver_id,
                class: MessageClass::Control,
                payload: handshake,
            }]
        );
        assert!(
            !receiver.backend_mut().connections[&receiver_id]
                .state
                .authenticated
        );
        receiver
            .install(
                receiver_id,
                AuthenticatedSecret::fixture(&[7; 16]),
                receiver_role,
            )
            .unwrap();
        events.clear();
        receiver.poll(&mut events).unwrap();
        assert_eq!(events.len(), 2);
        assert!(events.contains(&TransportEvent::Message {
            connection: receiver_id,
            class: MessageClass::Control,
            payload: control,
        }));
        assert!(events.contains(&TransportEvent::Message {
            connection: receiver_id,
            class: MessageClass::Bulk,
            payload: bulk,
        }));
        assert!(
            receiver.backend_mut().connections[&receiver_id]
                .state
                .authenticated
        );
    }
}

#[test]
fn gns_localhost_preauth_barrier_keeps_tail_native_and_rechecks_or_closes() {
    for close_listener in [false, true] {
        let mut host = GnsDirectIp::new().unwrap();
        let mut client = GnsDirectIp::new().unwrap();
        let (listener, incoming, outgoing) = connect_pair_unauthenticated(&mut host, &mut client);
        let handshake = plaintext_control(wire::HEADER_SIZE + 1);
        client
            .send(outgoing, MessageClass::Control, &handshake)
            .unwrap();
        let oversized =
            plaintext_control(wire::HEADER_SIZE + wire::MAX_SESSION_CONTROL_PAYLOAD + 1);
        for _ in 0..RECEIVE_CHUNK {
            client
                .send(outgoing, MessageClass::Control, &oversized)
                .unwrap();
        }
        flush_and_wait_for_reliable_ack(&client, outgoing);
        assert_eq!(
            poll(&mut host),
            vec![TransportEvent::Message {
                connection: incoming,
                class: MessageClass::Control,
                payload: handshake,
            }]
        );
        assert!(!host.connections[&incoming].state.authenticated);
        if close_listener {
            host.close_listener(listener).unwrap();
        }
        assert_eq!(
            poll(&mut host),
            vec![TransportEvent::Disconnected {
                connection: incoming,
                reason: if close_listener {
                    DisconnectReason::Requested
                } else {
                    DisconnectReason::InvalidMessage
                },
            }]
        );
        assert!(!host.connections.contains_key(&incoming));
        assert!(poll(&mut host).is_empty());
    }
}

#[test]
fn gns_localhost_transient_drop_preserves_connection_and_receive_budget() {
    let mut host = GnsDirectIp::with_rate_policy(&TEST_POLICY).unwrap();
    let mut client = GnsDirectIp::new().unwrap();
    let (_, incoming, outgoing) = connect_pair(&mut host, &mut client);
    // No SessionConnections/PlayerId assignment: post-auth protection is active.
    // Malformed wire bytes are still charged before any routing or decode.
    native_burst(&client, outgoing, lane(MessageClass::Transient), 513, 4);
    let events = poll(&mut host);
    assert_eq!(
        events,
        (0_u32..2)
            .map(|sequence| TransportEvent::Message {
                connection: incoming,
                class: MessageClass::Transient,
                payload: sequence.to_le_bytes().to_vec(),
            })
            .collect::<Vec<_>>()
    );
    assert!(host.connections[&incoming].state.connected);
    // 510 rejected messages consumed budget; message 513 remains native-owned.
    let connection = &host.connections[&incoming];
    let socket = &host.sockets[&connection.endpoint];
    let mut slots = [MessageSlot::uninit()];
    let remaining = socket.receive(&mut slots).unwrap();
    assert_eq!(remaining.len(), 1);
    for message in remaining {
        assert_eq!(message.payload(), &512_u32.to_le_bytes());
    }
    assert!(poll(&mut host).is_empty());

    // Further over-limit data generates no event, and Control still passes.
    native_burst(&client, outgoing, lane(MessageClass::Transient), 3, 4);
    assert!(poll(&mut host).is_empty());
    send_burst(&mut client, outgoing, 1);
    assert_eq!(receive_sequences(&mut host), vec![(incoming, 0)]);
    assert!(poll(&mut client).is_empty());

    let mut other = GnsDirectIp::new().unwrap();
    let (_, other_incoming, other_outgoing) = connect_pair(&mut host, &mut other);
    native_burst(&other, other_outgoing, lane(MessageClass::Transient), 1, 4);
    assert_eq!(
        poll(&mut host),
        vec![TransportEvent::Message {
            connection: other_incoming,
            class: MessageClass::Transient,
            payload: 0_u32.to_le_bytes().to_vec(),
        }]
    );
}

#[test]
fn gns_localhost_reliable_excess_disconnects_incoming_and_outgoing_once() {
    for class in [MessageClass::Control, MessageClass::Bulk] {
        for outgoing_receive in [false, true] {
            let mut host = GnsDirectIp::with_rate_policy(&TEST_POLICY).unwrap();
            let mut client = GnsDirectIp::with_rate_policy(&TEST_POLICY).unwrap();
            let (_, incoming, outgoing) = connect_pair(&mut host, &mut client);
            let (receiver, receiver_id, sender, sender_id) = if outgoing_receive {
                (&mut client, outgoing, &mut host, incoming)
            } else {
                (&mut host, incoming, &mut client, outgoing)
            };
            native_burst(sender, sender_id, lane(class), 20, 4);
            let mut expected: Vec<_> = (0_u32..2)
                .map(|sequence| TransportEvent::Message {
                    connection: receiver_id,
                    class,
                    payload: sequence.to_le_bytes().to_vec(),
                })
                .collect();
            expected.push(TransportEvent::Disconnected {
                connection: receiver_id,
                reason: DisconnectReason::RateLimited,
            });
            assert_eq!(poll(receiver), expected);
            assert!(!receiver.connections.contains_key(&receiver_id));
            assert!(poll(receiver).is_empty());
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                let events = poll(sender);
                if !events.is_empty() {
                    assert_eq!(
                        events,
                        vec![TransportEvent::Disconnected {
                            connection: sender_id,
                            reason: DisconnectReason::RemoteClosed,
                        }]
                    );
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "timeout waiting for rate-limited close"
                );
                std::thread::park_timeout(Duration::from_millis(1));
            }
        }
    }
}

#[test]
fn gns_localhost_oversize_keeps_invalid_message_reason_before_rate_limit() {
    for class in [
        MessageClass::Transient,
        MessageClass::Control,
        MessageClass::Bulk,
    ] {
        let mut host = GnsDirectIp::with_rate_policy(&TEST_POLICY).unwrap();
        let mut client = GnsDirectIp::new().unwrap();
        let (_, incoming, outgoing) = connect_pair(&mut host, &mut client);
        native_burst(&client, outgoing, lane(class), 1, record_limit(class) + 1);
        assert_eq!(
            poll(&mut host),
            vec![TransportEvent::Disconnected {
                connection: incoming,
                reason: DisconnectReason::InvalidMessage,
            }]
        );
        assert!(poll(&mut host).is_empty());
    }
}

#[test]
fn gns_localhost_inbound_decisions_match_p2p_receive_loop() {
    let handshake = plaintext_control(wire::HEADER_SIZE + 1);
    let maximum = plaintext_control(wire::HEADER_SIZE + wire::MAX_SESSION_CONTROL_PAYLOAD);
    let mut trailing = maximum.clone();
    trailing.push(0);
    let mut gameplay = handshake.clone();
    gameplay[6] = 1;
    // (authenticated, class, payload, count, allowed count, disconnect)
    let mut cases = vec![
        (false, MessageClass::Control, maximum, 1, 1, None),
        (
            false,
            MessageClass::Control,
            handshake.clone(),
            9,
            8,
            Some(DisconnectReason::RateLimited),
        ),
    ];
    for payload in [vec![0; 4], trailing, gameplay] {
        cases.push((
            false,
            MessageClass::Control,
            payload,
            1,
            0,
            Some(DisconnectReason::InvalidMessage),
        ));
    }
    for class in [
        MessageClass::Transient,
        MessageClass::Control,
        MessageClass::Bulk,
    ] {
        if class != MessageClass::Control {
            cases.push((
                false,
                class,
                handshake.clone(),
                1,
                0,
                Some(DisconnectReason::InvalidMessage),
            ));
        }
        for authenticated in [false, true] {
            cases.push((
                authenticated,
                class,
                vec![0; record_limit(class) + 1],
                1,
                0,
                Some(DisconnectReason::InvalidMessage),
            ));
        }
        cases.push((
            true,
            class,
            vec![0; 4],
            3,
            2,
            (class != MessageClass::Transient).then_some(DisconnectReason::RateLimited),
        ));
    }
    for (authenticated, class, payload, count, allowed, disconnect) in cases {
        let mut host = GnsDirectIp::with_rate_policy(&TEST_POLICY).unwrap();
        let mut client = GnsDirectIp::new().unwrap();
        let (_, incoming, outgoing) = connect_pair_unauthenticated(&mut host, &mut client);
        if authenticated {
            host.activate_secure_channel(incoming).unwrap();
        }
        let policy = if authenticated {
            &TEST_POLICY
        } else {
            &TEST_PREAUTH_POLICY
        };
        host.connections.get_mut(&incoming).unwrap().state.limiter =
            InboundRateLimiter::with_policy(policy, Instant::now());
        let connection = &client.connections[&outgoing];
        for _ in 0..count {
            let message = client
                .global
                .utils()
                .allocate_message(connection.native, SendFlags::RELIABLE, payload.clone())
                .set_lane(lane(class));
            client.sockets[&connection.endpoint].send(message).unwrap();
        }
        flush_and_wait_for_reliable_ack(&client, outgoing);
        let mut direct_events = Vec::new();
        for _ in 0..=count {
            host.poll(&mut direct_events).unwrap();
        }
        let p2p_events = super::super::p2p::tests::inbound_events(
            incoming,
            authenticated,
            policy,
            lane(class),
            &payload,
            count,
        );
        let mut expected = vec![
            TransportEvent::Message {
                connection: incoming,
                class,
                payload,
            };
            allowed
        ];
        if let Some(reason) = disconnect {
            expected.push(TransportEvent::Disconnected {
                connection: incoming,
                reason,
            });
        }
        assert_eq!(
            direct_events, expected,
            "Direct-IP {authenticated} {class:?}"
        );
        assert_eq!(p2p_events, expected, "P2P {authenticated} {class:?}");
    }
}

fn receive_sequences(transport: &mut GnsDirectIp) -> Vec<(ConnectionId, u32)> {
    poll(transport)
        .into_iter()
        .map(|event| {
            let TransportEvent::Message {
                connection,
                class,
                payload,
            } = event
            else {
                panic!("unexpected event while draining: {event:?}");
            };
            assert_eq!(class, MessageClass::Control);
            (connection, u32::from_le_bytes(payload.try_into().unwrap()))
        })
        .collect()
}

#[test]
fn lane_and_flag_policy() {
    assert_eq!(close_code(DisconnectReason::Requested), 1000);
    assert_eq!(close_code(DisconnectReason::InvalidMessage), 1001);
    assert_eq!(close_code(DisconnectReason::BackendFailure), 1002);
    assert_eq!(close_code(DisconnectReason::RateLimited), 1003);
    assert_eq!(lane(MessageClass::Transient), 0);
    assert_eq!(lane(MessageClass::Control), 1);
    assert_eq!(lane(MessageClass::Bulk), 2);
    assert_eq!(class(3), None);
    assert_eq!(class(u16::MAX), None);
    assert!(flags(MessageClass::Transient).contains(SendFlags::NO_NAGLE | SendFlags::NO_DELAY));
    assert!(!flags(MessageClass::Transient).contains(SendFlags::RELIABLE));
    assert!(flags(MessageClass::Control).contains(SendFlags::RELIABLE));
    assert!(flags(MessageClass::Bulk).contains(SendFlags::RELIABLE));
    assert!(LANES[2].priority > LANES[1].priority);
}

#[test]
fn gns_localhost_receive_budget_is_shared_and_preserves_partial_chunk() {
    let mut host = GnsDirectIp::with_rate_policy(&DRAIN_POLICY).unwrap();
    let mut clients: [_; 3] =
        std::array::from_fn(|_| GnsDirectIp::with_rate_policy(&DRAIN_POLICY).unwrap());
    let pairs = clients
        .each_mut()
        .map(|client| connect_pair(&mut host, client));
    for (index, client) in clients.iter_mut().enumerate() {
        send_burst(client, pairs[index].2, [300, 400, 400][index]);
    }
    let first = receive_sequences(&mut host);
    // All listeners take turns within one shared budget.
    assert_eq!(first.len(), 512);
    let counts: [usize; 3] =
        std::array::from_fn(|index| first.iter().filter(|(id, _)| *id == pairs[index].1).count());
    assert_eq!(counts, [192, 160, 160]);
    let second = receive_sequences(&mut host);
    // Resume at listener B, not A. A's last short chunk (12) eventually
    // leaves only 20 slots in the budget; the rest must remain queued.
    assert_eq!(second.len(), 512);
    assert_eq!(second[0].0, pairs[1].1);
    let third = receive_sequences(&mut host);
    assert_eq!(third.len(), 76);
    assert!(receive_sequences(&mut host).is_empty());
    for (index, (_, incoming, _)) in pairs.into_iter().enumerate() {
        let sequences: Vec<_> = first
            .iter()
            .chain(&second)
            .chain(&third)
            .filter_map(|&(id, sequence)| (id == incoming).then_some(sequence))
            .collect();
        assert_eq!(sequences, (0..[300, 400, 400][index]).collect::<Vec<_>>());
    }
}

#[test]
fn gns_localhost_outgoing_socket_drains_and_server_close_removes_once() {
    let mut host = GnsDirectIp::with_rate_policy(&DRAIN_POLICY).unwrap();
    let mut client = GnsDirectIp::with_rate_policy(&DRAIN_POLICY).unwrap();
    let (listener, incoming, outgoing) = connect_pair(&mut host, &mut client);
    send_burst(&mut host, incoming, 513);
    let first = receive_sequences(&mut client);
    assert_eq!(
        first,
        (0..512)
            .map(|sequence| (outgoing, sequence))
            .collect::<Vec<_>>()
    );
    assert_eq!(receive_sequences(&mut client), vec![(outgoing, 512)]);
    assert!(receive_sequences(&mut client).is_empty());

    host.close(incoming, DisconnectReason::Requested).unwrap();
    assert_eq!(
        host.close(incoming, DisconnectReason::Requested),
        Err(TransportError::UnknownConnection)
    );
    assert_eq!(
        poll(&mut host),
        vec![TransportEvent::Disconnected {
            connection: incoming,
            reason: DisconnectReason::Requested,
        }]
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let events = poll(&mut client);
        if !events.is_empty() {
            assert_eq!(
                events,
                vec![TransportEvent::Disconnected {
                    connection: outgoing,
                    reason: DisconnectReason::RemoteClosed,
                }]
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "timeout waiting for server close"
        );
        std::thread::park_timeout(Duration::from_millis(1));
    }
    assert_eq!(
        client.close(outgoing, DisconnectReason::Requested),
        Err(TransportError::UnknownConnection)
    );
    assert!(poll(&mut host).is_empty());
    assert!(poll(&mut client).is_empty());
    host.close_listener(listener).unwrap();
}
