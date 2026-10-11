//! Shared monotonic response, bulk-progress and lifetime accounting.
use super::*;

#[derive(Clone, Copy, Debug)]
pub(super) struct BulkProgress {
    pub(super) started: Instant,
    pub(super) last_delivery: Instant,
    pub(super) delivered_base: u64,
    pub(super) delivered: u64,
    pub(super) size: u64,
}
#[derive(Clone, Copy, Debug)]
pub struct SyncTiming {
    pub started: Instant,
    pub phase_entered: Instant,
    pub last_peer_progress: Instant,
    pub(super) response_wait: Option<(ResponseWait, Instant)>,
    pub(super) bulk: Option<BulkProgress>,
    extra_lifetime: Duration,
    image_budgeted: bool,
    baseline_budgeted: bool,
}
impl SyncTiming {
    pub fn waiting_for(self) -> Option<ResponseWait> {
        self.response_wait.map(|(wait, _)| wait)
    }
    pub fn bulk_progress_at(self) -> Option<Instant> {
        self.bulk.map(|b| b.last_delivery)
    }
    pub(super) fn new(now: Instant) -> Self {
        Self {
            started: now,
            phase_entered: now,
            last_peer_progress: now,
            response_wait: None,
            bulk: None,
            extra_lifetime: Duration::ZERO,
            image_budgeted: false,
            baseline_budgeted: false,
        }
    }
    pub(super) fn wait(&mut self, response: ResponseWait, now: Instant) {
        // Host retries/activity cannot postpone an outstanding response.
        if self.response_wait.is_none() {
            self.phase_entered = now;
            self.response_wait = Some((response, now));
        }
    }
    pub(super) fn peer_progress(&mut self, now: Instant) {
        self.last_peer_progress = now;
        self.response_wait = None;
    }
    pub(super) fn budget(&mut self, kind: BulkTransferKind, size: u64, policy: SyncPolicy) {
        let credited = match kind {
            BulkTransferKind::PuzzleImage => &mut self.image_budgeted,
            BulkTransferKind::JoinBaseline => &mut self.baseline_budgeted,
        };
        if !*credited {
            *credited = true;
            // Credited once per kind, never renewed by restarts or packets.
            self.extra_lifetime += policy.transfer_budget(size);
        }
    }
    pub(super) fn start_bulk(&mut self, size: u64, delivered_base: u64, now: Instant) {
        self.response_wait = None;
        self.phase_entered = now;
        self.bulk = Some(BulkProgress {
            started: now,
            last_delivery: now,
            delivered_base,
            delivered: 0,
            size,
        });
    }
    pub(super) fn observe_delivery(&mut self, delivered: u64, now: Instant) {
        if let Some(bulk) = &mut self.bulk {
            let bytes = delivered.saturating_sub(bulk.delivered_base).min(bulk.size);
            if bytes > bulk.delivered {
                bulk.delivered = bytes;
                bulk.last_delivery = now;
            }
        }
    }
    pub fn timeout(self, now: Instant, policy: SyncPolicy) -> Option<SyncError> {
        if now.saturating_duration_since(self.started)
            >= policy.sync_hard_limit + self.extra_lifetime
        {
            return Some(SyncError::LifetimeTimeout);
        }
        if let Some((response, entered)) = self.response_wait {
            if now.saturating_duration_since(entered) >= policy.control_response {
                return Some(SyncError::PhaseTimeout(response));
            }
        }
        if let Some(bulk) = self.bulk {
            let elapsed = now.saturating_duration_since(bulk.started);
            let required = elapsed
                .saturating_sub(policy.throughput_grace)
                .as_secs_f64()
                * policy.minimum_bytes_per_second as f64;
            if now.saturating_duration_since(bulk.last_delivery) >= policy.bulk_stall
                || elapsed >= policy.transfer_budget(bulk.size)
                || (bulk.delivered as f64) < required.min(bulk.size as f64)
            {
                return Some(SyncError::BulkStalled);
            }
        }
        None
    }
}

#[cfg(test)]
mod timing_tests {
    use super::*;
    #[test]
    pub(super) fn progressing_large_transfer_survives_300_seconds_but_slowloris_does_not() {
        let now = Instant::now();
        let policy = SyncPolicy::default();
        let size = MAX_PUZZLE_IMAGE_TRANSFER_BYTES;
        let mut timing = SyncTiming::new(now);
        timing.budget(BulkTransferKind::PuzzleImage, size, policy);
        timing.start_bulk(size, 100, now);
        for second in (10..=400).step_by(10) {
            let at = now + Duration::from_secs(second);
            timing.observe_delivery(100 + second * policy.minimum_bytes_per_second, at);
            assert_eq!(timing.timeout(at, policy), None);
        }
        assert_eq!(
            timing.timeout(now + Duration::from_secs(430), policy),
            Some(SyncError::BulkStalled)
        );
        let mut slowloris = SyncTiming::new(now);
        slowloris.budget(BulkTransferKind::PuzzleImage, size, policy);
        slowloris.start_bulk(size, 0, now);
        for second in 1..=30 {
            slowloris.observe_delivery(second, now + Duration::from_secs(second));
        }
        slowloris.observe_delivery(31, now + Duration::from_secs(31));
        assert_eq!(
            slowloris.timeout(now + Duration::from_secs(31), policy),
            Some(SyncError::BulkStalled)
        );
    }
    #[test]
    pub(super) fn retries_messages_and_restarts_cannot_renew_hard_lifetime() {
        let now = Instant::now();
        let policy = SyncPolicy::default();
        let mut timing = SyncTiming::new(now);
        timing.budget(BulkTransferKind::PuzzleImage, 1024, policy);
        let deadline = now + policy.sync_hard_limit + timing.extra_lifetime;
        for n in 0..100 {
            timing.peer_progress(now + Duration::from_secs(n));
            timing.budget(
                BulkTransferKind::PuzzleImage,
                MAX_PUZZLE_IMAGE_TRANSFER_BYTES,
                policy,
            );
        }
        assert_eq!(
            timing.timeout(deadline, policy),
            Some(SyncError::LifetimeTimeout)
        );
        timing.wait(ResponseWait::FinalizeAck, now);
        timing.wait(ResponseWait::FinalizeAck, now + Duration::from_secs(11));
        assert_eq!(
            timing.timeout(now + policy.control_response, policy),
            Some(SyncError::PhaseTimeout(ResponseWait::FinalizeAck))
        );
    }
}
