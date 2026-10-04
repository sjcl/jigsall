//! Backend-independent transfer framing for one secure connection/direction.
//! Reliable ordered delivery permits contiguous chunks, without sparse ranges or
//! tombstones. SHA-256 detects content corruption; it provides no authorization.
use serde::{de::SeqAccess, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fmt, sync::Arc};

pub const MAX_BULK_DATA_BYTES: usize = 32 * 1024 - 64;
pub const MAX_JOIN_BASELINE_TRANSFER_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_PUZZLE_IMAGE_TRANSFER_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_ACTIVE_BULK_TRANSFERS: usize = 2;
pub const MAX_TOTAL_BULK_IN_FLIGHT_BYTES: u64 =
    MAX_JOIN_BASELINE_TRANSFER_BYTES + MAX_PUZZLE_IMAGE_TRANSFER_BYTES;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TransferId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BulkTransferKind {
    JoinBaseline,
    PuzzleImage,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BulkTransferMessage {
    Start {
        transfer_id: TransferId,
        kind: BulkTransferKind,
        total_size: u64,
        sha256: [u8; 32],
    },
    Chunk {
        transfer_id: TransferId,
        offset: u64,
        #[serde(deserialize_with = "deserialize_chunk_data")]
        data: Vec<u8>,
    },
    Finish {
        transfer_id: TransferId,
    },
    Abort {
        transfer_id: TransferId,
    },
}

fn deserialize_chunk_data<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<u8>, D::Error> {
    struct ChunkVisitor;
    impl<'de> serde::de::Visitor<'de> for ChunkVisitor {
        type Value = Vec<u8>;
        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            write!(formatter, "at most {MAX_BULK_DATA_BYTES} chunk bytes")
        }
        fn visit_bytes<E: serde::de::Error>(self, data: &[u8]) -> Result<Self::Value, E> {
            // Postcard borrows the input after checking its length prefix. Check
            // the chunk limit before allocating, then copy the whole slice once.
            if data.len() > MAX_BULK_DATA_BYTES {
                return Err(E::custom("bulk chunk exceeds data limit"));
            }
            let mut bytes = Vec::new();
            bytes.try_reserve_exact(data.len()).map_err(E::custom)?;
            bytes.extend_from_slice(data);
            Ok(bytes)
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            // Fallback for formats that expose bytes as a sequence (e.g. JSON).
            // Length prefixes/size_hint are untrusted. Reserve only for real bytes.
            let mut bytes = Vec::new();
            while bytes.len() < MAX_BULK_DATA_BYTES {
                let Some(byte) = seq.next_element::<u8>()? else {
                    return Ok(bytes);
                };
                bytes.try_reserve(1).map_err(serde::de::Error::custom)?;
                bytes.push(byte);
            }
            // Probe excess without decoding/retaining another data byte.
            if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                return Err(serde::de::Error::custom("bulk chunk exceeds data limit"));
            }
            Ok(bytes)
        }
    }
    deserializer.deserialize_bytes(ChunkVisitor)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BulkTransferLimits {
    pub max_active_transfers: usize,
    pub max_total_declared_bytes: u64,
    pub max_join_baseline_bytes: u64,
    pub max_puzzle_image_bytes: u64,
}

impl Default for BulkTransferLimits {
    fn default() -> Self {
        Self {
            max_active_transfers: MAX_ACTIVE_BULK_TRANSFERS,
            max_total_declared_bytes: MAX_TOTAL_BULK_IN_FLIGHT_BYTES,
            max_join_baseline_bytes: MAX_JOIN_BASELINE_TRANSFER_BYTES,
            max_puzzle_image_bytes: MAX_PUZZLE_IMAGE_TRANSFER_BYTES,
        }
    }
}

impl BulkTransferLimits {
    fn validate_size(
        self,
        kind: BulkTransferKind,
        total_size: u64,
    ) -> Result<(), BulkTransferError> {
        if total_size == 0 {
            return Err(BulkTransferError::ZeroLength);
        }
        let limit = match kind {
            BulkTransferKind::JoinBaseline => self.max_join_baseline_bytes,
            BulkTransferKind::PuzzleImage => self.max_puzzle_image_bytes,
        };
        if total_size > limit {
            return Err(BulkTransferError::TransferTooLarge);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BulkTransferError {
    ZeroLength,
    TransferTooLarge,
    TooManyTransfers,
    TotalInFlightLimit,
    NonMonotonicTransferId,
    UnknownTransfer,
    EmptyChunk,
    ChunkTooLarge,
    NonCanonicalChunkSize,
    OffsetMismatch,
    OffsetOverflow,
    ChunkOutOfBounds,
    PrematureFinish,
    HashMismatch,
    AllocationFailed,
    CounterExhausted,
}

struct InboundTransfer {
    kind: BulkTransferKind,
    total_size: u64,
    sha256: [u8; 32],
    chunks: Vec<Vec<u8>>,
    received: u64,
    hasher: Sha256,
}

/// Caller owns one receiver per secure connection/direction, including cleanup.
/// Invalid Start/Chunk and premature Finish leave existing state intact. A hash
/// mismatch removes the failed transfer and reclaims its entire declared budget.
pub struct BulkTransferReceiver {
    limits: BulkTransferLimits,
    active: BTreeMap<TransferId, InboundTransfer>,
    declared_bytes: u64,
    last_started_id: Option<TransferId>,
}

impl Default for BulkTransferReceiver {
    fn default() -> Self {
        Self::new(BulkTransferLimits::default())
    }
}

impl BulkTransferReceiver {
    pub fn new(limits: BulkTransferLimits) -> Self {
        Self {
            limits,
            active: BTreeMap::new(),
            declared_bytes: 0,
            last_started_id: None,
        }
    }

    pub fn active_transfer_count(&self) -> usize {
        self.active.len()
    }

    pub fn declared_in_flight_bytes(&self) -> u64 {
        self.declared_bytes
    }

    pub fn receive(
        &mut self,
        message: BulkTransferMessage,
    ) -> Result<Option<CompletedBulkTransfer>, BulkTransferError> {
        match message {
            BulkTransferMessage::Start {
                transfer_id,
                kind,
                total_size,
                sha256,
            } => {
                self.start(transfer_id, kind, total_size, sha256)?;
                Ok(None)
            }
            BulkTransferMessage::Chunk {
                transfer_id,
                offset,
                data,
            } => {
                self.chunk(transfer_id, offset, data)?;
                Ok(None)
            }
            BulkTransferMessage::Finish { transfer_id } => self.finish(transfer_id).map(Some),
            BulkTransferMessage::Abort { transfer_id } => {
                self.abort_local(transfer_id)?;
                Ok(None)
            }
        }
    }

    fn start(
        &mut self,
        transfer_id: TransferId,
        kind: BulkTransferKind,
        total_size: u64,
        sha256: [u8; 32],
    ) -> Result<(), BulkTransferError> {
        if self.last_started_id.is_some_and(|last| transfer_id <= last) {
            return Err(BulkTransferError::NonMonotonicTransferId);
        }
        self.limits.validate_size(kind, total_size)?;
        if self.active.len() >= self.limits.max_active_transfers {
            return Err(BulkTransferError::TooManyTransfers);
        }
        let declared = self
            .declared_bytes
            .checked_add(total_size)
            .filter(|sum| *sum <= self.limits.max_total_declared_bytes)
            .ok_or(BulkTransferError::TotalInFlightLimit)?;
        // Only metadata is allocated on Start, even for a legal 512 MiB image.
        self.active.insert(
            transfer_id,
            InboundTransfer {
                kind,
                total_size,
                sha256,
                chunks: Vec::new(),
                received: 0,
                hasher: Sha256::new(),
            },
        );
        self.declared_bytes = declared;
        // Only accepted Starts advance the high-water mark. No tombstone set.
        self.last_started_id = Some(transfer_id);
        Ok(())
    }

    fn chunk(
        &mut self,
        transfer_id: TransferId,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<(), BulkTransferError> {
        let transfer = self
            .active
            .get_mut(&transfer_id)
            .ok_or(BulkTransferError::UnknownTransfer)?;
        if data.is_empty() {
            return Err(BulkTransferError::EmptyChunk);
        }
        if data.len() > MAX_BULK_DATA_BYTES {
            return Err(BulkTransferError::ChunkTooLarge);
        }
        let end = offset
            .checked_add(data.len() as u64)
            .ok_or(BulkTransferError::OffsetOverflow)?;
        if offset != transfer.received {
            return Err(BulkTransferError::OffsetMismatch);
        }
        if end > transfer.total_size {
            return Err(BulkTransferError::ChunkOutOfBounds);
        }
        let required = (transfer.total_size - transfer.received).min(MAX_BULK_DATA_BYTES as u64);
        if data.len() as u64 != required {
            return Err(BulkTransferError::NonCanonicalChunkSize);
        }
        // Reserve metadata before changing the hash/progress, then move content.
        transfer
            .chunks
            .try_reserve(1)
            .map_err(|_| BulkTransferError::AllocationFailed)?;
        transfer.hasher.update(&data);
        transfer.chunks.push(data);
        transfer.received = end;
        Ok(())
    }

    fn finish(
        &mut self,
        transfer_id: TransferId,
    ) -> Result<CompletedBulkTransfer, BulkTransferError> {
        let transfer = self
            .active
            .get(&transfer_id)
            .ok_or(BulkTransferError::UnknownTransfer)?;
        if transfer.received != transfer.total_size {
            return Err(BulkTransferError::PrematureFinish);
        }
        let transfer = self.remove(transfer_id)?;
        let actual: [u8; 32] = transfer.hasher.finalize().into();
        if actual != transfer.sha256 {
            return Err(BulkTransferError::HashMismatch);
        }
        Ok(CompletedBulkTransfer {
            transfer_id,
            kind: transfer.kind,
            sha256: actual,
            total_size: transfer.total_size,
            chunks: transfer.chunks,
        })
    }

    fn remove(&mut self, transfer_id: TransferId) -> Result<InboundTransfer, BulkTransferError> {
        let transfer = self
            .active
            .remove(&transfer_id)
            .ok_or(BulkTransferError::UnknownTransfer)?;
        self.declared_bytes = self
            .declared_bytes
            .checked_sub(transfer.total_size)
            .expect("active sizes are included in the declared budget");
        Ok(transfer)
    }

    pub fn abort_local(&mut self, transfer_id: TransferId) -> Result<(), BulkTransferError> {
        self.remove(transfer_id).map(drop)
    }

    /// Release content/budget but retain the ID high-water mark. Construct a new
    /// receiver only for a new secure connection if its ID sequence resets.
    pub fn clear(&mut self) {
        self.active.clear();
        self.declared_bytes = 0;
    }
}

/// Verified content remains chunked until the caller explicitly flattens it.
/// Application authorization (e.g. SessionDefinition.image_hash) is still needed.
#[derive(Debug, PartialEq, Eq)]
pub struct CompletedBulkTransfer {
    pub transfer_id: TransferId,
    pub kind: BulkTransferKind,
    pub sha256: [u8; 32],
    total_size: u64,
    chunks: Vec<Vec<u8>>,
}

impl CompletedBulkTransfer {
    pub fn total_size(&self) -> u64 {
        self.total_size
    }

    pub fn chunks(&self) -> impl ExactSizeIterator<Item = &[u8]> {
        self.chunks.iter().map(Vec::as_slice)
    }

    pub fn into_chunks(self) -> impl ExactSizeIterator<Item = Vec<u8>> {
        self.chunks.into_iter()
    }

    /// A deliberate post-completion contiguous allocation. Image callers can
    /// instead stream chunks directly to storage without this extra buffer.
    pub fn into_bytes(self) -> Result<Vec<u8>, BulkTransferError> {
        let size =
            usize::try_from(self.total_size).map_err(|_| BulkTransferError::AllocationFailed)?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(size)
            .map_err(|_| BulkTransferError::AllocationFailed)?;
        for chunk in self.chunks {
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
}

/// Own one sender per secure connection/direction. Its IDs cannot wrap/reuse.
/// Validated immutable session content. Arbitrary callers cannot supply a digest
/// to the generic sender; construction verifies both bytes and session identity.
#[derive(Clone)]
pub struct VerifiedPuzzleImage {
    bytes: Arc<[u8]>,
    session: jigsall_core::session::SessionDefinition,
}
#[cfg(test)]
thread_local! { static PAYLOAD_DIGESTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
fn payload_digest(bytes: &[u8]) -> [u8; 32] {
    #[cfg(test)]
    PAYLOAD_DIGESTS.with(|n| n.set(n.get() + 1));
    Sha256::digest(bytes).into()
}
impl VerifiedPuzzleImage {
    pub fn verify(
        bytes: Arc<[u8]>,
        session: jigsall_core::session::SessionDefinition,
    ) -> Result<Self, BulkTransferError> {
        BulkTransferLimits::default()
            .validate_size(BulkTransferKind::PuzzleImage, bytes.len() as u64)?;
        if payload_digest(&bytes) != session.image_hash.0 {
            return Err(BulkTransferError::HashMismatch);
        }
        Ok(Self { bytes, session })
    }
    pub fn session(&self) -> jigsall_core::session::SessionDefinition {
        self.session
    }
    pub fn size(&self) -> u64 {
        self.bytes.len() as u64
    }
}

pub struct BulkTransferSender {
    limits: BulkTransferLimits,
    last_issued_id: u64,
}

impl Default for BulkTransferSender {
    fn default() -> Self {
        Self::new(BulkTransferLimits::default())
    }
}

impl BulkTransferSender {
    pub fn new(limits: BulkTransferLimits) -> Self {
        Self {
            limits,
            last_issued_id: 0,
        }
    }

    pub fn begin(
        &mut self,
        kind: BulkTransferKind,
        bytes: Arc<[u8]>,
    ) -> Result<OutboundBulkTransfer, BulkTransferError> {
        self.limits.validate_size(kind, bytes.len() as u64)?;
        let sha256 = payload_digest(&bytes);
        self.begin_hashed(kind, bytes, sha256)
    }
    pub fn begin_image(
        &mut self,
        image: &VerifiedPuzzleImage,
    ) -> Result<OutboundBulkTransfer, BulkTransferError> {
        self.limits
            .validate_size(BulkTransferKind::PuzzleImage, image.size())?;
        self.begin_hashed(
            BulkTransferKind::PuzzleImage,
            image.bytes.clone(),
            image.session.image_hash.0,
        )
    }
    fn begin_hashed(
        &mut self,
        kind: BulkTransferKind,
        bytes: Arc<[u8]>,
        sha256: [u8; 32],
    ) -> Result<OutboundBulkTransfer, BulkTransferError> {
        let id = self
            .last_issued_id
            .checked_add(1)
            .ok_or(BulkTransferError::CounterExhausted)?;
        self.last_issued_id = id;
        Ok(OutboundBulkTransfer {
            transfer_id: TransferId(id),
            kind,
            sha256,
            bytes,
            position: 0,
            started: false,
            finished: false,
        })
    }
}

/// At most one newly allocated chunk per call; never a Vec of all messages.
/// Dropping this object stops generation; the caller can send Abort using its ID.
pub struct OutboundBulkTransfer {
    transfer_id: TransferId,
    kind: BulkTransferKind,
    sha256: [u8; 32],
    bytes: Arc<[u8]>,
    position: usize,
    started: bool,
    finished: bool,
}

impl OutboundBulkTransfer {
    pub(crate) fn next_message_max_bytes(&self) -> u64 {
        // Serialization metadata only; no chunk allocation or sender advancement.
        if self.started && self.position < self.bytes.len() {
            ((self.bytes.len() - self.position).min(MAX_BULK_DATA_BYTES) + 64) as u64
        } else {
            96
        }
    }
    pub fn transfer_id(&self) -> TransferId {
        self.transfer_id
    }

    pub fn next_message(&mut self) -> Result<Option<BulkTransferMessage>, BulkTransferError> {
        if !self.started {
            self.started = true;
            return Ok(Some(BulkTransferMessage::Start {
                transfer_id: self.transfer_id,
                kind: self.kind,
                total_size: self.bytes.len() as u64,
                sha256: self.sha256,
            }));
        }
        if self.position < self.bytes.len() {
            let len = (self.bytes.len() - self.position).min(MAX_BULK_DATA_BYTES);
            let end = self.position + len; // At most bytes.len(), no overflow.
            let mut data = Vec::new();
            data.try_reserve_exact(len)
                .map_err(|_| BulkTransferError::AllocationFailed)?;
            data.extend_from_slice(&self.bytes[self.position..end]);
            let offset = self.position as u64;
            self.position = end;
            return Ok(Some(BulkTransferMessage::Chunk {
                transfer_id: self.transfer_id,
                offset,
                data,
            }));
        }
        if !self.finished {
            self.finished = true;
            return Ok(Some(BulkTransferMessage::Finish {
                transfer_id: self.transfer_id,
            }));
        }
        Ok(None)
    }
}

#[cfg(test)]
#[path = "bulk_tests.rs"]
mod tests;
