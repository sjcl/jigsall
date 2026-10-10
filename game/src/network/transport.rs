use std::{
    fmt,
    net::{IpAddr, SocketAddr},
};

/// Abuse-accounting key; independent of password/player authentication.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Origin {
    Ip(IpAddr),
    Route(RouteOrigin),
}
impl Origin {
    /// Accounting only: never use this prefix as a communication address.
    pub fn ip(ip: IpAddr) -> Self {
        Self::Ip(ip_prefix(ip))
    }
    pub(crate) fn normalized(self) -> Self {
        match self {
            Self::Ip(ip) => Self::ip(ip),
            Self::Route(route) => route.abuse_origin(),
        }
    }
}
pub(crate) fn ip_prefix(ip: IpAddr) -> IpAddr {
    match ip.to_canonical() {
        IpAddr::V6(ip) => IpAddr::V6(std::net::Ipv6Addr::from(u128::from(ip) & (u128::MAX << 64))),
        ip => ip,
    }
}

/// Abuse key supplied by a trusted local adapter after verifying the rendezvous
/// authority, session, membership and abuse key. Never construct this from a peer's envelope
/// claims. It does not authenticate a game player or replace password bootstrap.
/// MemberId identifies a route; the server's room-scoped AbuseKey accounts for
/// abuse independently of reconnects. Neither authenticates a game player.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RouteOrigin {
    authority: [u8; 16],
    session: [u8; 16],
    account: [u8; 16],
    abuse: [u8; 16],
}
impl RouteOrigin {
    /// Routing incarnation only. Production adapters must also supply the
    /// server-issued AbuseKey with with_abuse_key before installing a binding.
    pub const fn from_authenticated_route(
        authority: [u8; 16],
        session: [u8; 16],
        account: [u8; 16],
    ) -> Self {
        Self {
            authority,
            session,
            account,
            abuse: account,
        }
    }
    pub const fn with_abuse_key(mut self, abuse: [u8; 16]) -> Self {
        self.abuse = abuse;
        self
    }
    pub(crate) const fn abuse_origin(self) -> Origin {
        Origin::Route(Self {
            account: self.abuse,
            ..self
        })
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ReliableEgress {
    /// Pending AND sent-unacknowledged bytes across all lanes.
    pub queued_bytes: u64,
    pub bulk_queued_bytes: u64,
    /// Monotonic conservative delivery estimate for the Bulk lane only.
    pub bulk_delivered_bytes: u64,
}

/// Backend-issued identity. The number is a Jigsall token, never a native handle.
/// Backends must not reuse a token during their lifetime.
/// Only backends inside this crate can construct tokens.
/// The underlying value cannot be read as a native handle by callers:
/// ```compile_fail
/// use jigsall_game::network::transport::ConnectionId;
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
    AuthenticationFailed,
    AuthenticationTimeout,
    AuthenticatedHandoffTimeout,
    BackendConnectionTimeout,
    JoinCapacity,
    SyncPhaseTimeout,
    SyncLifetime,
    HostCapacityTimeout,
    BulkStalled,
    ProtocolViolation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransportError {
    UnknownConnection,
    UnknownListener,
    NotConnected,
    PayloadTooLarge,
    Capacity,
    ProtocolViolation,
    Backend(String),
    Backpressure,
    EgressUnavailable,
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
    fn origin(&self, _connection: ConnectionId) -> Option<Origin> {
        None
    }
    /// Missing telemetry fails closed for Bulk, rather than assuming delivery.
    fn reliable_egress(&self, _connection: ConnectionId) -> Result<ReliableEgress, TransportError> {
        Err(TransportError::EgressUnavailable)
    }
    /// Release pre-Ready admission occupancy only after a valid Ready commit.
    fn mark_ready(&mut self, _connection: ConnectionId) -> Result<(), TransportError> {
        Ok(())
    }
    fn poll(&mut self, events: &mut Vec<TransportEvent>) -> Result<(), TransportError>;
    /// Called by SecureTransport only after bootstrap verifies the peer, during
    /// channel installation. Native backends may then lift pre-auth receive
    /// limits. Delegating backends must forward this notification.
    fn activate_secure_channel(&mut self, connection: ConnectionId) -> Result<(), TransportError>;
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
    /// Queue lifecycle events for every closed peer for the next poll, including
    /// peers already closed when a later close fails partway through the listener.
    fn close_listener(&mut self, listener: ListenerId) -> Result<(), TransportError>;
    fn connect(&mut self, address: SocketAddr) -> Result<ConnectionId, TransportError>;
}
