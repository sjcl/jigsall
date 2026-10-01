//! Operation-local membership: small ID sets stay on the stack.
use crate::{PieceId, MAX_PIECES};

// Up to four boundary roots per member of an ordinary 32-piece component.
const INLINE_IDS: usize = 128;

/// Deduplication without initializing a puzzle-sized mask for small operations.
/// Large sets promote once to dense words; clear touches only nonzero words.
pub struct PieceScratchSet {
    bit_len: usize,
    inline: [PieceId; INLINE_IDS],
    inline_len: usize,
    words: Vec<u32>,
    touched_words: Vec<u32>,
}
impl PieceScratchSet {
    pub fn new(bit_len: usize) -> Self {
        assert!(bit_len <= MAX_PIECES);
        Self {
            bit_len,
            inline: [PieceId(0); INLINE_IDS],
            inline_len: 0,
            words: Vec::new(),
            touched_words: Vec::new(),
        }
    }

    #[inline]
    pub fn contains(&self, id: &PieceId) -> bool {
        if id.0 as usize >= self.bit_len {
            return false;
        }
        if self.words.is_empty() {
            self.inline[..self.inline_len].contains(id)
        } else {
            self.words[id.0 as usize / 32] & (1 << (id.0 % 32)) != 0
        }
    }

    #[inline]
    pub fn insert(&mut self, id: PieceId) -> bool {
        if id.0 as usize >= self.bit_len {
            return false;
        }
        if self.words.is_empty() {
            if self.inline[..self.inline_len].contains(&id) {
                return false;
            }
            if self.inline_len < INLINE_IDS {
                self.inline[self.inline_len] = id;
                self.inline_len += 1;
                return true;
            }
            self.words.resize(self.bit_len.div_ceil(32), 0);
            for index in 0..self.inline_len {
                self.insert_dense(self.inline[index]);
            }
            self.inline_len = 0;
        }
        self.insert_dense(id)
    }

    #[inline]
    fn insert_dense(&mut self, id: PieceId) -> bool {
        let index = id.0 as usize / 32;
        let word = &mut self.words[index];
        if *word == 0 {
            self.touched_words.push(index as u32);
        }
        let bit = 1 << (id.0 % 32);
        let added = *word & bit == 0;
        *word |= bit;
        added
    }

    pub fn clear(&mut self) {
        self.inline_len = 0;
        for index in self.touched_words.drain(..) {
            self.words[index as usize] = 0;
        }
    }

    /// Heap storage only; the inline IDs are part of the stack-resident value.
    pub fn heap_bytes(&self) -> usize {
        (self.words.capacity() + self.touched_words.capacity()) * 4
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_operations_allocate_nothing_and_dense_promotion_reuses_storage() {
        let mut seen = PieceScratchSet::new(MAX_PIECES);
        for index in 0..INLINE_IDS as u32 {
            let id = PieceId(index * (MAX_PIECES / INLINE_IDS) as u32);
            assert!(seen.insert(id));
            assert!(seen.contains(&id));
            assert!(!seen.insert(id));
        }
        assert_eq!(seen.heap_bytes(), 0);
        assert!(!seen.insert(PieceId(MAX_PIECES as u32)));
        assert!(!seen.contains(&PieceId(MAX_PIECES as u32)));
        seen.clear();
        assert!(!seen.contains(&PieceId(0)));
        assert_eq!(seen.heap_bytes(), 0);
        for id in 0..=INLINE_IDS as u32 {
            assert!(seen.insert(PieceId(id)));
        }
        assert!(seen.insert(PieceId(999_999)));
        let storage = seen.heap_bytes();
        assert!(storage >= 125_000);
        seen.clear();
        for id in [0, 31, 32, 999_999] {
            assert!(!seen.contains(&PieceId(id)));
            assert!(seen.insert(PieceId(id)));
        }
        assert_eq!(seen.heap_bytes(), storage);
        seen.clear();
        assert!(!seen.contains(&PieceId(999_999)));
    }
}
