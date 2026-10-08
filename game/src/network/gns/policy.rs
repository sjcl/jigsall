//! Shared GNS lane, send-flag and local termination definitions.
use crate::network::transport::{ConnectionId, DisconnectReason, MessageClass, TransportEvent};
use ::gns::{GnsLane, SendFlags};

// The pinned native header specifies lower numbers as higher priority.
// P2P derives its FFI arrays from the same safe values used by Direct IP.
pub(super) const LANES: [GnsLane; 3] = [GnsLane::new(0, 1), GnsLane::new(0, 4), GnsLane::new(1, 1)];

pub(super) fn lane(class: MessageClass) -> u16 {
    match class {
        MessageClass::Transient => 0,
        MessageClass::Control => 1,
        MessageClass::Bulk => 2,
    }
}

pub(super) fn flags(class: MessageClass) -> SendFlags {
    match class {
        MessageClass::Transient => {
            SendFlags::UNRELIABLE | SendFlags::NO_NAGLE | SendFlags::NO_DELAY
        }
        MessageClass::Control | MessageClass::Bulk => SendFlags::RELIABLE,
    }
}

// Outgoing Direct-IP socket Drop uses the wrapper's generic code, while its
// local event retains the precise reason. Backend ownership is unchanged.
pub(super) fn close_code(reason: DisconnectReason) -> u32 {
    match reason {
        DisconnectReason::Requested => 1000,
        DisconnectReason::InvalidMessage => 1001,
        DisconnectReason::RateLimited => 1003,
        _ => 1002,
    }
}

pub(super) fn termination_event(
    connection: ConnectionId,
    connected: bool,
    reason: DisconnectReason,
) -> TransportEvent {
    if connected {
        TransportEvent::Disconnected { connection, reason }
    } else {
        TransportEvent::ConnectionFailed { connection, reason }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::gns::sys::*;

    #[test]
    fn safe_lanes_and_flags_match_native_policy() {
        assert_eq!(LANES.map(|l| l.priority), [0, 0, 1]);
        assert_eq!(LANES.map(|l| l.weight), [1, 4, 1]);
        for (class, index, expected_flags) in [
            (
                MessageClass::Transient,
                0,
                k_nSteamNetworkingSend_Unreliable
                    | k_nSteamNetworkingSend_NoNagle
                    | k_nSteamNetworkingSend_NoDelay,
            ),
            (MessageClass::Control, 1, k_nSteamNetworkingSend_Reliable),
            (MessageClass::Bulk, 2, k_nSteamNetworkingSend_Reliable),
        ] {
            assert_eq!(lane(class), index);
            assert_eq!(flags(class).bits(), expected_flags);
            assert_eq!(super::super::inbound::class(index), Some(class));
        }
    }

    #[test]
    fn termination_preserves_all_reasons_and_both_connection_states() {
        let id = ConnectionId::new(42);
        for (reason, code) in [
            (DisconnectReason::Requested, 1000),
            (DisconnectReason::InvalidMessage, 1001),
            (DisconnectReason::RateLimited, 1003),
            (DisconnectReason::RemoteClosed, 1002),
            (DisconnectReason::ConnectionProblem, 1002),
            (DisconnectReason::BackendFailure, 1002),
            (DisconnectReason::AuthenticationFailed, 1002),
            (DisconnectReason::AuthenticationTimeout, 1002),
            (DisconnectReason::AuthenticatedHandoffTimeout, 1002),
            (DisconnectReason::BackendConnectionTimeout, 1002),
            (DisconnectReason::JoinCapacity, 1002),
            (DisconnectReason::SyncPhaseTimeout, 1002),
            (DisconnectReason::SyncLifetime, 1002),
            (DisconnectReason::HostCapacityTimeout, 1002),
            (DisconnectReason::BulkStalled, 1002),
            (DisconnectReason::ProtocolViolation, 1002),
        ] {
            assert_eq!(close_code(reason), code);
            assert_eq!(close_code(reason) as i32, code as i32);
            assert_eq!(
                termination_event(id, true, reason),
                TransportEvent::Disconnected {
                    connection: id,
                    reason
                }
            );
            assert_eq!(
                termination_event(id, false, reason),
                TransportEvent::ConnectionFailed {
                    connection: id,
                    reason
                }
            );
        }
    }
}
