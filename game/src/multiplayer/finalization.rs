//! Bounded full-set join reconciliation. No targets, membership or piece scans.
use super::{protocol::ProtocolDragContexts, MAX_BASELINE_DRAGS};
use crate::resources::PieceDataStore;
use bevy::math::Vec2;
use jigsall_core::{session::AuthoritySession, PlayerId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct FinalDragState {
    pub player: PlayerId,
    pub grab_sequence: u64,
    pub basis_sequence: u64,
    pub last_tick: Option<u64>,
    pub delta: Vec2,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FinalDragSet {
    #[serde(deserialize_with = "deserialize_drags")]
    pub entries: Vec<FinalDragState>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FinalDragError {
    TooManyDrags,
    NonCanonicalPlayers,
    InvalidScalar,
    ContextMismatch,
    InvalidScope,
}

impl FinalDragSet {
    /// Finalize compares only Reliable drag identity. Unchanged authority cursor
    /// and scope separately guarantee the accepted targets/holds are unchanged.
    /// Tick/delta must instead be recaptured and handed off in ReadyCommit.
    pub(crate) fn same_reliable_structure(&self, other: &Self) -> bool {
        self.entries.len() == other.entries.len()
            && self.entries.iter().zip(&other.entries).all(|(a, b)| {
                a.player == b.player
                    && a.grab_sequence == b.grab_sequence
                    && a.basis_sequence == b.basis_sequence
            })
    }

    pub fn validate(&self) -> Result<(), FinalDragError> {
        if self.entries.len() > MAX_BASELINE_DRAGS {
            return Err(FinalDragError::TooManyDrags);
        }
        let mut previous = None;
        for drag in &self.entries {
            if previous.is_some_and(|player| player >= drag.player.0) {
                return Err(FinalDragError::NonCanonicalPlayers);
            }
            if !drag.delta.is_finite() || drag.basis_sequence < drag.grab_sequence {
                return Err(FinalDragError::InvalidScalar);
            }
            previous = Some(drag.player.0);
        }
        Ok(())
    }

    /// At complete authority command boundaries only. Copies at most 64 scalars.
    pub fn capture(
        contexts: &ProtocolDragContexts,
        session: &AuthoritySession,
        store: &PieceDataStore,
    ) -> Result<Self, FinalDragError> {
        if !session.is_active() {
            return Err(FinalDragError::InvalidScope);
        }
        let mut entries = Vec::new();
        for (player, drag) in contexts.active_drags(session, store) {
            if entries.len() == MAX_BASELINE_DRAGS {
                return Err(FinalDragError::TooManyDrags);
            }
            entries.push(FinalDragState {
                player,
                grab_sequence: drag.grab_sequence,
                basis_sequence: drag.basis_sequence,
                last_tick: drag.last_tick,
                delta: drag.delta,
            });
        }
        entries.sort_unstable_by_key(|drag| drag.player.0);
        let set = Self { entries };
        set.validate()?;
        Ok(set)
    }
}

fn deserialize_drags<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<FinalDragState>, D::Error> {
    struct Drags;
    impl<'de> serde::de::Visitor<'de> for Drags {
        type Value = Vec<FinalDragState>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "at most {MAX_BASELINE_DRAGS} final drag scalars")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            // Do not reserve from an untrusted size hint.
            let mut entries = Vec::new();
            while let Some(drag) = seq.next_element::<FinalDragState>()? {
                if entries.len() == MAX_BASELINE_DRAGS {
                    return Err(serde::de::Error::custom("Too many final drags"));
                }
                entries.push(drag);
            }
            Ok(entries)
        }
    }
    deserializer.deserialize_seq(Drags)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jigsall_core::{
        protocol::{ComponentRef, PieceTarget, ProtocolCommandEnvelope, ProtocolPieceCommand},
        session::{
            AuthorityCursor, ClientCommandSequence, ImageHash, SessionDefinition, SessionId,
        },
        PieceId,
    };

    #[test]
    fn decode_bounds_real_entries_and_ignores_gigantic_hints() {
        let scalar = FinalDragState {
            player: PlayerId(1),
            grab_sequence: 0,
            basis_sequence: 0,
            last_tick: None,
            delta: Vec2::ZERO,
        };
        let set = FinalDragSet {
            entries: vec![scalar; MAX_BASELINE_DRAGS],
        };
        assert_eq!(
            postcard::from_bytes::<FinalDragSet>(&postcard::to_allocvec(&set).unwrap()).unwrap(),
            set
        );
        let mut oversized = set;
        oversized.entries.push(scalar);
        assert!(
            postcard::from_bytes::<FinalDragSet>(&postcard::to_allocvec(&oversized).unwrap())
                .is_err()
        );
        assert!(
            serde_json::from_slice::<FinalDragSet>(&serde_json::to_vec(&oversized).unwrap())
                .is_err()
        );
        use crate::network::{
            sync_control::{SyncControlMessage, SyncFinalization},
            wire::{self, WireError, WireMessage},
        };
        let token = SyncFinalization {
            generation: 0,
            cursor: AuthorityCursor::new(1, 0),
            revision: 1,
        };
        for control in [
            SyncControlMessage::Finalize {
                token,
                drags: oversized.clone(),
            },
            SyncControlMessage::ReadyCommit {
                token,
                roster: crate::players::PlayerRoster::host_only(PlayerId(1), None).snapshot(),
                drags: oversized,
            },
        ] {
            assert_eq!(
                wire::encode(&WireMessage::SyncControl(control.clone())),
                Err(WireError::Oversized)
            );
            let payload = postcard::to_allocvec(&control).unwrap();
            let mut frame = b"PZLA".to_vec();
            frame.extend_from_slice(&wire::WIRE_VERSION.to_le_bytes());
            frame.extend_from_slice(&[7, 0]);
            frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            frame.extend_from_slice(&payload);
            assert_eq!(wire::decode(&frame), Err(WireError::MalformedPayload));
        }
        struct HugeHint;
        impl<'de> serde::de::SeqAccess<'de> for HugeHint {
            type Error = serde::de::value::Error;
            fn next_element_seed<T: serde::de::DeserializeSeed<'de>>(
                &mut self,
                _: T,
            ) -> Result<Option<T::Value>, Self::Error> {
                Ok(None)
            }
            fn size_hint(&self) -> Option<usize> {
                Some(usize::MAX)
            }
        }
        let decoded =
            deserialize_drags(serde::de::value::SeqAccessDeserializer::new(HugeHint)).unwrap();
        assert!(decoded.is_empty());
        assert_eq!(decoded.capacity(), 0);
        // Truncated/untrusted vector lengths never reserve that claimed size.
        assert!(
            postcard::from_bytes::<FinalDragSet>(&postcard::to_allocvec(&usize::MAX).unwrap())
                .is_err()
        );
    }

    #[test]
    fn capture_is_canonical_and_bounded_at_64_even_without_piece_scans() {
        let mut session = AuthoritySession::new(
            SessionDefinition {
                id: SessionId(1),
                image_hash: ImageHash([0; 32]),
            },
            PlayerId(100),
            AuthorityCursor::new(1, 0),
        );
        let mut store = PieceDataStore::default();
        store.initialize((0..65).map(|i| Vec2::new(i as f32, 0.0)).collect());
        let mut contexts = ProtocolDragContexts::default();
        let definition = jigsall_core::PuzzleDefinition {
            generator_version: jigsall_core::GENERATOR_VERSION,
            seed: 42,
            grid_size: bevy::math::UVec2::new(65, 1),
            image_size: bevy::math::UVec2::new(1300, 20),
            snap_distance: 5.0,
            rotation_enabled: true,
        };
        for i in (0..64).rev() {
            let cmd = ProtocolCommandEnvelope {
                session: session.session_id(),
                authority_epoch: session.cursor().epoch,
                player: PlayerId(i),
                sequence: ClientCommandSequence::Control(0),
                command: ProtocolPieceCommand::Grab {
                    target: PieceTarget::Component(ComponentRef {
                        member: PieceId(i as u32),
                        expected_size: 1,
                    }),
                },
            };
            contexts
                .apply_replicated(
                    &mut session,
                    &mut store,
                    cmd.player,
                    &cmd,
                    Some(&definition),
                    PlayerId(100),
                )
                .unwrap();
        }
        let set = FinalDragSet::capture(&contexts, &session, &store).unwrap();
        assert_eq!(set.entries.len(), 64);
        assert!(set
            .entries
            .iter()
            .enumerate()
            .all(|(i, drag)| drag.player.0 == i as u64));
        let cmd = ProtocolCommandEnvelope {
            session: session.session_id(),
            authority_epoch: session.cursor().epoch,
            player: PlayerId(64),
            sequence: ClientCommandSequence::Control(0),
            command: ProtocolPieceCommand::Grab {
                target: PieceTarget::Component(ComponentRef {
                    member: PieceId(64),
                    expected_size: 1,
                }),
            },
        };
        contexts
            .apply_replicated(
                &mut session,
                &mut store,
                cmd.player,
                &cmd,
                Some(&definition),
                PlayerId(100),
            )
            .unwrap();
        assert_eq!(
            FinalDragSet::capture(&contexts, &session, &store),
            Err(FinalDragError::TooManyDrags)
        );
    }
}
