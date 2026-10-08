//! Shared send checks and conservative Bulk delivery accounting.
use crate::network::{
    lifecycle::{MAX_BULK_QUEUE_BYTES, MAX_RELIABLE_QUEUE_BYTES},
    secure::record_limit,
    transport::{MessageClass, ReliableEgress, TransportError},
};
use std::cell::Cell;

/// Run before connection lookup, preserving PayloadTooLarge precedence.
pub(super) fn check_payload_size(class: MessageClass, len: usize) -> Result<(), TransportError> {
    if len > record_limit(class) {
        Err(TransportError::PayloadTooLarge)
    } else {
        Ok(())
    }
}

/// None also skips the native queue query: Transient never waits for reliability.
pub(super) fn queue_limit(class: MessageClass) -> Option<u64> {
    match class {
        MessageClass::Transient => None,
        MessageClass::Control => Some(MAX_RELIABLE_QUEUE_BYTES),
        MessageClass::Bulk => Some(MAX_BULK_QUEUE_BYTES),
    }
}

/// Native pending + sent-unacknowledged bytes, with room for record framing.
pub(super) fn check_queue(queued: u64, len: usize, limit: u64) -> Result<(), TransportError> {
    let reservation = (len as u64).saturating_add(64);
    if queued.saturating_add(reservation) > limit {
        Err(TransportError::Backpressure)
    } else {
        Ok(())
    }
}

#[derive(Default)]
pub(super) struct BulkDelivery {
    enqueued: u64,
    delivered: Cell<u64>,
}

impl BulkDelivery {
    /// Call only after a successful native Bulk send.
    pub(super) fn record_sent(&mut self, len: usize) {
        self.enqueued = self.enqueued.saturating_add(len as u64);
    }

    pub(super) fn egress(&self, queued_bytes: u64, bulk_queued_bytes: u64) -> ReliableEgress {
        // Native backlog includes framing: subtraction underestimates delivery.
        // A high-water mark makes this conservative estimate monotonic.
        let delivered = self
            .delivered
            .get()
            .max(self.enqueued.saturating_sub(bulk_queued_bytes));
        self.delivered.set(delivered);
        ReliableEgress {
            queued_bytes,
            bulk_queued_bytes,
            bulk_delivered_bytes: delivered,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_boundaries_for_every_class() {
        for class in [
            MessageClass::Transient,
            MessageClass::Control,
            MessageClass::Bulk,
        ] {
            let limit = record_limit(class);
            for len in [0, limit - 1, limit] {
                assert_eq!(check_payload_size(class, len), Ok(()));
            }
            for len in [limit + 1, usize::MAX] {
                assert_eq!(
                    check_payload_size(class, len),
                    Err(TransportError::PayloadTooLarge)
                );
            }
        }
    }

    #[test]
    fn reliable_queue_boundaries_include_framing_and_saturate() {
        for (class, expected) in [
            (MessageClass::Control, MAX_RELIABLE_QUEUE_BYTES),
            (MessageClass::Bulk, MAX_BULK_QUEUE_BYTES),
        ] {
            let limit = queue_limit(class).unwrap();
            assert_eq!(limit, expected);
            for len in [0, 1, record_limit(class)] {
                let boundary = limit - len as u64 - 64;
                for queued in [0, boundary - 1, boundary] {
                    assert_eq!(check_queue(queued, len, limit), Ok(()));
                }
                for queued in [boundary + 1, limit, u64::MAX - 63, u64::MAX] {
                    assert_eq!(
                        check_queue(queued, len, limit),
                        Err(TransportError::Backpressure)
                    );
                }
            }
            for queued in [0, u64::MAX] {
                assert_eq!(
                    check_queue(queued, usize::MAX, limit),
                    Err(TransportError::Backpressure)
                );
            }
        }
    }

    #[test]
    fn transient_skips_queue_checks() {
        assert_eq!(queue_limit(MessageClass::Transient), None);
    }

    #[test]
    fn bulk_delivery_is_conservative_and_monotonic_with_growing_backlogs() {
        let mut delivery = BulkDelivery::default();
        assert_eq!(delivery.egress(500, 500).bulk_delivered_bytes, 0);
        delivery.record_sent(100);
        for (bulk, expected) in [(164, 0), (80, 20), (90, 20), (0, 100)] {
            let egress = delivery.egress(bulk + 700, bulk);
            assert_eq!(egress.queued_bytes, bulk + 700);
            assert_eq!(egress.bulk_queued_bytes, bulk);
            assert_eq!(egress.bulk_delivered_bytes, expected);
        }
        delivery.record_sent(200);
        for (bulk, expected) in [(264, 100), (150, 150), (u64::MAX, 150), (0, 300)] {
            assert_eq!(delivery.egress(bulk, bulk).bulk_delivered_bytes, expected);
        }
    }

    #[test]
    fn bulk_enqueued_saturates_instead_of_wrapping() {
        let mut delivery = BulkDelivery {
            enqueued: u64::MAX - 1,
            ..Default::default()
        };
        delivery.record_sent(2);
        assert_eq!(delivery.egress(1, 1).bulk_delivered_bytes, u64::MAX - 1);
        delivery.record_sent(1);
        assert_eq!(delivery.egress(0, 0).bulk_delivered_bytes, u64::MAX);
    }
}
