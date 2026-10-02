//! Backend-independent inbound protection, checked before copying native payloads.
use super::transport::MessageClass;
use std::time::{Duration, Instant};

const NANOS_PER_SECOND: u128 = 1_000_000_000;

#[derive(Clone, Copy, Debug)]
pub struct BucketPolicy {
    pub bytes_per_second: u64,
    pub burst_bytes: u64,
    /// Charge tiny/empty messages too, bounding per-message work as well as bytes.
    pub minimum_charge: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct InboundRatePolicy {
    pub transient: BucketPolicy,
    pub control: BucketPolicy,
    pub bulk: BucketPolicy,
}

/// Generous two-second bursts; independent reliable lanes cannot spend drag credit.
pub const DEFAULT_INBOUND_POLICY: InboundRatePolicy = InboundRatePolicy {
    transient: BucketPolicy {
        bytes_per_second: 128 * 1024,
        burst_bytes: 256 * 1024,
        minimum_charge: 512,
    },
    control: BucketPolicy {
        bytes_per_second: 4 * 1024 * 1024,
        burst_bytes: 8 * 1024 * 1024,
        minimum_charge: 16 * 1024,
    },
    bulk: BucketPolicy {
        bytes_per_second: 8 * 1024 * 1024,
        burst_bytes: 16 * 1024 * 1024,
        minimum_charge: 32 * 1024,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RateDecision {
    Allow,
    Drop,
    Disconnect,
}

struct RateBucket {
    tokens: u64,
    last_refill: Instant,
    // Fractional byte credit in billionths; prevents frequent checks losing refill.
    fractional: u32,
}

impl RateBucket {
    fn new(policy: BucketPolicy, now: Instant) -> Self {
        Self {
            tokens: policy.burst_bytes,
            last_refill: now,
            fractional: 0,
        }
    }

    fn refill(&mut self, policy: BucketPolicy, elapsed: Duration) {
        let credit = elapsed
            .as_nanos()
            .saturating_mul(u128::from(policy.bytes_per_second))
            .saturating_add(u128::from(self.fractional));
        let earned = credit / NANOS_PER_SECOND;
        let available = policy.burst_bytes - self.tokens;
        if earned >= u128::from(available) {
            self.tokens = policy.burst_bytes;
            self.fractional = 0;
        } else {
            // earned < available <= u64::MAX, so neither cast nor sum overflows.
            self.tokens += earned as u64;
            self.fractional = (credit % NANOS_PER_SECOND) as u32;
        }
    }

    fn charge(&mut self, policy: BucketPolicy, charge: u64, now: Instant) -> bool {
        self.refill(policy, now.duration_since(self.last_refill));
        self.last_refill = now;
        if charge > self.tokens {
            return false;
        }
        self.tokens -= charge;
        true
    }
}

/// Store directly in each backend connection, from creation until removal.
/// Policies are shared immutable configuration; checks allocate nothing.
pub struct InboundRateLimiter {
    transient: RateBucket,
    control: RateBucket,
    bulk: RateBucket,
    policy: &'static InboundRatePolicy,
}

impl InboundRateLimiter {
    pub fn new(now: Instant) -> Self {
        Self::with_policy(&DEFAULT_INBOUND_POLICY, now)
    }

    pub fn with_policy(policy: &'static InboundRatePolicy, now: Instant) -> Self {
        Self {
            transient: RateBucket::new(policy.transient, now),
            control: RateBucket::new(policy.control, now),
            bulk: RateBucket::new(policy.bulk, now),
            policy,
        }
    }

    /// Charge the whole native payload (including any wire header) before decode.
    /// Reliable over-limit messages require closing the connection, never dropping
    /// a frame and continuing its ordered stream with a sequence gap.
    pub fn check(&mut self, class: MessageClass, payload_len: usize, now: Instant) -> RateDecision {
        let (bucket, policy) = match class {
            MessageClass::Transient => (&mut self.transient, self.policy.transient),
            MessageClass::Control => (&mut self.control, self.policy.control),
            MessageClass::Bulk => (&mut self.bulk, self.policy.bulk),
        };
        let charge = u64::try_from(payload_len)
            .unwrap_or(u64::MAX)
            .max(policy.minimum_charge);
        if bucket.charge(policy, charge, now) {
            RateDecision::Allow
        } else if class == MessageClass::Transient {
            RateDecision::Drop
        } else {
            RateDecision::Disconnect
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SMALL_BUCKET: BucketPolicy = BucketPolicy {
        bytes_per_second: 64,
        burst_bytes: 128,
        minimum_charge: 64,
    };
    const SMALL_POLICY: InboundRatePolicy = InboundRatePolicy {
        transient: SMALL_BUCKET,
        control: SMALL_BUCKET,
        bulk: SMALL_BUCKET,
    };

    #[test]
    fn initial_burst_spends_tokens_and_zero_elapsed_does_not_refill() {
        let now = Instant::now();
        let mut bucket = RateBucket::new(SMALL_BUCKET, now);
        assert_eq!(bucket.tokens, 128);
        assert!(bucket.charge(SMALL_BUCKET, 100, now));
        assert_eq!(bucket.tokens, 28);
        assert!(!bucket.charge(SMALL_BUCKET, 29, now));
        assert_eq!(bucket.tokens, 28);
        assert!(bucket.charge(SMALL_BUCKET, 28, now));
        assert_eq!(bucket.tokens, 0);
    }

    #[test]
    fn elapsed_refill_retains_fractional_credit_and_caps_capacity() {
        let now = Instant::now();
        let mut bucket = RateBucket::new(SMALL_BUCKET, now);
        assert!(bucket.charge(SMALL_BUCKET, 128, now));
        // Each individual check earns less than one byte; credit must accumulate.
        for millis in 1..=1000 {
            assert!(!bucket.charge(SMALL_BUCKET, 129, now + Duration::from_millis(millis)));
        }
        assert_eq!(bucket.tokens, 64);
        assert_eq!(bucket.fractional, 0);
        assert!(bucket.charge(SMALL_BUCKET, 64, now + Duration::from_secs(1)));
        bucket.refill(SMALL_BUCKET, Duration::from_secs(100));
        assert_eq!(bucket.tokens, 128);
        assert_eq!(bucket.fractional, 0);
    }

    #[test]
    fn long_idle_and_extreme_rates_cannot_overflow() {
        let policy = BucketPolicy {
            bytes_per_second: u64::MAX,
            burst_bytes: u64::MAX,
            minimum_charge: 64,
        };
        let mut bucket = RateBucket::new(policy, Instant::now());
        bucket.tokens = 0;
        bucket.fractional = 999_999_999;
        bucket.refill(policy, Duration::MAX);
        assert_eq!(bucket.tokens, u64::MAX);
        assert_eq!(bucket.fractional, 0);
        bucket.refill(policy, Duration::MAX);
        assert_eq!(bucket.tokens, u64::MAX);
    }

    #[test]
    fn minimum_charge_limits_empty_and_tiny_messages_for_each_class() {
        let now = Instant::now();
        for class in [
            MessageClass::Transient,
            MessageClass::Control,
            MessageClass::Bulk,
        ] {
            let excess = if class == MessageClass::Transient {
                RateDecision::Drop
            } else {
                RateDecision::Disconnect
            };
            for payload_len in [0, 1, 63, 64] {
                let mut limiter = InboundRateLimiter::with_policy(&SMALL_POLICY, now);
                assert_eq!(limiter.check(class, payload_len, now), RateDecision::Allow);
                assert_eq!(limiter.check(class, payload_len, now), RateDecision::Allow);
                assert_eq!(limiter.check(class, payload_len, now), excess);
            }
            // Payload bytes above the minimum still count in full.
            let mut limiter = InboundRateLimiter::with_policy(&SMALL_POLICY, now);
            assert_eq!(limiter.check(class, 129, now), excess);
        }
    }

    #[test]
    fn default_class_minimums_bound_tiny_bursts_and_steady_message_rates() {
        let now = Instant::now();
        for class in [
            MessageClass::Transient,
            MessageClass::Control,
            MessageClass::Bulk,
        ] {
            let excess = if class == MessageClass::Transient {
                RateDecision::Drop
            } else {
                RateDecision::Disconnect
            };
            for payload_len in [0, 1, 63] {
                let mut limiter = InboundRateLimiter::new(now);
                // All three policies bound a full tiny-message burst to 512.
                for _ in 0..512 {
                    assert_eq!(limiter.check(class, payload_len, now), RateDecision::Allow);
                }
                assert_eq!(limiter.check(class, payload_len, now), excess);
                // One second restores only 256 tiny messages of credit.
                let later = now + Duration::from_secs(1);
                for _ in 0..256 {
                    assert_eq!(
                        limiter.check(class, payload_len, later),
                        RateDecision::Allow
                    );
                }
                assert_eq!(limiter.check(class, payload_len, later), excess);
            }
        }
    }

    #[test]
    fn transient_excess_drops_and_refill_allows_recovery() {
        let now = Instant::now();
        let mut limiter = InboundRateLimiter::with_policy(&SMALL_POLICY, now);
        assert_eq!(
            limiter.check(MessageClass::Transient, 128, now),
            RateDecision::Allow
        );
        for millis in 0..1000 {
            assert_eq!(
                limiter.check(
                    MessageClass::Transient,
                    1,
                    now + Duration::from_millis(millis)
                ),
                RateDecision::Drop
            );
        }
        assert_eq!(
            limiter.check(MessageClass::Transient, 1, now + Duration::from_secs(1)),
            RateDecision::Allow
        );
    }

    #[test]
    fn reliable_bursts_allow_but_sustained_excess_disconnects() {
        let now = Instant::now();
        for class in [MessageClass::Control, MessageClass::Bulk] {
            let mut limiter = InboundRateLimiter::with_policy(&SMALL_POLICY, now);
            assert_eq!(limiter.check(class, 64, now), RateDecision::Allow);
            assert_eq!(limiter.check(class, 64, now), RateDecision::Allow);
            // Twice the steady rate drains the burst; never silently Drop.
            assert_eq!(
                limiter.check(class, 64, now + Duration::from_millis(500)),
                RateDecision::Disconnect
            );
        }
    }

    #[test]
    fn buckets_and_connections_are_independent() {
        let now = Instant::now();
        let mut a = InboundRateLimiter::with_policy(&SMALL_POLICY, now);
        let mut b = InboundRateLimiter::with_policy(&SMALL_POLICY, now);
        assert_eq!(
            a.check(MessageClass::Transient, 128, now),
            RateDecision::Allow
        );
        assert_eq!(a.check(MessageClass::Transient, 1, now), RateDecision::Drop);
        assert_eq!(
            a.check(MessageClass::Control, 128, now),
            RateDecision::Allow
        );
        assert_eq!(a.check(MessageClass::Bulk, 128, now), RateDecision::Allow);
        assert_eq!(
            b.check(MessageClass::Transient, 128, now),
            RateDecision::Allow
        );
    }

    #[test]
    fn defaults_allow_120_hz_drags_and_maximum_frame_bursts() {
        use crate::network::secure::record_limit;
        let now = Instant::now();
        let mut limiter = InboundRateLimiter::new(now);
        for tick in 0..7200 {
            assert_eq!(
                limiter.check(
                    MessageClass::Transient,
                    record_limit(MessageClass::Transient),
                    now + Duration::from_nanos(tick * 1_000_000_000 / 120)
                ),
                RateDecision::Allow
            );
        }
        for class in [MessageClass::Control, MessageClass::Bulk] {
            for _ in 0..16 {
                assert_eq!(
                    limiter.check(class, record_limit(class), now),
                    RateDecision::Allow
                );
            }
        }
    }

    #[test]
    fn zero_rate_and_zero_capacity_are_well_defined() {
        const ZERO: BucketPolicy = BucketPolicy {
            bytes_per_second: 0,
            burst_bytes: 0,
            minimum_charge: 64,
        };
        const POLICY: InboundRatePolicy = InboundRatePolicy {
            transient: ZERO,
            control: ZERO,
            bulk: ZERO,
        };
        let now = Instant::now();
        let mut limiter = InboundRateLimiter::with_policy(&POLICY, now);
        assert_eq!(
            limiter.check(MessageClass::Transient, 0, now + Duration::from_secs(10)),
            RateDecision::Drop
        );
        assert_eq!(
            limiter.check(MessageClass::Control, 0, now + Duration::from_secs(10)),
            RateDecision::Disconnect
        );
        assert_eq!(
            limiter.check(MessageClass::Bulk, 0, now + Duration::from_secs(10)),
            RateDecision::Disconnect
        );
    }
}
