use super::{BranchNodeCompact, PackedStoredNibblesSubKey, StoredNibblesSubKey};
use reth_primitives_traits::ValueWithSubKey;

/// Account storage trie node.
///
/// `nibbles` is the subkey when used as a value in the `StorageTrie` table.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(any(test, feature = "serde"), derive(serde::Serialize, serde::Deserialize))]
pub struct StorageTrieEntry {
    /// The nibbles of the intermediate node
    pub nibbles: StoredNibblesSubKey,
    /// Encoded node.
    pub node: BranchNodeCompact,
}

impl ValueWithSubKey for StorageTrieEntry {
    type SubKey = StoredNibblesSubKey;

    fn get_subkey(&self) -> Self::SubKey {
        self.nibbles.clone()
    }
}

// NOTE: Removing reth_codec and manually encode subkey
// and compress second part of the value. If we have compression
// over whole value (Even SubKey) that would mess up fetching of values with seek_by_key_subkey
#[cfg(any(test, feature = "reth-codec"))]
impl reth_codecs::Compact for StorageTrieEntry {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let nibbles_len = self.nibbles.to_compact(buf);
        let node_len = self.node.to_compact(buf);
        nibbles_len + node_len
    }

    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        let (nibbles, buf) = StoredNibblesSubKey::from_compact(buf, 65);
        let (node, buf) = BranchNodeCompact::from_compact(buf, len - 65);
        let this = Self { nibbles, node };
        (this, buf)
    }
}

/// Account storage trie node with packed nibble encoding (storage v2).
///
/// Same as [`StorageTrieEntry`] but uses [`PackedStoredNibblesSubKey`] (33 bytes)
/// instead of [`StoredNibblesSubKey`] (65 bytes).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(any(test, feature = "serde"), derive(serde::Serialize, serde::Deserialize))]
pub struct PackedStorageTrieEntry {
    /// The nibbles of the intermediate node
    pub nibbles: PackedStoredNibblesSubKey,
    /// Encoded node.
    pub node: BranchNodeCompact,
}

impl ValueWithSubKey for PackedStorageTrieEntry {
    type SubKey = PackedStoredNibblesSubKey;

    fn get_subkey(&self) -> Self::SubKey {
        self.nibbles.clone()
    }
}

#[cfg(any(test, feature = "reth-codec"))]
impl reth_codecs::Compact for PackedStorageTrieEntry {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let nibbles_len = self.nibbles.to_compact(buf);
        let node_len = self.node.to_compact(buf);
        nibbles_len + node_len
    }

    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        let (nibbles, buf) = PackedStoredNibblesSubKey::from_compact(buf, 33);
        let (node, buf) = BranchNodeCompact::from_compact(buf, len - 33);
        (Self { nibbles, node }, buf)
    }
}
