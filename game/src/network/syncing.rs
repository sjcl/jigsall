//! Opt-in runtime foundation for authenticated host-to-client joins.
//! A connection owns one offered transfer; no completed-payload queue exists.
//! Reliable completion enters Finalizing. Only the authoritative drag-set
//! reconciliation/barrier may issue a Ready permit.
use super::session::SessionConnections;
use super::{
    bootstrap::{BootstrapError, ClientBootstrap, ConnectionState, HostBootstrap},
    bulk::*,
    lifecycle::{
        Admission, Bucket, SyncPolicy, BULK_BYTES_PER_SECOND, BULK_FRAME_BYTES,
        MAX_BULK_QUEUE_BYTES,
    },
    secure::SecureTransport,
    session_control::SessionMetadata,
    sync_control::{SyncControlMessage as Control, SyncFinalization, SyncTransferBinding},
    transport::{
        ConnectionId, DisconnectReason, MessageClass, Origin, Transport, TransportError,
        TransportEvent,
    },
    wire::{self, WireError, WireMessage},
};
use crate::{
    multiplayer::{
        catch_up::{
            CatchUpError, JoinCatchUpCoordinator, JoinCatchUpPhase, MAX_PENDING_JOIN_SYNCS,
        },
        finalization::{FinalDragError, FinalDragSet},
        protocol::{HostCommandOutcome, ProtocolDragContexts},
        replication::{PeerReplicationState, ReplicationError},
        JoinBaseline, JoinBaselineError, SnapshotExpectation,
    },
    players::{PlayerRoster, RosterError, RosterPlayer},
    resources::PieceDataStore,
};
use jigsall_core::{
    protocol::ProtocolAuthorityEventEnvelope,
    session::{AuthorityCursor, AuthoritySession},
    PlayerId, PuzzleDefinition,
};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
    time::{Duration, Instant},
};

mod client;
mod host;
mod timing;
pub use client::{ClientSyncOutcome, ClientSyncRouter, SyncReplica};
pub use host::{HostSyncCoordinator, SyncAuthority, SyncHost, SyncResources};
pub use timing::SyncTiming;

/// Separate image and baseline slots keep cached-image joins independent.
pub const MAX_CONCURRENT_BASELINE_TRANSFERS: usize = 2;
pub const MAX_CONCURRENT_IMAGE_TRANSFERS: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncPhase {
    ImageNegotiation,
    AwaitingImageSlot,
    ImageTransfer,
    AwaitingImageReady,
    /// Host-only FIFO wait: verified image, no baseline payload or new retention.
    AwaitingBaselineSlot,
    BaselineTransfer,
    CatchingUp,
    /// Reliable stream is current; final full-set reconciliation/ACK is pending.
    Finalizing,
    Ready,
    RestartRequired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResponseWait {
    ImageAvailability,
    ImageAcceptance,
    ImageReady,
    BaselineAcceptance,
    BaselineInstalled,
    CatchUpAck,
    FinalizeAck,
    Session,
    ReadyCommit,
}
#[derive(Debug, PartialEq)]
pub enum SyncError {
    Bootstrap(BootstrapError),
    NotSyncing,
    TooManyJoins,
    Admission(DisconnectReason),
    PhaseTimeout(ResponseWait),
    LifetimeTimeout,
    CapacityWaitTimeout,
    BulkStalled,
    WrongConnection,
    WrongDirection,
    WrongPhase,
    /// A host API was called in an incompatible local sync phase.
    HostWrongPhase,
    WrongIdentity,
    WrongGeneration,
    WrongFinalization,
    /// Local authority/session scope changed since authentication.
    AuthorityChanged,
    HostImageUnavailable,
    Roster(RosterError),
    HostRoster(RosterError),
    FinalizationExhausted,
    FinalDrag(FinalDragError),
    HostFinalDrag(FinalDragError),
    UnofferedTransfer,
    TransferMismatch,
    ImageHashMismatch,
    MalformedBaseline,
    BaselineEncoding,
    Baseline(JoinBaselineError),
    CatchUp(CatchUpError),
    HostCatchUp(CatchUpError),
    Replication(ReplicationError),
    Bulk(BulkTransferError),
    HostBulk(BulkTransferError),
    Wire(WireError),
    Encoding(WireError),
    Transport(TransportError),
}
impl SyncError {
    pub fn host_policy(&self) -> super::policy::HostFailurePolicy {
        use super::policy::{HostFailurePolicy, HostInternalRecovery};
        match self {
            Self::Transport(error) | Self::Bootstrap(BootstrapError::Transport(error)) => {
                HostFailurePolicy::transport(error)
            }
            Self::Bootstrap(error) => HostFailurePolicy::bootstrap(error),
            Self::BaselineEncoding
            | Self::Encoding(_)
            | Self::HostFinalDrag(_)
            | Self::HostCatchUp(CatchUpError::Baseline(_) | CatchUpError::GenerationExhausted)
            | Self::HostRoster(
                RosterError::NoncanonicalPlayers
                | RosterError::MissingHost
                | RosterError::MissingLocalPlayer
                | RosterError::InvalidRevision
                | RosterError::RevisionExhausted
                | RosterError::DuplicatePlayer
                | RosterError::UnknownPlayer
                | RosterError::HostLeft,
            ) => HostFailurePolicy::InternalFailure(HostInternalRecovery::StopSession),
            Self::NotSyncing
            | Self::TooManyJoins
            | Self::Admission(_)
            | Self::PhaseTimeout(_)
            | Self::LifetimeTimeout
            | Self::CapacityWaitTimeout
            | Self::BulkStalled
            | Self::WrongConnection
            | Self::WrongDirection
            | Self::WrongPhase
            | Self::HostWrongPhase
            | Self::WrongIdentity
            | Self::WrongGeneration
            | Self::WrongFinalization
            | Self::AuthorityChanged
            | Self::HostImageUnavailable
            | Self::Roster(_)
            | Self::HostRoster(RosterError::TooManyPlayers)
            | Self::FinalizationExhausted
            | Self::FinalDrag(_)
            | Self::UnofferedTransfer
            | Self::TransferMismatch
            | Self::ImageHashMismatch
            | Self::MalformedBaseline
            | Self::Baseline(_)
            | Self::CatchUp(_)
            | Self::HostCatchUp(_)
            | Self::Replication(_)
            | Self::Bulk(_)
            | Self::HostBulk(_)
            | Self::Wire(_) => match self.disconnect_reason() {
                DisconnectReason::BackendFailure | DisconnectReason::ConnectionProblem => {
                    HostFailurePolicy::InternalFailure(HostInternalRecovery::DisconnectPeer(
                        self.disconnect_reason(),
                    ))
                }
                reason => HostFailurePolicy::Disconnect(reason),
            },
        }
    }
    pub fn disconnect_reason(&self) -> DisconnectReason {
        match self {
            Self::TooManyJoins
            | Self::HostRoster(RosterError::TooManyPlayers)
            | Self::HostCatchUp(CatchUpError::TooManyPendingJoins) => {
                DisconnectReason::JoinCapacity
            }
            Self::Admission(reason) | Self::Bootstrap(BootstrapError::Rejected(reason)) => *reason,
            Self::PhaseTimeout(_) => DisconnectReason::SyncPhaseTimeout,
            Self::LifetimeTimeout => DisconnectReason::SyncLifetime,
            Self::CapacityWaitTimeout => DisconnectReason::HostCapacityTimeout,
            Self::BulkStalled => DisconnectReason::BulkStalled,
            Self::Transport(error) | Self::Bootstrap(BootstrapError::Transport(error)) => {
                super::policy::HostFailurePolicy::transport(error).disconnect_reason()
            }
            Self::NotSyncing | Self::AuthorityChanged => DisconnectReason::ConnectionProblem,
            Self::Bootstrap(
                BootstrapError::InvalidTransition | BootstrapError::Registration(_),
            )
            | Self::HostImageUnavailable
            | Self::HostWrongPhase
            | Self::HostRoster(_)
            | Self::FinalizationExhausted
            | Self::HostFinalDrag(_)
            | Self::BaselineEncoding
            | Self::HostCatchUp(_)
            | Self::HostBulk(_)
            | Self::Encoding(_)
            | Self::Bulk(
                BulkTransferError::AllocationFailed | BulkTransferError::CounterExhausted,
            )
            | Self::CatchUp(
                CatchUpError::NotJoining
                | CatchUpError::WrongPhase(_)
                | CatchUpError::RestartRequired(_),
            ) => DisconnectReason::BackendFailure,
            Self::WrongConnection
            | Self::WrongDirection
            | Self::WrongPhase
            | Self::WrongIdentity
            | Self::WrongGeneration
            | Self::WrongFinalization
            | Self::Roster(_)
            | Self::FinalDrag(_)
            | Self::UnofferedTransfer
            | Self::TransferMismatch
            | Self::ImageHashMismatch
            | Self::MalformedBaseline
            | Self::Baseline(_)
            | Self::CatchUp(_)
            | Self::Replication(_)
            | Self::Bulk(_)
            | Self::Wire(_) => DisconnectReason::ProtocolViolation,
        }
    }
}

struct FinalizationCandidate {
    token: SyncFinalization,
    drags: FinalDragSet,
    store_epoch: u64,
}

/// Private construction: only a validated final barrier or its host commit can
/// certify readiness. Bound to the original authenticated connection identity.
pub struct SyncReadyPermit {
    connection: ConnectionId,
    metadata: SessionMetadata,
    player: PlayerId,
}
impl SyncReadyPermit {
    pub(crate) fn matches(
        &self,
        connection: ConnectionId,
        metadata: SessionMetadata,
        player: PlayerId,
    ) -> bool {
        self.connection == connection && self.metadata == metadata && self.player == player
    }
    #[cfg(test)]
    pub(crate) fn fixture(
        connection: ConnectionId,
        metadata: SessionMetadata,
        player: PlayerId,
    ) -> Self {
        Self {
            connection,
            metadata,
            player,
        }
    }
}

fn require_identity(
    authenticated: SessionMetadata,
    current: SessionMetadata,
) -> Result<(), SyncError> {
    if authenticated.definition != current.definition
        || authenticated.host != current.host
        || authenticated.cursor.epoch != current.cursor.epoch
        || current.cursor < authenticated.cursor
    {
        return Err(SyncError::WrongIdentity);
    }
    Ok(())
}
fn sync_message(event: &TransportEvent) -> Result<(ConnectionId, WireMessage), SyncError> {
    let TransportEvent::Message {
        connection,
        class,
        payload,
    } = event
    else {
        return Err(SyncError::WrongDirection);
    };
    if wire::frame_route_for_class(payload, *class).map_err(SyncError::Wire)?
        != wire::FrameRoute::Syncing
    {
        return Err(SyncError::WrongDirection);
    }
    Ok((
        *connection,
        wire::decode_for_class(payload, *class).map_err(SyncError::Wire)?,
    ))
}
fn send<T: Transport>(
    transport: &mut SecureTransport<T>,
    connection: ConnectionId,
    control: Control,
) -> Result<(), SyncError> {
    let bytes = wire::encode(&WireMessage::SyncControl(control)).map_err(SyncError::Encoding)?;
    transport
        .send(connection, MessageClass::Control, &bytes)
        .map_err(SyncError::Transport)
}
fn send_bulk<T: Transport>(
    transport: &mut SecureTransport<T>,
    connection: ConnectionId,
    message: BulkTransferMessage,
) -> Result<(), SyncError> {
    let bytes = wire::encode(&WireMessage::BulkTransfer(message)).map_err(SyncError::Encoding)?;
    transport
        .send(connection, MessageClass::Bulk, &bytes)
        .map_err(SyncError::Transport)
}
