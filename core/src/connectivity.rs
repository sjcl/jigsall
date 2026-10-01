//! Permanent connectivity without per-component allocations or entity hierarchies.
use crate::{PieceBitSet, PieceId, PieceScratchSet, MAX_PIECES};

const ID_BITS: u32 = 20;
const ID_MASK: u32 = (1 << ID_BITS) - 1;
const MIN_LOW_BITS: u32 = 9;
const MIN_LOW_MASK: u32 = (1 << MIN_LOW_BITS) - 1;

/// Union-by-size forest and circular lists: 8 bytes per piece. Spare bits in root
/// words also encode the minimum member ID, so canonical offsets survive restore
/// even when union history produces a different root. Children still store parent IDs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PieceConnectivity {
    parent_or_size: Vec<i32>,
    next_member: Vec<u32>,
}

impl PieceConnectivity {
    pub fn new(count: usize) -> Self {
        assert!(count <= MAX_PIECES && count <= ID_MASK as usize);
        Self {
            parent_or_size: (0..count as u32).map(|id| Self::root_word(1, id)).collect(),
            next_member: (0..count as u32)
                .map(|id| id | ((id & MIN_LOW_MASK) << ID_BITS))
                .collect(),
        }
    }

    pub fn len(&self) -> usize {
        self.parent_or_size.len()
    }

    pub fn is_empty(&self) -> bool {
        self.parent_or_size.is_empty()
    }

    pub fn storage_bytes(&self) -> usize {
        (self.parent_or_size.capacity() + self.next_member.capacity()) * 4
    }

    /// Read-only lookup is at most log2(N) links with union-by-size.
    #[inline]
    pub fn find_root(&self, id: PieceId) -> PieceId {
        let mut root = id.0 as usize;
        while self.parent_or_size[root] >= 0 {
            root = self.parent_or_size[root] as usize;
        }
        PieceId(root as u32)
    }

    fn compress_root(&mut self, id: PieceId) -> PieceId {
        let root = self.find_root(id);
        let mut index = id.0 as usize;
        while self.parent_or_size[index] >= 0 {
            let parent = self.parent_or_size[index] as usize;
            self.parent_or_size[index] = root.0 as i32;
            index = parent;
        }
        root
    }

    pub fn same_component(&self, a: PieceId, b: PieceId) -> bool {
        self.find_root(a) == self.find_root(b)
    }

    #[inline]
    pub fn component_size(&self, id: PieceId) -> usize {
        self.root_size(self.find_root(id)) as usize
    }

    #[inline]
    fn root_size(&self, root: PieceId) -> u32 {
        (-self.parent_or_size[root.0 as usize] as u32) & ID_MASK
    }

    fn root_word(size: u32, minimum: u32) -> i32 {
        -((size | ((minimum >> MIN_LOW_BITS) << ID_BITS)) as i32)
    }

    /// Stable representative derived from membership, independent of DSU parent history.
    #[inline]
    pub fn minimum_member(&self, id: PieceId) -> PieceId {
        let root = self.find_root(id).0 as usize;
        let high = (-self.parent_or_size[root] as u32) >> ID_BITS;
        let low = (self.next_member[root] >> ID_BITS) & MIN_LOW_MASK;
        PieceId((high << MIN_LOW_BITS) | low)
    }

    /// Starts at any member; each member is visited once, independent of all other pieces.
    pub fn iter_component(&self, id: PieceId) -> impl Iterator<Item = PieceId> + '_ {
        let mut next = Some(id);
        std::iter::from_fn(move || {
            let current = next?;
            let successor = PieceId(self.next_member[current.0 as usize] & ID_MASK);
            next = (successor != id).then_some(successor);
            Some(current)
        })
    }

    /// O(1) list splice; no member copies. Equal-sized trees choose the smaller root.
    pub fn union(&mut self, a: PieceId, b: PieceId) -> PieceId {
        let mut a = self.compress_root(a);
        let mut b = self.compress_root(b);
        if a == b {
            return a;
        }
        let sa = self.root_size(a);
        let sb = self.root_size(b);
        let minimum = self.minimum_member(a).0.min(self.minimum_member(b).0);
        if sa < sb || (sa == sb && a > b) {
            std::mem::swap(&mut a, &mut b);
        }
        self.parent_or_size[a.0 as usize] = Self::root_word(sa + sb, minimum);
        self.parent_or_size[b.0 as usize] = a.0 as i32;
        self.next_member.swap(a.0 as usize, b.0 as usize);
        self.next_member[a.0 as usize] =
            (self.next_member[a.0 as usize] & ID_MASK) | ((minimum & MIN_LOW_MASK) << ID_BITS);
        a
    }

    /// Preserves shared all-valid masks. Scratch root bits prevent repeated expansion.
    pub fn expand(&self, requested: &PieceBitSet) -> PieceBitSet {
        let mut expanded = if requested.bit_len() == self.len() {
            requested.clone()
        } else {
            let mut normalized = PieceBitSet::new(self.len());
            normalized.union(requested);
            normalized
        };
        let mut seen = PieceScratchSet::new(self.len());
        for id in requested.iter().filter(|id| (id.0 as usize) < self.len()) {
            if self.parent_or_size[id.0 as usize] < 0 && self.root_size(id) == 1 {
                continue;
            }
            let root = self.find_root(id);
            if !seen.contains(&root) {
                seen.insert(root);
                for member in self.iter_component(root) {
                    if !expanded.contains(&member) {
                        expanded.insert(member);
                    }
                }
            }
        }
        expanded
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_survive_balanced_unions_and_nonroot_splices() {
        let mut c = PieceConnectivity::new(100);
        for stride in [1, 2, 4, 8, 16, 32, 64] {
            for start in (0..100).step_by(stride * 2) {
                if start + stride < 100 {
                    c.union(PieceId(start as u32), PieceId((start + stride) as u32));
                }
            }
        }
        for id in 0..100 {
            assert_eq!(c.find_root(PieceId(id)), PieceId(0));
            assert_eq!(c.component_size(PieceId(id)), 100);
            let mut members: Vec<_> = c.iter_component(PieceId(id)).collect();
            members.sort_unstable();
            assert_eq!(members, (0..100).map(PieceId).collect::<Vec<_>>());
        }
        let mut requested = PieceBitSet::new(100);
        requested.insert(PieceId(73));
        assert_eq!(c.expand(&requested).count(), 100);
    }

    #[test]
    fn million_chain_uses_eight_mb_and_visits_each_member_once() {
        let mut c = PieceConnectivity::new(MAX_PIECES);
        assert_eq!(c.storage_bytes(), 8_000_000);
        for id in 1..MAX_PIECES as u32 {
            c.union(PieceId(id - 1), PieceId(id));
        }
        assert_eq!(c.component_size(PieceId(999_999)), MAX_PIECES);
        assert_eq!(c.iter_component(PieceId(123)).count(), MAX_PIECES);
        let mut all = PieceBitSet::new(MAX_PIECES);
        all.fill();
        let expanded = c.expand(&all);
        assert!(std::sync::Arc::ptr_eq(all.words(), expanded.words()));
    }

    #[test]
    fn packed_minimum_survives_high_ids_root_changes_and_list_splices() {
        let mut c = PieceConnectivity::new(MAX_PIECES);
        for id in [0, 511, 512, 524_287, 524_288, 999_999] {
            assert_eq!(c.minimum_member(PieceId(id)), PieceId(id));
            assert_eq!(c.component_size(PieceId(id)), 1);
            assert_eq!(
                c.iter_component(PieceId(id)).collect::<Vec<_>>(),
                [PieceId(id)]
            );
        }
        c.union(PieceId(999_998), PieceId(999_999));
        c.union(PieceId(999_998), PieceId(512));
        c.union(PieceId(511), PieceId(999_999));
        for id in [511, 512, 999_998, 999_999] {
            assert_eq!(c.find_root(PieceId(id)), PieceId(999_998));
            assert_eq!(c.minimum_member(PieceId(id)), PieceId(511));
            assert_eq!(c.component_size(PieceId(id)), 4);
            let mut members: Vec<_> = c.iter_component(PieceId(id)).collect();
            members.sort_unstable();
            assert_eq!(
                members,
                [
                    PieceId(511),
                    PieceId(512),
                    PieceId(999_998),
                    PieceId(999_999)
                ]
            );
        }
    }
}
