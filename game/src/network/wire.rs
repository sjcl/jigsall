//! One GNS message = one frame. Stable header; Postcard payload, no JSON.
use super::session_control::SessionControlMessage;
use super::transport::MessageClass;
use puzzella_core::protocol::{
    ProtocolAuthorityEventEnvelope, ProtocolCommandEnvelope, ProtocolPieceCommand, RemoteDragUpdate,
};
use serde::{de::DeserializeOwned, Serialize};

pub const WIRE_VERSION: u16 = 4;
pub const HEADER_SIZE: usize = 12;
pub const MAX_CONTROL_PAYLOAD: usize = 256 * 1024;
pub const MAX_SESSION_CONTROL_PAYLOAD: usize = 4096;
pub const MAX_TRANSIENT_PAYLOAD: usize = 128;
pub const MAX_BULK_CHUNK: usize = 32 * 1024;
pub const MAX_WIRE_MESSAGE: usize = HEADER_SIZE + MAX_CONTROL_PAYLOAD;
const MAGIC: &[u8; 4] = b"PZLA";

#[derive(Clone, Debug, PartialEq)]
pub enum WireMessage {
    ClientCommand(ProtocolCommandEnvelope),
    AuthorityEvent(ProtocolAuthorityEventEnvelope),
    DragUpdate(RemoteDragUpdate),
    /// Bounded opaque bytes reserved for future application chunk protocols.
    /// No snapshot/image transfer semantics or reassembly are implemented.
    BulkChunk(Vec<u8>),
    SessionControl(SessionControlMessage),
}

impl WireMessage {
    pub fn class(&self) -> MessageClass {
        match self {
            Self::ClientCommand(command)
                if matches!(command.command, ProtocolPieceCommand::DragUpdate { .. }) =>
            {
                MessageClass::Transient
            }
            Self::ClientCommand(_) | Self::AuthorityEvent(_) | Self::SessionControl(_) => {
                MessageClass::Control
            }
            Self::DragUpdate(_) => MessageClass::Transient,
            Self::BulkChunk(_) => MessageClass::Bulk,
        }
    }
    fn kind(&self) -> u8 {
        match self {
            Self::ClientCommand(_) if self.class() == MessageClass::Transient => 4,
            Self::ClientCommand(_) => 1,
            Self::AuthorityEvent(_) => 2,
            Self::DragUpdate(_) => 3,
            Self::BulkChunk(_) => 5,
            Self::SessionControl(_) => 6,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WireError {
    Truncated,
    BadMagic,
    UnsupportedVersion(u16),
    UnknownKind(u8),
    ReservedBits,
    Oversized,
    LengthMismatch,
    MalformedPayload,
    WrongClass,
}

pub const fn payload_limit(class: MessageClass) -> usize {
    match class {
        MessageClass::Transient => MAX_TRANSIENT_PAYLOAD,
        MessageClass::Control => MAX_CONTROL_PAYLOAD,
        MessageClass::Bulk => MAX_BULK_CHUNK,
    }
}

/// Inner plaintext limit. Native backends allow the secure record overhead too.
pub const fn frame_limit(class: MessageClass) -> usize {
    HEADER_SIZE + payload_limit(class)
}

pub fn encode(message: &WireMessage) -> Result<Vec<u8>, WireError> {
    let payload = match message {
        WireMessage::ClientCommand(v) => binary(v)?,
        WireMessage::AuthorityEvent(v) => binary(v)?,
        WireMessage::DragUpdate(v) => binary(v)?,
        WireMessage::SessionControl(v) => binary(v)?,
        WireMessage::BulkChunk(v) => {
            if v.len() > MAX_BULK_CHUNK {
                return Err(WireError::Oversized);
            }
            v.clone()
        }
    };
    if payload.len() > kind_payload_limit(message.kind(), message.class()) {
        return Err(WireError::Oversized);
    }
    let mut frame = Vec::with_capacity(HEADER_SIZE + payload.len());
    frame.extend_from_slice(MAGIC);
    frame.extend_from_slice(&WIRE_VERSION.to_le_bytes());
    frame.push(message.kind());
    frame.push(0); // Reserved, required to be zero.
    frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

fn binary<T: Serialize>(value: &T) -> Result<Vec<u8>, WireError> {
    postcard::to_allocvec(value).map_err(|_| WireError::MalformedPayload)
}

fn parse<T: DeserializeOwned>(payload: &[u8]) -> Result<T, WireError> {
    let (value, rest) =
        postcard::take_from_bytes(payload).map_err(|_| WireError::MalformedPayload)?;
    if !rest.is_empty() {
        return Err(WireError::MalformedPayload);
    }
    Ok(value)
}

const fn kind_payload_limit(kind: u8, class: MessageClass) -> usize {
    if kind == 6 {
        MAX_SESSION_CONTROL_PAYLOAD
    } else {
        payload_limit(class)
    }
}

/// Reject the frame before deserialization/allocation. Core's bounded visitors
/// further cap masks at 1M bits / 31,250 words and sparse lists at 32 entries.
fn checked_payload(frame: &[u8]) -> Result<(u8, MessageClass, &[u8]), WireError> {
    if frame.len() > MAX_WIRE_MESSAGE {
        return Err(WireError::Oversized);
    }
    if frame.len() < HEADER_SIZE {
        return Err(WireError::Truncated);
    }
    if &frame[..4] != MAGIC {
        return Err(WireError::BadMagic);
    }
    let version = u16::from_le_bytes([frame[4], frame[5]]);
    if version != WIRE_VERSION {
        return Err(WireError::UnsupportedVersion(version));
    }
    let class = match frame[6] {
        1 | 2 | 6 => MessageClass::Control,
        3 | 4 => MessageClass::Transient,
        5 => MessageClass::Bulk,
        kind => return Err(WireError::UnknownKind(kind)),
    };
    if frame[7] != 0 {
        return Err(WireError::ReservedBits);
    }
    let len = u32::from_le_bytes([frame[8], frame[9], frame[10], frame[11]]) as usize;
    if len > kind_payload_limit(frame[6], class)
        || frame.len() - HEADER_SIZE > kind_payload_limit(frame[6], class)
    {
        return Err(WireError::Oversized);
    }
    if len != frame.len() - HEADER_SIZE {
        return Err(WireError::LengthMismatch);
    }
    Ok((frame[6], class, &frame[HEADER_SIZE..]))
}

/// Header-only gate: reject gameplay before allocating/deserializing its payload,
/// and let Ready gameplay be decoded just once by the existing router.
pub fn is_session_control_for_class(frame: &[u8], class: MessageClass) -> Result<bool, WireError> {
    if frame.len() > frame_limit(class) {
        return Err(WireError::Oversized);
    }
    let (kind, declared_class, _) = checked_payload(frame)?;
    if declared_class != class {
        return Err(WireError::WrongClass);
    }
    Ok(kind == 6)
}

pub fn decode(frame: &[u8]) -> Result<WireMessage, WireError> {
    let (kind, class, payload) = checked_payload(frame)?;
    let message = match kind {
        1 | 4 => WireMessage::ClientCommand(parse(payload)?),
        2 => WireMessage::AuthorityEvent(parse(payload)?),
        3 => WireMessage::DragUpdate(parse(payload)?),
        5 => WireMessage::BulkChunk(payload.to_vec()),
        6 => WireMessage::SessionControl(parse(payload)?),
        _ => unreachable!("kind checked above"),
    };
    if message.class() != class {
        return Err(WireError::WrongClass);
    }
    Ok(message)
}

/// Enforce both the backend lane's limit and the decoded message's class.
pub fn decode_for_class(frame: &[u8], class: MessageClass) -> Result<WireMessage, WireError> {
    if frame.len() > frame_limit(class) {
        return Err(WireError::Oversized);
    }
    let message = decode(frame)?;
    if message.class() != class {
        return Err(WireError::WrongClass);
    }
    Ok(message)
}
