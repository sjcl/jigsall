use super::*;
use crate::network::{
    rate_limit::{BucketPolicy, RateDecision, DEFAULT_INBOUND_POLICY},
    transport::MessageClass,
};

// (total, Connecting, not-Ready, capacity). Connected pre-auth and authenticated
// syncing handles both remain pending; Ready handles still occupy total slots.
pub(super) const CAPACITY_CASES: [(usize, usize, usize, bool); 12] = [
    (0, 0, 0, true),
    (
        MAX_CONNECTING - 1,
        MAX_CONNECTING - 1,
        MAX_CONNECTING - 1,
        true,
    ),
    (MAX_CONNECTING, MAX_CONNECTING, MAX_CONNECTING, false),
    (
        MAX_CONNECTING + 1,
        MAX_CONNECTING + 1,
        MAX_CONNECTING + 1,
        false,
    ),
    (
        MAX_PENDING_CONNECTIONS - 1,
        0,
        MAX_PENDING_CONNECTIONS - 1,
        true,
    ),
    (MAX_PENDING_CONNECTIONS, 0, MAX_PENDING_CONNECTIONS, false),
    (
        MAX_PENDING_CONNECTIONS + 1,
        0,
        MAX_PENDING_CONNECTIONS + 1,
        false,
    ),
    (MAX_CONNECTIONS - 1, 0, 0, true),
    (MAX_CONNECTIONS, 0, 0, false),
    (MAX_CONNECTIONS + 1, 0, 0, false),
    (
        MAX_CONNECTIONS - 1,
        MAX_CONNECTING - 1,
        MAX_PENDING_CONNECTIONS - 1,
        true,
    ),
    (
        MAX_CONNECTIONS,
        MAX_CONNECTING,
        MAX_PENDING_CONNECTIONS,
        false,
    ),
];

pub(super) fn states(
    total: usize,
    connecting: usize,
    pending: usize,
) -> impl Iterator<Item = ConnectionState> {
    let now = Instant::now();
    (0..total).map(move |index| {
        let mut state = ConnectionState::new(now);
        state.connected = index >= connecting;
        state.authenticated = state.connected && index % 2 == 0;
        state.ready = index >= pending;
        state
    })
}

#[test]
fn connection_capacity_counts_connecting_pending_and_ready_handles() {
    for (total, connecting, pending, expected) in CAPACITY_CASES {
        let states: Vec<_> = states(total, connecting, pending).collect();
        assert_eq!(has_connection_capacity(states.iter()), expected);
    }
    // Authentication does not free pending capacity; Ready does. Native
    // Connected frees Connecting capacity while retaining the pending slot.
    let mut states: Vec<_> = states(MAX_PENDING_CONNECTIONS, 0, MAX_PENDING_CONNECTIONS).collect();
    for state in &mut states {
        state.authenticated = true;
    }
    assert!(!has_connection_capacity(states.iter()));
    states[0].mark_ready();
    assert!(has_connection_capacity(states.iter()));
    let mut states: Vec<_> = self::states(MAX_CONNECTING, MAX_CONNECTING, MAX_CONNECTING).collect();
    states[0].connected = true;
    assert!(has_connection_capacity(states.iter()));
}

const TEST_BUCKET: BucketPolicy = BucketPolicy {
    bytes_per_second: 0,
    burst_bytes: 2,
    minimum_charge: 1,
};
const TEST_POLICY: InboundRatePolicy = InboundRatePolicy {
    transient: TEST_BUCKET,
    control: TEST_BUCKET,
    bulk: TEST_BUCKET,
};

#[test]
fn authentication_validates_before_native_config_and_preserves_state_on_failure() {
    let now = Instant::now();
    let mut state = ConnectionState::new(now);
    assert!(!state.connected && !state.authenticated && !state.ready);
    assert_eq!(state.created, now);
    assert_eq!(state.bulk_delivery.egress(0, 0).bulk_delivered_bytes, 0);
    assert_eq!(
        state.activate_secure_channel(&TEST_POLICY, || panic!("not connected")),
        Err(TransportError::NotConnected)
    );
    // Exhaust the pre-auth bucket. A failed native setting must not reset it.
    for _ in 0..8 {
        assert_eq!(
            state.limiter.check(MessageClass::Control, 1, now),
            RateDecision::Allow
        );
    }
    assert!(state.mark_connected());
    assert!(!state.mark_connected());
    state.bulk_delivery.record_sent(7);
    assert_eq!(
        state.activate_secure_channel(&TEST_POLICY, || Err(TransportError::Backend(
            "test failure".into()
        ))),
        Err(TransportError::Backend("test failure".into()))
    );
    assert!(!state.authenticated && !state.ready);
    assert_eq!(state.created, now);
    assert_eq!(
        state.limiter.check(MessageClass::Control, 1, now),
        RateDecision::Disconnect
    );
    state
        .activate_secure_channel(&TEST_POLICY, || Ok(()))
        .unwrap();
    assert!(state.authenticated && !state.ready);
    assert_eq!(state.created, now);
    assert_eq!(state.bulk_delivery.egress(0, 0).bulk_delivered_bytes, 7);
    let now = Instant::now();
    // The supplied custom post-auth policy replaces the exhausted pre-auth one.
    for _ in 0..2 {
        assert_eq!(
            state.limiter.check(MessageClass::Control, 1, now),
            RateDecision::Allow
        );
    }
    assert_eq!(
        state.activate_secure_channel(&DEFAULT_INBOUND_POLICY, || panic!("already authenticated")),
        Err(TransportError::ProtocolViolation)
    );
    assert_eq!(
        state.limiter.check(MessageClass::Control, 1, now),
        RateDecision::Disconnect
    );
    state.mark_ready();
    state.mark_ready();
    assert!(state.connected && state.authenticated && state.ready);
}
