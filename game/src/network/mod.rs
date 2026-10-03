//! Opt-in, frame-driven networking beneath the existing authority and replicas.
//! Establishment/authentication belong to a backend/session; gameplay sees only
//! opaque connections and protocol objects. No network work runs on idle pieces.
pub mod auth;
pub mod bootstrap;
pub mod bulk;
pub mod client;
#[cfg(feature = "gns")]
pub mod gns;
pub mod host;
pub mod rate_limit;
pub mod runtime;
pub mod secure;
pub mod session;
pub mod session_control;
pub mod sync_control;
pub mod syncing;
pub mod transient;
pub mod transport;
pub mod wire;

#[cfg(test)]
mod tests;
