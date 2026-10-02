use std::{fmt, net::SocketAddr};

/// Backend-issued identity. The number is a Puzzella token, never a native handle.
/// Backends must not reuse a token during their lifetime.
/// Only backends inside this crate can construct tokens.
/// The underlying value cannot be read as a native handle by callers:
/// ```compile_fail
/// use puzzella_game::network::transport::ConnectionId;
/// fn native_handle(connection: ConnectionId) -> u64 {
///     connection.0
/// }
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConnectionId(u64);

impl ConnectionId {
    // Token issuance is unused in default builds without an opt-in backend.
    #[cfg_attr(not(feature = "gns"), allow(dead_code))]
    pub(crate) const fn new(token: u64) -> Self {
        Self(token)
    }
}

/// Backend-issued listener token, with the same opaque boundary as ConnectionId.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ListenerId(u64);

impl ListenerId {
    #[cfg_attr(not(feature = "gns"), allow(dead_code))]
    pub(crate) const fn new(token: u64) -> Self {
        Self(token)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageClass {
    /// Best-effort, unordered, latency first; may be dropped instead of queued.
    Transient,
    /// Reliable and ordered per connection, independently of Bulk delivery.
    Control,
    /// Reliable separate stream, lower scheduling priority than Control.
    Bulk,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisconnectReason {
    Requested,
    RemoteClosed,
    ConnectionProblem,
    InvalidMessage,
    RateLimited,
    BackendFailure,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransportError {
    UnknownConnection,
    UnknownListener,
    NotConnected,
    PayloadTooLarge,
    Capacity,
    Backend(String),
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for TransportError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransportEvent {
    Connected {
        connection: ConnectionId,
    },
    ConnectionFailed {
        connection: ConnectionId,
        reason: DisconnectReason,
    },
    Disconnected {
        connection: ConnectionId,
        reason: DisconnectReason,
    },
    Message {
        connection: ConnectionId,
        class: MessageClass,
        payload: Vec<u8>,
    },
}

/// Append normalized events, running callbacks and receiving bounded batches.
/// Call once per Bevy frame. Each send is one message, preserving boundaries.
/// An accepted reliable send is queued, not an application acknowledgement.
pub trait Transport {
    fn poll(&mut self, events: &mut Vec<TransportEvent>) -> Result<(), TransportError>;
    fn send(
        &mut self,
        connection: ConnectionId,
        class: MessageClass,
        payload: &[u8],
    ) -> Result<(), TransportError>;
    fn close(
        &mut self,
        connection: ConnectionId,
        reason: DisconnectReason,
    ) -> Result<(), TransportError>;
}

/// Direct-IP establishment is separate from the shared connected-message API.
/// A future Steamworks implementation can expose its own connect_peer/listen_p2p.
pub trait DirectIpTransport: Transport {
    fn listen(&mut self, address: SocketAddr) -> Result<ListenerId, TransportError>;
    fn listener_address(&self, listener: ListenerId) -> Result<SocketAddr, TransportError>;
    fn close_listener(&mut self, listener: ListenerId) -> Result<(), TransportError>;
    fn connect(&mut self, address: SocketAddr) -> Result<ConnectionId, TransportError>;
}
