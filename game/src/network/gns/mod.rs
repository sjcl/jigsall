//! Open-source GNS details are confined to this optional backend.
use crate::network::transport::TransportError;
use std::sync::atomic::{AtomicU64, Ordering};

/// Shared v1 foundation/runtime virtual port; never a user-entered network port.
pub const P2P_VIRTUAL_PORT: u16 = 0;

mod direct_ip;
pub use direct_ip::GnsDirectIp;
mod p2p;
pub use p2p::{GnsP2p, IceConfig};
#[cfg(feature = "rendezvous")]
pub mod rendezvous;
pub mod signaling;

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
