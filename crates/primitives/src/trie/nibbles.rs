use crate::Bytes;
use derive_more::Deref;
use reth_codecs::{main_codec, Compact};
use serde::{Deserialize, Serialize};

/// The nibbles are the keys for the AccountsTrie and the subkeys for the StorageTrie.
#[main_codec]
#[derive(Debug, Default, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct StoredNibbles {
    /// The inner nibble bytes
    pub inner: Bytes,
}

impl From<Vec<u8>> for StoredNibbles {
    fn from(inner: Vec<u8>) -> Self {
        Self { inner: inner.into() }
    }
}

/// The representation of nibbles of the merkle trie stored in the database.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord, Deref)]
pub struct StoredNibblesSubKey(StoredNibbles);

impl From<Vec<u8>> for StoredNibblesSubKey {
    fn from(inner: Vec<u8>) -> Self {
        Self(StoredNibbles { inner: inner.into() })
    }
}

impl Compact for StoredNibblesSubKey {
    fn to_compact<B>(self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        assert!(self.inner.len() <= 64);
        let mut padded = vec![0; 64];
        padded[..self.inner.len()].copy_from_slice(&self.inner[..]);
        buf.put_slice(&padded);
        buf.put_u8(self.inner.len() as u8);
        64 + 1
    }

    fn from_compact(buf: &[u8], _len: usize) -> (Self, &[u8])
    where
        Self: Sized,
    {
        let len = buf[64] as usize;
        let inner = Vec::from(&buf[..len]).into();
        (Self(StoredNibbles { inner }), &buf[65..])
    }
}
