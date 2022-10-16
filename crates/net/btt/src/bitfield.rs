//! Bittorrent bitfield

use bitvec::prelude::Msb0;
use std::{
    fmt,
    ops::{Deref, DerefMut},
};

// Helper alias.
type BitVec = bitvec::vec::BitVec<u8, Msb0>;

/// The bittorrent bitfield tracks the availability of all the pieces of one torrent for one peer.
///
/// This is essentially a vec of bools that represent if a peer has the piece at the specific
/// position. Where the first highest bit corresponds to the first piece, etc..
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BitField {
    // The wrapped bitfield.
    inner: BitVec,
}

impl BitField {
    /// Returns the index of the first set bit.
    pub fn first_set(&self) -> Option<u32> {
        for (i, nbit) in self.inner.iter().enumerate() {
            if *nbit {
                return Some(i as u32)
            }
        }
        None
    }

    /// Creates a new bitfiled from a vec.
    pub fn from_vec(vec: Vec<u8>) -> Self {
        Self { inner: BitVec::from_vec(vec) }
    }

    /// Creates a new bitfield from a slice of bytes.
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self { inner: BitVec::from_slice(bytes) }
    }

    /// Create a new `Bitfield` with `nbits` bits all set to true.
    pub fn new_all_set(nbits: usize) -> Self {
        Self { inner: BitVec::repeat(true, nbits) }
    }

    /// Create a new `Bitfield` with `nbits` bits all set to false.
    pub fn new_all_clear(nbits: usize) -> Self {
        Self { inner: BitVec::repeat(false, nbits) }
    }
}

impl Deref for BitField {
    type Target = BitVec;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for BitField {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl fmt::Display for BitField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(f)
    }
}
