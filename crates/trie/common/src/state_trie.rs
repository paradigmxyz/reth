//! Complete state trie nodes and sorted, maskable updates.

use crate::{
    utils::{kway_merge_disjoint_sorted_by, kway_merge_sorted},
    BranchNodeRef, BranchNodeV2, LeafNode, Nibbles, PackedStoredNibblesSubKey, ProofTrieNodeV2,
    RlpNode, TrieAccount, TrieMask, TrieNodeV2,
};
use alloc::vec::Vec;
use alloy_primitives::{map::B256Map, U256};
use alloy_rlp::Encodable;

/// A complete persisted node. Extensions are folded into their child branch.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum StateTrieNode<V> {
    /// A leaf, indexed by its full 64-nibble key.
    Leaf {
        /// Length of the leaf's short key.
        short_key_len: u8,
        /// Account or storage value.
        value: V,
    },
    /// A branch, indexed by its logical path (after its extension).
    Branch {
        /// Length of the parent extension's short key, or zero.
        short_key_len: u8,
        /// Present children, in nibble order.
        state_mask: TrieMask,
        /// RLP references to each present child.
        children: Vec<RlpNode>,
    },
}

impl<V: Encodable> StateTrieNode<V> {
    /// Reconstructs the combined proof node at its physical path.
    pub fn proof_node(self, path: Nibbles) -> ProofTrieNodeV2 {
        let short_key_len = match &self {
            Self::Leaf { short_key_len, .. } | Self::Branch { short_key_len, .. } => *short_key_len,
        } as usize;
        let start = path.len() - short_key_len;
        let key = path.slice(start..);
        let node = match self {
            Self::Leaf { value, .. } => {
                TrieNodeV2::Leaf(LeafNode::new(key, alloy_rlp::encode(&value)))
            }
            Self::Branch { state_mask, children, .. } => {
                let branch_rlp_node = (!key.is_empty())
                    .then(|| BranchNodeRef::new(&children, state_mask).rlp(&mut Vec::new()));
                TrieNodeV2::Branch(BranchNodeV2::new(key, children, state_mask, branch_rlp_node))
            }
        };
        ProofTrieNodeV2 { path: path.slice(..start), node, masks: None }
    }
}

/// Storage node with a fixed-width packed path preceding its tagged value.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct StateTrieStorageEntry {
    /// Database duplicate subkey.
    pub nibbles: PackedStoredNibblesSubKey,
    /// Complete storage node.
    pub node: StateTrieNode<U256>,
}

impl reth_primitives_traits::ValueWithSubKey for StateTrieStorageEntry {
    type SubKey = PackedStoredNibblesSubKey;
    fn get_subkey(&self) -> Self::SubKey {
        self.nibbles.clone()
    }
}

/// Sorted changes for one complete trie. `None` deletes a node.
pub type StateTrieNodes<V> = Vec<(Nibbles, Option<StateTrieNode<V>>)>;

/// Complete account and storage trie updates, sorted by path within each trie.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct StateTrieUpdatesSorted {
    /// Account leaves and branches.
    pub account_nodes: StateTrieNodes<TrieAccount>,
    /// Storage leaves and branches by hashed address.
    pub storage_tries: B256Map<StateTrieNodes<U256>>,
}

impl StateTrieUpdatesSorted {
    /// Whether neither trie has changes.
    pub fn is_empty(&self) -> bool {
        self.account_nodes.is_empty() && self.storage_tries.is_empty()
    }

    /// Number of changed nodes.
    pub fn total_len(&self) -> usize {
        self.account_nodes.len() + self.storage_tries.values().map(Vec::len).sum::<usize>()
    }

    /// Merge updates supplied newest first.
    pub fn merge_batch<T: AsRef<Self> + From<Self>>(items: impl IntoIterator<Item = T>) -> T {
        let items: Vec<_> = items.into_iter().collect();
        Self::merge_slice(&items).into()
    }

    /// Merge updates supplied newest first.
    pub fn merge_slice<T: AsRef<Self>>(items: &[T]) -> Self {
        let account_nodes =
            kway_merge_sorted(items.iter().map(|i| i.as_ref().account_nodes.as_slice()));
        let mut storage: B256Map<Vec<_>> = B256Map::default();
        for item in items {
            for (address, nodes) in &item.as_ref().storage_tries {
                storage.entry(*address).or_default().push(nodes.as_slice());
            }
        }
        Self {
            account_nodes,
            storage_tries: storage.into_iter().map(|(a, s)| (a, kway_merge_sorted(s))).collect(),
        }
    }

    /// Merge oldest-first updates, masking keys changed by the suffix unless a suffix value
    /// matches.
    pub fn disjointed_merge_batch(batch: &[&Self], mask: &[&Self]) -> Self {
        if batch.is_empty() {
            return Self::default()
        }
        let accounts = || {
            let merge = |lower, upper| {
                kway_merge_disjoint_sorted_by(
                    batch.iter().rev().map(|i| node_range(&i.account_nodes, lower, upper)),
                    mask.iter().map(|i| node_range(&i.account_nodes, lower, upper)),
                    equal_mask_value,
                )
                .collect::<Vec<_>>()
            };
            #[cfg(feature = "rayon")]
            if batch.iter().map(|i| i.account_nodes.len()).sum::<usize>() >= 4096 {
                use rayon::iter::{IntoParallelIterator, ParallelIterator};
                return (0u8..16)
                    .into_par_iter()
                    .map(|prefix| {
                        // Include the root in the first range. A shorter prefix sorts before
                        // its descendants, so every path belongs to exactly one range.
                        let lower = if prefix == 0 {
                            Nibbles::new()
                        } else {
                            Nibbles::from_nibbles([prefix])
                        };
                        let upper = (prefix < 15).then(|| Nibbles::from_nibbles([prefix + 1]));
                        merge(lower, upper)
                    })
                    .collect::<Vec<_>>()
                    .into_iter()
                    .flatten()
                    .collect()
            }
            merge(Nibbles::new(), None)
        };
        let storages = || {
            let mut storage: B256Map<Vec<_>> = B256Map::default();
            for item in batch.iter().rev() {
                for (address, nodes) in &item.storage_tries {
                    storage.entry(*address).or_default().push(nodes.as_slice());
                }
            }
            let merge = |(address, slices)| {
                let nodes: Vec<_> = kway_merge_disjoint_sorted_by(
                    slices,
                    mask.iter().filter_map(|i| i.storage_tries.get(&address).map(Vec::as_slice)),
                    equal_mask_value,
                )
                .collect();
                (!nodes.is_empty()).then_some((address, nodes))
            };
            #[cfg(feature = "rayon")]
            {
                use rayon::iter::{IntoParallelIterator, ParallelIterator};
                storage
                    .into_iter()
                    .collect::<Vec<_>>()
                    .into_par_iter()
                    .filter_map(merge)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .collect()
            }
            #[cfg(not(feature = "rayon"))]
            storage.into_iter().filter_map(merge).collect()
        };
        #[cfg(feature = "rayon")]
        let (account_nodes, storage_tries) = rayon::join(accounts, storages);
        #[cfg(not(feature = "rayon"))]
        let (account_nodes, storage_tries) = (accounts(), storages());
        Self { account_nodes, storage_tries }
    }
}

impl AsRef<Self> for StateTrieUpdatesSorted {
    fn as_ref(&self) -> &Self {
        self
    }
}

fn node_range<V>(
    nodes: &StateTrieNodes<V>,
    lower: Nibbles,
    upper: Option<Nibbles>,
) -> &[(Nibbles, Option<StateTrieNode<V>>)] {
    let start = nodes.partition_point(|(path, _)| *path < lower);
    let end = upper.map_or(nodes.len(), |upper| nodes.partition_point(|(path, _)| *path < upper));
    &nodes[start..end]
}

fn equal_mask_value<V: PartialEq>(
    left: &Option<StateTrieNode<V>>,
    right: &Option<StateTrieNode<V>>,
) -> bool {
    match (left, right) {
        // Moving a leaf changes its short key without a BundleState update. Its value must
        // reach the durable frontier because execution does not consult the trie overlay.
        (
            Some(StateTrieNode::Leaf { value: left, .. }),
            Some(StateTrieNode::Leaf { value: right, .. }),
        ) => left == right,
        _ => left == right,
    }
}

#[cfg(any(test, feature = "reth-codec"))]
mod codec {
    use super::*;
    use alloy_rlp::Decodable;
    use reth_codecs::{Compact, Compress, Decompress, DecompressError};

    impl<V: Encodable + core::fmt::Debug + Send + Sync> Compress for StateTrieNode<V> {
        type Compressed = Vec<u8>;
        fn compress_to_buf<B: bytes::BufMut + AsMut<[u8]>>(&self, buf: &mut B) {
            match self {
                Self::Leaf { short_key_len, value } => {
                    buf.put_u8(0);
                    buf.put_u8(*short_key_len);
                    value.encode(buf);
                }
                Self::Branch { short_key_len, state_mask, children } => {
                    assert_eq!(state_mask.count_bits() as usize, children.len());
                    buf.put_u8(1);
                    buf.put_u8(*short_key_len);
                    buf.put_u16(state_mask.get());
                    for child in children {
                        buf.put_u8(child.len() as u8);
                        buf.put_slice(child);
                    }
                }
            }
        }
    }

    impl<V: Decodable + core::fmt::Debug + Send + Sync> Decompress for StateTrieNode<V> {
        fn decompress(mut buf: &[u8]) -> Result<Self, DecompressError> {
            let decode = |buf: &mut &[u8]| -> Result<Self, alloy_rlp::Error> {
                if buf.len() < 2 {
                    return Err(alloy_rlp::Error::InputTooShort)
                }
                let tag = buf[0];
                let short_key_len = buf[1];
                *buf = &buf[2..];
                if short_key_len > 64 {
                    return Err(alloy_rlp::Error::Custom("invalid short key length"))
                }
                let node = match tag {
                    0 => Self::Leaf { short_key_len, value: V::decode(buf)? },
                    1 => {
                        if buf.len() < 2 {
                            return Err(alloy_rlp::Error::InputTooShort)
                        }
                        let state_mask = TrieMask::new(u16::from_be_bytes([buf[0], buf[1]]));
                        *buf = &buf[2..];
                        if state_mask.count_bits() < 2 || short_key_len == 64 {
                            return Err(alloy_rlp::Error::Custom("invalid branch"))
                        }
                        let mut children = Vec::with_capacity(state_mask.count_bits() as usize);
                        for _ in state_mask.iter() {
                            let len = *buf.first().ok_or(alloy_rlp::Error::InputTooShort)? as usize;
                            *buf = &buf[1..];
                            if len == 0 || len > 33 || buf.len() < len {
                                return Err(alloy_rlp::Error::InputTooShort)
                            }
                            children.push(
                                RlpNode::from_raw(&buf[..len])
                                    .ok_or(alloy_rlp::Error::Custom("invalid child RLP"))?,
                            );
                            *buf = &buf[len..];
                        }
                        Self::Branch { short_key_len, state_mask, children }
                    }
                    _ => return Err(alloy_rlp::Error::Custom("invalid state trie tag")),
                };
                if !buf.is_empty() {
                    return Err(alloy_rlp::Error::Custom("trailing state trie bytes"))
                }
                Ok(node)
            };
            decode(&mut buf).map_err(DecompressError::new)
        }
    }

    impl Compress for StateTrieStorageEntry {
        type Compressed = Vec<u8>;
        fn compress_to_buf<B: bytes::BufMut + AsMut<[u8]>>(&self, buf: &mut B) {
            self.nibbles.to_compact(buf);
            self.node.compress_to_buf(buf);
        }
    }
    impl Decompress for StateTrieStorageEntry {
        fn decompress(buf: &[u8]) -> Result<Self, DecompressError> {
            if buf.len() < 33 || buf[32] > 64 {
                return Err(DecompressError::new(alloy_rlp::Error::InputTooShort))
            }
            let (nibbles, buf) = PackedStoredNibblesSubKey::from_compact(buf, 33);
            Ok(Self { nibbles, node: StateTrieNode::decompress(buf)? })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;
    use reth_codecs::{Compress, Decompress};

    #[cfg(feature = "rayon")]
    #[test]
    fn partitioned_account_merge_matches_sequential() {
        let groups: Vec<_> = (0u8..12)
            .map(|epoch| {
                let nodes: alloc::collections::BTreeMap<_, _> = (0u64..1024)
                    .filter_map(|seed| {
                        if epoch >= 6 && seed % 4 == 0 {
                            return None
                        }
                        let path = Nibbles::unpack(alloy_primitives::keccak256(seed.to_le_bytes()))
                            .slice(..(seed as usize % 65));
                        let node = if (seed + u64::from(epoch)) % 7 == 0 {
                            None
                        } else if path.len() == 64 {
                            Some(StateTrieNode::Leaf {
                                short_key_len: epoch,
                                value: TrieAccount {
                                    balance: U256::from(epoch % 3),
                                    ..Default::default()
                                },
                            })
                        } else {
                            Some(StateTrieNode::Branch {
                                short_key_len: (usize::from(epoch) % (path.len() + 1)) as u8,
                                state_mask: TrieMask::new(3),
                                children: vec![
                                    RlpNode::word_rlp(
                                        &alloy_primitives::B256::repeat_byte(epoch % 5)
                                    );
                                    2
                                ],
                            })
                        };
                        Some((path, node))
                    })
                    .collect();
                StateTrieUpdatesSorted {
                    account_nodes: nodes.into_iter().collect(),
                    storage_tries: Default::default(),
                }
            })
            .collect();
        let batch: Vec<_> = groups[..6].iter().collect();
        let mask: Vec<_> = groups[6..].iter().collect();
        assert!(batch.iter().map(|i| i.account_nodes.len()).sum::<usize>() >= 4096);
        let expected: Vec<_> = kway_merge_disjoint_sorted_by(
            batch.iter().rev().map(|i| i.account_nodes.as_slice()),
            mask.iter().map(|i| i.account_nodes.as_slice()),
            equal_mask_value,
        )
        .collect();
        let actual = StateTrieUpdatesSorted::disjointed_merge_batch(&batch, &mask);
        assert_eq!(actual.account_nodes, expected);
        assert!(actual.storage_tries.is_empty());
    }

    #[test]
    fn codec_roundtrip_and_rejects_malformed() {
        let nodes = [
            StateTrieNode::Leaf { short_key_len: 64, value: U256::MAX },
            StateTrieNode::Branch {
                short_key_len: 0,
                state_mask: TrieMask::new(u16::MAX),
                children: (0..16).map(|i| RlpNode::word_rlp(&B256::repeat_byte(i))).collect(),
            },
            StateTrieNode::Branch {
                short_key_len: 63,
                state_mask: TrieMask::new(3),
                children: vec![RlpNode::from_raw(&[0xc0]).unwrap(); 2],
            },
        ];
        for node in nodes {
            let bytes = node.clone().compress();
            assert_eq!(StateTrieNode::<U256>::decompress(&bytes).unwrap(), node);
            for len in 0..bytes.len() {
                assert!(StateTrieNode::<U256>::decompress(&bytes[..len]).is_err());
            }
            let mut extra = bytes.clone();
            extra.push(0);
            assert!(StateTrieNode::<U256>::decompress(&extra).is_err());
            let mut invalid = bytes;
            invalid[0] = 2;
            assert!(StateTrieNode::<U256>::decompress(&invalid).is_err());
        }
        let account = StateTrieNode::Leaf { short_key_len: 0, value: TrieAccount::default() };
        assert_eq!(
            StateTrieNode::<TrieAccount>::decompress(&account.clone().compress()).unwrap(),
            account
        );
    }

    #[test]
    fn merging_and_masking_preserve_deletions_and_equal_values() {
        let path = Nibbles::unpack(B256::ZERO);
        let address = B256::ZERO;
        let old = StateTrieUpdatesSorted {
            storage_tries: std::iter::once((
                address,
                vec![(path, Some(StateTrieNode::Leaf { short_key_len: 64, value: U256::from(1) }))],
            ))
            .collect(),
            ..Default::default()
        };
        let deleted = StateTrieUpdatesSorted {
            storage_tries: std::iter::once((address, vec![(path, None)])).collect(),
            ..Default::default()
        };
        assert_eq!(StateTrieUpdatesSorted::merge_slice(&[&deleted, &old]), deleted);
        assert!(StateTrieUpdatesSorted::disjointed_merge_batch(&[&old], &[&deleted]).is_empty());
        assert_eq!(
            StateTrieUpdatesSorted::disjointed_merge_batch(&[&old, &deleted], &[&old, &deleted]),
            deleted
        );
        let mut moved = old.clone();
        let Some(StateTrieNode::Leaf { short_key_len, .. }) =
            &mut moved.storage_tries.get_mut(&address).unwrap()[0].1
        else {
            unreachable!()
        };
        *short_key_len = 62;
        assert_eq!(StateTrieUpdatesSorted::disjointed_merge_batch(&[&old], &[&moved]), old);
    }
}
