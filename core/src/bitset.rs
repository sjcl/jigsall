//! Compact row-major membership. Clones share words until a mutation (125 KB / 1M).
use crate::PieceId;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub const MAX_PIECES: usize = 1_000_000;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "BitSetWire", into = "BitSetWire")]
pub struct PieceBitSet {
    words: Arc<[u32]>,
    bit_len: usize,
    count: usize,
}

#[derive(Serialize, Deserialize)]
struct BitSetWire {
    bit_len: usize,
    #[serde(deserialize_with = "deserialize_words")]
    words: Vec<u32>,
}
fn deserialize_words<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<u32>, D::Error> {
    struct Words;
    impl<'de> serde::de::Visitor<'de> for Words {
        type Value = Vec<u32>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("at most 31,250 piece mask words")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let limit = MAX_PIECES.div_ceil(32);
            let mut words = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(limit));
            while let Some(word) = seq.next_element()? {
                if words.len() == limit {
                    return Err(serde::de::Error::custom("Oversized piece mask"));
                }
                words.push(word);
            }
            Ok(words)
        }
    }
    deserializer.deserialize_seq(Words)
}
impl From<PieceBitSet> for BitSetWire {
    fn from(set: PieceBitSet) -> Self {
        Self {
            bit_len: set.bit_len,
            words: set.words.to_vec(),
        }
    }
}
impl TryFrom<BitSetWire> for PieceBitSet {
    type Error = &'static str;
    fn try_from(wire: BitSetWire) -> Result<Self, Self::Error> {
        Self::from_words(wire.bit_len, wire.words)
    }
}
impl PieceBitSet {
    pub fn new(bit_len: usize) -> Self {
        assert!(bit_len <= MAX_PIECES);
        Self {
            words: vec![0; bit_len.div_ceil(32)].into(),
            bit_len,
            count: 0,
        }
    }
    /// Reject malformed dimensions, oversized payloads and nonzero padding bits.
    pub fn from_words(bit_len: usize, words: Vec<u32>) -> Result<Self, &'static str> {
        if bit_len > MAX_PIECES || words.len() != bit_len.div_ceil(32) {
            return Err("Invalid piece mask dimensions");
        }
        if !bit_len.is_multiple_of(32) && words.last().is_some_and(|w| w >> (bit_len % 32) != 0) {
            return Err("Nonzero piece mask padding");
        }
        let count = words.iter().map(|w| w.count_ones() as usize).sum();
        Ok(Self {
            words: words.into(),
            bit_len,
            count,
        })
    }
    pub fn bit_len(&self) -> usize {
        self.bit_len
    }
    pub fn words(&self) -> &Arc<[u32]> {
        &self.words
    }
    pub fn len(&self) -> usize {
        self.count
    }
    pub fn count(&self) -> usize {
        self.count
    }
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
    pub fn contains(&self, id: &PieceId) -> bool {
        (id.0 as usize) < self.bit_len && self.words[id.0 as usize / 32] & (1 << (id.0 % 32)) != 0
    }
    pub fn insert(&mut self, id: PieceId) -> bool {
        if id.0 as usize >= self.bit_len {
            return false;
        }
        let word = &mut Arc::make_mut(&mut self.words)[id.0 as usize / 32];
        let bit = 1 << (id.0 % 32);
        let added = *word & bit == 0;
        *word |= bit;
        self.count += usize::from(added);
        added
    }
    pub fn remove(&mut self, id: &PieceId) -> bool {
        if !self.contains(id) {
            return false;
        }
        Arc::make_mut(&mut self.words)[id.0 as usize / 32] &= !(1 << (id.0 % 32));
        self.count -= 1;
        true
    }
    pub fn clear(&mut self) {
        if self.count != 0 {
            Arc::make_mut(&mut self.words).fill(0);
            self.count = 0;
        }
    }
    pub fn fill(&mut self) {
        let words = Arc::make_mut(&mut self.words);
        words.fill(u32::MAX);
        if !self.bit_len.is_multiple_of(32) {
            *words.last_mut().unwrap() = (1 << (self.bit_len % 32)) - 1;
        }
        self.count = self.bit_len;
    }
    fn combine(&mut self, other: &Self, op: impl Fn(u32, u32) -> u32) {
        self.count = 0;
        for (i, word) in Arc::make_mut(&mut self.words).iter_mut().enumerate() {
            *word = op(*word, other.words.get(i).copied().unwrap_or(0));
            self.count += word.count_ones() as usize;
        }
        if !self.bit_len.is_multiple_of(32) {
            let last = Arc::make_mut(&mut self.words).last_mut().unwrap();
            let masked = *last & ((1 << (self.bit_len % 32)) - 1);
            self.count -= (*last ^ masked).count_ones() as usize;
            *last = masked;
        }
    }
    pub fn union(&mut self, other: &Self) {
        self.combine(other, |a, b| a | b);
    }
    pub fn difference(&mut self, other: &Self) {
        self.combine(other, |a, b| a & !b);
    }
    pub fn iter(&self) -> impl Iterator<Item = PieceId> + '_ {
        self.words
            .iter()
            .enumerate()
            .flat_map(|(index, &word)| SetBits {
                base: index as u32 * 32,
                word,
            })
    }
    pub fn retain(&mut self, mut predicate: impl FnMut(PieceId) -> bool) {
        for index in 0..self.words.len() {
            let word = self.words[index];
            let mut kept = 0;
            for id in (SetBits {
                base: index as u32 * 32,
                word,
            }) {
                if predicate(id) {
                    kept |= 1 << (id.0 % 32);
                }
            }
            // The common all-valid mask keeps its shared allocation. COW only
            // when authority rejects a member, never once per set bit.
            if kept != word {
                Arc::make_mut(&mut self.words)[index] = kept;
                self.count -= (word ^ kept).count_ones() as usize;
            }
        }
    }
}
struct SetBits {
    base: u32,
    word: u32,
}
impl Iterator for SetBits {
    type Item = PieceId;
    fn next(&mut self) -> Option<Self::Item> {
        if self.word == 0 {
            return None;
        }
        let id = PieceId(self.base + self.word.trailing_zeros());
        self.word &= self.word - 1;
        Some(id)
    }
}
impl FromIterator<PieceId> for PieceBitSet {
    fn from_iter<T: IntoIterator<Item = PieceId>>(iter: T) -> Self {
        let mut words = Vec::<u32>::new();
        let mut bit_len = 0;
        for id in iter {
            if id.0 as usize >= MAX_PIECES {
                continue;
            }
            bit_len = bit_len.max(id.0 as usize + 1);
            words.resize(bit_len.div_ceil(32), 0);
            words[id.0 as usize / 32] |= 1 << (id.0 % 32);
        }
        Self::from_words(bit_len, words).unwrap()
    }
}
impl Extend<PieceId> for PieceBitSet {
    fn extend<T: IntoIterator<Item = PieceId>>(&mut self, iter: T) {
        for id in iter {
            self.insert(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn serialized_word_sequence_is_bounded_before_allocation() {
        use serde::de::value::{Error, SeqDeserializer};
        let input = SeqDeserializer::<_, Error>::new(std::iter::repeat_n(0u32, 31_251));
        assert!(deserialize_words(input).is_err());
        let input = SeqDeserializer::<_, Error>::new([1u32, 2].into_iter());
        assert_eq!(deserialize_words(input).unwrap(), [1, 2]);
    }
    #[test]
    fn boundaries_algebra_snapshot_and_malformed_masks() {
        let mut set = PieceBitSet::new(33);
        for id in [0, 31, 32] {
            assert!(set.insert(PieceId(id)));
        }
        assert!(!set.insert(PieceId(33)));
        let snapshot = set.clone();
        assert!(Arc::ptr_eq(set.words(), snapshot.words()));
        set.retain(|_| true);
        assert!(Arc::ptr_eq(set.words(), snapshot.words()));
        set.remove(&PieceId(31));
        assert_eq!(snapshot.count(), 3);
        assert_eq!(
            set.iter().collect::<Vec<_>>(),
            vec![PieceId(0), PieceId(32)]
        );
        let mut removed = snapshot.clone();
        removed.difference(&set);
        assert_eq!(removed.iter().collect::<Vec<_>>(), vec![PieceId(31)]);
        set.union(&snapshot);
        set.difference(&snapshot);
        assert!(set.is_empty());
        set.fill();
        assert_eq!(set.count(), 33);
        set.clear();
        assert!(set.is_empty());
        assert!(PieceBitSet::from_words(33, vec![0, u32::MAX]).is_err());
        assert!(PieceBitSet::from_words(MAX_PIECES + 1, vec![]).is_err());
        assert!(PieceBitSet::from_words(32, vec![0, 0]).is_err());
    }
    #[test]
    fn million_selection_is_125kb_and_iterates_every_id() {
        let mut set = PieceBitSet::new(MAX_PIECES);
        set.fill();
        assert_eq!(set.words().len() * 4, 125_000);
        assert_eq!(set.iter().count(), MAX_PIECES);
        assert_eq!(set.iter().last(), Some(PieceId(999_999)));
    }
}
