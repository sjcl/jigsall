//! Application join protocol. Bulk framing deliberately has no join metadata.
use super::{
    bulk::{BulkTransferKind, TransferId},
    session_control::SessionMetadata,
};
use puzzella_core::{
    protocol::ProtocolAuthorityEventEnvelope,
    session::{AuthorityCursor, ImageHash},
    PuzzleDefinition,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncTransferBinding {
    pub transfer_id: TransferId,
    pub kind: BulkTransferKind,
    pub total_size: u64,
    pub sha256: [u8; 32],
}

/// Reliable Control only, authenticated by SecureTransport. Variant direction and
/// phase are checked by the sync routers, never by the generic Bulk receiver.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SyncControlMessage {
    Session {
        metadata: SessionMetadata,
        definition: PuzzleDefinition,
    },
    ImageAvailability {
        image_hash: ImageHash,
        available: bool,
    },
    ImageOffer(SyncTransferBinding),
    TransferAccepted {
        transfer_id: TransferId,
    },
    ImageReady {
        image_hash: ImageHash,
    },
    BaselineOffer {
        generation: u64,
        cursor: AuthorityCursor,
        transfer: SyncTransferBinding,
    },
    BaselineInstalled {
        generation: u64,
        cursor: AuthorityCursor,
        transfer_id: TransferId,
    },
    CatchUpEvent {
        generation: u64,
        event: ProtocolAuthorityEventEnvelope,
    },
    CatchUpAck {
        generation: u64,
        cursor: AuthorityCursor,
    },
    ReliableComplete {
        generation: u64,
        cursor: AuthorityCursor,
    },
    /// Obsoletes all baseline state/ACKs for this generation, even if Bulk is late.
    Restart {
        generation: u64,
    },
}
