//! Lane reorder is a presentation drop, not Reliable divergence. Call these
//! classifiers only for decoded Transient messages, never for Reliable events.
use crate::multiplayer::{protocol::ProtocolCommandError, replication::ReplicationError};
use jigsall_core::session::ProtocolError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransientDrop {
    MissingDragContext,
    WrongDragContext,
    DuplicateUpdate,
    StaleUpdate,
}

pub fn replication_drop(error: &ReplicationError) -> Option<TransientDrop> {
    Some(match error {
        ReplicationError::MissingDragContext => TransientDrop::MissingDragContext,
        ReplicationError::WrongDragContext => TransientDrop::WrongDragContext,
        ReplicationError::DuplicateUpdate => TransientDrop::DuplicateUpdate,
        ReplicationError::StaleUpdate => TransientDrop::StaleUpdate,
        _ => return None,
    })
}
pub fn command_drop(error: &ProtocolCommandError) -> Option<TransientDrop> {
    Some(match error {
        ProtocolCommandError::NoActiveDrag => TransientDrop::MissingDragContext,
        ProtocolCommandError::WrongDragContext
        | ProtocolCommandError::Sequence(ProtocolError::ControlNotProcessed { .. }) => {
            TransientDrop::WrongDragContext
        }
        ProtocolCommandError::Sequence(ProtocolError::DuplicateCommand) => {
            TransientDrop::DuplicateUpdate
        }
        ProtocolCommandError::Sequence(
            ProtocolError::StaleCommand | ProtocolError::StaleMoveContext,
        ) => TransientDrop::StaleUpdate,
        _ => return None,
    })
}
