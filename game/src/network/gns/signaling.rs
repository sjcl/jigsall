//! Caller-owned opaque signaling queues. A rendezvous adapter drains outbound
//! signals and enqueues inbound signals once per frame; no network runtime here.
use crate::network::transport::TransportError;
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
};

pub const MAX_SIGNAL_BYTES: usize = 16 * 1024;
pub const MAX_QUEUED_SIGNALS: usize = 128;
pub const MAX_SIGNAL_QUEUE_BYTES: usize = 256 * 1024;

/// Random process identity used only for signaling routing, never authentication.
/// The bytes are a UUID-sized application identity, not a GNS handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PeerId([u8; 16]);
impl PeerId {
    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }
    pub fn to_bytes(self) -> [u8; 16] {
        self.0
    }
    pub(super) fn random() -> Self {
        Self(*uuid::Uuid::new_v4().as_bytes())
    }
}

#[derive(Debug)]
pub struct OutboundSignal {
    pub peer: PeerId,
    pub payload: Vec<u8>,
}

#[derive(Default)]
struct Queue {
    messages: VecDeque<OutboundSignal>,
    bytes: usize,
}
impl Queue {
    fn push(&mut self, peer: PeerId, payload: &[u8]) -> Result<(), TransportError> {
        if payload.is_empty() || payload.len() > MAX_SIGNAL_BYTES {
            return Err(TransportError::PayloadTooLarge);
        }
        if self.messages.len() >= MAX_QUEUED_SIGNALS
            || self.bytes + payload.len() > MAX_SIGNAL_QUEUE_BYTES
        {
            return Err(TransportError::Backpressure);
        }
        self.bytes += payload.len();
        self.messages.push_back(OutboundSignal {
            peer,
            payload: payload.to_vec(),
        });
        Ok(())
    }
    fn pop(&mut self) -> Option<OutboundSignal> {
        let signal = self.messages.pop_front()?;
        self.bytes -= signal.payload.len();
        Some(signal)
    }
}
#[derive(Default)]
struct Queues {
    closed: bool,
    inbound: Queue,
    outbound: Queue,
}

/// Thread-safe mailbox; cloning it does not keep native sockets alive.
/// Queue locks are always released before entering GNS or an external adapter.
#[derive(Clone, Default)]
pub struct SignalingEndpoint(Arc<Mutex<Queues>>);
impl SignalingEndpoint {
    pub fn receive(&self, peer: PeerId, payload: &[u8]) -> Result<(), TransportError> {
        let mut queues = self
            .0
            .lock()
            .map_err(|_| TransportError::ProtocolViolation)?;
        if queues.closed {
            return Err(TransportError::NotConnected);
        }
        queues.inbound.push(peer, payload)
    }
    pub fn pop_outbound(&self) -> Option<OutboundSignal> {
        self.0.lock().ok()?.outbound.pop()
    }
    pub(super) fn pop_inbound(&self) -> Option<OutboundSignal> {
        self.0.lock().ok()?.inbound.pop()
    }
    pub(super) fn send(&self, peer: PeerId, payload: &[u8]) -> bool {
        let Ok(mut queues) = self.0.lock() else {
            return false;
        };
        !queues.closed && queues.outbound.push(peer, payload).is_ok()
    }
    pub(super) fn close(&self) {
        if let Ok(mut queues) = self.0.lock() {
            queues.closed = true;
            queues.inbound = Queue::default();
            queues.outbound = Queue::default();
        }
    }
}

/// Fake rendezvous: routes bytes verbatim in memory, with bounded peer/queue
/// counts. Undeliverable signals are best-effort drops; GNS retries/timeouts.
#[derive(Default)]
pub struct InMemorySignaling(BTreeMap<PeerId, SignalingEndpoint>);
impl InMemorySignaling {
    pub fn register(
        &mut self,
        peer: PeerId,
        endpoint: SignalingEndpoint,
    ) -> Result<(), TransportError> {
        if self.0.contains_key(&peer) {
            return Err(TransportError::ProtocolViolation);
        }
        if self.0.len() >= crate::network::lifecycle::MAX_CONNECTIONS {
            return Err(TransportError::Capacity);
        }
        self.0.insert(peer, endpoint);
        Ok(())
    }
    pub fn unregister(&mut self, peer: PeerId) {
        self.0.remove(&peer);
    }
    pub fn poll(&mut self) {
        for (&from, endpoint) in &self.0 {
            for _ in 0..MAX_QUEUED_SIGNALS {
                let Some(signal) = endpoint.pop_outbound() else {
                    break;
                };
                if let Some(target) = self.0.get(&signal.peer) {
                    let _ = target.receive(from, &signal.payload);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signaling_is_opaque_bounded_and_closed_mailboxes_do_not_revive() {
        let a = PeerId::from_bytes([1; 16]);
        let b = PeerId::from_bytes([2; 16]);
        let qa = SignalingEndpoint::default();
        let qb = SignalingEndpoint::default();
        let mut relay = InMemorySignaling::default();
        relay.register(a, qa.clone()).unwrap();
        relay.register(b, qb.clone()).unwrap();
        assert!(qa.send(b, &[0, 255, 7]));
        relay.poll();
        let message = qb.pop_inbound().unwrap();
        assert_eq!((message.peer, message.payload), (a, vec![0, 255, 7]));
        for _ in 0..MAX_QUEUED_SIGNALS {
            qb.receive(a, &[7]).unwrap();
        }
        assert_eq!(qb.receive(a, &[7]), Err(TransportError::Backpressure));
        qb.close();
        assert!(qb.pop_inbound().is_none());
        assert_eq!(qb.receive(a, &[7]), Err(TransportError::NotConnected));
        assert!(!qb.send(a, &[7]));
        assert_eq!(
            qa.receive(b, &vec![0; MAX_SIGNAL_BYTES + 1]),
            Err(TransportError::PayloadTooLarge)
        );
        for _ in 0..MAX_SIGNAL_QUEUE_BYTES / MAX_SIGNAL_BYTES {
            qa.receive(b, &vec![0; MAX_SIGNAL_BYTES]).unwrap();
        }
        assert_eq!(qa.receive(b, &[1]), Err(TransportError::Backpressure));
    }
}
