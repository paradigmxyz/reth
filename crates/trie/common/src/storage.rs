use super::{
    BranchNodeCompact, MemcmpStoredNibblesSubKey, PackedStoredNibblesSubKey, StoredNibblesSubKey,
};
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

#[cfg(any(test, feature = "reth-codec"))]
reth_codecs::impl_compression_for_compact!(StorageTrieEntry);

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

#[cfg(any(test, feature = "reth-codec"))]
reth_codecs::impl_compression_for_compact!(PackedStorageTrieEntry);

/// Account storage trie node with memcmp group nibble encoding.
///
/// Same as [`PackedStorageTrieEntry`] but uses [`MemcmpStoredNibblesSubKey`] (5 bytes for paths
/// up to 8 nibbles, at most 40 bytes) instead of [`PackedStoredNibblesSubKey`] (33 bytes).
///
/// The `Compact` encoding is `subkey || node`. The subkey is self-delimiting, so decoding finds
/// the start of the node from the subkey markers.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(any(test, feature = "serde"), derive(serde::Serialize, serde::Deserialize))]
pub struct MemcmpStorageTrieEntry {
    /// The nibbles of the intermediate node.
    pub nibbles: MemcmpStoredNibblesSubKey,
    /// Encoded node.
    pub node: BranchNodeCompact,
}

impl ValueWithSubKey for MemcmpStorageTrieEntry {
    type SubKey = MemcmpStoredNibblesSubKey;

    fn get_subkey(&self) -> Self::SubKey {
        self.nibbles.clone()
    }
}

#[cfg(any(test, feature = "reth-codec"))]
impl reth_codecs::Compact for MemcmpStorageTrieEntry {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let nibbles_len = self.nibbles.to_compact(buf);
        let node_len = self.node.to_compact(buf);
        nibbles_len + node_len
    }

    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        let (nibbles, rest) = MemcmpStoredNibblesSubKey::from_compact(buf, len);
        let nibbles_len = buf.len() - rest.len();
        // `BranchNodeCompact::from_compact` ignores `len` and decodes the whole remaining buffer.
        let (node, rest) = BranchNodeCompact::from_compact(rest, len - nibbles_len);
        (Self { nibbles, node }, rest)
    }
}

#[cfg(any(test, feature = "reth-codec"))]
reth_codecs::impl_compression_for_compact!(MemcmpStorageTrieEntry);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Nibbles;
    use alloy_primitives::B256;
    use proptest::{collection::vec, prelude::*};
    use reth_codecs::Compact;

    fn arb_branch_node() -> impl Strategy<Value = BranchNodeCompact> {
        (any::<u16>(), any::<u16>(), any::<u16>(), any::<Option<[u8; 32]>>()).prop_flat_map(
            |(state_mask, tree_bits, hash_bits, root_hash)| {
                let tree_mask = tree_bits & state_mask;
                let hash_mask = hash_bits & state_mask;
                vec(any::<[u8; 32]>(), hash_mask.count_ones() as usize).prop_map(move |hashes| {
                    BranchNodeCompact::new(
                        state_mask,
                        tree_mask,
                        hash_mask,
                        hashes.into_iter().map(B256::from).collect(),
                        root_hash.map(B256::from),
                    )
                })
            },
        )
    }

    #[test]
    fn test_memcmp_storage_trie_entry_layout() {
        let entry = MemcmpStorageTrieEntry {
            nibbles: MemcmpStoredNibblesSubKey(Nibbles::from_nibbles_unchecked([0x0A, 0x0B, 0x0C])),
            node: BranchNodeCompact::new(0b11, 0b01, 0b10, vec![B256::repeat_byte(0x42)], None),
        };
        let mut buf = Vec::new();
        let len = entry.to_compact(&mut buf);
        assert_eq!(len, buf.len());
        assert_eq!(len, 5 + 6 + 32);

        let mut node_buf = Vec::new();
        entry.node.to_compact(&mut node_buf);
        assert_eq!(&buf[..5], &[0xAB, 0xC0, 0x00, 0x00, 0x03]);
        assert_eq!(&buf[5..], &node_buf[..]);

        let (decoded, rest) = MemcmpStorageTrieEntry::from_compact(&buf, len);
        assert_eq!(decoded, entry);
        assert!(rest.is_empty());
    }

    proptest! {
        #[test]
        fn proptest_memcmp_storage_trie_entry_roundtrip(
            nibbles in vec(0u8..16, 0..=64),
            node in arb_branch_node(),
        ) {
            let entry = MemcmpStorageTrieEntry {
                nibbles: MemcmpStoredNibblesSubKey(Nibbles::from_nibbles_unchecked(nibbles)),
                node,
            };
            let mut buf = Vec::new();
            let len = entry.to_compact(&mut buf);
            prop_assert_eq!(len, buf.len());
            prop_assert!(buf.starts_with(&entry.nibbles.to_compact_array()));

            let (decoded, rest) = MemcmpStorageTrieEntry::from_compact(&buf, len);
            prop_assert_eq!(decoded, entry);
            prop_assert!(rest.is_empty());
        }
    }
}
