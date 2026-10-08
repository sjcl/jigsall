use super::*;
use crate::network::{
    rate_limit::{BucketPolicy, InboundRatePolicy, PREAUTH_INBOUND_POLICY},
    session_control::SessionControlMessage,
};

const CLASSES: [MessageClass; 3] = [
    MessageClass::Transient,
    MessageClass::Control,
    MessageClass::Bulk,
];
const INVALID: InboundDecision = InboundDecision::Disconnect(DisconnectReason::InvalidMessage);
const RATE_LIMITED: InboundDecision = InboundDecision::Disconnect(DisconnectReason::RateLimited);
const BUCKET: BucketPolicy = BucketPolicy {
    bytes_per_second: 0,
    burst_bytes: 128,
    minimum_charge: 64,
};
const POLICY: InboundRatePolicy = InboundRatePolicy {
    transient: BUCKET,
    control: BUCKET,
    bulk: BUCKET,
};

fn session_control(size: usize) -> Vec<u8> {
    let mut payload = vec![0; wire::HEADER_SIZE + size];
    payload[..4].copy_from_slice(b"PZLA");
    payload[4..6].copy_from_slice(&wire::WIRE_VERSION.to_le_bytes());
    payload[6] = 6;
    payload[8..12].copy_from_slice(&(size as u32).to_le_bytes());
    payload
}

#[test]
fn lanes_accept_only_the_three_message_classes() {
    for (lane, expected) in CLASSES.into_iter().enumerate() {
        assert_eq!(class(lane as u16), Some(expected));
    }
    for lane in 3..=u16::MAX {
        assert_eq!(class(lane), None);
    }
}

#[test]
fn valid_preauth_session_control_and_authenticated_records_are_allowed() {
    let now = Instant::now();
    let handshake = wire::encode(&wire::WireMessage::SessionControl(
        SessionControlMessage::SecureChannelReady,
    ))
    .unwrap();
    let mut limiter = InboundRateLimiter::with_policy(&PREAUTH_INBOUND_POLICY, now);
    for payload in [
        handshake,
        session_control(wire::MAX_SESSION_CONTROL_PAYLOAD),
    ] {
        assert_eq!(
            check_message(MessageClass::Control, &payload, false, &mut limiter, now),
            InboundDecision::Allow
        );
    }
    for class in CLASSES {
        for len in [0, 1, wire::frame_limit(class), record_limit(class)] {
            let mut limiter = InboundRateLimiter::new(now);
            // Secured records are opaque here; body decoding belongs upstream.
            assert_eq!(
                check_message(class, &vec![0xff; len], true, &mut limiter, now),
                InboundDecision::Allow
            );
        }
    }
}

#[test]
fn class_record_limits_reject_before_rate_limits_even_after_exhaustion() {
    let now = Instant::now();
    for class in CLASSES {
        let mut limiter = InboundRateLimiter::with_policy(&POLICY, now);
        for _ in 0..2 {
            assert_eq!(
                check_message(class, &[], true, &mut limiter, now),
                InboundDecision::Allow
            );
        }
        for authenticated in [false, true] {
            assert_eq!(
                check_message(
                    class,
                    &vec![0; record_limit(class) + 1],
                    authenticated,
                    &mut limiter,
                    now
                ),
                INVALID
            );
        }
    }
}

#[test]
fn preauth_gate_rejects_bad_headers_lengths_and_wrong_classes_without_charging() {
    let now = Instant::now();
    let valid = session_control(1);
    let mut cases = vec![
        vec![],
        vec![0; wire::HEADER_SIZE - 1],
        session_control(wire::MAX_SESSION_CONTROL_PAYLOAD + 1),
    ];
    // Bad magic/version, gameplay/unknown kind, reserved bits, declared length.
    for (index, value) in [(0, 0), (4, 0xff), (6, 1), (6, 0xff), (7, 1), (8, 2)] {
        let mut invalid = valid.clone();
        invalid[index] = value;
        cases.push(invalid);
    }
    let mut trailing = session_control(wire::MAX_SESSION_CONTROL_PAYLOAD);
    trailing.push(0); // Declared length is legal, actual length is not.
    cases.push(trailing);
    let mut declared_oversize = valid.clone();
    declared_oversize[8..12]
        .copy_from_slice(&((wire::MAX_SESSION_CONTROL_PAYLOAD + 1) as u32).to_le_bytes());
    cases.push(declared_oversize);

    let mut limiter = InboundRateLimiter::with_policy(&POLICY, now);
    for payload in cases {
        assert_eq!(
            check_message(MessageClass::Control, &payload, false, &mut limiter, now),
            INVALID
        );
    }
    for class in [MessageClass::Transient, MessageClass::Bulk] {
        assert_eq!(
            check_message(class, &valid, false, &mut limiter, now),
            INVALID
        );
    }
    // Rejections did not spend any class's tokens.
    for class in CLASSES {
        for _ in 0..2 {
            assert_eq!(
                check_message(class, &valid, true, &mut limiter, now),
                InboundDecision::Allow
            );
        }
    }
    // Malformed pre-auth data still wins over an exhausted Control bucket.
    assert_eq!(
        check_message(MessageClass::Control, &[], false, &mut limiter, now),
        INVALID
    );
}

#[test]
fn reliable_excess_disconnects_and_transient_excess_drops() {
    let now = Instant::now();
    for class in CLASSES {
        for len in [0, 1, 64, 128] {
            let mut limiter = InboundRateLimiter::with_policy(&POLICY, now);
            let payload = vec![0; len];
            for _ in 0..BUCKET.burst_bytes as usize / len.max(BUCKET.minimum_charge as usize) {
                assert_eq!(
                    check_message(class, &payload, true, &mut limiter, now),
                    InboundDecision::Allow
                );
            }
            assert_eq!(
                check_message(class, &payload, true, &mut limiter, now),
                if class == MessageClass::Transient {
                    InboundDecision::Drop
                } else {
                    RATE_LIMITED
                }
            );
        }
    }
}

#[test]
fn preauth_rate_limits_charge_the_full_frame() {
    let now = Instant::now();
    for (size, allowed) in [(1, 8), (wire::MAX_SESSION_CONTROL_PAYLOAD, 7)] {
        let mut limiter = InboundRateLimiter::with_policy(&PREAUTH_INBOUND_POLICY, now);
        let payload = session_control(size);
        for _ in 0..allowed {
            assert_eq!(
                check_message(MessageClass::Control, &payload, false, &mut limiter, now),
                InboundDecision::Allow
            );
        }
        assert_eq!(
            check_message(MessageClass::Control, &payload, false, &mut limiter, now),
            RATE_LIMITED
        );
    }
}

#[test]
fn p2p_native_cap_cannot_reject_a_record_allowed_by_the_shared_policy() {
    let native_cap = ::gns::sys::k_cbMaxSteamNetworkingSocketsMessageSizeSend as usize;
    for class in CLASSES {
        assert!(record_limit(class) <= native_cap);
    }
}
