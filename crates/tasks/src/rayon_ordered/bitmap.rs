use std::sync::atomic::{AtomicU64, Ordering};

/// A fixed-size bitmap backed by `AtomicU64` words.
///
/// Designed for a single-producer-per-bit, single-consumer pattern.
pub(super) struct AtomicBitmap {
    len: usize,
    words: Box<[AtomicU64]>,
}

impl AtomicBitmap {
    const BITS: usize = 64;

    pub(super) fn new(len: usize) -> Self {
        let num_words = len.div_ceil(Self::BITS);
        let words = (0..num_words).map(|_| AtomicU64::new(0)).collect();
        Self { len, words }
    }

    #[inline]
    const fn word_and_mask(index: usize) -> (usize, u64) {
        (index >> 6, 1u64 << (index & 63))
    }

    /// # Safety
    /// `index` must be less than `len`.
    #[inline]
    pub(super) unsafe fn set_unchecked(&self, index: usize, ordering: Ordering) {
        let (w, mask) = Self::word_and_mask(index);
        unsafe { self.words.get_unchecked(w) }.fetch_or(mask, ordering);
    }

    /// # Safety
    /// `index` must be less than `len`.
    #[inline]
    pub(super) unsafe fn is_set_unchecked(&self, index: usize, ordering: Ordering) -> bool {
        let (w, mask) = Self::word_and_mask(index);
        unsafe { self.words.get_unchecked(w) }.load(ordering) & mask != 0
    }

    /// # Safety
    /// `index` must be less than `len`.
    #[inline]
    pub(super) unsafe fn clear_unchecked(&self, index: usize, ordering: Ordering) {
        let (w, mask) = Self::word_and_mask(index);
        unsafe { self.words.get_unchecked(w) }.fetch_and(!mask, ordering);
    }

    /// Iterate over all set bit indices. Only valid when there are no concurrent writers
    /// (i.e. in `Drop` via `&mut self`). Uses non-atomic `get_mut` reads.
    pub(super) fn iter_set_mut(&mut self) -> impl Iterator<Item = usize> + '_ {
        let len = self.len;
        self.words.iter_mut().enumerate().flat_map(move |(wi, word)| {
            let mut bits = *word.get_mut();
            let base = wi * Self::BITS;
            let valid = (len - base).min(Self::BITS);
            if valid < Self::BITS {
                bits &= (1u64 << valid) - 1;
            }
            BitIter { bits, base }
        })
    }
}

struct BitIter {
    bits: u64,
    base: usize,
}

impl Iterator for BitIter {
    type Item = usize;

    #[inline]
    fn next(&mut self) -> Option<usize> {
        if self.bits == 0 {
            return None;
        }
        let bit = self.bits.trailing_zeros() as usize;
        self.bits &= self.bits - 1;
        Some(self.base + bit)
    }
}
