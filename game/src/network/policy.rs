//! Host failure actions. Successful refusals and lane drops are routing outcomes;
//! neither is an error that can silently discard a Reliable command.
use super::{bootstrap::BootstrapError, transport::*};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostInternalRecovery {
    /// Discard this peer's local bootstrap/sync state and permit a fresh join.
    DisconnectPeer(DisconnectReason),
    /// Shared authority or generated data is unsafe for every peer.
    StopSession,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostFailurePolicy {
    Disconnect(DisconnectReason),
    /// A connection-scoped send/egress failure, without a host-wide diagnostic.
    PeerCommunication(DisconnectReason),
    InternalFailure(HostInternalRecovery),
}
impl HostFailurePolicy {
    pub fn disconnect_reason(self) -> DisconnectReason {
        match self {
            Self::Disconnect(reason)
            | Self::PeerCommunication(reason)
            | Self::InternalFailure(HostInternalRecovery::DisconnectPeer(reason)) => reason,
            Self::InternalFailure(HostInternalRecovery::StopSession) => {
                DisconnectReason::BackendFailure
            }
        }
    }
    pub fn transport(error: &TransportError) -> Self {
        match error {
            TransportError::UnknownConnection
            | TransportError::NotConnected
            | TransportError::Backpressure => {
                Self::PeerCommunication(DisconnectReason::ConnectionProblem)
            }
            TransportError::Capacity => Self::PeerCommunication(DisconnectReason::JoinCapacity),
            TransportError::Backend(_) | TransportError::EgressUnavailable => {
                Self::PeerCommunication(DisconnectReason::BackendFailure)
            }
            TransportError::UnknownListener
            | TransportError::PayloadTooLarge
            | TransportError::ProtocolViolation => {
                Self::InternalFailure(HostInternalRecovery::StopSession)
            }
        }
    }
    pub fn bootstrap(error: &BootstrapError) -> Self {
        match error {
            BootstrapError::Rejected(reason) => Self::Disconnect(*reason),
            BootstrapError::Transport(error) => Self::transport(error),
            BootstrapError::InvalidTransition | BootstrapError::Registration(_) => {
                Self::InternalFailure(HostInternalRecovery::DisconnectPeer(
                    DisconnectReason::BackendFailure,
                ))
            }
        }
    }
}
