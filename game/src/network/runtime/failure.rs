//! Stable presentation categories; diagnostics remain available independently.
use super::*;
use crate::network::{bootstrap::BootstrapError, syncing::SyncError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetworkFailureKind {
    Authentication,
    Timeout,
    Capacity,
    Protocol,
    Image,
    Connection,
}

impl NetworkFailureKind {
    pub fn transport(error: &TransportError) -> Self {
        match error {
            TransportError::Capacity | TransportError::Backpressure => Self::Capacity,
            TransportError::ProtocolViolation | TransportError::PayloadTooLarge => Self::Protocol,
            _ => Self::Connection,
        }
    }
    pub fn disconnect(reason: DisconnectReason) -> Self {
        match reason {
            DisconnectReason::AuthenticationFailed => Self::Authentication,
            DisconnectReason::AuthenticationTimeout
            | DisconnectReason::AuthenticatedHandoffTimeout
            | DisconnectReason::BackendConnectionTimeout
            | DisconnectReason::SyncPhaseTimeout
            | DisconnectReason::SyncLifetime
            | DisconnectReason::BulkStalled => Self::Timeout,
            DisconnectReason::JoinCapacity | DisconnectReason::HostCapacityTimeout => {
                Self::Capacity
            }
            DisconnectReason::InvalidMessage | DisconnectReason::ProtocolViolation => {
                Self::Protocol
            }
            DisconnectReason::RateLimited => Self::Capacity,
            DisconnectReason::Requested
            | DisconnectReason::RemoteClosed
            | DisconnectReason::ConnectionProblem
            | DisconnectReason::BackendFailure => Self::Connection,
        }
    }
    pub fn bootstrap(error: &BootstrapError) -> Self {
        match error {
            BootstrapError::Rejected(reason) => Self::disconnect(*reason),
            BootstrapError::Transport(error) => Self::transport(error),
            _ => Self::Protocol,
        }
    }
    pub fn sync(error: &SyncError) -> Self {
        match error {
            SyncError::HostImageUnavailable | SyncError::ImageHashMismatch => Self::Image,
            SyncError::Bootstrap(error) => Self::bootstrap(error),
            SyncError::Transport(error) => Self::transport(error),
            _ => Self::disconnect(error.disconnect_reason()),
        }
    }
}

#[derive(Debug)]
pub(super) struct RuntimeFailure {
    pub kind: NetworkFailureKind,
    pub diagnostic: String,
}
impl RuntimeFailure {
    pub fn new(kind: NetworkFailureKind, error: impl std::fmt::Debug) -> Self {
        Self {
            kind,
            diagnostic: format!("{error:?}"),
        }
    }
    pub fn protocol(error: impl std::fmt::Debug) -> Self {
        Self::new(NetworkFailureKind::Protocol, error)
    }
    pub fn transport(error: TransportError) -> Self {
        Self::new(NetworkFailureKind::transport(&error), error)
    }
    pub fn bootstrap(error: BootstrapError) -> Self {
        Self::new(NetworkFailureKind::bootstrap(&error), error)
    }
    pub fn sync(error: SyncError) -> Self {
        Self::new(NetworkFailureKind::sync(&error), error)
    }
    pub fn send(error: super::super::client::ClientSendError) -> Self {
        let kind = match &error {
            super::super::client::ClientSendError::Transport(error) => {
                NetworkFailureKind::transport(error)
            }
            _ => NetworkFailureKind::Protocol,
        };
        Self::new(kind, error)
    }
}
impl From<&str> for RuntimeFailure {
    fn from(error: &str) -> Self {
        Self::protocol(error)
    }
}

impl NetworkStatus {
    pub(super) fn set_failure(&mut self, kind: NetworkFailureKind, error: impl std::fmt::Debug) {
        self.failure = Some(kind);
        self.error = Some(format!("{error:?}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_causes_classify_independently_of_diagnostic_text() {
        use crate::network::syncing::ResponseWait;
        for (error, expected) in [
            (
                SyncError::Bootstrap(BootstrapError::Rejected(
                    DisconnectReason::AuthenticationFailed,
                )),
                NetworkFailureKind::Authentication,
            ),
            (
                SyncError::PhaseTimeout(ResponseWait::ImageReady),
                NetworkFailureKind::Timeout,
            ),
            (SyncError::LifetimeTimeout, NetworkFailureKind::Timeout),
            (SyncError::BulkStalled, NetworkFailureKind::Timeout),
            (SyncError::CapacityWaitTimeout, NetworkFailureKind::Capacity),
            (SyncError::TooManyJoins, NetworkFailureKind::Capacity),
            (
                SyncError::Wire(wire::WireError::UnsupportedVersion(99)),
                NetworkFailureKind::Protocol,
            ),
            (SyncError::HostImageUnavailable, NetworkFailureKind::Image),
            (SyncError::ImageHashMismatch, NetworkFailureKind::Image),
            (
                SyncError::Transport(TransportError::Backend(
                    "AuthenticationFailed Timeout".into(),
                )),
                NetworkFailureKind::Connection,
            ),
        ] {
            assert_eq!(NetworkFailureKind::sync(&error), expected, "{error:?}");
        }
    }
}
