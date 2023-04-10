use derive_more::{BitAnd, Deref, From};
use reth_codecs::Compact;
use serde::{Deserialize, Serialize};

/// A struct representing a mask of 16 bits, used for Ethereum trie operations.
#[derive(
    Debug,
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
)]
pub struct TrieMask(u16);

impl TrieMask {
    /// Creates a new `TrieMask` from the given nibble.
    pub fn from_nibble(nibble: i8) -> Self {
        Self(1u16 << nibble)
    }

    /// Returns `true` if the current `TrieMask` is a subset of `other`.
    pub fn is_subset_of(&self, other: &Self) -> bool {
        *self & *other == *self
    }
}

impl Compact for TrieMask {
    fn to_compact(self, buf: &mut impl bytes::BufMut) -> usize {
        buf.put_slice(self.to_be_bytes().as_slice());
        2
    }

    fn from_compact(buf: &[u8], _len: usize) -> (Self, &[u8])
    where
        Self: Sized,
    {
        (Self(u16::from_be_bytes(buf[..2].try_into().unwrap())), &buf[2..])
    }
}
