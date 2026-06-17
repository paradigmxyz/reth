use alloy_primitives::{Address, B256, Bytes};
use serde::{Deserialize, Serialize};
use crate::witness::WitnessResult;
use reth_trie_common::{MultiProof, StorageMultiProof, BranchNodeMasks, Nibbles, TrieMask};
use reth_trie_common::proof::ProofNodes;
use alloy_primitives::map::{B256Map, HashMap};

/// Hashed / Raw target keys that were missed in the network state cache for a block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WitnessTargets {
    /// List of missed account addresses.
    pub missed_accounts: Vec<Address>,
    /// List of missed storage slots.
    pub missed_storage: Vec<(Address, B256)>,
    /// List of missed contract code hashes.
    pub missed_code_hashes: Vec<B256>,
}

/// A serialized representation of a `StorageMultiProof` that can be easily serialized/deserialized with `serde`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SerializableStorageMultiProof {
    pub root: B256,
    pub subtree: Vec<(Vec<u8>, Vec<u8>)>,
    pub branch_node_masks: Vec<(Vec<u8>, u16, u16)>,
}

/// A serialized representation of a `MultiProof` that can be easily serialized/deserialized with `serde`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SerializableMultiProof {
    pub account_subtree: Vec<(Vec<u8>, Vec<u8>)>,
    pub branch_node_masks: Vec<(Vec<u8>, u16, u16)>,
    pub storages: Vec<(B256, SerializableStorageMultiProof)>,
}

impl SerializableMultiProof {
    /// Convert `MultiProof` into `SerializableMultiProof`.
    pub fn from_multiproof(proof: &MultiProof) -> Self {
        let mut account_subtree = Vec::new();
        for (nibbles, node_bytes) in proof.account_subtree.iter() {
            account_subtree.push((nibbles.to_vec(), node_bytes.to_vec()));
        }

        let mut branch_node_masks = Vec::new();
        for (nibbles, masks) in &proof.branch_node_masks {
            branch_node_masks.push((nibbles.to_vec(), masks.hash_mask.get(), masks.tree_mask.get()));
        }

        let mut storages = Vec::new();
        for (hashed_address, storage_proof) in &proof.storages {
            let mut subtree = Vec::new();
            for (nibbles, node_bytes) in storage_proof.subtree.iter() {
                subtree.push((nibbles.to_vec(), node_bytes.to_vec()));
            }

            let mut storage_masks = Vec::new();
            for (nibbles, masks) in &storage_proof.branch_node_masks {
                storage_masks.push((nibbles.to_vec(), masks.hash_mask.get(), masks.tree_mask.get()));
            }

            storages.push((
                *hashed_address,
                SerializableStorageMultiProof {
                    root: storage_proof.root,
                    subtree,
                    branch_node_masks: storage_masks,
                },
            ));
        }

        Self { account_subtree, branch_node_masks, storages }
    }

    /// Convert `SerializableMultiProof` back into `MultiProof`.
    pub fn to_multiproof(&self) -> MultiProof {
        let mut account_subtree_map: HashMap<Nibbles, Bytes> = HashMap::default();
        for (nibbles_bytes, node_bytes) in &self.account_subtree {
            account_subtree_map.insert(Nibbles::from_nibbles(nibbles_bytes.clone()), Bytes::from(node_bytes.clone()));
        }
        let account_subtree = ProofNodes::from_iter(account_subtree_map);

        let mut branch_node_masks: HashMap<Nibbles, BranchNodeMasks> = HashMap::default();
        for (nibbles_bytes, hash_mask, tree_mask) in &self.branch_node_masks {
            branch_node_masks.insert(
                Nibbles::from_nibbles(nibbles_bytes.clone()),
                BranchNodeMasks {
                    hash_mask: TrieMask::new(*hash_mask),
                    tree_mask: TrieMask::new(*tree_mask),
                },
            );
        }

        let mut storages = B256Map::default();
        for (hashed_address, storage_proof) in &self.storages {
            let mut subtree_map: HashMap<Nibbles, Bytes> = HashMap::default();
            for (nibbles_bytes, node_bytes) in &storage_proof.subtree {
                subtree_map.insert(Nibbles::from_nibbles(nibbles_bytes.clone()), Bytes::from(node_bytes.clone()));
            }
            let subtree = ProofNodes::from_iter(subtree_map);

            let mut storage_masks: HashMap<Nibbles, BranchNodeMasks> = HashMap::default();
            for (nibbles_bytes, hash_mask, tree_mask) in &storage_proof.branch_node_masks {
                storage_masks.insert(
                    Nibbles::from_nibbles(nibbles_bytes.clone()),
                    BranchNodeMasks {
                        hash_mask: TrieMask::new(*hash_mask),
                        tree_mask: TrieMask::new(*tree_mask),
                    },
                );
            }

            storages.insert(
                *hashed_address,
                StorageMultiProof {
                    root: storage_proof.root,
                    subtree,
                    branch_node_masks: storage_masks,
                },
            );
        }

        MultiProof { account_subtree, branch_node_masks, storages }
    }
}

/// The Builder-Side Witness Sidecar for a block in the Partial Stateless prototype.
///
/// Contains all the necessary data (Merkle proofs, bytecodes) for a stateless validator
/// to execute a block given only the parent state root and network state cache.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartialStatelessSidecar {
    /// Hash of the parent block header.
    pub parent_hash: B256,
    /// State root of the parent block header (what the Merkle proof is generated against).
    pub parent_state_root: B256,
    /// Hash of the current block header.
    pub block_hash: B256,
    /// Number of the current block.
    pub block_number: u64,
    /// Block number that the network cache is synchronized with (should be block_number - 1).
    pub cache_block: u64,
    /// Metadata describing the cache eviction policy (e.g. "LastNBlocks(60, 30)").
    pub cache_policy_metadata: String,
    /// The targets that were missed in the cache.
    pub raw_targets: WitnessTargets,
    /// The serialized `MultiProof` (containing account subtree and storage subtrees).
    pub serialized_multiproof: Vec<u8>,
    /// Missed contract bytecodes in bytes.
    pub missed_bytecodes: Vec<Bytes>,
    /// Summary stats about the witness.
    pub stats: WitnessResult,
}
