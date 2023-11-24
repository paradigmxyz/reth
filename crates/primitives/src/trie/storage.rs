use super::{BranchNodeCompact, StoredNibblesSubKey};
use reth_codecs::Compact;
use serde::{Deserialize, Serialize};

/// Account storage trie node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord)]
pub struct StorageTrieEntry {
    /// The nibbles of the intermediate node
    pub nibbles: StoredNibblesSubKey,
    /// Encoded node.
    pub node: BranchNodeCompact,
}

// NOTE: Removing main_codec and manually encode subkey
// and compress second part of the value. If we have compression
// over whole value (Even SubKey) that would mess up fetching of values with seek_by_key_subkey
impl Compact for StorageTrieEntry {
    fn to_compact<B>(self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let nibbles_len = self.nibbles.to_compact(buf);
        let node_len = self.node.to_compact(buf);
        nibbles_len + node_len
    }

    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        let (nibbles, buf) = StoredNibblesSubKey::from_compact(buf, 33);
        let (node, buf) = BranchNodeCompact::from_compact(buf, len - 33);
        let this = Self { nibbles, node };
        (this, buf)
    }
}
