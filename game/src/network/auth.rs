//! Replaceable PAKE adapter. Only this module handles pakery types and secrets.
use super::{session_control::*, wire::WIRE_VERSION};
use pakery_core::crypto::{CpaceGroup, Hash};
use pakery_crypto::{P256Group, Sha512Hash, Spake2P256};
use pakery_spake2::{PartyA, PartyAState, PartyB, Spake2Output};
use puzzella_core::PlayerId;
use std::fmt;
use zeroize::Zeroizing;

pub const MIN_PASSWORD_BYTES: usize = 8;
pub const MAX_PASSWORD_BYTES: usize = 128;

/// Memory-only UTF-8 secret. Intentionally neither Serialize nor Clone nor Display.
pub struct SessionPassword(Zeroizing<String>);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasswordError {
    TooShort,
    TooLong,
}
impl SessionPassword {
    /// Takes ownership so the input buffer is also zeroized, including invalid input.
    pub fn new(value: String) -> Result<Self, PasswordError> {
        let value = Zeroizing::new(value);
        match value.len() {
            0..MIN_PASSWORD_BYTES => Err(PasswordError::TooShort),
            n if n > MAX_PASSWORD_BYTES => Err(PasswordError::TooLong),
            _ => Ok(Self(value)),
        }
    }
}
impl fmt::Debug for SessionPassword {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SessionPassword([REDACTED])")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AuthError;
pub(crate) struct ServerHandshake(PartyAState<Spake2P256>);
pub(crate) struct ClientHandshake {
    output: Spake2Output,
    player: PlayerId,
}

fn scalar(
    password: &SessionPassword,
) -> Result<Zeroizing<<P256Group as CpaceGroup>::Scalar>, AuthError> {
    // RFC 9382 leaves the password-to-scalar function to the application. Use
    // pakery's documented SHA-512/wide-reduction recipe; never send/store w.
    let hash = Zeroizing::new(Sha512Hash::digest(password.0.as_bytes()));
    P256Group::scalar_from_wide_bytes(&hash)
        .map(Zeroizing::new)
        .map_err(|_| AuthError)
}

fn context(hello: &ServerHello) -> Vec<u8> {
    let mut aad = b"puzzella-session-auth-v1".to_vec();
    aad.extend_from_slice(&WIRE_VERSION.to_le_bytes());
    aad.extend_from_slice(&hello.metadata.definition.id.0.to_le_bytes());
    aad.extend_from_slice(&hello.metadata.definition.image_hash.0);
    aad.extend_from_slice(&hello.metadata.host.0.to_le_bytes());
    aad.extend_from_slice(&hello.metadata.cursor.epoch.0.to_le_bytes());
    aad.extend_from_slice(&hello.metadata.cursor.sequence.0.to_le_bytes());
    aad.extend_from_slice(&hello.nonce);
    aad.extend_from_slice(&hello.reserved_player.0.to_le_bytes());
    aad
}
fn public(bytes: &[u8]) -> Result<PakePublicMessage, AuthError> {
    if bytes.len() != 65 || bytes[0] != 4 {
        return Err(AuthError);
    }
    Ok(PakePublicMessage {
        x: bytes[1..33].try_into().map_err(|_| AuthError)?,
        y: bytes[33..].try_into().map_err(|_| AuthError)?,
    })
}
fn encoded(message: PakePublicMessage) -> [u8; 65] {
    let mut bytes = [0; 65];
    bytes[0] = 4;
    bytes[1..33].copy_from_slice(&message.x);
    bytes[33..].copy_from_slice(&message.y);
    bytes
}

impl ServerHandshake {
    pub(crate) fn start(
        password: &SessionPassword,
        metadata: SessionMetadata,
        player: PlayerId,
    ) -> Result<(Self, ServerHello), AuthError> {
        let mut hello = ServerHello {
            metadata,
            reserved_player: player,
            nonce: [0; 32],
            public_message: PakePublicMessage {
                x: [0; 32],
                y: [0; 32],
            },
        };
        getrandom::fill(&mut hello.nonce).map_err(|_| AuthError)?;
        let w = scalar(password)?;
        let (message, state) = PartyA::<Spake2P256>::start(
            &w,
            b"puzzella-host",
            b"puzzella-client",
            &context(&hello),
            &mut rand_core::UnwrapErr(getrandom::SysRng),
        )
        .map_err(|_| AuthError)?;
        hello.public_message = public(&message)?;
        Ok((Self(state), hello))
    }
    pub(crate) fn finish(self, proof: ClientProof) -> Result<[u8; 32], AuthError> {
        let output = self
            .0
            .finish(&encoded(proof.public_message))
            .map_err(|_| AuthError)?;
        output
            .verify_peer_confirmation(&proof.confirmation)
            .map_err(|_| AuthError)?;
        output
            .confirmation_mac
            .as_slice()
            .try_into()
            .map_err(|_| AuthError)
    }
}
impl ClientHandshake {
    pub(crate) fn start(
        password: &SessionPassword,
        hello: &ServerHello,
    ) -> Result<(Self, ClientProof), AuthError> {
        if hello.reserved_player == hello.metadata.host {
            return Err(AuthError);
        }
        let w = scalar(password)?;
        let (message, state) = PartyB::<Spake2P256>::start(
            &w,
            b"puzzella-host",
            b"puzzella-client",
            &context(hello),
            &mut rand_core::UnwrapErr(getrandom::SysRng),
        )
        .map_err(|_| AuthError)?;
        let output = state
            .finish(&encoded(hello.public_message))
            .map_err(|_| AuthError)?;
        let proof = ClientProof {
            public_message: public(&message)?,
            confirmation: output
                .confirmation_mac
                .as_slice()
                .try_into()
                .map_err(|_| AuthError)?,
        };
        Ok((
            Self {
                output,
                player: hello.reserved_player,
            },
            proof,
        ))
    }
    pub(crate) fn finish(self, accepted: AuthAccepted) -> Result<PlayerId, AuthError> {
        if accepted.player != self.player {
            return Err(AuthError);
        }
        self.output
            .verify_peer_confirmation(&accepted.confirmation)
            .map_err(|_| AuthError)?;
        Ok(accepted.player)
    }
}
