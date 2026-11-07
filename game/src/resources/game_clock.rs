use std::time::Duration;

/// Shared interval accounting for offline and host-authoritative play time.
#[derive(Default)]
pub(crate) struct GameClock {
    epoch: Option<u64>,
    running: bool,
}

impl GameClock {
    pub(crate) fn advance(
        &mut self,
        epoch: u64,
        elapsed: Duration,
        running: bool,
        delta: Duration,
    ) -> Duration {
        // A frame's delta belongs to the preceding interval. Do not charge a
        // paused/loading interval on the first resumed frame or a new puzzle.
        let elapsed = if self.epoch == Some(epoch) && self.running && running {
            elapsed.saturating_add(delta)
        } else {
            elapsed
        };
        self.epoch = Some(epoch);
        self.running = running;
        elapsed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fractional_intervals_exclude_pause_resume_and_puzzle_changes() {
        let mut clock = GameClock::default();
        let mut elapsed = Duration::ZERO;
        let mut advance = |epoch, running, delta| {
            elapsed = clock.advance(epoch, elapsed, running, delta);
            elapsed
        };
        assert_eq!(advance(1, true, Duration::from_secs(60)), Duration::ZERO);
        for _ in 0..125 {
            advance(1, true, Duration::from_millis(16));
        }
        let frozen = Duration::from_secs(2);
        assert_eq!(advance(1, false, Duration::from_secs(60)), frozen);
        assert_eq!(advance(1, false, Duration::from_secs(3600)), frozen);
        assert_eq!(advance(1, true, Duration::from_secs(3600)), frozen);
        assert_eq!(
            advance(1, true, Duration::from_secs(3)),
            Duration::from_secs(5)
        );
        assert_eq!(
            advance(2, true, Duration::from_secs(60)),
            Duration::from_secs(5)
        );
        assert_eq!(
            advance(2, true, Duration::from_secs(1)),
            Duration::from_secs(6)
        );
    }
}
