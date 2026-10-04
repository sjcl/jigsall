use super::token;
use crate::network::{
    lifecycle::{
        self, Admission, BULK_BYTES_PER_SECOND, CONNECTING_TIMEOUT, MAX_BULK_QUEUE_BYTES,
        MAX_CONNECTING, MAX_PENDING_CONNECTIONS, MAX_RELIABLE_QUEUE_BYTES,
    },
    rate_limit::{
        InboundRateLimiter, InboundRatePolicy, RateDecision, DEFAULT_INBOUND_POLICY,
        PREAUTH_INBOUND_POLICY,
    },
    secure::record_limit,
    transport::{
        ConnectionId, DirectIpTransport, DisconnectReason, ListenerId, MessageClass, Origin,
        ReliableEgress, Transport, TransportError, TransportEvent,
    },
    wire,
};
use ::gns::{
    sys::{ESteamNetworkingConfigValue, ESteamNetworkingConnectionState as State},
    GnsConfig, GnsConnection, GnsConnectionEvent, GnsGlobal, GnsLane, GnsNetworkMessage, GnsSocket,
    IsClient, IsCreated, IsServer, MessageSlot, ReceivedMessagesInto, SendFlags, ToSend,
};
use std::{
    cell::Cell,
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    net::{SocketAddr, UdpSocket},
    time::Instant,
};

pub use crate::network::lifecycle::MAX_CONNECTIONS;
pub const MAX_LISTENERS: usize = 8;
const RECEIVE_CHUNK: usize = 32;
const MAX_RECEIVE_PER_POLL: usize = 512;
const CALLBACK_BATCH: usize = 128;
// The pinned native header specifies lower numbers as higher priority.
const LANES: [GnsLane; 3] = [GnsLane::new(0, 1), GnsLane::new(0, 4), GnsLane::new(1, 1)];
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
    fn queued_reliable(&self, native: GnsConnection) -> Result<u64, TransportError> {
        let (s, _) = match self {
            Self::Server(s) => s.get_connection_real_time_status(native, 0),
            Self::Client(s) => s.get_connection_real_time_status(native, 0),
        }
        .map_err(backend)?;
        Ok(s.pending_bytes_reliable() as u64 + s.bytes_sent_unacked_reliable() as u64)
    }
    fn egress(&self, native: GnsConnection) -> Result<(u64, u64), TransportError> {
        let (status, lanes) = match self {
            Self::Server(s) => s.get_connection_real_time_status(native, 3),
            Self::Client(s) => s.get_connection_real_time_status(native, 3),
        }
        .map_err(backend)?;
        let total =
            status.pending_bytes_reliable() as u64 + status.bytes_sent_unacked_reliable() as u64;
        let bulk = &lanes[2];
        Ok((
            total,
            bulk.pending_bytes_reliable() as u64 + bulk.bytes_sent_unacked_reliable() as u64,
        ))
    }
    fn connection_state(&self, native: GnsConnection) -> Option<State> {
        match self {
            Self::Server(s) => s.get_connection_info(native),
            Self::Client(s) => s.get_connection_info(native),
        }
        .map(|info| info.state())
    }
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
    authenticated: bool,
    ready: bool,
    created: Instant,
    origin: Origin,
    bulk_enqueued: u64,
    bulk_delivered: Cell<u64>,
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
    admission: Admission,
    next_admission_log: Option<Instant>,
}

impl GnsDirectIp {
    pub fn new() -> Result<Self, TransportError> {
        Self::with_rate_policy(&DEFAULT_INBOUND_POLICY)
    }
    fn with_rate_policy(rate_policy: &'static InboundRatePolicy) -> Result<Self, TransportError> {
        Ok(Self {
            global: super::global()?,
            sockets: BTreeMap::new(),
            connections: BTreeMap::new(),
            native_ids: HashMap::new(),
            pending: Vec::new(),
            next_receive_endpoint: None,
            rate_policy,
            admission: Admission::connections(),
            next_admission_log: None,
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
        self.terminate_at(id, reason, events, Instant::now());
    }
    fn terminate_at(
        &mut self,
        id: ConnectionId,
        reason: DisconnectReason,
        events: &mut Vec<TransportEvent>,
        now: Instant,
    ) {
        if let Some(connection) = self.connections.get(&id) {
            if lifecycle::is_abuse(reason) {
                self.admission.penalize(Some(connection.origin), now);
            }
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
                let now = Instant::now();
                let origin = Origin::Ip(info.remote_address().to_canonical());
                let pending = self.connections.values().filter(|c| !c.ready).count();
                let connecting = self.connections.values().filter(|c| !c.connected).count();
                let origin_pending = self
                    .connections
                    .values()
                    .filter(|c| !c.ready && c.origin == origin)
                    .count();
                let refusal = if self.connections.len() >= MAX_CONNECTIONS
                    || pending >= MAX_PENDING_CONNECTIONS
                    || connecting >= MAX_CONNECTING
                {
                    Some(DisconnectReason::JoinCapacity)
                } else {
                    self.admission
                        .admit(Some(origin), origin_pending, now)
                        .err()
                };
                if let Some(reason) = refusal {
                    if self.next_admission_log.is_none_or(|next| now >= next) {
                        bevy::log::warn!("Direct IP connection admission: {reason:?}");
                        self.next_admission_log = Some(now + std::time::Duration::from_secs(1));
                    }
                    let _ = socket.close_connection(native, close_code(reason), None, false);
                    return Ok(());
                }
                let issued = ConnectionId::new(token()?);
                self.connections.insert(
                    issued,
                    Connection {
                        native,
                        endpoint,
                        connected: false,
                        authenticated: false,
                        ready: false,
                        created: now,
                        origin,
                        bulk_enqueued: 0,
                        bulk_delivered: Cell::new(0),
                        rate_limit: InboundRateLimiter::with_policy(
                            &PREAUTH_INBOUND_POLICY,
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
        self.apply_connection_state(id, info.state(), events);
        Ok(())
    }
    fn apply_connection_state(
        &mut self,
        id: ConnectionId,
        state: State,
        events: &mut Vec<TransportEvent>,
    ) {
        match state {
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
    }
    fn expire_connecting(&mut self, now: Instant, events: &mut Vec<TransportEvent>) {
        let expired: Vec<_> = self
            .connections
            .iter()
            .filter(|(_, c)| {
                !c.connected && now.saturating_duration_since(c.created) >= CONNECTING_TIMEOUT
            })
            .map(|(&id, _)| id)
            .collect();
        for id in expired {
            let connection = &self.connections[&id];
            // A listener's bounded callback batch may leave a completed state
            // transition queued. Inspect only overdue Connecting slots (at most
            // MAX_CONNECTING) before assigning a peer-stall timeout/penalty.
            let state = self
                .sockets
                .get(&connection.endpoint)
                .and_then(|socket| socket.connection_state(connection.native));
            match state {
                Some(
                    state @ (State::k_ESteamNetworkingConnectionState_Connected
                    | State::k_ESteamNetworkingConnectionState_ClosedByPeer
                    | State::k_ESteamNetworkingConnectionState_ProblemDetectedLocally),
                ) => self.apply_connection_state(id, state, events),
                Some(
                    State::k_ESteamNetworkingConnectionState_Connecting
                    | State::k_ESteamNetworkingConnectionState_FindingRoute,
                ) => {
                    self.terminate_at(id, DisconnectReason::BackendConnectionTimeout, events, now);
                }
                _ => self.terminate_at(id, DisconnectReason::BackendFailure, events, now),
            }
        }
    }
}

impl Transport for GnsDirectIp {
    fn origin(&self, id: ConnectionId) -> Option<Origin> {
        self.connections.get(&id).map(|c| c.origin)
    }
    fn reliable_egress(&self, id: ConnectionId) -> Result<ReliableEgress, TransportError> {
        let c = self
            .connections
            .get(&id)
            .ok_or(TransportError::UnknownConnection)?;
        let (queued_bytes, bulk_queued_bytes) = self.sockets[&c.endpoint].egress(c.native)?;
        // Native backlog includes framing: subtraction underestimates delivery.
        // A high-water mark makes this conservative estimate monotonic.
        let delivered = c
            .bulk_delivered
            .get()
            .max(c.bulk_enqueued.saturating_sub(bulk_queued_bytes));
        c.bulk_delivered.set(delivered);
        Ok(ReliableEgress {
            queued_bytes,
            bulk_queued_bytes,
            bulk_delivered_bytes: delivered,
        })
    }
    fn mark_ready(&mut self, id: ConnectionId) -> Result<(), TransportError> {
        self.connections
            .get_mut(&id)
            .ok_or(TransportError::UnknownConnection)?
            .ready = true;
        Ok(())
    }
    fn activate_secure_channel(&mut self, id: ConnectionId) -> Result<(), TransportError> {
        let connection = self
            .connections
            .get_mut(&id)
            .ok_or(TransportError::UnknownConnection)?;
        if !connection.connected {
            return Err(TransportError::NotConnected);
        }
        if connection.authenticated {
            return Err(TransportError::ProtocolViolation);
        }
        // GNS defaults both limits to 256 KiB/s. Its pinned bandwidth estimate
        // does not grow when only SendRateMax is raised. Match the application's
        // bounded Bulk budget once the connection has been authenticated.
        for option in [
            ESteamNetworkingConfigValue::k_ESteamNetworkingConfig_SendRateMin,
            ESteamNetworkingConfigValue::k_ESteamNetworkingConfig_SendRateMax,
        ] {
            self.global
                .utils()
                .set_connection_config_value(
                    connection.native,
                    option,
                    GnsConfig::Int32(BULK_BYTES_PER_SECOND as i32),
                )
                .map_err(backend)?;
        }
        connection.authenticated = true;
        connection.rate_limit = InboundRateLimiter::with_policy(self.rate_policy, Instant::now());
        Ok(())
    }
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
        self.expire_connecting(Instant::now(), events);
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
        // A plaintext handshake may install keys before its queued encrypted
        // tail can be consumed. Receive one at a time on sockets with pending
        // authentication, leaving that tail in GNS's queue without any copy.
        let unauthenticated_endpoints: BTreeSet<_> = self
            .connections
            .values()
            .filter(|connection| !connection.authenticated)
            .map(|connection| connection.endpoint)
            .collect();
        while remaining != 0 {
            let Some(endpoint) = endpoints.pop_front() else {
                break;
            };
            let Some(socket) = self.sockets.get(&endpoint) else {
                continue;
            };
            // Never receive more than the remaining budget. Unconsumed wrapper
            // messages are released on Drop, which would discard reliable data.
            let chunk = if unauthenticated_endpoints.contains(&endpoint) {
                1
            } else {
                RECEIVE_CHUNK
            };
            let messages = socket.receive(&mut slots[..chunk.min(remaining)])?;
            let received = messages.len();
            remaining -= received; // Unknown, invalid and rate-limited messages count too.
            let mut handshake_barrier = false;
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
                if payload.len() > record_limit(class)
                    || (!connection.authenticated
                        && !matches!(wire::is_session_control_for_class(payload, class), Ok(true)))
                {
                    self.terminate(id, DisconnectReason::InvalidMessage, events);
                    continue;
                }
                match connection.rate_limit.check(class, payload.len(), now) {
                    RateDecision::Allow => {
                        handshake_barrier = !connection.authenticated;
                        events.push(TransportEvent::Message {
                            connection: id,
                            class,
                            payload: payload.to_vec(),
                        });
                    }
                    RateDecision::Drop => {}
                    RateDecision::Disconnect => {
                        self.terminate(id, DisconnectReason::RateLimited, events);
                    }
                }
            }
            // Let bootstrap handle this handshake before consuming the tail.
            if received != 0 && !handshake_barrier {
                endpoints.push_back(endpoint);
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
        if payload.len() > record_limit(class) {
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
        if class != MessageClass::Transient {
            let queued = socket.queued_reliable(connection.native)?;
            let limit = if class == MessageClass::Bulk {
                MAX_BULK_QUEUE_BYTES
            } else {
                MAX_RELIABLE_QUEUE_BYTES
            };
            if queued.saturating_add(payload.len() as u64 + 64) > limit {
                return Err(TransportError::Backpressure);
            }
        }
        let message = self
            .global
            .utils()
            .allocate_message(connection.native, flags(class), payload.to_vec())
            .set_lane(lane(class));
        socket.send(message)?;
        if class == MessageClass::Bulk {
            let c = self.connections.get_mut(&id).expect("sent connection");
            c.bulk_enqueued = c.bulk_enqueued.saturating_add(payload.len() as u64);
        }
        Ok(())
    }
    fn close(&mut self, id: ConnectionId, reason: DisconnectReason) -> Result<(), TransportError> {
        if lifecycle::is_abuse(reason) {
            self.admission.penalize(self.origin(id), Instant::now());
        }
        let connection = self
            .connections
            .get(&id)
            .ok_or(TransportError::UnknownConnection)?;
        let close_result = self
            .sockets
            .get(&connection.endpoint)
            .ok_or(TransportError::UnknownConnection)?
            .close_before_removal(connection.native, reason);
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
        close_result
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
        if self.connections.len() >= MAX_CONNECTIONS
            || self.connections.values().filter(|c| !c.ready).count() >= MAX_PENDING_CONNECTIONS
            || self.connections.values().filter(|c| !c.connected).count() >= MAX_CONNECTING
        {
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
                authenticated: false,
                ready: false,
                created: Instant::now(),
                origin: Origin::Ip(address.ip().to_canonical()),
                bulk_enqueued: 0,
                bulk_delivered: Cell::new(0),
                rate_limit: InboundRateLimiter::with_policy(
                    &PREAUTH_INBOUND_POLICY,
                    Instant::now(),
                ),
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
            backend.connect(sink.local_addr().unwrap()),
            Err(TransportError::Capacity)
        );
        let at = backend
            .connections
            .values()
            .map(|c| c.created)
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
                assert!(!connection.connected);
                connection.created = now - CONNECTING_TIMEOUT - Duration::from_secs(1);
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
                    assert!(client.connections[&outgoing].connected);
                    assert!(!client.connections[&outgoing].authenticated);
                    assert!(!client.connections[&outgoing].ready);
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
        backend.connections.get_mut(&id).unwrap().created = Instant::now() - CONNECTING_TIMEOUT;
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
            assert!(!receiver.backend_mut().connections[&receiver_id].authenticated);
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
            assert!(receiver.backend_mut().connections[&receiver_id].authenticated);
        }
    }

    #[test]
    fn gns_localhost_preauth_barrier_keeps_tail_native_and_rechecks_or_closes() {
        for close_listener in [false, true] {
            let mut host = GnsDirectIp::new().unwrap();
            let mut client = GnsDirectIp::new().unwrap();
            let (listener, incoming, outgoing) =
                connect_pair_unauthenticated(&mut host, &mut client);
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
            assert!(!host.connections[&incoming].authenticated);
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
