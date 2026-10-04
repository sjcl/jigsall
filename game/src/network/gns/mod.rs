//! Open-source GNS details are confined to this optional backend.
use crate::network::transport::TransportError;
use std::sync::atomic::{AtomicU64, Ordering};

mod direct_ip;
pub use direct_ip::GnsDirectIp;
mod p2p;
pub use p2p::{GnsP2p, IceConfig};
pub mod signaling;

// Initialize the process identity before either establishment path creates
// sockets. This never resets the identity of a live Direct IP connection.
fn global() -> Result<&'static ::gns::GnsGlobal, TransportError> {
    p2p::native::global()
}

static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);
fn token() -> Result<u64, TransportError> {
    NEXT_TOKEN
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
        .map_err(|_| TransportError::Capacity)
}
