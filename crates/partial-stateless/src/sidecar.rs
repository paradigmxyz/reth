use crate::witness::WitnessResult;
use alloy_primitives::map::{B256Map, HashMap};
use alloy_primitives::{Address, B256, Bytes};
use reth_trie_common::proof::ProofNodes;
use reth_trie_common::{BranchNodeMasks, MultiProof, Nibbles, StorageMultiProof, TrieMask};
use serde::{Deserialize, Serialize};

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

impl WitnessTargets {
    pub fn key_preimages(&self) -> Vec<Bytes> {
        let mut addresses = Vec::with_capacity(self.missed_accounts.len() + self.missed_storage.len());
        addresses.extend_from_slice(&self.missed_accounts);
        addresses.extend(self.missed_storage.iter().map(|(address, _)| *address));
        addresses.sort();
        addresses.dedup();

        let mut slots: Vec<B256> = self.missed_storage.iter().map(|(_, slot)| *slot).collect();
        slots.sort();
        slots.dedup();

        let mut keys = Vec::with_capacity(addresses.len() + slots.len());
        keys.extend(addresses.into_iter().map(|address| address.to_vec().into()));
        keys.extend(slots.into_iter().map(Bytes::from));
        keys
    }
}

/// Generic target set used by the benchmark manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateTargetSet {
    pub accounts: Vec<Address>,
    pub storage: Vec<(Address, B256)>,
    pub code_hashes: Vec<B256>,
}

impl StateTargetSet {
    pub fn sort_dedup(&mut self) {
        self.accounts.sort();
        self.accounts.dedup();
        self.storage.sort();
        self.storage.dedup();
        self.code_hashes.sort();
        self.code_hashes.dedup();
    }
}

impl From<&WitnessTargets> for StateTargetSet {
    fn from(value: &WitnessTargets) -> Self {
        let mut set = Self {
            accounts: value.missed_accounts.clone(),
            storage: value.missed_storage.clone(),
            code_hashes: value.missed_code_hashes.clone(),
        };
        set.sort_dedup();
        set
    }
}

/// Exact partition check for `accessed == cache_hit disjoint_union sidecar_miss`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartitionCheck {
    pub accounts_ok: bool,
    pub storage_ok: bool,
    pub code_hashes_ok: bool,
    pub accounts_disjoint: bool,
    pub storage_disjoint: bool,
    pub code_hashes_disjoint: bool,
    pub partition_ok: bool,
}

impl PartitionCheck {
    pub fn new(
        accessed: &StateTargetSet,
        cache_hit: &StateTargetSet,
        sidecar_miss: &StateTargetSet,
    ) -> Self {
        fn check<T: Ord + Clone>(accessed: &[T], hit: &[T], miss: &[T]) -> (bool, bool) {
            let mut union = Vec::with_capacity(hit.len() + miss.len());
            union.extend_from_slice(hit);
            union.extend_from_slice(miss);
            union.sort();
            union.dedup();

            let mut expected = accessed.to_vec();
            expected.sort();
            expected.dedup();

            let mut h = hit.to_vec();
            h.sort();
            h.dedup();
            let mut m = miss.to_vec();
            m.sort();
            m.dedup();

            let disjoint = h.iter().all(|item| m.binary_search(item).is_err());
            (union == expected, disjoint)
        }

        let (accounts_ok, accounts_disjoint) =
            check(&accessed.accounts, &cache_hit.accounts, &sidecar_miss.accounts);
        let (storage_ok, storage_disjoint) =
            check(&accessed.storage, &cache_hit.storage, &sidecar_miss.storage);
        let (code_hashes_ok, code_hashes_disjoint) =
            check(&accessed.code_hashes, &cache_hit.code_hashes, &sidecar_miss.code_hashes);
        let partition_ok = accounts_ok
            && storage_ok
            && code_hashes_ok
            && accounts_disjoint
            && storage_disjoint
            && code_hashes_disjoint;

        Self {
            accounts_ok,
            storage_ok,
            code_hashes_ok,
            accounts_disjoint,
            storage_disjoint,
            code_hashes_disjoint,
            partition_ok,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CacheFootprintStats {
    pub accounts: usize,
    pub storage_slots: usize,
    pub codes: usize,
    pub estimated_memory_bytes: usize,
}

impl CacheFootprintStats {
    pub fn new(
        accounts: usize,
        storage_slots: usize,
        codes: usize,
        estimated_memory_bytes: usize,
    ) -> Self {
        Self { accounts, storage_slots, codes, estimated_memory_bytes }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WitnessReductionStats {
    pub total_reduction_ratio: Option<f64>,
    pub mpt_reduction_ratio: Option<f64>,
    pub account_proof_reduction_ratio: Option<f64>,
    pub storage_proof_reduction_ratio: Option<f64>,
    pub bytecode_reduction_ratio: Option<f64>,
}

impl WitnessReductionStats {
    pub fn new(partial: &WitnessResult, full: &WitnessResult) -> Self {
        fn reduction(partial: usize, full: usize) -> Option<f64> {
            if full == 0 {
                None
            } else {
                Some(1.0 - partial as f64 / full as f64)
            }
        }

        let partial_mpt = partial.account_proof_bytes + partial.storage_proof_bytes;
        let full_mpt = full.account_proof_bytes + full.storage_proof_bytes;

        Self {
            total_reduction_ratio: reduction(partial.total_size_bytes, full.total_size_bytes),
            mpt_reduction_ratio: reduction(partial_mpt, full_mpt),
            account_proof_reduction_ratio: reduction(
                partial.account_proof_bytes,
                full.account_proof_bytes,
            ),
            storage_proof_reduction_ratio: reduction(
                partial.storage_proof_bytes,
                full.storage_proof_bytes,
            ),
            bytecode_reduction_ratio: reduction(partial.bytecode_bytes, full.bytecode_bytes),
        }
    }
}

/// Benchmark-only metadata written next to the lean sidecar.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SidecarBenchmarkManifest {
    pub schema_version: u64,
    pub block_number: u64,
    pub block_hash: B256,
    pub parent_hash: B256,
    pub parent_state_root: B256,
    pub cache_block: u64,
    pub cache_policy_metadata: String,
    pub sidecar_file: String,
    pub sidecar_bytes: usize,
    pub cache_before: CacheFootprintStats,
    pub cache_after: CacheFootprintStats,
    pub accessed: StateTargetSet,
    pub cache_hit: StateTargetSet,
    pub sidecar_miss: StateTargetSet,
    pub partition: PartitionCheck,
    /// Full-witness baseline (all accessed state, ignoring the cache). `None` when
    /// the comparison is disabled (`PS_WITNESS_BASELINE` unset).
    pub full_sidecar_baseline_stats: Option<WitnessResult>,
    pub partial_sidecar_stats: WitnessResult,
    /// Reduction of the partial sidecar vs the full baseline. `None` when the
    /// baseline comparison is disabled.
    pub reduction: Option<WitnessReductionStats>,
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

/// State component of a partial execution witness.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PartialExecutionWitnessState {
    /// Reth MPT multiproof encoded in the local serializable representation.
    MptMultiProof(Vec<u8>),
}

impl PartialExecutionWitnessState {
    pub fn mpt_multiproof_bytes(&self) -> &[u8] {
        match self {
            Self::MptMultiProof(bytes) => bytes,
        }
    }
}

/// ExecutionWitness-shaped material carried by the sidecar.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PartialExecutionWitness {
    pub state: PartialExecutionWitnessState,
    pub codes: Vec<Bytes>,
    pub keys: Vec<Bytes>,
    pub headers: Vec<Bytes>,
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
    /// Cache-missed targets covered by `witness`.
    pub miss_manifest: WitnessTargets,
    /// Execution material split like Reth `ExecutionWitness`: state, codes, keys, headers.
    pub witness: PartialExecutionWitness,
    /// Summary stats about the witness.
    pub stats: WitnessResult,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partition_check_accepts_exact_disjoint_cover() {
        let account_a = Address::repeat_byte(0x01);
        let account_b = Address::repeat_byte(0x02);
        let slot_a = B256::repeat_byte(0x0a);
        let slot_b = B256::repeat_byte(0x0b);
        let code_a = B256::repeat_byte(0xca);
        let code_b = B256::repeat_byte(0xcb);

        let accessed = StateTargetSet {
            accounts: vec![account_a, account_b],
            storage: vec![(account_a, slot_a), (account_b, slot_b)],
            code_hashes: vec![code_a, code_b],
        };
        let cache_hit = StateTargetSet {
            accounts: vec![account_a],
            storage: vec![(account_a, slot_a)],
            code_hashes: vec![code_a],
        };
        let sidecar_miss = StateTargetSet {
            accounts: vec![account_b],
            storage: vec![(account_b, slot_b)],
            code_hashes: vec![code_b],
        };

        let check = PartitionCheck::new(&accessed, &cache_hit, &sidecar_miss);
        assert!(check.partition_ok);
    }

    #[test]
    fn partition_check_rejects_overlap() {
        let account = Address::repeat_byte(0x01);
        let code_hash = B256::repeat_byte(0xca);
        let accessed = StateTargetSet {
            accounts: vec![account],
            storage: vec![],
            code_hashes: vec![code_hash],
        };
        let cache_hit = accessed.clone();
        let sidecar_miss = accessed;

        let check = PartitionCheck::new(&cache_hit, &cache_hit, &sidecar_miss);
        assert!(!check.accounts_disjoint);
        assert!(!check.code_hashes_disjoint);
        assert!(!check.partition_ok);
    }

    #[test]
    fn reduction_stats_compare_partial_to_full() {
        let partial = WitnessResult {
            total_size_bytes: 40,
            account_proof_bytes: 10,
            storage_proof_bytes: 20,
            bytecode_bytes: 10,
            account_proof_nodes: 1,
            storage_proof_nodes: 2,
            target_accounts: 1,
            target_storage_slots: 2,
            computation_time_ms: None,
        };
        let full = WitnessResult {
            total_size_bytes: 100,
            account_proof_bytes: 20,
            storage_proof_bytes: 40,
            bytecode_bytes: 40,
            account_proof_nodes: 2,
            storage_proof_nodes: 4,
            target_accounts: 2,
            target_storage_slots: 4,
            computation_time_ms: None,
        };

        let reduction = WitnessReductionStats::new(&partial, &full);
        assert_eq!(reduction.total_reduction_ratio, Some(0.6));
        assert_eq!(reduction.mpt_reduction_ratio, Some(0.5));
        assert_eq!(reduction.bytecode_reduction_ratio, Some(0.75));
    }
}
