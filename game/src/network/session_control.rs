//! Bounded session-control data; no crypto-library types cross this boundary.
use puzzella_core::{
    session::{AuthorityCursor, SessionDefinition},
    PlayerId,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub definition: SessionDefinition,
    pub cursor: AuthorityCursor,
    pub host: PlayerId,
}

/// Canonical uncompressed P-256 point, with the fixed 0x04 prefix implicit.
/// Fixed arrays keep malformed postcard lengths from allocating attacker-sized buffers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PakePublicMessage {
    pub x: [u8; 32],
    pub y: [u8; 32],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerHello {
    pub metadata: SessionMetadata,
    /// Reserved independently by the host; committed only after authentication.
    /// Binding it prevents alteration of AuthAccepted's assigned identity.
    pub reserved_player: PlayerId,
    pub nonce: [u8; 32],
    pub public_message: PakePublicMessage,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientProof {
    pub public_message: PakePublicMessage,
    pub confirmation: [u8; 32],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthAccepted {
    pub player: PlayerId,
    pub confirmation: [u8; 32],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionControlMessage {
    ServerHello(ServerHello),
    ClientProof(ClientProof),
    AuthAccepted(AuthAccepted),
    /// First encrypted Control record; confirms possession of the channel keys.
    SecureChannelReady,
}
