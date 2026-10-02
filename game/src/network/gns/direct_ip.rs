use crate::network::{
    transport::{
        ConnectionId, DirectIpTransport, DisconnectReason, ListenerId, MessageClass, Transport,
        TransportError, TransportEvent,
    },
    wire::frame_limit,
};
use ::gns::{
    sys::ESteamNetworkingConnectionState as State, GnsConnection, GnsConnectionEvent, GnsGlobal,
    GnsLane, GnsNetworkMessage, GnsSocket, IsClient, IsCreated, IsServer, ReceivedMessages,
    SendFlags, ToSend,
};
use std::{
    collections::{BTreeMap, HashMap},
    net::{SocketAddr, UdpSocket},
    sync::atomic::{AtomicU64, Ordering},
};

pub const MAX_CONNECTIONS: usize = 64;
pub const MAX_LISTENERS: usize = 8;
const RECEIVE_BATCH: usize = 32;
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
    fn close(
        &self,
        connection: GnsConnection,
        reason: DisconnectReason,
    ) -> Result<(), TransportError> {
        let code = match reason {
            DisconnectReason::Requested => 1000,
            DisconnectReason::InvalidMessage => 1001,
            _ => 1002,
        };
        match self {
            Self::Server(s) => s.close_connection(connection, code, None, false),
            Self::Client(s) => s.close_connection(connection, code, None, false),
        }
        .map_err(backend)
    }
    fn send(&self, message: GnsNetworkMessage<ToSend>) -> Result<(), TransportError> {
        match self {
            Self::Server(s) => s.send_message(message),
            Self::Client(s) => s.send_message(message),
        }
        .map(|_| ())
        .map_err(backend)
    }
    fn receive(&self) -> Result<ReceivedMessages<RECEIVE_BATCH>, TransportError> {
        match self {
            Self::Server(s) => s.receive_messages(),
            Self::Client(s) => s.receive_messages(),
        }
        .map_err(backend)
    }
}
struct Connection {
    native: GnsConnection,
    endpoint: Endpoint,
    connected: bool,
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
}

impl GnsDirectIp {
    pub fn new() -> Result<Self, TransportError> {
        Ok(Self {
            global: GnsGlobal::get().map_err(backend)?,
            sockets: BTreeMap::new(),
            connections: BTreeMap::new(),
            native_ids: HashMap::new(),
            pending: Vec::new(),
        })
    }
    fn forget(&mut self, id: ConnectionId, reason: DisconnectReason) {
        if let Some(connection) = self.connections.remove(&id) {
            self.native_ids.remove(&connection.native);
            if let Some(socket) = self.sockets.get(&connection.endpoint) {
                let _ = socket.close(connection.native, reason);
            }
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
            self.forget(id, reason);
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
                    let _ = socket.close_connection(native, 1002, None, false);
                    return Ok(());
                }
                let issued = ConnectionId::new(token()?);
                self.connections.insert(
                    issued,
                    Connection {
                        native,
                        endpoint,
                        connected: false,
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
        let endpoints: Vec<_> = self.sockets.keys().copied().collect();
        for endpoint in endpoints {
            let Some(socket) = self.sockets.get(&endpoint) else {
                continue;
            };
            let messages = socket.receive()?;
            for message in messages {
                let Some(&id) = self.native_ids.get(&message.connection()) else {
                    continue;
                };
                if !self.connections.get(&id).is_some_and(|c| c.connected) {
                    continue;
                }
                let Some(class) = class(message.lane()) else {
                    self.terminate(id, DisconnectReason::InvalidMessage, events);
                    continue;
                };
                if message.payload().len() > frame_limit(class) {
                    self.terminate(id, DisconnectReason::InvalidMessage, events);
                    continue;
                }
                events.push(TransportEvent::Message {
                    connection: id,
                    class,
                    payload: message.payload().to_vec(),
                });
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
            .close(connection.native, reason)?;
        let connected = connection.connected;
        self.forget(id, reason);
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
                let _ = socket.close(connection.native, DisconnectReason::Requested);
            }
        }
        self.connections.clear();
        self.sockets.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lane_and_flag_policy() {
        assert_eq!(lane(MessageClass::Transient), 0);
        assert_eq!(lane(MessageClass::Control), 1);
        assert_eq!(lane(MessageClass::Bulk), 2);
        assert!(flags(MessageClass::Transient).contains(SendFlags::NO_NAGLE | SendFlags::NO_DELAY));
        assert!(!flags(MessageClass::Transient).contains(SendFlags::RELIABLE));
        assert!(flags(MessageClass::Control).contains(SendFlags::RELIABLE));
        assert!(flags(MessageClass::Bulk).contains(SendFlags::RELIABLE));
        assert!(LANES[2].priority > LANES[1].priority);
    }
}
