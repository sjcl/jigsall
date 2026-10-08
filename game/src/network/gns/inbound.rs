//! Shared receive policy, after native lane and payload metadata validation.
use crate::network::{
    rate_limit::{InboundRateLimiter, RateDecision},
    secure::record_limit,
    transport::{DisconnectReason, MessageClass},
    wire,
};
use std::time::Instant;

pub(super) use super::policy::class;
pub(super) const MAX_RECEIVE_PER_POLL: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum InboundDecision {
    Allow,
    Drop,
    Disconnect(DisconnectReason),
}

/// Inspect borrowed bytes without copying or decoding the body. Invalid records
/// take precedence over rate limits and never spend rate credit. The caller
/// retains native ownership and may copy only after Allow; handshake barriers
/// and receive budgets remain the responsibility of each transport's loop.
pub(super) fn check_message(
    class: MessageClass,
    payload: &[u8],
    authenticated: bool,
    limiter: &mut InboundRateLimiter,
    now: Instant,
) -> InboundDecision {
    if payload.len() > record_limit(class)
        || (!authenticated
            && !matches!(wire::is_session_control_for_class(payload, class), Ok(true)))
    {
        return InboundDecision::Disconnect(DisconnectReason::InvalidMessage);
    }
    match limiter.check(class, payload.len(), now) {
        RateDecision::Allow => InboundDecision::Allow,
        RateDecision::Drop => InboundDecision::Drop,
        RateDecision::Disconnect => InboundDecision::Disconnect(DisconnectReason::RateLimited),
    }
}

#[cfg(test)]
mod tests;
