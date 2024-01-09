use bytes::Buf;
use derive_more::Deref;
use reth_codecs::Compact;
use serde::{Deserialize, Serialize};

pub use nybbles::Nibbles;

/// The representation of nibbles of the merkle trie stored in the database.
#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    derive_more::Index,
)]
pub struct StoredNibbles(pub Nibbles);

impl From<Nibbles> for StoredNibbles {
    #[inline]
    fn from(value: Nibbles) -> Self {
        Self(value)
    }
}

impl From<Vec<u8>> for StoredNibbles {
    #[inline]
    fn from(value: Vec<u8>) -> Self {
        Self(Nibbles::from_nibbles_unchecked(value))
    }
}

impl PartialEq<[u8]> for StoredNibbles {
    #[inline]
    fn eq(&self, other: &[u8]) -> bool {
        self.0.as_slice() == other
    }
}

impl PartialOrd<[u8]> for StoredNibbles {
    #[inline]
    fn partial_cmp(&self, other: &[u8]) -> Option<std::cmp::Ordering> {
        self.0.as_slice().partial_cmp(other)
    }
}

impl core::borrow::Borrow<[u8]> for StoredNibbles {
    #[inline]
    fn borrow(&self) -> &[u8] {
        self.0.as_slice()
    }
}

impl Compact for StoredNibbles {
    fn to_compact<B>(self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        buf.put_slice(self.0.as_slice());
        self.0.len()
    }

    fn from_compact(mut buf: &[u8], len: usize) -> (Self, &[u8]) {
        let nibbles = &buf[..len];
        buf.advance(len);
        (Self(Nibbles::from_nibbles_unchecked(nibbles)), buf)
    }
}

/// The representation of nibbles of the merkle trie stored in the database.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord, Hash, Deref)]
pub struct StoredNibblesSubKey(pub Nibbles);

impl From<Nibbles> for StoredNibblesSubKey {
    #[inline]
    fn from(value: Nibbles) -> Self {
        Self(value)
    }
}

impl From<Vec<u8>> for StoredNibblesSubKey {
    #[inline]
    fn from(value: Vec<u8>) -> Self {
        Self(Nibbles::from_nibbles_unchecked(value))
    }
}

impl From<StoredNibblesSubKey> for Nibbles {
    #[inline]
    fn from(value: StoredNibblesSubKey) -> Self {
        value.0
    }
}

impl Compact for StoredNibblesSubKey {
    fn to_compact<B>(self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        assert!(self.0.len() <= 64);

        // right-pad with zeros
        buf.put_slice(&self.0[..]);
        static ZERO: &[u8; 64] = &[0; 64];
        buf.put_slice(&ZERO[self.0.len()..]);

        buf.put_u8(self.0.len() as u8);
        64 + 1
    }

    fn from_compact(buf: &[u8], _len: usize) -> (Self, &[u8]) {
        let len = buf[64] as usize;
        (Self(Nibbles::from_nibbles_unchecked(&buf[..len])), &buf[65..])
    }
}
