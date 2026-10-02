use crate::network::{
    rate_limit::{InboundRateLimiter, InboundRatePolicy, RateDecision, DEFAULT_INBOUND_POLICY},
    transport::{
        ConnectionId, DirectIpTransport, DisconnectReason, ListenerId, MessageClass, Transport,
        TransportError, TransportEvent,
    },
    wire::frame_limit,
};
use ::gns::{
    sys::ESteamNetworkingConnectionState as State, GnsConnection, GnsConnectionEvent, GnsGlobal,
    GnsLane, GnsNetworkMessage, GnsSocket, IsClient, IsCreated, IsServer, MessageSlot,
    ReceivedMessagesInto, SendFlags, ToSend,
};
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    net::{SocketAddr, UdpSocket},
    sync::atomic::{AtomicU64, Ordering},
    time::Instant,
};

pub const MAX_CONNECTIONS: usize = 64;
pub const MAX_LISTENERS: usize = 8;
const RECEIVE_CHUNK: usize = 32;
const MAX_RECEIVE_PER_POLL: usize = 512;
const CALLBACK_BATCH: usize = 128;
// The pinned native header specifies lower numbers as higher priority.
const LANES: [GnsLane; 3] = [GnsLane::new(0, 1), GnsLane::new(0, 4), GnsLane::new(1, 1)];
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

fn token() -> Result<u64, TransportError> {
    NEXT_TOKEN
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
        .map_err(|_| TransportError::Capacity)
}
fn backend(error: ::gns::GnsError) -> TransportError {
    TransportError::Backend(error.to_string())
}
fn lane(class: MessageClass) -> u16 {
    match class {
        MessageClass::Transient => 0,
        MessageClass::Control => 1,
        MessageClass::Bulk => 2,
    }
}
fn class(lane: u16) -> Option<MessageClass> {
    match lane {
        0 => Some(MessageClass::Transient),
        1 => Some(MessageClass::Control),
        2 => Some(MessageClass::Bulk),
        _ => None,
    }
}
fn flags(class: MessageClass) -> SendFlags {
    match class {
        MessageClass::Transient => {
            SendFlags::UNRELIABLE | SendFlags::NO_NAGLE | SendFlags::NO_DELAY
        }
        MessageClass::Control | MessageClass::Bulk => SendFlags::RELIABLE,
    }
}

// Local application close codes share one mapping. Outgoing socket Drop uses
// the wrapper's generic code, while its local event retains the precise reason.
fn close_code(reason: DisconnectReason) -> u32 {
    match reason {
        DisconnectReason::Requested => 1000,
        DisconnectReason::InvalidMessage => 1001,
        DisconnectReason::RateLimited => 1003,
        _ => 1002,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Endpoint {
    Listener(ListenerId),
    Outgoing(ConnectionId),
}
enum Socket {
    Server(GnsSocket<IsServer>),
    Client(GnsSocket<IsClient>),
}
impl Socket {
    fn events(&self) -> Vec<GnsConnectionEvent> {
        match self {
            Self::Server(s) => s.receive_events().take(CALLBACK_BATCH).collect(),
            Self::Client(s) => s.receive_events().take(CALLBACK_BATCH).collect(),
        }
    }
    fn configure(&self, connection: GnsConnection) -> Result<(), TransportError> {
        match self {
            Self::Server(s) => s.configure_connection_lanes(connection, &LANES),
            Self::Client(s) => s.configure_connection_lanes(connection, &LANES),
        }
        .map_err(backend)
    }
    fn close_before_removal(
        &self,
        connection: GnsConnection,
        reason: DisconnectReason,
    ) -> Result<(), TransportError> {
        let code = close_code(reason);
        // A listener outlives its individual incoming connections, so close
        // those explicitly. Outgoing sockets own one connection: their wrapper
        // Drop closes it when bookkeeping removes the socket. Do not close twice.
        match self {
            Self::Server(s) => s
                .close_connection(connection, code, None, false)
                .map_err(backend),
            Self::Client(_) => Ok(()),
        }
    }
    fn send(&self, message: GnsNetworkMessage<ToSend>) -> Result<(), TransportError> {
        match self {
            Self::Server(s) => s.send_message(message),
            Self::Client(s) => s.send_message(message),
        }
        .map(|_| ())
        .map_err(backend)
    }
    fn receive<'a>(
        &self,
        slots: &'a mut [MessageSlot],
    ) -> Result<ReceivedMessagesInto<'a>, TransportError> {
        match self {
            Self::Server(s) => s.receive_messages_into(slots),
            Self::Client(s) => s.receive_messages_into(slots),
        }
        .map_err(backend)
    }
}
struct Connection {
    native: GnsConnection,
    endpoint: Endpoint,
    connected: bool,
    rate_limit: InboundRateLimiter,
}

/// Owns listeners/connections; Drop closes all of them, including pending connects.
/// No GNS types appear in Transport, wire, routing or gameplay. Call poll once per
/// frame on the owning thread; no application background worker is required.
pub struct GnsDirectIp {
    global: &'static GnsGlobal,
    sockets: BTreeMap<Endpoint, Socket>,
    connections: BTreeMap<ConnectionId, Connection>,
    native_ids: HashMap<GnsConnection, ConnectionId>,
    pending: Vec<TransportEvent>,
    next_receive_endpoint: Option<Endpoint>,
    rate_policy: &'static InboundRatePolicy,
}

impl GnsDirectIp {
    pub fn new() -> Result<Self, TransportError> {
        Self::with_rate_policy(&DEFAULT_INBOUND_POLICY)
    }
    fn with_rate_policy(rate_policy: &'static InboundRatePolicy) -> Result<Self, TransportError> {
        Ok(Self {
            global: GnsGlobal::get().map_err(backend)?,
            sockets: BTreeMap::new(),
            connections: BTreeMap::new(),
            native_ids: HashMap::new(),
            pending: Vec::new(),
            next_receive_endpoint: None,
            rate_policy,
        })
    }
    /// Remove mappings; dropping an outgoing socket performs its owned close.
    /// Incoming connection bookkeeping never calls the native close function.
    fn forget(&mut self, id: ConnectionId) {
        if let Some(connection) = self.connections.remove(&id) {
            self.native_ids.remove(&connection.native);
            if matches!(connection.endpoint, Endpoint::Outgoing(_)) {
                self.sockets.remove(&connection.endpoint);
            }
        }
    }
    fn terminate(
        &mut self,
        id: ConnectionId,
        reason: DisconnectReason,
        events: &mut Vec<TransportEvent>,
    ) {
        if let Some(connection) = self.connections.get(&id) {
            events.push(if connection.connected {
                TransportEvent::Disconnected {
                    connection: id,
                    reason,
                }
            } else {
                TransportEvent::ConnectionFailed {
                    connection: id,
                    reason,
                }
            });
            if let Some(socket) = self.sockets.get(&connection.endpoint) {
                let _ = socket.close_before_removal(connection.native, reason);
            }
            self.forget(id);
        }
    }
    fn state_change(
        &mut self,
        endpoint: Endpoint,
        event: GnsConnectionEvent,
        events: &mut Vec<TransportEvent>,
    ) -> Result<(), TransportError> {
        let native = event.connection();
        let info = event.info();
        let mut id = self.native_ids.get(&native).copied();
        if info.state() == State::k_ESteamNetworkingConnectionState_Connecting && id.is_none() {
            if let Some(Socket::Server(socket)) = self.sockets.get(&endpoint) {
                if self.connections.len() >= MAX_CONNECTIONS {
                    let _ = socket.close_connection(
                        native,
                        close_code(DisconnectReason::BackendFailure),
                        None,
                        false,
                    );
                    return Ok(());
                }
                let issued = ConnectionId::new(token()?);
                self.connections.insert(
                    issued,
                    Connection {
                        native,
                        endpoint,
                        connected: false,
                        rate_limit: InboundRateLimiter::with_policy(
                            self.rate_policy,
                            Instant::now(),
                        ),
                    },
                );
                self.native_ids.insert(native, issued);
                id = Some(issued);
                if socket
                    .configure_connection_lanes(native, &LANES)
                    .and_then(|()| socket.accept(native))
                    .is_err()
                {
                    self.terminate(issued, DisconnectReason::BackendFailure, events);
                    return Ok(());
                }
            }
        }
        let Some(id) = id else {
            return Ok(());
        };
        match info.state() {
            State::k_ESteamNetworkingConnectionState_Connected => {
                if let Some(connection) = self.connections.get_mut(&id) {
                    if !connection.connected {
                        connection.connected = true;
                        events.push(TransportEvent::Connected { connection: id });
                    }
                }
            }
            State::k_ESteamNetworkingConnectionState_ClosedByPeer => {
                self.terminate(id, DisconnectReason::RemoteClosed, events)
            }
            State::k_ESteamNetworkingConnectionState_ProblemDetectedLocally => {
                self.terminate(id, DisconnectReason::ConnectionProblem, events)
            }
            _ => {}
        }
        Ok(())
    }
}

impl Transport for GnsDirectIp {
    fn poll(&mut self, events: &mut Vec<TransportEvent>) -> Result<(), TransportError> {
        events.append(&mut self.pending);
        self.global.poll_callbacks();
        let callbacks: Vec<_> = self
            .sockets
            .iter()
            .flat_map(|(&endpoint, socket)| {
                socket
                    .events()
                    .into_iter()
                    .map(move |event| (endpoint, event))
            })
            .collect();
        for (endpoint, event) in callbacks {
            self.state_change(endpoint, event, events)?;
        }
        let mut endpoints: VecDeque<_> = self.sockets.keys().copied().collect();
        if let Some(next) = self.next_receive_endpoint {
            let start = endpoints
                .iter()
                .position(|endpoint| *endpoint >= next)
                .unwrap_or(0);
            endpoints.rotate_left(start);
        }
        let mut slots = [const { MessageSlot::uninit() }; RECEIVE_CHUNK];
        let mut remaining = MAX_RECEIVE_PER_POLL;
        let now = Instant::now();
        while remaining != 0 {
            let Some(endpoint) = endpoints.pop_front() else {
                break;
            };
            let Some(socket) = self.sockets.get(&endpoint) else {
                continue;
            };
            // Never receive more than the remaining budget. Unconsumed wrapper
            // messages are released on Drop, which would discard reliable data.
            let messages = socket.receive(&mut slots[..RECEIVE_CHUNK.min(remaining)])?;
            let received = messages.len();
            remaining -= received; // Unknown, invalid and rate-limited messages count too.
            if received != 0 {
                endpoints.push_back(endpoint);
            }
            for message in messages {
                let Some(&id) = self.native_ids.get(&message.connection()) else {
                    continue;
                };
                let Some(connection) = self.connections.get_mut(&id) else {
                    continue;
                };
                if !connection.connected {
                    continue;
                }
                let Some(class) = class(message.lane()) else {
                    self.terminate(id, DisconnectReason::InvalidMessage, events);
                    continue;
                };
                let payload = message.payload();
                if payload.len() > frame_limit(class) {
                    self.terminate(id, DisconnectReason::InvalidMessage, events);
                    continue;
                }
                match connection.rate_limit.check(class, payload.len(), now) {
                    RateDecision::Allow => events.push(TransportEvent::Message {
                        connection: id,
                        class,
                        payload: payload.to_vec(),
                    }),
                    RateDecision::Drop => {}
                    RateDecision::Disconnect => {
                        self.terminate(id, DisconnectReason::RateLimited, events);
                    }
                }
            }
        }
        // Continue with the next socket if budget ran out, rather than letting a
        // busy listener always take precedence over another listener/client.
        self.next_receive_endpoint = endpoints.front().copied();
        Ok(())
    }
    fn send(
        &mut self,
        id: ConnectionId,
        class: MessageClass,
        payload: &[u8],
    ) -> Result<(), TransportError> {
        if payload.len() > frame_limit(class) {
            return Err(TransportError::PayloadTooLarge);
        }
        let connection = self
            .connections
            .get(&id)
            .ok_or(TransportError::UnknownConnection)?;
        if !connection.connected {
            return Err(TransportError::NotConnected);
        }
        let socket = self
            .sockets
            .get(&connection.endpoint)
            .ok_or(TransportError::UnknownConnection)?;
        let message = self
            .global
            .utils()
            .allocate_message(connection.native, flags(class), payload.to_vec())
            .set_lane(lane(class));
        socket.send(message)
    }
    fn close(&mut self, id: ConnectionId, reason: DisconnectReason) -> Result<(), TransportError> {
        let connection = self
            .connections
            .get(&id)
            .ok_or(TransportError::UnknownConnection)?;
        self.sockets
            .get(&connection.endpoint)
            .ok_or(TransportError::UnknownConnection)?
            .close_before_removal(connection.native, reason)?;
        let connected = connection.connected;
        self.forget(id);
        self.pending.push(if connected {
            TransportEvent::Disconnected {
                connection: id,
                reason,
            }
        } else {
            TransportEvent::ConnectionFailed {
                connection: id,
                reason,
            }
        });
        Ok(())
    }
}

impl DirectIpTransport for GnsDirectIp {
    fn listen(&mut self, address: SocketAddr) -> Result<ListenerId, TransportError> {
        if self
            .sockets
            .keys()
            .filter(|e| matches!(e, Endpoint::Listener(_)))
            .count()
            >= MAX_LISTENERS
        {
            return Err(TransportError::Capacity);
        }
        let id = ListenerId::new(token()?);
        let socket = if address.port() == 0 {
            // This bundled GNS version rejects port zero. Ask the OS for a free
            // port, release the reservation, and retry if another bind wins the
            // small handoff race. Never use a fixed test port.
            let mut result = Err(TransportError::Backend("ephemeral listen failed".into()));
            for _ in 0..16 {
                let reservation = UdpSocket::bind(address)
                    .map_err(|error| TransportError::Backend(error.to_string()))?;
                let port = reservation
                    .local_addr()
                    .map_err(|error| TransportError::Backend(error.to_string()))?
                    .port();
                drop(reservation);
                result = GnsSocket::<IsCreated>::new(self.global)
                    .listen(address.ip(), port)
                    .map_err(backend);
                if result.is_ok() {
                    break;
                }
            }
            result?
        } else {
            GnsSocket::<IsCreated>::new(self.global)
                .listen(address.ip(), address.port())
                .map_err(backend)?
        };
        self.sockets
            .insert(Endpoint::Listener(id), Socket::Server(socket));
        Ok(id)
    }
    fn listener_address(&self, listener: ListenerId) -> Result<SocketAddr, TransportError> {
        let Some(Socket::Server(socket)) = self.sockets.get(&Endpoint::Listener(listener)) else {
            return Err(TransportError::UnknownListener);
        };
        let (ip, port) = socket
            .get_listen_socket_address()
            .ok_or_else(|| TransportError::Backend("cannot query listen address".into()))?;
        Ok(SocketAddr::new(ip, port))
    }
    fn close_listener(&mut self, listener: ListenerId) -> Result<(), TransportError> {
        let endpoint = Endpoint::Listener(listener);
        if !self.sockets.contains_key(&endpoint) {
            return Err(TransportError::UnknownListener);
        }
        let ids: Vec<_> = self
            .connections
            .iter()
            .filter(|(_, c)| c.endpoint == endpoint)
            .map(|(&id, _)| id)
            .collect();
        for id in ids {
            self.close(id, DisconnectReason::Requested)?;
        }
        self.sockets.remove(&endpoint);
        Ok(())
    }
    fn connect(&mut self, address: SocketAddr) -> Result<ConnectionId, TransportError> {
        if self.connections.len() >= MAX_CONNECTIONS {
            return Err(TransportError::Capacity);
        }
        let id = ConnectionId::new(token()?);
        let socket = Socket::Client(
            GnsSocket::<IsCreated>::new(self.global)
                .connect(address.ip(), address.port())
                .map_err(backend)?,
        );
        let Socket::Client(client) = &socket else {
            unreachable!()
        };
        let native = client.connection();
        socket.configure(native)?;
        let endpoint = Endpoint::Outgoing(id);
        self.sockets.insert(endpoint, socket);
        self.connections.insert(
            id,
            Connection {
                native,
                endpoint,
                connected: false,
                rate_limit: InboundRateLimiter::with_policy(self.rate_policy, Instant::now()),
            },
        );
        self.native_ids.insert(native, id);
        Ok(id)
    }
}

impl Drop for GnsDirectIp {
    fn drop(&mut self) {
        for connection in self.connections.values() {
            if let Some(socket) = self.sockets.get(&connection.endpoint) {
                let _ = socket.close_before_removal(connection.native, DisconnectReason::Requested);
            }
        }
        self.connections.clear();
        self.sockets.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        net::Ipv4Addr,
        time::{Duration, Instant},
    };

    fn poll(transport: &mut GnsDirectIp) -> Vec<TransportEvent> {
        let mut events = Vec::new();
        transport.poll(&mut events).unwrap();
        events
    }

    fn connect_pair(
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
    fn native_burst(
        sender: &GnsDirectIp,
        id: ConnectionId,
        native_lane: u16,
        count: u32,
        size: usize,
    ) {
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
    fn gns_localhost_transient_drop_preserves_connection_and_receive_budget() {
        let mut host = GnsDirectIp::with_rate_policy(&TEST_POLICY).unwrap();
        let mut client = GnsDirectIp::new().unwrap();
        let (_, incoming, outgoing) = connect_pair(&mut host, &mut client);
        // No SessionConnections/PlayerId assignment: protection is already active.
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
        assert!(host.connections[&incoming].connected);
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
            native_burst(&client, outgoing, lane(class), 1, frame_limit(class) + 1);
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
        let counts: [usize; 3] = std::array::from_fn(|index| {
            first.iter().filter(|(id, _)| *id == pairs[index].1).count()
        });
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
}
