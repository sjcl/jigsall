//! Backend-independent authenticated channel. Bootstrap is the sole installer.
//! Plaintext connections only carry SessionControl; active channels never fall
//! back to plaintext. Consume each poll's events
//! before polling again: plaintext handshakes are per-connection batch barriers.
use super::{
    auth::AuthenticatedSecret,
    transport::*,
    wire::{self, WIRE_VERSION},
};
use chacha20poly1305::{aead::AeadInOut, ChaCha20Poly1305, KeyInit, Nonce, Tag};
use hkdf::Hkdf;
use sha2_channel::Sha256;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    net::SocketAddr,
};
use zeroize::Zeroizing;

pub const RECORD_OVERHEAD: usize = 8 + 16;
pub const fn record_limit(class: MessageClass) -> usize {
    wire::frame_limit(class) + RECORD_OVERHEAD
}
const RECORD_DOMAIN: &[u8; 25] = b"puzzella-secure-record-v1";
const KEY_LABELS: [&[u8]; 6] = [
    b"puzzella-secure-channel-v1/client-to-host/control",
    b"puzzella-secure-channel-v1/client-to-host/transient",
    b"puzzella-secure-channel-v1/client-to-host/bulk",
    b"puzzella-secure-channel-v1/host-to-client/control",
    b"puzzella-secure-channel-v1/host-to-client/transient",
    b"puzzella-secure-channel-v1/host-to-client/bulk",
];
#[derive(Clone, Copy)]
pub(crate) enum ChannelRole {
    Host,
    Client,
}
fn lane(class: MessageClass) -> usize {
    match class {
        MessageClass::Control => 0,
        MessageClass::Transient => 1,
        MessageClass::Bulk => 2,
    }
}
fn nonce(sequence: u64) -> Nonce {
    let mut bytes = [0; 12];
    bytes[4..].copy_from_slice(&sequence.to_le_bytes());
    bytes.into()
}
fn aad(class: MessageClass, sequence: u64) -> [u8; 36] {
    let mut bytes = [0; 36];
    bytes[..25].copy_from_slice(RECORD_DOMAIN);
    bytes[25..27].copy_from_slice(&WIRE_VERSION.to_le_bytes());
    bytes[27] = lane(class) as u8;
    bytes[28..].copy_from_slice(&sequence.to_le_bytes());
    bytes
}

struct Channel {
    keys: Zeroizing<[[u8; 32]; 6]>,
    send_offset: usize,
    recv_offset: usize,
    send_next: [u64; 3],
    recv_next: [u64; 3],
    transient_highest: Option<u64>,
}
impl Channel {
    fn new(secret: AuthenticatedSecret, role: ChannelRole) -> Self {
        let hk = Hkdf::<Sha256>::new(Some(b"puzzella-secure-channel-v1"), secret.as_bytes());
        let mut keys = Zeroizing::new([[0; 32]; 6]);
        for (key, label) in keys.iter_mut().zip(KEY_LABELS) {
            hk.expand_multi_info(&[label, &WIRE_VERSION.to_le_bytes(), secret.binding()], key)
                .expect("32-byte HKDF output is within the SHA-256 limit");
        }
        let (send_offset, recv_offset) = match role {
            ChannelRole::Client => (0, 3),
            ChannelRole::Host => (3, 0),
        };
        Self {
            keys,
            send_offset,
            recv_offset,
            send_next: [0; 3],
            recv_next: [0; 3],
            transient_highest: None,
        }
    }
    fn seal(&mut self, class: MessageClass, plaintext: &[u8]) -> Result<Vec<u8>, ()> {
        let index = lane(class);
        let sequence = self.send_next[index];
        // MAX is deliberately reserved; never wrap, and never roll back a nonce
        // after an ambiguous backend send error.
        if sequence == u64::MAX {
            return Err(());
        }
        self.send_next[index] += 1;
        let cipher = ChaCha20Poly1305::new_from_slice(&self.keys[self.send_offset + index])
            .map_err(|_| ())?;
        let mut record = Vec::with_capacity(plaintext.len() + RECORD_OVERHEAD);
        record.extend_from_slice(&sequence.to_le_bytes());
        record.extend_from_slice(plaintext);
        let tag = cipher
            .encrypt_inout_detached(
                &nonce(sequence),
                &aad(class, sequence),
                (&mut record[8..]).into(),
            )
            .map_err(|_| ())?;
        record.extend_from_slice(&tag);
        Ok(record)
    }
    fn open(&mut self, class: MessageClass, mut record: Vec<u8>) -> Result<Option<Vec<u8>>, ()> {
        // Check before any application allocation/decryption. Native backends
        // additionally check outer length before copying attacker-owned buffers.
        if record.len() < RECORD_OVERHEAD + wire::HEADER_SIZE || record.len() > record_limit(class)
        {
            return Err(());
        }
        let sequence = u64::from_le_bytes(record[..8].try_into().map_err(|_| ())?);
        if sequence == u64::MAX {
            return Err(());
        }
        let index = lane(class);
        if class == MessageClass::Transient {
            if self
                .transient_highest
                .is_some_and(|highest| sequence <= highest)
            {
                return Ok(None);
            }
        } else if sequence != self.recv_next[index] {
            return Err(());
        }
        let end = record.len() - 16;
        let tag: Tag = record[end..].try_into().map_err(|_| ())?;
        let cipher = ChaCha20Poly1305::new_from_slice(&self.keys[self.recv_offset + index])
            .map_err(|_| ())?;
        cipher
            .decrypt_inout_detached(
                &nonce(sequence),
                &aad(class, sequence),
                (&mut record[8..end]).into(),
                &tag,
            )
            .map_err(|_| ())?;
        // Only authenticated packets may advance receive state.
        if class == MessageClass::Transient {
            self.transient_highest = Some(sequence);
        } else {
            self.recv_next[index] += 1;
        }
        record.copy_within(8..end, 0);
        record.truncate(end - 8);
        Ok(Some(record))
    }
}

enum ConnectionChannel {
    Plaintext,
    Secure(Box<Channel>),
    Closed,
}
pub struct SecureTransport<T> {
    inner: T,
    connections: BTreeMap<ConnectionId, ConnectionChannel>,
    raw: VecDeque<TransportEvent>,
    pending: Vec<TransportEvent>,
}
impl<T: Transport> SecureTransport<T> {
    pub fn new(inner: T) -> Self {
        Self {
            inner,
            connections: BTreeMap::new(),
            raw: VecDeque::new(),
            pending: Vec::new(),
        }
    }
    pub fn has_channel(&self, connection: ConnectionId) -> bool {
        matches!(
            self.connections.get(&connection),
            Some(ConnectionChannel::Secure(_))
        )
    }
    // No public mutable-backend accessor or channel removal: callers cannot
    // downgrade an authenticated connection or bypass this Transport's send.
    pub(crate) fn start_connection(&mut self, connection: ConnectionId) {
        self.connections
            .insert(connection, ConnectionChannel::Plaintext);
    }
    pub(crate) fn forget_connection(&mut self, connection: ConnectionId) {
        // Bootstrap consumes our synthetic close before the backend's queued
        // lifecycle event. Keep this suppression marker until that event arrives.
        if !matches!(
            self.connections.get(&connection),
            Some(ConnectionChannel::Closed)
        ) {
            self.connections.remove(&connection);
        }
    }
    pub(crate) fn install(
        &mut self,
        connection: ConnectionId,
        secret: AuthenticatedSecret,
        role: ChannelRole,
    ) -> Result<(), TransportError> {
        let Some(state @ ConnectionChannel::Plaintext) = self.connections.get_mut(&connection)
        else {
            return Err(TransportError::ProtocolViolation);
        };
        self.inner.activate_secure_channel(connection)?;
        *state = ConnectionChannel::Secure(Box::new(Channel::new(secret, role)));
        Ok(())
    }
    fn fail(&mut self, connection: ConnectionId, reason: DisconnectReason) -> TransportEvent {
        // Key destruction and local rejection do not depend on backend close success.
        self.connections
            .insert(connection, ConnectionChannel::Closed);
        let _ = self.inner.close(connection, reason);
        TransportEvent::Disconnected { connection, reason }
    }
    #[cfg(test)]
    pub(crate) fn backend_mut(&mut self) -> &mut T {
        &mut self.inner
    }
}
impl<T: Transport> Transport for SecureTransport<T> {
    fn origin(&self, connection: ConnectionId) -> Option<super::transport::Origin> {
        self.inner.origin(connection)
    }
    fn reliable_egress(
        &self,
        connection: ConnectionId,
    ) -> Result<super::transport::ReliableEgress, TransportError> {
        self.inner.reliable_egress(connection)
    }
    fn mark_ready(&mut self, connection: ConnectionId) -> Result<(), TransportError> {
        self.inner.mark_ready(connection)
    }
    fn activate_secure_channel(&mut self, _connection: ConnectionId) -> Result<(), TransportError> {
        // Only install(), with an authenticated secret, may activate this wrapper.
        Err(TransportError::ProtocolViolation)
    }
    fn poll(&mut self, events: &mut Vec<TransportEvent>) -> Result<(), TransportError> {
        events.append(&mut self.pending);
        // Drain a bounded native batch completely before requesting another;
        // this also bounds deferred pre-auth data to one backend poll batch.
        if self.raw.is_empty() {
            let mut received = Vec::new();
            let result = self.inner.poll(&mut received);
            self.raw.extend(received);
            if let Err(error) = result {
                self.raw.clear();
                let live: Vec<_> = self
                    .connections
                    .iter()
                    .filter(|(_, state)| !matches!(state, ConnectionChannel::Closed))
                    .map(|(&id, _)| id)
                    .collect();
                for id in live {
                    events.push(self.fail(id, DisconnectReason::BackendFailure));
                }
                return Err(error);
            }
        }
        let mut barriers = BTreeSet::new();
        let count = self.raw.len();
        for _ in 0..count {
            let event = self.raw.pop_front().expect("initial raw length");
            let connection = match &event {
                TransportEvent::Connected { connection }
                | TransportEvent::Disconnected { connection, .. }
                | TransportEvent::ConnectionFailed { connection, .. }
                | TransportEvent::Message { connection, .. } => *connection,
            };
            if barriers.contains(&connection) {
                self.raw.push_back(event);
                continue;
            }
            match event {
                TransportEvent::Connected { .. } => {
                    if matches!(
                        self.connections.get(&connection),
                        Some(ConnectionChannel::Closed)
                    ) {
                        continue;
                    }
                    self.start_connection(connection);
                    barriers.insert(connection);
                    events.push(event);
                }
                TransportEvent::Disconnected { .. } | TransportEvent::ConnectionFailed { .. } => {
                    if !matches!(
                        self.connections.remove(&connection),
                        Some(ConnectionChannel::Closed)
                    ) {
                        events.push(event);
                    }
                }
                TransportEvent::Message { class, payload, .. } => {
                    let result = match self.connections.get_mut(&connection) {
                        Some(ConnectionChannel::Secure(channel)) => channel.open(class, payload),
                        Some(ConnectionChannel::Plaintext) => {
                            barriers.insert(connection);
                            // Only session-control headers pass before activation;
                            // bootstrap still owns body decoding and state gating.
                            if matches!(
                                wire::is_session_control_for_class(&payload, class),
                                Ok(true)
                            ) {
                                Ok(Some(payload))
                            } else {
                                Err(())
                            }
                        }
                        Some(ConnectionChannel::Closed) => continue,
                        None => Err(()),
                    };
                    match result {
                        Ok(Some(payload)) => events.push(TransportEvent::Message {
                            connection,
                            class,
                            payload,
                        }),
                        Ok(None) => {}
                        Err(()) => {
                            events.push(self.fail(connection, DisconnectReason::ProtocolViolation))
                        }
                    }
                }
            }
        }
        Ok(())
    }
    fn send(
        &mut self,
        connection: ConnectionId,
        class: MessageClass,
        payload: &[u8],
    ) -> Result<(), TransportError> {
        // Plaintext must reach the session-control gate even when oversized.
        if payload.len() > wire::frame_limit(class)
            && !matches!(
                self.connections.get(&connection),
                Some(ConnectionChannel::Plaintext)
            )
        {
            return Err(TransportError::PayloadTooLarge);
        }
        let record = match self.connections.get_mut(&connection) {
            Some(ConnectionChannel::Secure(channel)) => match channel.seal(class, payload) {
                Ok(record) => Some(record),
                Err(()) => {
                    let event = self.fail(connection, DisconnectReason::ProtocolViolation);
                    self.pending.push(event);
                    return Err(TransportError::ProtocolViolation);
                }
            },
            Some(ConnectionChannel::Closed) => return Err(TransportError::NotConnected),
            Some(ConnectionChannel::Plaintext) => {
                if !matches!(wire::is_session_control_for_class(payload, class), Ok(true)) {
                    let event = self.fail(connection, DisconnectReason::ProtocolViolation);
                    self.pending.push(event);
                    return Err(TransportError::ProtocolViolation);
                }
                None
            }
            None => return Err(TransportError::UnknownConnection),
        };
        let result = self
            .inner
            .send(connection, class, record.as_deref().unwrap_or(payload));
        if record.is_some() && result.is_err() {
            let event = self.fail(connection, DisconnectReason::BackendFailure);
            self.pending.push(event);
        }
        result
    }
    fn close(
        &mut self,
        connection: ConnectionId,
        reason: DisconnectReason,
    ) -> Result<(), TransportError> {
        if matches!(
            self.connections.get(&connection),
            Some(ConnectionChannel::Closed)
        ) {
            return Ok(());
        }
        if let std::collections::btree_map::Entry::Occupied(mut entry) =
            self.connections.entry(connection)
        {
            entry.insert(ConnectionChannel::Closed);
            self.pending
                .push(TransportEvent::Disconnected { connection, reason });
        }
        self.inner.close(connection, reason)
    }
}
impl<T: DirectIpTransport> DirectIpTransport for SecureTransport<T> {
    fn listen(&mut self, address: SocketAddr) -> Result<ListenerId, TransportError> {
        self.inner.listen(address)
    }
    fn listener_address(&self, listener: ListenerId) -> Result<SocketAddr, TransportError> {
        self.inner.listener_address(listener)
    }
    fn close_listener(&mut self, listener: ListenerId) -> Result<(), TransportError> {
        let close_result = self.inner.close_listener(listener);
        // Drain the backend's queued closes now, before any deferred raw tail
        // can be decrypted under a listener that the caller has already closed.
        // Preserve unrelated events in order for the next ordinary poll.
        let mut received = Vec::new();
        let result = self.inner.poll(&mut received);
        for event in received {
            match event {
                TransportEvent::Disconnected { connection, .. }
                | TransportEvent::ConnectionFailed { connection, .. } => {
                    self.raw.retain(|event| match event {
                        TransportEvent::Connected { connection: id }
                        | TransportEvent::Disconnected { connection: id, .. }
                        | TransportEvent::ConnectionFailed { connection: id, .. }
                        | TransportEvent::Message { connection: id, .. } => *id != connection,
                    });
                    if !matches!(
                        self.connections.remove(&connection),
                        Some(ConnectionChannel::Closed)
                    ) {
                        self.pending.push(event);
                    }
                }
                _ => self.raw.push_back(event),
            }
        }
        if let Err(error) = result {
            let live: Vec<_> = self
                .connections
                .iter()
                .filter(|(_, state)| !matches!(state, ConnectionChannel::Closed))
                .map(|(&id, _)| id)
                .collect();
            for id in live {
                let event = self.fail(id, DisconnectReason::BackendFailure);
                self.pending.push(event);
            }
            return Err(error);
        }
        close_result
    }
    fn connect(&mut self, address: SocketAddr) -> Result<ConnectionId, TransportError> {
        self.inner.connect(address)
    }
}

#[cfg(test)]
mod tests;
