//! Compact transport-facing commands. Local interaction keeps using PieceCommand.
use crate::{PieceBitSet, PieceConnectivity, PieceId, PieceScratchSet};
use bevy_math::Vec2;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Bounded sparse form; larger selections use the existing dense mask.
pub const MAX_COMPONENT_REFS: usize = 32;

/// Membership-derived identity, never a DSU root. Size detects intervening unions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ComponentRef {
    pub member: PieceId,
    pub expected_size: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetError {
    InvalidPieceId,
    StaleComponent,
    InvalidMaskDimensions,
    TooManyComponents,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RejectedComponentRef {
    pub reference: ComponentRef,
    pub reason: TargetError,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ResolvedTarget {
    pub components: Vec<ComponentRef>,
    pub rejected: Vec<RejectedComponentRef>,
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
    Components(#[serde(deserialize_with = "deserialize_components")] Vec<ComponentRef>),
    Dense(PieceBitSet),
}

fn deserialize_components<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<ComponentRef>, D::Error> {
    struct Components;
    impl<'de> serde::de::Visitor<'de> for Components {
        type Value = Vec<ComponentRef>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "at most {MAX_COMPONENT_REFS} component references")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut refs = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(MAX_COMPONENT_REFS));
            while let Some(reference) = seq.next_element()? {
                if refs.len() == MAX_COMPONENT_REFS {
                    return Err(serde::de::Error::custom("Too many component references"));
                }
                refs.push(reference);
            }
            Ok(refs)
        }
    }
    deserializer.deserialize_seq(Components)
}

impl PieceTarget {
    /// Canonicalize untrusted input. Invalid/stale sparse entries are rejected
    /// individually; duplicates cannot apply an operation twice. Dense bits name
    /// current components and carry no historical size assertion.
    pub fn resolve(&self, connectivity: &PieceConnectivity) -> Result<ResolvedTarget, TargetError> {
        let mut result = ResolvedTarget::default();
        let refs = match self {
            Self::Component(reference) => std::slice::from_ref(reference),
            Self::Components(refs) => {
                if refs.len() > MAX_COMPONENT_REFS {
                    return Err(TargetError::TooManyComponents);
                }
                refs
            }
            Self::Dense(mask) => {
                if mask.bit_len() != connectivity.len() {
                    return Err(TargetError::InvalidMaskDimensions);
                }
                let mut seen = PieceScratchSet::new(connectivity.len());
                for id in mask.iter() {
                    let minimum = connectivity.minimum_member(id);
                    if id == minimum || (!mask.contains(&minimum) && seen.insert(minimum)) {
                        result
                            .components
                            .push(ComponentRef::from_member(connectivity, minimum)?);
                    }
                }
                result.components.sort_unstable();
                return Ok(result);
            }
        };
        for reference in refs.iter().copied().collect::<BTreeSet<_>>() {
            match reference.resolve(connectivity) {
                Ok(_) => result.components.push(reference),
                Err(reason) => result
                    .rejected
                    .push(RejectedComponentRef { reference, reason }),
            }
        }
        Ok(result)
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
                return Ok(Self::Dense(selection.clone()));
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

/// Authority-created state; not serialized or trusted from clients. References
/// represent accepted components, not client membership or a puzzle-sized mask.
#[derive(Clone, Debug, PartialEq)]
pub struct ActiveDrag {
    pub grab_sequence: u64,
    pub components: Vec<ComponentRef>,
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
            matches!(&encoded, PieceTarget::Dense(mask) if std::sync::Arc::ptr_eq(mask.words(), dense.words()))
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
        assert_eq!(resolved.components, [a, b]);
        assert_eq!(
            resolved.rejected,
            [RejectedComponentRef {
                reference: stale,
                reason: TargetError::StaleComponent
            }]
        );
        assert_eq!(
            PieceTarget::Dense(selection(8, [2, 7]))
                .resolve(&c)
                .unwrap()
                .components,
            [a, b]
        );
        assert_eq!(
            PieceTarget::Dense(selection(8, [1, 2, 7]))
                .resolve(&c)
                .unwrap()
                .components,
            [a, b]
        );
        assert_eq!(
            PieceTarget::Dense(selection(7, [1])).resolve(&c),
            Err(TargetError::InvalidMaskDimensions)
        );
        assert_eq!(
            PieceTarget::Components(vec![a; MAX_COMPONENT_REFS + 1]).resolve(&c),
            Err(TargetError::TooManyComponents)
        );
    }
}
