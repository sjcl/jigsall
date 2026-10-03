use super::*;
use crate::network::{
    transport::MessageClass,
    wire::{self, WireError, WireMessage},
};
use postcard::de_flavors::{Flavor, Slice};
use serde::de::{
    value::{BytesDeserializer, Error, SeqAccessDeserializer, U8Deserializer, UnitDeserializer},
    DeserializeSeed,
};

fn small_limits() -> BulkTransferLimits {
    BulkTransferLimits {
        max_active_transfers: 2,
        max_total_declared_bytes: 100,
        max_join_baseline_bytes: 100,
        max_puzzle_image_bytes: 100,
    }
}
fn start(
    id: u64,
    kind: BulkTransferKind,
    total_size: u64,
    sha256: [u8; 32],
) -> BulkTransferMessage {
    BulkTransferMessage::Start {
        transfer_id: TransferId(id),
        kind,
        total_size,
        sha256,
    }
}
fn start_bytes(id: u64, bytes: &[u8]) -> BulkTransferMessage {
    start(
        id,
        BulkTransferKind::JoinBaseline,
        bytes.len() as u64,
        Sha256::digest(bytes).into(),
    )
}
fn chunk(id: u64, offset: u64, data: Vec<u8>) -> BulkTransferMessage {
    BulkTransferMessage::Chunk {
        transfer_id: TransferId(id),
        offset,
        data,
    }
}
fn finish(id: u64) -> BulkTransferMessage {
    BulkTransferMessage::Finish {
        transfer_id: TransferId(id),
    }
}
fn abort(id: u64) -> BulkTransferMessage {
    BulkTransferMessage::Abort {
        transfer_id: TransferId(id),
    }
}
fn assert_accounting(receiver: &BulkTransferReceiver) {
    assert_eq!(receiver.active_transfer_count(), receiver.active.len());
    assert_eq!(
        receiver.declared_in_flight_bytes(),
        receiver.active.values().map(|t| t.total_size).sum::<u64>()
    );
    for transfer in receiver.active.values() {
        assert_eq!(
            transfer.received,
            transfer.chunks.iter().map(|c| c.len() as u64).sum::<u64>()
        );
        assert!(transfer.received <= transfer.total_size);
        assert!(
            transfer.chunks.len() as u64
                <= transfer.total_size.div_ceil(MAX_BULK_DATA_BYTES as u64)
        );
        for (i, chunk) in transfer.chunks.iter().enumerate() {
            assert!(!chunk.is_empty());
            assert!(chunk.len() <= MAX_BULK_DATA_BYTES);
            if i + 1 < transfer.chunks.len() || transfer.received < transfer.total_size {
                assert_eq!(chunk.len(), MAX_BULK_DATA_BYTES);
            }
        }
    }
}

#[test]
fn legal_large_starts_allocate_only_metadata_and_empty_content() {
    let limits = BulkTransferLimits::default();
    assert_eq!(limits.max_active_transfers, 2);
    assert_eq!(limits.max_join_baseline_bytes, 64 * 1024 * 1024);
    assert_eq!(limits.max_puzzle_image_bytes, 512 * 1024 * 1024);
    assert_eq!(limits.max_total_declared_bytes, 576 * 1024 * 1024);
    let mut receiver = BulkTransferReceiver::default();
    for (id, kind, size) in [
        (
            1,
            BulkTransferKind::PuzzleImage,
            MAX_PUZZLE_IMAGE_TRANSFER_BYTES,
        ),
        (
            2,
            BulkTransferKind::JoinBaseline,
            MAX_JOIN_BASELINE_TRANSFER_BYTES,
        ),
    ] {
        assert_eq!(receiver.receive(start(id, kind, size, [0; 32])), Ok(None));
        let transfer = &receiver.active[&TransferId(id)];
        assert_eq!(transfer.received, 0);
        assert!(transfer.chunks.is_empty());
        assert_eq!(transfer.chunks.capacity(), 0);
    }
    assert_eq!(
        receiver.declared_in_flight_bytes(),
        MAX_TOTAL_BULK_IN_FLIGHT_BYTES
    );
    assert_accounting(&receiver);
}

#[test]
fn malicious_total_sizes_reject_without_creating_storage() {
    for (kind, cap) in [
        (
            BulkTransferKind::JoinBaseline,
            MAX_JOIN_BASELINE_TRANSFER_BYTES,
        ),
        (
            BulkTransferKind::PuzzleImage,
            MAX_PUZZLE_IMAGE_TRANSFER_BYTES,
        ),
    ] {
        for (size, error) in [
            (0, BulkTransferError::ZeroLength),
            (cap + 1, BulkTransferError::TransferTooLarge),
            (100_000_000_000, BulkTransferError::TransferTooLarge),
            (u64::MAX, BulkTransferError::TransferTooLarge),
        ] {
            let mut receiver = BulkTransferReceiver::default();
            assert_eq!(receiver.receive(start(1, kind, size, [0; 32])), Err(error));
            assert!(receiver.active.is_empty());
            assert_eq!(receiver.last_started_id, None);
            assert_eq!(receiver.declared_in_flight_bytes(), 0);
        }
    }
}

#[test]
fn declared_budget_rejects_then_abort_and_completion_reclaim_it() {
    let mut receiver = BulkTransferReceiver::new(small_limits());
    receiver.receive(start_bytes(1, &[7; 60])).unwrap();
    assert_eq!(
        receiver.receive(start_bytes(2, &[8; 50])),
        Err(BulkTransferError::TotalInFlightLimit)
    );
    assert_eq!(receiver.active.len(), 1);
    assert_eq!(receiver.declared_in_flight_bytes(), 60);
    assert_accounting(&receiver);
    receiver.receive(abort(1)).unwrap();
    assert_eq!(receiver.declared_in_flight_bytes(), 0);
    receiver.receive(start_bytes(2, &[8; 50])).unwrap();
    receiver.receive(chunk(2, 0, vec![8; 50])).unwrap();
    receiver.receive(finish(2)).unwrap().unwrap();
    assert_eq!(receiver.declared_in_flight_bytes(), 0);
    receiver.receive(start_bytes(3, &[9; 100])).unwrap();
    assert_accounting(&receiver);
}

#[test]
fn declared_budget_cannot_overflow_even_with_custom_limits() {
    let mut receiver = BulkTransferReceiver::new(BulkTransferLimits {
        max_join_baseline_bytes: u64::MAX,
        max_puzzle_image_bytes: u64::MAX,
        max_total_declared_bytes: u64::MAX,
        ..small_limits()
    });
    receiver
        .receive(start(
            1,
            BulkTransferKind::PuzzleImage,
            u64::MAX - 1,
            [0; 32],
        ))
        .unwrap();
    assert_eq!(
        receiver.receive(start_bytes(2, &[1; 2])),
        Err(BulkTransferError::TotalInFlightLimit)
    );
    assert_eq!(receiver.declared_in_flight_bytes(), u64::MAX - 1);
    assert_eq!(receiver.active[&TransferId(1)].chunks.capacity(), 0);
    assert_accounting(&receiver);
}

#[test]
fn active_count_limit_releases_on_completion_and_abort() {
    let mut receiver = BulkTransferReceiver::new(BulkTransferLimits {
        max_active_transfers: 1,
        ..small_limits()
    });
    receiver.receive(start_bytes(1, &[1])).unwrap();
    assert_eq!(
        receiver.receive(start_bytes(2, &[2])),
        Err(BulkTransferError::TooManyTransfers)
    );
    receiver.receive(chunk(1, 0, vec![1])).unwrap();
    receiver.receive(finish(1)).unwrap();
    receiver.receive(start_bytes(2, &[2])).unwrap();
    assert_eq!(
        receiver.receive(start_bytes(3, &[3])),
        Err(BulkTransferError::TooManyTransfers)
    );
    receiver.abort_local(TransferId(2)).unwrap();
    receiver.receive(start_bytes(3, &[3])).unwrap();
    assert_accounting(&receiver);
}

#[test]
fn start_ids_never_reuse_completed_aborted_or_cleared_ids() {
    let mut receiver = BulkTransferReceiver::default();
    receiver.receive(start_bytes(10, &[1])).unwrap();
    for id in [10, 9] {
        assert_eq!(
            receiver.receive(start_bytes(id, &[1])),
            Err(BulkTransferError::NonMonotonicTransferId)
        );
    }
    receiver.receive(chunk(10, 0, vec![1])).unwrap();
    receiver.receive(finish(10)).unwrap();
    assert_eq!(
        receiver.receive(start_bytes(10, &[1])),
        Err(BulkTransferError::NonMonotonicTransferId)
    );
    receiver.receive(start_bytes(11, &[1])).unwrap();
    receiver.receive(abort(11)).unwrap();
    assert_eq!(
        receiver.receive(start_bytes(11, &[1])),
        Err(BulkTransferError::NonMonotonicTransferId)
    );
    receiver.receive(start_bytes(12, &[1])).unwrap();
    receiver.clear();
    assert_eq!(
        receiver.receive(start_bytes(12, &[1])),
        Err(BulkTransferError::NonMonotonicTransferId)
    );
    receiver.receive(start_bytes(13, &[1])).unwrap();
    assert_accounting(&receiver);
    BulkTransferReceiver::default()
        .receive(start_bytes(1, &[1]))
        .unwrap();
}

#[test]
fn duplicate_overlap_gap_offsets_preserve_progress_and_hash() {
    let bytes = vec![0x31; 2 * MAX_BULK_DATA_BYTES];
    let mut receiver = BulkTransferReceiver::default();
    receiver.receive(start_bytes(1, &bytes)).unwrap();
    receiver
        .receive(chunk(1, 0, bytes[..MAX_BULK_DATA_BYTES].to_vec()))
        .unwrap();
    for offset in [
        0,
        MAX_BULK_DATA_BYTES as u64 - 1,
        MAX_BULK_DATA_BYTES as u64 + 1,
    ] {
        assert_eq!(
            receiver.receive(chunk(1, offset, vec![0x31; MAX_BULK_DATA_BYTES])),
            Err(BulkTransferError::OffsetMismatch)
        );
        let transfer = &receiver.active[&TransferId(1)];
        assert_eq!(transfer.received, MAX_BULK_DATA_BYTES as u64);
        assert_eq!(transfer.chunks.len(), 1);
        assert_eq!(
            transfer.hasher.clone().finalize(),
            Sha256::digest(&bytes[..MAX_BULK_DATA_BYTES])
        );
        assert_accounting(&receiver);
    }
    receiver
        .receive(chunk(
            1,
            MAX_BULK_DATA_BYTES as u64,
            bytes[MAX_BULK_DATA_BYTES..].to_vec(),
        ))
        .unwrap();
    assert_eq!(
        receiver
            .receive(finish(1))
            .unwrap()
            .unwrap()
            .into_bytes()
            .unwrap(),
        bytes
    );
}

#[test]
fn only_full_full_final_sequence_is_canonical() {
    let bytes = vec![6; 2 * MAX_BULK_DATA_BYTES + 5];
    let mut receiver = BulkTransferReceiver::default();
    receiver.receive(start_bytes(1, &bytes)).unwrap();
    for len in [1, MAX_BULK_DATA_BYTES - 1] {
        assert_eq!(
            receiver.receive(chunk(1, 0, vec![6; len])),
            Err(BulkTransferError::NonCanonicalChunkSize)
        );
    }
    for offset in [0, MAX_BULK_DATA_BYTES] {
        receiver
            .receive(chunk(1, offset as u64, vec![6; MAX_BULK_DATA_BYTES]))
            .unwrap();
    }
    assert_eq!(
        receiver.receive(chunk(1, (2 * MAX_BULK_DATA_BYTES) as u64, vec![6; 4])),
        Err(BulkTransferError::NonCanonicalChunkSize)
    );
    assert_eq!(
        receiver.receive(chunk(1, (2 * MAX_BULK_DATA_BYTES) as u64, vec![6; 6])),
        Err(BulkTransferError::ChunkOutOfBounds)
    );
    receiver
        .receive(chunk(1, (2 * MAX_BULK_DATA_BYTES) as u64, vec![6; 5]))
        .unwrap();
    assert_eq!(receiver.active[&TransferId(1)].chunks.len(), 3);
    assert_accounting(&receiver);
    receiver.receive(finish(1)).unwrap();
}

#[test]
fn empty_oversized_overflow_out_of_bounds_and_unknown_chunks_reject() {
    let mut receiver = BulkTransferReceiver::new(small_limits());
    receiver.receive(start_bytes(1, &[1; 5])).unwrap();
    for (offset, data, error) in [
        (0, vec![], BulkTransferError::EmptyChunk),
        (
            0,
            vec![1; MAX_BULK_DATA_BYTES + 1],
            BulkTransferError::ChunkTooLarge,
        ),
        (u64::MAX, vec![1], BulkTransferError::OffsetOverflow),
        (u64::MAX - 2, vec![1; 3], BulkTransferError::OffsetOverflow),
        (0, vec![1; 6], BulkTransferError::ChunkOutOfBounds),
    ] {
        assert_eq!(receiver.receive(chunk(1, offset, data)), Err(error));
        assert_eq!(receiver.active[&TransferId(1)].received, 0);
        assert_eq!(receiver.active[&TransferId(1)].chunks.capacity(), 0);
        assert_accounting(&receiver);
    }
    for message in [chunk(2, 0, vec![1]), finish(2), abort(2)] {
        assert_eq!(
            receiver.receive(message),
            Err(BulkTransferError::UnknownTransfer)
        );
    }
    receiver.receive(chunk(1, 0, vec![1; 5])).unwrap();
    assert_eq!(
        receiver.receive(chunk(1, 5, vec![1])),
        Err(BulkTransferError::ChunkOutOfBounds)
    );
    receiver.receive(finish(1)).unwrap();
}

#[test]
fn premature_finish_keeps_state_and_bad_hash_drops_only_failed_transfer() {
    let mut receiver = BulkTransferReceiver::new(small_limits());
    let mut hash: [u8; 32] = Sha256::digest([1; 3]).into();
    hash[0] ^= 1;
    receiver
        .receive(start(1, BulkTransferKind::JoinBaseline, 3, hash))
        .unwrap();
    receiver.receive(start_bytes(2, &[2; 5])).unwrap();
    assert_eq!(
        receiver.receive(finish(1)),
        Err(BulkTransferError::PrematureFinish)
    );
    assert_eq!(receiver.active_transfer_count(), 2);
    assert_eq!(receiver.declared_in_flight_bytes(), 8);
    receiver.receive(chunk(1, 0, vec![1; 3])).unwrap();
    assert_eq!(
        receiver.receive(finish(1)),
        Err(BulkTransferError::HashMismatch)
    );
    assert_eq!(receiver.active_transfer_count(), 1);
    assert_eq!(receiver.declared_in_flight_bytes(), 5);
    assert_eq!(
        receiver.receive(finish(1)),
        Err(BulkTransferError::UnknownTransfer)
    );
    assert_eq!(
        receiver.receive(start_bytes(1, &[1])),
        Err(BulkTransferError::NonMonotonicTransferId)
    );
    receiver.receive(chunk(2, 0, vec![2; 5])).unwrap();
    assert_eq!(
        receiver
            .receive(finish(2))
            .unwrap()
            .unwrap()
            .into_bytes()
            .unwrap(),
        vec![2; 5]
    );
    assert_accounting(&receiver);
}

#[test]
fn abort_halfway_and_clear_drop_chunks_and_reclaim_budget() {
    let bytes = vec![1; MAX_BULK_DATA_BYTES + 3];
    let mut receiver = BulkTransferReceiver::default();
    receiver.receive(start_bytes(1, &bytes)).unwrap();
    receiver
        .receive(chunk(1, 0, bytes[..MAX_BULK_DATA_BYTES].to_vec()))
        .unwrap();
    receiver.receive(abort(1)).unwrap();
    assert_eq!(receiver.active_transfer_count(), 0);
    assert_eq!(receiver.declared_in_flight_bytes(), 0);
    for message in [
        chunk(1, MAX_BULK_DATA_BYTES as u64, vec![1; 3]),
        finish(1),
        abort(1),
    ] {
        assert_eq!(
            receiver.receive(message),
            Err(BulkTransferError::UnknownTransfer)
        );
    }
    assert_eq!(
        receiver.abort_local(TransferId(1)),
        Err(BulkTransferError::UnknownTransfer)
    );
    for id in [2, 3] {
        receiver.receive(start_bytes(id, &bytes)).unwrap();
        receiver
            .receive(chunk(id, 0, bytes[..MAX_BULK_DATA_BYTES].to_vec()))
            .unwrap();
    }
    receiver.clear();
    receiver.clear();
    assert_eq!(receiver.active_transfer_count(), 0);
    assert_eq!(receiver.declared_in_flight_bytes(), 0);
    receiver.receive(start_bytes(4, &[4])).unwrap();
    receiver.abort_local(TransferId(4)).unwrap();
    assert_accounting(&receiver);
}

#[test]
fn sender_wire_receiver_roundtrip_at_boundaries_and_multiple_chunks() {
    let mut sender = BulkTransferSender::default();
    for kind in [
        BulkTransferKind::JoinBaseline,
        BulkTransferKind::PuzzleImage,
    ] {
        for size in [
            1,
            MAX_BULK_DATA_BYTES,
            MAX_BULK_DATA_BYTES + 1,
            2 * MAX_BULK_DATA_BYTES + 5,
        ] {
            let bytes: Arc<[u8]> = (0..size)
                .map(|i| (i % 251) as u8)
                .collect::<Vec<_>>()
                .into();
            let mut outbound = sender.begin(kind, bytes.clone()).unwrap();
            let id = outbound.transfer_id();
            let mut receiver = BulkTransferReceiver::default();
            let mut completed = None;
            let mut offset = 0;
            let mut messages = 0;
            while let Some(message) = outbound.next_message().unwrap() {
                if let BulkTransferMessage::Chunk {
                    offset: actual,
                    ref data,
                    ..
                } = message
                {
                    assert_eq!(actual, offset);
                    assert_eq!(
                        data.len(),
                        (size - offset as usize).min(MAX_BULK_DATA_BYTES)
                    );
                    offset += data.len() as u64;
                }
                messages += 1;
                let framed = WireMessage::BulkTransfer(message);
                assert_eq!(framed.class(), MessageClass::Bulk);
                let frame = wire::encode(&framed).unwrap();
                let WireMessage::BulkTransfer(decoded) =
                    wire::decode_for_class(&frame, MessageClass::Bulk).unwrap()
                else {
                    panic!("typed bulk expected");
                };
                if let Some(value) = receiver.receive(decoded).unwrap() {
                    completed = Some(value);
                }
                assert_accounting(&receiver);
            }
            assert!(outbound.next_message().unwrap().is_none());
            assert_eq!(messages, size.div_ceil(MAX_BULK_DATA_BYTES) + 2);
            let completed = completed.unwrap();
            assert_eq!(completed.transfer_id, id);
            assert_eq!(completed.kind, kind);
            assert_eq!(completed.sha256, <[u8; 32]>::from(Sha256::digest(&bytes)));
            assert_eq!(completed.total_size(), size as u64);
            assert_eq!(completed.chunks().len(), size.div_ceil(MAX_BULK_DATA_BYTES));
            assert_eq!(
                completed.chunks().flatten().copied().collect::<Vec<_>>(),
                &*bytes
            );
            assert_eq!(completed.into_bytes().unwrap(), &*bytes);
            assert_eq!(receiver.active_transfer_count(), 0);
            assert_eq!(receiver.declared_in_flight_bytes(), 0);
        }
    }
}

#[test]
fn inbound_moves_content_and_completion_does_not_flatten() {
    let bytes = vec![8; MAX_BULK_DATA_BYTES + 3];
    let mut receiver = BulkTransferReceiver::default();
    receiver.receive(start_bytes(1, &bytes)).unwrap();
    let data = vec![8; MAX_BULK_DATA_BYTES];
    let pointer = data.as_ptr();
    receiver.receive(chunk(1, 0, data)).unwrap();
    assert_eq!(receiver.active[&TransferId(1)].chunks[0].as_ptr(), pointer);
    let data = vec![8; 3];
    let last_pointer = data.as_ptr();
    receiver
        .receive(chunk(1, MAX_BULK_DATA_BYTES as u64, data))
        .unwrap();
    let completed = receiver.receive(finish(1)).unwrap().unwrap();
    assert_eq!(completed.chunks().len(), 2);
    assert_eq!(completed.chunks().next().unwrap().as_ptr(), pointer);
    assert_eq!(completed.chunks().last().unwrap().as_ptr(), last_pointer);
    let mut owned = completed.into_chunks();
    assert_eq!(owned.len(), 2);
    let first = owned.next().unwrap();
    let last = owned.next().unwrap();
    assert_eq!(first.as_ptr(), pointer);
    assert_eq!(last.as_ptr(), last_pointer);
}

#[test]
fn interleaved_transfers_complete_independently() {
    let mut sender = BulkTransferSender::default();
    let a: Arc<[u8]> = vec![3; 2 * MAX_BULK_DATA_BYTES + 5].into();
    let b: Arc<[u8]> = vec![4; MAX_BULK_DATA_BYTES + 2].into();
    let mut first = sender
        .begin(BulkTransferKind::JoinBaseline, a.clone())
        .unwrap();
    let mut second = sender
        .begin(BulkTransferKind::PuzzleImage, b.clone())
        .unwrap();
    let mut receiver = BulkTransferReceiver::default();
    receiver
        .receive(first.next_message().unwrap().unwrap())
        .unwrap();
    receiver
        .receive(first.next_message().unwrap().unwrap())
        .unwrap();
    receiver
        .receive(second.next_message().unwrap().unwrap())
        .unwrap();
    let mut done = BTreeMap::new();
    loop {
        let mut progress = false;
        for outbound in [&mut second, &mut first] {
            if let Some(message) = outbound.next_message().unwrap() {
                progress = true;
                if let Some(completed) = receiver.receive(message).unwrap() {
                    done.insert(completed.transfer_id, completed);
                }
                assert_accounting(&receiver);
            }
        }
        if !progress {
            break;
        }
    }
    assert_eq!(
        done.remove(&first.transfer_id())
            .unwrap()
            .into_bytes()
            .unwrap(),
        &*a
    );
    assert_eq!(
        done.remove(&second.transfer_id())
            .unwrap()
            .into_bytes()
            .unwrap(),
        &*b
    );
    assert_eq!(receiver.active_transfer_count(), 0);
    assert_eq!(receiver.declared_in_flight_bytes(), 0);
}

#[test]
fn canonical_chunk_count_bounds_metadata_without_large_payload() {
    let count = MAX_PUZZLE_IMAGE_TRANSFER_BYTES.div_ceil(MAX_BULK_DATA_BYTES as u64);
    assert_eq!(count, 16_417);
    assert!((count - 1) * (MAX_BULK_DATA_BYTES as u64) < MAX_PUZZLE_IMAGE_TRANSFER_BYTES);
    assert!(count * (MAX_BULK_DATA_BYTES as u64) >= MAX_PUZZLE_IMAGE_TRANSFER_BYTES);
    let mut receiver = BulkTransferReceiver::default();
    receiver
        .receive(start(
            1,
            BulkTransferKind::PuzzleImage,
            MAX_PUZZLE_IMAGE_TRANSFER_BYTES,
            [0; 32],
        ))
        .unwrap();
    assert_eq!(
        receiver.receive(chunk(1, 0, vec![1])),
        Err(BulkTransferError::NonCanonicalChunkSize)
    );
    assert_eq!(receiver.active[&TransferId(1)].chunks.capacity(), 0);
}

#[test]
fn sender_validates_size_and_ids_never_wrap_or_reuse() {
    let mut sender = BulkTransferSender::new(small_limits());
    assert!(matches!(
        sender.begin(BulkTransferKind::JoinBaseline, Arc::from([])),
        Err(BulkTransferError::ZeroLength)
    ));
    assert!(matches!(
        sender.begin(BulkTransferKind::PuzzleImage, Arc::from([1; 101])),
        Err(BulkTransferError::TransferTooLarge)
    ));
    assert_eq!(sender.last_issued_id, 0);
    for (kind, cap) in [
        (
            BulkTransferKind::JoinBaseline,
            MAX_JOIN_BASELINE_TRANSFER_BYTES,
        ),
        (
            BulkTransferKind::PuzzleImage,
            MAX_PUZZLE_IMAGE_TRANSFER_BYTES,
        ),
    ] {
        assert_eq!(
            BulkTransferLimits::default().validate_size(kind, cap),
            Ok(())
        );
        for size in [cap + 1, u64::MAX] {
            assert_eq!(
                BulkTransferLimits::default().validate_size(kind, size),
                Err(BulkTransferError::TransferTooLarge)
            );
        }
    }
    for id in [1, 2] {
        assert_eq!(
            sender
                .begin(BulkTransferKind::JoinBaseline, Arc::from([1]))
                .unwrap()
                .transfer_id(),
            TransferId(id)
        );
    }
    sender.last_issued_id = u64::MAX - 1;
    let mut last = sender
        .begin(BulkTransferKind::JoinBaseline, Arc::from([1]))
        .unwrap();
    assert_eq!(last.transfer_id(), TransferId(u64::MAX));
    assert!(matches!(
        last.next_message().unwrap(),
        Some(BulkTransferMessage::Start {
            transfer_id: TransferId(u64::MAX),
            ..
        })
    ));
    for _ in 0..2 {
        assert!(matches!(
            sender.begin(BulkTransferKind::JoinBaseline, Arc::from([1])),
            Err(BulkTransferError::CounterExhausted)
        ));
        assert_eq!(sender.last_issued_id, u64::MAX);
    }
}

struct CountingSlice<'de> {
    inner: Slice<'de>,
    byte_reads: usize,
    slice_reads: usize,
}

impl<'de> Flavor<'de> for CountingSlice<'de> {
    type Source = &'de [u8];
    type Remainder = (&'de [u8], usize, usize);

    fn pop(&mut self) -> postcard::Result<u8> {
        self.byte_reads += 1;
        self.inner.pop()
    }

    fn try_take_n(&mut self, count: usize) -> postcard::Result<&'de [u8]> {
        self.slice_reads += 1;
        self.inner.try_take_n(count)
    }

    fn finalize(self) -> postcard::Result<Self::Remainder> {
        Ok((self.inner.finalize()?, self.byte_reads, self.slice_reads))
    }
}

#[test]
fn postcard_chunk_decoding_reads_data_as_one_slice() {
    for size in [0, 1, 127, 128, MAX_BULK_DATA_BYTES] {
        let message = chunk(
            u64::MAX,
            u64::MAX,
            (0..=u8::MAX).cycle().take(size).collect(),
        );
        let payload = postcard::to_allocvec(&message).unwrap();
        let mut deserializer = postcard::Deserializer::from_flavor(CountingSlice {
            inner: Slice::new(&payload),
            byte_reads: 0,
            slice_reads: 0,
        });
        assert_eq!(
            BulkTransferMessage::deserialize(&mut deserializer).unwrap(),
            message
        );
        let (rest, byte_reads, slice_reads) = deserializer.finalize().unwrap();
        assert!(rest.is_empty());
        // Only metadata/length varints use byte reads, independent of data size.
        assert_eq!(byte_reads, payload.len() - size);
        assert_eq!(slice_reads, 1);
    }
}

#[test]
fn non_borrowed_chunk_bytes_accept_up_to_limit_and_reject_excess() {
    for size in [0, 1, MAX_BULK_DATA_BYTES, MAX_BULK_DATA_BYTES + 1] {
        let data = vec![0xff; size];
        let result = deserialize_chunk_data(BytesDeserializer::<Error>::new(&data));
        if size <= MAX_BULK_DATA_BYTES {
            assert_eq!(result.unwrap(), data);
        } else {
            assert_eq!(
                result.unwrap_err().to_string(),
                "bulk chunk exceeds data limit"
            );
        }
    }
}

struct RepeatedBytes<'a> {
    remaining: usize,
    hint: Option<usize>,
    decoded: &'a mut usize,
}
impl<'de> SeqAccess<'de> for RepeatedBytes<'_> {
    type Error = Error;
    fn next_element_seed<T: DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, Error> {
        if self.remaining == 0 {
            return Ok(None);
        }
        self.remaining -= 1;
        if *self.decoded == MAX_BULK_DATA_BYTES {
            // IgnoredAny accepts this marker; decoding it as u8 would fail.
            return seed.deserialize(UnitDeserializer::new()).map(Some);
        }
        *self.decoded += 1;
        seed.deserialize(U8Deserializer::new(7)).map(Some)
    }
    fn size_hint(&self) -> Option<usize> {
        self.hint
    }
}

#[test]
fn huge_size_hint_with_tiny_content_does_not_reserve_large_capacity() {
    for count in [0, 1, 3] {
        let mut decoded = 0;
        let bytes = deserialize_chunk_data(SeqAccessDeserializer::new(RepeatedBytes {
            remaining: count,
            hint: Some(usize::MAX),
            decoded: &mut decoded,
        }))
        .unwrap();
        assert_eq!(bytes, vec![7; count]);
        assert_eq!(decoded, count);
        assert!(bytes.capacity() <= 8);
        if count == 0 {
            assert_eq!(bytes.capacity(), 0);
        }
    }
}

#[test]
fn actual_decode_count_is_bounded_independently_of_hint() {
    for hint in [None, Some(0), Some(usize::MAX)] {
        for count in [MAX_BULK_DATA_BYTES, MAX_BULK_DATA_BYTES + 1, usize::MAX] {
            let mut decoded = 0;
            let result = deserialize_chunk_data(SeqAccessDeserializer::new(RepeatedBytes {
                remaining: count,
                hint,
                decoded: &mut decoded,
            }));
            assert_eq!(decoded, count.min(MAX_BULK_DATA_BYTES));
            if count <= MAX_BULK_DATA_BYTES {
                assert_eq!(result.unwrap().len(), count);
            } else {
                assert!(result.is_err());
            }
        }
    }
}

#[test]
fn max_chunk_fits_wire_margin_and_max_plus_one_rejects_encode_and_decode() {
    for size in [MAX_BULK_DATA_BYTES, MAX_BULK_DATA_BYTES + 1] {
        let message = chunk(u64::MAX, u64::MAX, vec![0xff; size]);
        let payload = postcard::to_allocvec(&message).unwrap();
        assert!(payload.len() <= wire::MAX_BULK_WIRE_PAYLOAD);
        let framed = WireMessage::BulkTransfer(message.clone());
        if size == MAX_BULK_DATA_BYTES {
            assert_eq!(
                postcard::from_bytes::<BulkTransferMessage>(&payload).unwrap(),
                message
            );
            let frame = wire::encode(&framed).unwrap();
            assert_eq!(frame.len(), wire::HEADER_SIZE + size + 24);
            assert_eq!(wire::decode(&frame).unwrap(), framed);
        } else {
            assert!(postcard::from_bytes::<BulkTransferMessage>(&payload).is_err());
            assert_eq!(wire::encode(&framed), Err(WireError::Oversized));
            let mut frame = b"PZLA".to_vec();
            frame.extend_from_slice(&wire::WIRE_VERSION.to_le_bytes());
            frame.extend_from_slice(&[5, 0]);
            frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            frame.extend_from_slice(&payload);
            assert_eq!(wire::decode(&frame), Err(WireError::MalformedPayload));
        }
    }
    for message in [
        start_bytes(1, &[1]),
        chunk(1, 0, vec![1]),
        finish(1),
        abort(1),
    ] {
        assert_eq!(
            WireMessage::BulkTransfer(message).class(),
            MessageClass::Bulk
        );
    }
}

#[test]
fn huge_truncated_and_malformed_postcard_length_prefixes_reject() {
    for count in [4, MAX_BULK_DATA_BYTES, MAX_BULK_DATA_BYTES + 1, usize::MAX] {
        for tail in [&[][..], &[1, 2, 3][..]] {
            // Chunk=01, ID=01, offset=00, then untrusted Vec length varint.
            let mut payload = vec![1, 1, 0];
            payload.extend(postcard::to_allocvec(&count).unwrap());
            payload.extend_from_slice(tail);
            assert!(postcard::from_bytes::<BulkTransferMessage>(&payload).is_err());
            let mut frame = b"PZLA".to_vec();
            frame.extend_from_slice(&wire::WIRE_VERSION.to_le_bytes());
            frame.extend_from_slice(&[5, 0]);
            frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            frame.extend_from_slice(&payload);
            assert_eq!(wire::decode(&frame), Err(WireError::MalformedPayload));
        }
    }
    let mut malformed = vec![1, 1, 0];
    malformed.extend(std::iter::repeat_n(
        0xff,
        (usize::BITS as usize).div_ceil(7) + 1,
    ));
    assert!(postcard::from_bytes::<BulkTransferMessage>(&malformed).is_err());
}

#[test]
fn flatten_capacity_failure_returns_result_without_large_allocation() {
    // Synthetic private state exercises capacity overflow; real completion is
    // consistent and requires no allocator instrumentation for this check.
    let completed = CompletedBulkTransfer {
        transfer_id: TransferId(1),
        kind: BulkTransferKind::PuzzleImage,
        sha256: [0; 32],
        total_size: u64::MAX,
        chunks: Vec::new(),
    };
    assert_eq!(
        completed.into_bytes(),
        Err(BulkTransferError::AllocationFailed)
    );
}
