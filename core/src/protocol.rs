//! Compact transport-facing commands. Local interaction keeps using PieceCommand.
use crate::{PieceBitSet, PieceConnectivity, PieceId, PlayerId};
use bevy_math::Vec2;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

/// Bounded sparse form; larger selections use the existing dense mask.
pub const MAX_COMPONENT_REFS: usize = 32;

/// Membership-derived identity, never a DSU root. Size detects intervening unions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ComponentRef {
    pub member: PieceId,
    pub expected_size: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TargetError {
    InvalidPieceId,
    StaleComponent,
    InvalidMaskDimensions,
    TooManyComponents,
    StaleTopology,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RejectedComponentRef {
    pub reference: ComponentRef,
    pub reason: TargetError,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ResolvedPieceTarget {
    Sparse(Vec<ComponentRef>),
    Dense(PieceBitSet),
}

#[derive(Debug, PartialEq, Eq)]
pub struct ResolvedTarget {
    pub target: ResolvedPieceTarget,
    pub rejected: Vec<RejectedComponentRef>,
}

/// Historical topology of exactly the components touched by members. This is a
/// stale-state fingerprint, not sender authentication or an authorization token.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DenseTarget {
    pub members: PieceBitSet,
    pub component_count: u32,
    pub topology_digest: u128,
}

impl DenseTarget {
    pub fn from_selection(
        connectivity: &PieceConnectivity,
        members: &PieceBitSet,
    ) -> Result<Self, TargetError> {
        let canonical = canonical_mask(connectivity, members)?;
        let (component_count, topology_digest) = topology_fingerprint(connectivity, &canonical);
        Ok(Self {
            members: members.clone(),
            component_count,
            topology_digest,
        })
    }

    /// No component list is materialized. Validate topology before gameplay and
    /// return a canonical authority mask only if the entire fingerprint matches.
    pub fn resolve(&self, connectivity: &PieceConnectivity) -> Result<PieceBitSet, TargetError> {
        let canonical = canonical_mask(connectivity, &self.members)?;
        if topology_fingerprint(connectivity, &canonical)
            != (self.component_count, self.topology_digest)
        {
            return Err(TargetError::StaleTopology);
        }
        Ok(canonical)
    }
}

fn canonical_mask(
    connectivity: &PieceConnectivity,
    members: &PieceBitSet,
) -> Result<PieceBitSet, TargetError> {
    if members.bit_len() != connectivity.len() {
        return Err(TargetError::InvalidMaskDimensions);
    }
    Ok(connectivity.expand(members))
}

/// Versioned, serializer-independent SHA-256 input: domain, ascending minimum
/// member/size pairs as u32 LE, then component count as u32 LE. Retain its first
/// 16 bytes interpreted as a little-endian u128. Traversal uses the canonical
/// mask's ascending IDs, not DSU roots or an allocated component-reference list.
fn topology_fingerprint(connectivity: &PieceConnectivity, canonical: &PieceBitSet) -> (u32, u128) {
    let mut hash = Sha256::new();
    hash.update(b"puzzella/component-topology/v1\0");
    let mut count = 0u32;
    for id in canonical.iter() {
        if connectivity.minimum_member(id) == id {
            hash.update(id.0.to_le_bytes());
            hash.update((connectivity.component_size(id) as u32).to_le_bytes());
            count += 1;
        }
    }
    hash.update(count.to_le_bytes());
    let bytes = hash.finalize();
    (count, u128::from_le_bytes(bytes[..16].try_into().unwrap()))
}

impl ComponentRef {
    pub fn from_member(
        connectivity: &PieceConnectivity,
        member: PieceId,
    ) -> Result<Self, TargetError> {
        if member.0 as usize >= connectivity.len() {
            return Err(TargetError::InvalidPieceId);
        }
        Ok(Self {
            member: connectivity.minimum_member(member),
            expected_size: connectivity.component_size(member) as u32,
        })
    }

    /// Resolve only against current authority; an old reference never grows its target.
    pub fn resolve(self, connectivity: &PieceConnectivity) -> Result<PieceId, TargetError> {
        let current = Self::from_member(connectivity, self.member)?;
        if self != current {
            return Err(TargetError::StaleComponent);
        }
        Ok(self.member)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PieceTarget {
    Component(ComponentRef),
    Components(#[serde(deserialize_with = "deserialize_entries")] Vec<ComponentRef>),
    Dense(DenseTarget),
}

fn deserialize_entries<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Vec<T>, D::Error> {
    struct Entries<T>(std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>> serde::de::Visitor<'de> for Entries<T> {
        type Value = Vec<T>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "at most {MAX_COMPONENT_REFS} protocol entries")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut refs = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(MAX_COMPONENT_REFS));
            while let Some(reference) = seq.next_element()? {
                if refs.len() == MAX_COMPONENT_REFS {
                    return Err(serde::de::Error::custom("Too many protocol entries"));
                }
                refs.push(reference);
            }
            Ok(refs)
        }
    }
    deserializer.deserialize_seq(Entries(std::marker::PhantomData))
}

impl PieceTarget {
    /// Canonicalize untrusted input. Invalid/stale sparse entries are rejected
    /// individually; duplicates cannot apply an operation twice. Dense topology
    /// must match as a whole before any component is eligible for gameplay.
    pub fn resolve(&self, connectivity: &PieceConnectivity) -> Result<ResolvedTarget, TargetError> {
        let mut components = Vec::new();
        let mut rejected = Vec::new();
        let refs = match self {
            Self::Component(reference) => std::slice::from_ref(reference),
            Self::Components(refs) => {
                if refs.len() > MAX_COMPONENT_REFS {
                    return Err(TargetError::TooManyComponents);
                }
                refs
            }
            Self::Dense(dense) => {
                return Ok(ResolvedTarget {
                    target: ResolvedPieceTarget::Dense(dense.resolve(connectivity)?),
                    rejected,
                });
            }
        };
        for reference in refs.iter().copied().collect::<BTreeSet<_>>() {
            match reference.resolve(connectivity) {
                Ok(_) => components.push(reference),
                Err(reason) => rejected.push(RejectedComponentRef { reference, reason }),
            }
        }
        Ok(ResolvedTarget {
            target: ResolvedPieceTarget::Sparse(components),
            rejected,
        })
    }

    /// Encode only at a protocol boundary, never on idle or pointer frames.
    /// Partial selections address whole current components. Empty selections encode
    /// as Components([]). Sparse entries are sorted by stable minimum member.
    pub fn from_selection(
        connectivity: &PieceConnectivity,
        selection: &PieceBitSet,
    ) -> Result<Self, TargetError> {
        if selection.bit_len() != connectivity.len() {
            return Err(TargetError::InvalidMaskDimensions);
        }
        let mut refs = BTreeSet::new();
        for id in selection.iter() {
            refs.insert(ComponentRef::from_member(connectivity, id)?);
            if refs.len() > MAX_COMPONENT_REFS {
                // Retain the local COW mask; authority will expand/revalidate it.
                return Ok(Self::Dense(DenseTarget::from_selection(
                    connectivity,
                    selection,
                )?));
            }
        }
        if refs.len() == 1 {
            Ok(Self::Component(*refs.first().unwrap()))
        } else {
            Ok(Self::Components(refs.into_iter().collect()))
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ProtocolPieceCommand {
    /// Reliable control creates a context from authority-accepted components only.
    Grab { target: PieceTarget },
    /// Absolute delta from Grab's base positions; latest tick wins. No membership.
    DragUpdate { delta: Vec2 },
    /// Reliable final absolute delta, independent of all transient updates.
    Release {
        grab_sequence: u64,
        final_delta: Vec2,
    },
}

pub type ProtocolCommandEnvelope = crate::session::ClientCommandEnvelope<ProtocolPieceCommand>;

/// Authority-created canonical accepted membership; never copy unvalidated wire
/// targets into a context. Dense selections remain bitsets, not millions of refs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActiveDragTarget {
    Sparse(Vec<ComponentRef>),
    Dense(DenseTarget),
}

impl ActiveDragTarget {
    pub fn from_accepted_members(
        connectivity: &PieceConnectivity,
        members: &PieceBitSet,
    ) -> Result<Self, TargetError> {
        Ok(match PieceTarget::from_selection(connectivity, members)? {
            PieceTarget::Component(reference) => Self::Sparse(vec![reference]),
            PieceTarget::Components(refs) => Self::Sparse(refs),
            PieceTarget::Dense(dense) => Self::Dense(dense),
        })
    }

    /// Small sparse copies or a shared dense mask for a semantic authority ACK.
    pub fn to_piece_target(&self) -> PieceTarget {
        match self {
            Self::Sparse(refs) if refs.len() == 1 => PieceTarget::Component(refs[0]),
            Self::Sparse(refs) => PieceTarget::Components(refs.clone()),
            Self::Dense(dense) => PieceTarget::Dense(dense.clone()),
        }
    }
}

/// Serializable semantic ACK; accepted names exact canonical membership, including
/// when ownership/placed/enabled validation accepted only part of a multi-grab.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrabAccepted {
    pub player: PlayerId,
    pub grab_sequence: u64,
    pub accepted: PieceTarget,
    #[serde(deserialize_with = "deserialize_entries")]
    pub rejected: Vec<RejectedComponentRef>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProtocolAuthorityEvent {
    GrabAccepted(GrabAccepted),
}

pub type ProtocolAuthorityEventEnvelope =
    crate::session::AuthorityEventEnvelope<ProtocolAuthorityEvent>;

/// Authority-created state; not deserialized from clients.
#[derive(Clone, Debug, PartialEq)]
pub struct ActiveDrag {
    pub grab_sequence: u64,
    pub target: ActiveDragTarget,
    pub delta: Vec2,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selection(count: usize, ids: impl IntoIterator<Item = u32>) -> PieceBitSet {
        let mut mask = PieceBitSet::new(count);
        mask.extend(ids.into_iter().map(PieceId));
        mask
    }

    #[test]
    fn references_resolve_only_current_minimum_and_exact_size() {
        let mut c = PieceConnectivity::new(8);
        let singleton = ComponentRef::from_member(&c, PieceId(7)).unwrap();
        assert_eq!(singleton.resolve(&c), Ok(PieceId(7)));
        c.union(PieceId(3), PieceId(4));
        c.union(PieceId(3), PieceId(0)); // Root 3, minimum 0.
        let r = ComponentRef::from_member(&c, PieceId(4)).unwrap();
        assert_eq!(
            r,
            ComponentRef {
                member: PieceId(0),
                expected_size: 3
            }
        );
        assert_eq!(r.resolve(&c), Ok(PieceId(0)));
        assert_eq!(
            ComponentRef {
                member: PieceId(8),
                expected_size: 1
            }
            .resolve(&c),
            Err(TargetError::InvalidPieceId)
        );
        assert_eq!(
            ComponentRef {
                member: PieceId(3),
                ..r
            }
            .resolve(&c),
            Err(TargetError::StaleComponent)
        );
        for expected_size in [0, 1, 4, u32::MAX] {
            assert_eq!(
                ComponentRef { expected_size, ..r }.resolve(&c),
                Err(TargetError::StaleComponent)
            );
        }
        c.union(PieceId(4), PieceId(7));
        assert_eq!(r.resolve(&c), Err(TargetError::StaleComponent));
        assert_eq!(singleton.resolve(&c), Err(TargetError::StaleComponent));
    }

    #[test]
    fn encoding_is_adaptive_deterministic_and_independent_of_root_history() {
        let mut a = PieceConnectivity::new(64);
        a.union(PieceId(3), PieceId(4));
        a.union(PieceId(3), PieceId(0));
        let mut b = PieceConnectivity::new(64);
        b.union(PieceId(0), PieceId(4));
        b.union(PieceId(0), PieceId(3));
        assert_ne!(a.find_root(PieceId(0)), b.find_root(PieceId(0)));
        let single = selection(64, [4]);
        assert!(matches!(
            PieceTarget::from_selection(&a, &single).unwrap(),
            PieceTarget::Component(ComponentRef {
                member: PieceId(0),
                expected_size: 3
            })
        ));
        let several = selection(64, [63, 4, 8]);
        let expected = PieceTarget::Components(
            [0, 8, 63]
                .map(|id| ComponentRef::from_member(&a, PieceId(id)).unwrap())
                .to_vec(),
        );
        for c in [&a, &b] {
            for _ in 0..3 {
                assert_eq!(
                    PieceTarget::from_selection(c, &several),
                    Ok(expected.clone())
                );
            }
        }
        let c = PieceConnectivity::new(crate::MAX_PIECES);
        for count in [2, 8, 32] {
            assert!(
                matches!(PieceTarget::from_selection(&c, &selection(c.len(), 0..count)).unwrap(), PieceTarget::Components(refs) if refs.len() == count as usize)
            );
        }
        let dense = selection(c.len(), 0..800_000);
        let encoded = PieceTarget::from_selection(&c, &dense).unwrap();
        assert!(
            matches!(&encoded, PieceTarget::Dense(target) if std::sync::Arc::ptr_eq(target.members.words(), dense.words()))
        );
        assert_eq!(PieceTarget::from_selection(&c, &dense).unwrap(), encoded);
        assert_eq!(
            PieceTarget::from_selection(&c, &selection(1, [0])),
            Err(TargetError::InvalidMaskDimensions)
        );
        assert_eq!(
            PieceTarget::from_selection(&c, &selection(c.len(), [])),
            Ok(PieceTarget::Components(vec![]))
        );
    }

    #[test]
    fn resolution_deduplicates_entries_rejects_stale_individually_and_expands_dense() {
        let mut c = PieceConnectivity::new(8);
        c.union(PieceId(1), PieceId(2));
        let a = ComponentRef::from_member(&c, PieceId(2)).unwrap();
        let b = ComponentRef::from_member(&c, PieceId(7)).unwrap();
        let stale = ComponentRef {
            expected_size: 10,
            ..a
        };
        let resolved = PieceTarget::Components(vec![b, a, stale, a, b])
            .resolve(&c)
            .unwrap();
        assert_eq!(resolved.target, ResolvedPieceTarget::Sparse(vec![a, b]));
        assert_eq!(
            resolved.rejected,
            [RejectedComponentRef {
                reference: stale,
                reason: TargetError::StaleComponent
            }]
        );
        assert_eq!(
            PieceTarget::Dense(DenseTarget::from_selection(&c, &selection(8, [2, 7])).unwrap())
                .resolve(&c)
                .unwrap()
                .target,
            ResolvedPieceTarget::Dense(selection(8, [1, 2, 7]))
        );
        assert_eq!(
            PieceTarget::Dense(DenseTarget::from_selection(&c, &selection(8, [1, 2, 7])).unwrap())
                .resolve(&c)
                .unwrap()
                .target,
            ResolvedPieceTarget::Dense(selection(8, [1, 2, 7]))
        );
        assert_eq!(
            PieceTarget::Dense(DenseTarget {
                members: selection(7, [1]),
                component_count: 1,
                topology_digest: 0
            })
            .resolve(&c),
            Err(TargetError::InvalidMaskDimensions)
        );
        assert_eq!(
            PieceTarget::Components(vec![a; MAX_COMPONENT_REFS + 1]).resolve(&c),
            Err(TargetError::TooManyComponents)
        );
    }

    #[test]
    fn dense_topology_rejects_selected_merges_but_ignores_unrelated_merges() {
        for merge in [(0, 1), (0, 60)] {
            let mut c = PieceConnectivity::new(64);
            let target = DenseTarget::from_selection(&c, &selection(64, 0..40)).unwrap();
            c.union(PieceId(50), PieceId(51));
            assert_eq!(target.resolve(&c), Ok(selection(64, 0..40)));
            c.union(PieceId(merge.0), PieceId(merge.1));
            assert_eq!(target.resolve(&c), Err(TargetError::StaleTopology));
        }
        // Even when the component count stays at one, A -> A-B must be rejected.
        let mut c = PieceConnectivity::new(2);
        let target = DenseTarget::from_selection(&c, &selection(2, [0])).unwrap();
        c.union(PieceId(0), PieceId(1));
        assert_eq!(target.resolve(&c), Err(TargetError::StaleTopology));
    }

    #[test]
    fn dense_fingerprint_is_stable_ordered_and_has_a_fixed_serializer_independent_vector() {
        let mut c = PieceConnectivity::new(4);
        c.union(PieceId(1), PieceId(2));
        let partial = DenseTarget::from_selection(&c, &selection(4, [2, 3])).unwrap();
        let full = DenseTarget::from_selection(&c, &selection(4, [1, 2, 3])).unwrap();
        assert_eq!(partial.component_count, 2);
        // Independently computed SHA-256 of the documented domain and LE pairs.
        assert_eq!(partial.topology_digest, 0x197335a049a92f3aa49e27c07ff76ea7);
        assert_eq!(partial.topology_digest, full.topology_digest);
        assert_eq!(partial.resolve(&c), Ok(full.members));

        let mut a = PieceConnectivity::new(64);
        a.union(PieceId(3), PieceId(4));
        a.union(PieceId(3), PieceId(0));
        let mut b = PieceConnectivity::new(64);
        b.union(PieceId(0), PieceId(4));
        b.union(PieceId(0), PieceId(3));
        assert_ne!(a.find_root(PieceId(0)), b.find_root(PieceId(0)));
        let mask = selection(64, 0..50);
        let target = PieceTarget::from_selection(&a, &mask).unwrap();
        assert!(matches!(target, PieceTarget::Dense(_)));
        assert_eq!(PieceTarget::from_selection(&b, &mask).unwrap(), target);
        assert_eq!(target.resolve(&a), target.resolve(&b));
    }

    #[test]
    fn dense_resolve_retains_a_mask_and_rejects_tampered_metadata() {
        let mut c = PieceConnectivity::new(128);
        c.union(PieceId(0), PieceId(1));
        let mask = selection(128, 1..96);
        let dense = DenseTarget::from_selection(&c, &mask).unwrap();
        let resolved = PieceTarget::Dense(dense.clone()).resolve(&c).unwrap();
        let ResolvedPieceTarget::Dense(canonical) = resolved.target else {
            panic!()
        };
        assert_eq!(canonical, selection(128, 0..96));
        for fake_count in [0, dense.component_count + 1, u32::MAX] {
            let mut fake = dense.clone();
            fake.component_count = fake_count;
            assert_eq!(fake.resolve(&c), Err(TargetError::StaleTopology));
        }
        let mut fake = dense;
        fake.topology_digest ^= 1;
        assert_eq!(fake.resolve(&c), Err(TargetError::StaleTopology));
    }
}
