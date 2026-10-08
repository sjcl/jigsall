//! Open-source GNS details are confined to this optional backend.
use crate::network::{
    lifecycle::{MAX_CONNECTING, MAX_CONNECTIONS, MAX_PENDING_CONNECTIONS},
    rate_limit::{InboundRateLimiter, InboundRatePolicy, PREAUTH_INBOUND_POLICY},
    transport::TransportError,
};
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Instant,
};

/// Shared v1 foundation/runtime virtual port; never a user-entered network port.
pub const P2P_VIRTUAL_PORT: u16 = 0;

mod direct_ip;
pub use direct_ip::GnsDirectIp;
mod inbound;
mod outbound;
mod p2p;
mod policy;
pub use p2p::{GnsP2p, IceConfig};
#[cfg(feature = "rendezvous")]
pub mod rendezvous;
pub mod signaling;

/// Transport-independent state stored inline with each backend's native owner.
struct ConnectionState {
    connected: bool,
    authenticated: bool,
    ready: bool,
    created: Instant,
    limiter: InboundRateLimiter,
    bulk_delivery: outbound::BulkDelivery,
}

impl ConnectionState {
    fn new(now: Instant) -> Self {
        Self {
            connected: false,
            authenticated: false,
            ready: false,
            created: now,
            limiter: InboundRateLimiter::with_policy(&PREAUTH_INBOUND_POLICY, now),
            bulk_delivery: outbound::BulkDelivery::default(),
        }
    }

    /// Whether the backend should emit the first Connected notification.
    fn mark_connected(&mut self) -> bool {
        !std::mem::replace(&mut self.connected, true)
    }

    fn activate_secure_channel(
        &mut self,
        policy: &'static InboundRatePolicy,
        configure_native: impl FnOnce() -> Result<(), TransportError>,
    ) -> Result<(), TransportError> {
        if !self.connected {
            return Err(TransportError::NotConnected);
        }
        if self.authenticated {
            return Err(TransportError::ProtocolViolation);
        }
        // Native configuration must succeed before changing either auth state
        // or rate credit. The closure is statically dispatched and owns no handle.
        configure_native()?;
        self.authenticated = true;
        self.limiter = InboundRateLimiter::with_policy(policy, Instant::now());
        Ok(())
    }

    fn mark_ready(&mut self) {
        // The caller validates the Ready commit. Preserve the backend contract:
        // marking an existing connection Ready is idempotent.
        self.ready = true;
    }
}

/// Connecting also occupies a not-Ready slot; authentication alone releases
/// neither slot. Count owned handles without allocating or querying native state.
fn has_connection_capacity<'a>(states: impl Iterator<Item = &'a ConnectionState>) -> bool {
    let (total, connecting, pending) = states.fold((0, 0, 0), |(total, connecting, pending), c| {
        (
            total + 1,
            connecting + usize::from(!c.connected),
            pending + usize::from(!c.ready),
        )
    });
    total < MAX_CONNECTIONS && connecting < MAX_CONNECTING && pending < MAX_PENDING_CONNECTIONS
}

// Initialize the process identity before either establishment path creates
// sockets. This never resets the identity of a live Direct IP connection.
fn global() -> Result<&'static ::gns::GnsGlobal, TransportError> {
    p2p::native::global()
}

fn configure_authenticated_send_rate(
    connection: ::gns::GnsConnection,
) -> Result<(), TransportError> {
    use ::gns::{sys::ESteamNetworkingConfigValue::*, GnsConfig};
    // GNS defaults both limits to 256 KiB/s. Raising only the maximum does not
    // grow its pinned bandwidth estimate. Both establishment paths must match
    // the application's bounded Bulk budget after authentication, before sync.
    for option in [
        k_ESteamNetworkingConfig_SendRateMin,
        k_ESteamNetworkingConfig_SendRateMax,
    ] {
        global()?
            .utils()
            .set_connection_config_value(
                connection,
                option,
                GnsConfig::Int32(crate::network::lifecycle::BULK_BYTES_PER_SECOND as i32),
            )
            .map_err(|error| TransportError::Backend(error.to_string()))?;
    }
    Ok(())
}

static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);
fn token() -> Result<u64, TransportError> {
    NEXT_TOKEN
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
        .map_err(|_| TransportError::Capacity)
}

#[cfg(test)]
mod tests;
