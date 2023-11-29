use bytes::Buf;
use derive_more::{BitAnd, BitAndAssign, BitOr, BitOrAssign, Deref, From, Not};
use reth_codecs::{derive_arbitrary, Compact};
use serde::{Deserialize, Serialize};

/// A struct representing a mask of 16 bits, used for Ethereum trie operations.
///
/// Masks in a trie are used to efficiently represent and manage information about the presence or
/// absence of certain elements, such as child nodes, within a trie. Masks are usually implemented
/// as bit vectors, where each bit represents the presence (1) or absence (0) of a corresponding
/// element.
#[derive(
    Default,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    PartialOrd,
    Ord,
    Deref,
    From,
    BitAnd,
    BitAndAssign,
    BitOr,
    BitOrAssign,
    Not,
)]
#[derive_arbitrary(compact)]
pub struct TrieMask(u16);

impl TrieMask {
    /// Creates a new `TrieMask` from the given inner value.
    #[inline]
    pub fn new(inner: u16) -> Self {
        Self(inner)
    }

    /// Creates a new `TrieMask` from the given nibble.
    #[inline]
    pub fn from_nibble(nibble: u8) -> Self {
        Self(1u16 << nibble)
    }

    /// Returns `true` if the current `TrieMask` is a subset of `other`.
    #[inline]
    pub fn is_subset_of(self, other: Self) -> bool {
        self & other == self
    }

    /// Returns `true` if a given bit is set in a mask.
    #[inline]
    pub fn is_bit_set(self, index: u8) -> bool {
        self.0 & (1u16 << index) != 0
    }

    /// Returns `true` if the mask is empty.
    #[inline]
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl std::fmt::Debug for TrieMask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TrieMask({:016b})", self.0)
    }
}

impl Compact for TrieMask {
    fn to_compact<B>(self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        buf.put_u16(self.0);
        2
    }

    fn from_compact(mut buf: &[u8], _len: usize) -> (Self, &[u8]) {
        let mask = buf.get_u16();
        (Self(mask), buf)
    }
}
