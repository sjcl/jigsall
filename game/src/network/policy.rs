//! Host failure actions. Successful refusals and lane drops are routing outcomes;
//! neither is an error that can silently discard a Reliable command.
use super::{bootstrap::BootstrapError, transport::*};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostFailurePolicy {
    Disconnect(DisconnectReason),
    /// Close the affected connection for recovery, without blaming/penalizing it.
    InternalFailure(DisconnectReason),
}
impl HostFailurePolicy {
    pub fn disconnect_reason(self) -> DisconnectReason {
        match self {
            Self::Disconnect(reason) | Self::InternalFailure(reason) => reason,
        }
    }
    pub fn transport(error: &TransportError) -> Self {
        Self::InternalFailure(match error {
            TransportError::UnknownConnection
            | TransportError::NotConnected
            | TransportError::Backpressure => DisconnectReason::ConnectionProblem,
            TransportError::Capacity => DisconnectReason::JoinCapacity,
            TransportError::UnknownListener
            | TransportError::PayloadTooLarge
            | TransportError::ProtocolViolation
            | TransportError::Backend(_)
            | TransportError::EgressUnavailable => DisconnectReason::BackendFailure,
        })
    }
    pub fn bootstrap(error: &BootstrapError) -> Self {
        match error {
            BootstrapError::Rejected(reason) => Self::Disconnect(*reason),
            BootstrapError::Transport(error) => Self::transport(error),
            BootstrapError::InvalidTransition | BootstrapError::Registration(_) => {
                Self::InternalFailure(DisconnectReason::BackendFailure)
            }
        }
    }
}
