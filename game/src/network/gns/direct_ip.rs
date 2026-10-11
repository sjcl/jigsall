use super::{
    inbound::{check_message, class, InboundDecision, MAX_RECEIVE_PER_POLL},
    outbound,
    policy::{close_code, flags, lane, termination_event, LANES},
    token, ConnectionState,
};
use crate::network::{
    lifecycle::{self, Admission, CONNECTING_TIMEOUT},
    rate_limit::{InboundRatePolicy, DEFAULT_INBOUND_POLICY},
    transport::{
        ConnectionId, DirectIpTransport, DisconnectReason, ListenerId, MessageClass, Origin,
        ReliableEgress, Transport, TransportError, TransportEvent,
    },
};
use ::gns::{
    sys::ESteamNetworkingConnectionState as State, GnsConnection, GnsConnectionEvent, GnsGlobal,
    GnsNetworkMessage, GnsSocket, IsClient, IsCreated, IsServer, MessageSlot, ReceivedMessagesInto,
    ToSend,
};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    net::{SocketAddr, UdpSocket},
    time::Instant,
};

pub const MAX_LISTENERS: usize = 8;
const RECEIVE_CHUNK: usize = 32;
const CALLBACK_BATCH: usize = 128;
fn backend(error: ::gns::GnsError) -> TransportError {
    TransportError::Backend(error.to_string())
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
    state: ConnectionState,
    origin: Origin,
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
    fn has_connection_capacity(&self) -> bool {
        super::has_connection_capacity(self.connections.values().map(|c| &c.state))
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
            if lifecycle::is_transport_abuse(reason) {
                self.admission.penalize(Some(connection.origin), now);
            }
            events.push(termination_event(id, connection.state.connected, reason));
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
                let origin = Origin::ip(info.remote_address());
                let origin_pending = self
                    .connections
                    .values()
                    .filter(|c| !c.state.ready && c.origin == origin)
                    .count();
                self.admission
                    .protect(self.connections.values().map(|c| Some(c.origin)));
                let refusal = if !self.has_connection_capacity() {
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
                        state: ConnectionState::new(now),
                        origin,
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
                    if connection.state.mark_connected() {
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
                !c.state.connected
                    && now.saturating_duration_since(c.state.created) >= CONNECTING_TIMEOUT
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
        Ok(c.state
            .bulk_delivery
            .egress(queued_bytes, bulk_queued_bytes))
    }
    fn mark_ready(&mut self, id: ConnectionId) -> Result<(), TransportError> {
        self.connections
            .get_mut(&id)
            .ok_or(TransportError::UnknownConnection)?
            .state
            .mark_ready();
        Ok(())
    }
    fn activate_secure_channel(&mut self, id: ConnectionId) -> Result<(), TransportError> {
        let connection = self
            .connections
            .get_mut(&id)
            .ok_or(TransportError::UnknownConnection)?;
        connection
            .state
            .activate_secure_channel(self.rate_policy, || {
                super::configure_authenticated_send_rate(connection.native)
            })?;
        self.admission.succeed(Some(connection.origin));
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
            .filter(|connection| !connection.state.authenticated)
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
                if !connection.state.connected {
                    continue;
                }
                let Some(class) = class(message.lane()) else {
                    self.terminate(id, DisconnectReason::InvalidMessage, events);
                    continue;
                };
                let payload = match message.try_payload() {
                    Ok(payload) => payload,
                    Err(_) => {
                        self.terminate(id, DisconnectReason::InvalidMessage, events);
                        continue;
                    }
                };
                match check_message(
                    class,
                    payload,
                    connection.state.authenticated,
                    &mut connection.state.limiter,
                    now,
                ) {
                    InboundDecision::Allow => {
                        handshake_barrier = !connection.state.authenticated;
                        events.push(TransportEvent::Message {
                            connection: id,
                            class,
                            payload: payload.to_vec(),
                        });
                    }
                    InboundDecision::Drop => {}
                    InboundDecision::Disconnect(reason) => {
                        self.terminate(id, reason, events);
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
        outbound::check_payload_size(class, payload.len())?;
        let connection = self
            .connections
            .get(&id)
            .ok_or(TransportError::UnknownConnection)?;
        if !connection.state.connected {
            return Err(TransportError::NotConnected);
        }
        let socket = self
            .sockets
            .get(&connection.endpoint)
            .ok_or(TransportError::UnknownConnection)?;
        if let Some(limit) = outbound::queue_limit(class) {
            let queued = socket.queued_reliable(connection.native)?;
            outbound::check_queue(queued, payload.len(), limit)?;
        }
        let message = self
            .global
            .utils()
            .allocate_message(connection.native, flags(class), payload.to_vec())
            .set_lane(lane(class));
        socket.send(message)?;
        if class == MessageClass::Bulk {
            let c = self.connections.get_mut(&id).expect("sent connection");
            c.state.bulk_delivery.record_sent(payload.len());
        }
        Ok(())
    }
    fn close(&mut self, id: ConnectionId, reason: DisconnectReason) -> Result<(), TransportError> {
        // The calling application layer owns this failure's accounting. Native
        // receive/establishment failures are counted by terminate_at only.
        let connection = self
            .connections
            .get(&id)
            .ok_or(TransportError::UnknownConnection)?;
        let close_result = self
            .sockets
            .get(&connection.endpoint)
            .ok_or(TransportError::UnknownConnection)?
            .close_before_removal(connection.native, reason);
        let connected = connection.state.connected;
        self.forget(id);
        self.pending.push(termination_event(id, connected, reason));
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
        if !self.has_connection_capacity() {
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
                state: ConnectionState::new(Instant::now()),
                origin: Origin::ip(address.ip()),
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
pub(super) mod tests;
