use crate::witness::WitnessResult;
use alloy_primitives::map::{B256Map, HashMap};
use alloy_primitives::{keccak256, Address, Bytes, B256};
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
        let mut addresses =
            Vec::with_capacity(self.missed_accounts.len() + self.missed_storage.len());
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

/// Generic state target set used by sidecar claims and diagnostics.
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

fn encode_state_targets(out: &mut Vec<u8>, label: &[u8], targets: &StateTargetSet) {
    let mut targets = targets.clone();
    targets.sort_dedup();

    out.extend_from_slice(label);

    out.extend_from_slice(b"accounts");
    out.extend_from_slice(&(targets.accounts.len() as u64).to_be_bytes());
    for address in targets.accounts {
        out.extend_from_slice(address.as_slice());
    }

    out.extend_from_slice(b"storage");
    out.extend_from_slice(&(targets.storage.len() as u64).to_be_bytes());
    for (address, slot) in targets.storage {
        out.extend_from_slice(address.as_slice());
        out.extend_from_slice(slot.as_slice());
    }

    out.extend_from_slice(b"code_hashes");
    out.extend_from_slice(&(targets.code_hashes.len() as u64).to_be_bytes());
    for code_hash in targets.code_hashes {
        out.extend_from_slice(code_hash.as_slice());
    }
}

/// Fork- and policy-scoped commitment to a concrete cache state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheAnchor {
    pub block_number: u64,
    pub block_hash: B256,
    pub cache_policy_id: B256,
    pub cache_root: B256,
}

/// Failure reasons for the minimal sidecar fingerprint checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidecarCheckError {
    SidecarContextMismatch { field: &'static str, expected: String, actual: String },
    PrevCacheAnchorMismatch { expected: CacheAnchor, actual: CacheAnchor },
    NextCacheAnchorMismatch { expected: CacheAnchor, actual: CacheAnchor },
    MissManifestMismatch { cache_miss_targets: StateTargetSet, miss_manifest: StateTargetSet },
    WitnessCommitmentMismatch { expected: B256, actual: B256 },
    MissTargetsMismatch { expected: StateTargetSet, actual: StateTargetSet },
}

fn context_mismatch(
    field: &'static str,
    expected: impl ToString,
    actual: impl ToString,
) -> SidecarCheckError {
    SidecarCheckError::SidecarContextMismatch {
        field,
        expected: expected.to_string(),
        actual: actual.to_string(),
    }
}

fn check_anchor_context(sidecar: &PartialStatelessSidecar) -> Result<(), SidecarCheckError> {
    if sidecar.prev_cache_anchor.block_hash != sidecar.parent_hash {
        return Err(context_mismatch(
            "prev_cache_anchor.block_hash",
            sidecar.parent_hash,
            sidecar.prev_cache_anchor.block_hash,
        ));
    }
    if sidecar.prev_cache_anchor.block_number != sidecar.cache_block {
        return Err(context_mismatch(
            "prev_cache_anchor.block_number",
            sidecar.cache_block,
            sidecar.prev_cache_anchor.block_number,
        ));
    }
    if sidecar.next_cache_anchor.block_hash != sidecar.block_hash {
        return Err(context_mismatch(
            "next_cache_anchor.block_hash",
            sidecar.block_hash,
            sidecar.next_cache_anchor.block_hash,
        ));
    }
    if sidecar.next_cache_anchor.block_number != sidecar.block_number {
        return Err(context_mismatch(
            "next_cache_anchor.block_number",
            sidecar.block_number,
            sidecar.next_cache_anchor.block_number,
        ));
    }
    if sidecar.prev_cache_anchor.cache_policy_id != sidecar.cache_policy_id {
        return Err(context_mismatch(
            "prev_cache_anchor.cache_policy_id",
            sidecar.cache_policy_id,
            sidecar.prev_cache_anchor.cache_policy_id,
        ));
    }
    if sidecar.next_cache_anchor.cache_policy_id != sidecar.cache_policy_id {
        return Err(context_mismatch(
            "next_cache_anchor.cache_policy_id",
            sidecar.cache_policy_id,
            sidecar.next_cache_anchor.cache_policy_id,
        ));
    }

    Ok(())
}

/// Check that the sidecar was built from the same previous cache context.
pub fn check_sidecar_context(
    sidecar: &PartialStatelessSidecar,
    local_prev_anchor: &CacheAnchor,
) -> Result<(), SidecarCheckError> {
    check_anchor_context(sidecar)?;

    if sidecar.prev_cache_anchor != *local_prev_anchor {
        return Err(SidecarCheckError::PrevCacheAnchorMismatch {
            expected: *local_prev_anchor,
            actual: sidecar.prev_cache_anchor,
        });
    }

    Ok(())
}

/// Check sidecar-internal miss-only and witness-payload fingerprints.
pub fn check_sidecar_self_consistency(
    sidecar: &PartialStatelessSidecar,
) -> Result<StateTargetSet, SidecarCheckError> {
    check_anchor_context(sidecar)?;

    let mut declared_miss = sidecar.cache_miss_targets.clone();
    declared_miss.sort_dedup();
    let manifest_miss = StateTargetSet::from(&sidecar.miss_manifest);
    if declared_miss != manifest_miss {
        return Err(SidecarCheckError::MissManifestMismatch {
            cache_miss_targets: declared_miss,
            miss_manifest: manifest_miss,
        });
    }

    let expected_witness_commitment =
        partial_witness_commitment(sidecar.parent_state_root, &declared_miss, &sidecar.witness);
    if sidecar.witness_commitment != expected_witness_commitment {
        return Err(SidecarCheckError::WitnessCommitmentMismatch {
            expected: expected_witness_commitment,
            actual: sidecar.witness_commitment,
        });
    }

    Ok(declared_miss)
}

/// Check that the sidecar only carries targets that were actually cache misses.
pub fn check_sidecar_miss_targets(
    sidecar: &PartialStatelessSidecar,
    expected_miss: &StateTargetSet,
) -> Result<(), SidecarCheckError> {
    let declared_miss = check_sidecar_self_consistency(sidecar)?;
    let mut expected_miss = expected_miss.clone();
    expected_miss.sort_dedup();
    if declared_miss != expected_miss {
        return Err(SidecarCheckError::MissTargetsMismatch {
            expected: expected_miss,
            actual: declared_miss,
        });
    }

    Ok(())
}

/// Check the cache state reached after a successful block execution.
pub fn check_next_cache_anchor(
    sidecar: &PartialStatelessSidecar,
    local_next_anchor: &CacheAnchor,
) -> Result<(), SidecarCheckError> {
    check_anchor_context(sidecar)?;
    if sidecar.next_cache_anchor != *local_next_anchor {
        return Err(SidecarCheckError::NextCacheAnchorMismatch {
            expected: *local_next_anchor,
            actual: sidecar.next_cache_anchor,
        });
    }

    Ok(())
}

/// Deterministic identifier for the prototype's split LastNBlocks cache policy.
pub fn last_n_blocks_cache_policy_id(account_window: u64, storage_code_window: u64) -> B256 {
    let mut preimage = Vec::new();
    preimage.extend_from_slice(b"LastNBlocksPolicy/v1");
    preimage.extend_from_slice(b"account_window");
    preimage.extend_from_slice(&account_window.to_be_bytes());
    preimage.extend_from_slice(b"storage_code_window");
    preimage.extend_from_slice(&storage_code_window.to_be_bytes());
    keccak256(preimage)
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StateTargetStats {
    pub accounts: usize,
    pub storage_slots: usize,
    pub code_hashes: usize,
}

impl StateTargetStats {
    pub fn from_targets(targets: &StateTargetSet) -> Self {
        Self {
            accounts: targets.accounts.len(),
            storage_slots: targets.storage.len(),
            code_hashes: targets.code_hashes.len(),
        }
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
    pub cache_policy_id: B256,
    pub prev_cache_anchor: CacheAnchor,
    pub next_cache_anchor: CacheAnchor,
    pub cache_policy_metadata: String,
    pub sidecar_file: String,
    pub sidecar_bytes: usize,
    pub cache_before: CacheFootprintStats,
    pub cache_after: CacheFootprintStats,
    pub accessed: StateTargetStats,
    pub cache_hit: StateTargetStats,
    pub sidecar_miss: StateTargetStats,
    /// Whether the ExEx ran the provider-assisted validator preflight for this sidecar.
    pub provider_assisted_preflight: bool,
    /// Diagnostic-only readiness for a future fully trustless state-root proof.
    pub root_witness_completeness: RootWitnessCompletenessSummary,
    /// Full-witness baseline (all accessed state, ignoring the cache). `None` when
    /// the comparison is disabled (`PS_WITNESS_BASELINE` unset).
    pub full_sidecar_baseline_stats: Option<WitnessResult>,
    pub partial_sidecar_stats: WitnessResult,
    /// Reduction of the partial sidecar vs the full baseline. `None` when the
    /// baseline comparison is disabled.
    pub reduction: Option<WitnessReductionStats>,
}

/// Diagnostic report for whether the current sidecar/cache shape contains enough
/// trie paths to compute the post-state root without provider assistance.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RootWitnessCompletenessReport {
    pub trustless_root_ready: bool,
    pub missing_account_paths: Vec<Address>,
    pub missing_storage_paths: Vec<(Address, B256)>,
    pub covered_account_paths: usize,
    pub covered_storage_paths: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RootWitnessCompletenessSummary {
    pub trustless_root_ready: bool,
    pub missing_account_paths: usize,
    pub missing_storage_paths: usize,
    pub covered_account_paths: usize,
    pub covered_storage_paths: usize,
}

impl RootWitnessCompletenessSummary {
    pub fn from_report(report: &RootWitnessCompletenessReport) -> Self {
        Self {
            trustless_root_ready: report.trustless_root_ready,
            missing_account_paths: report.missing_account_paths.len(),
            missing_storage_paths: report.missing_storage_paths.len(),
            covered_account_paths: report.covered_account_paths,
            covered_storage_paths: report.covered_storage_paths,
        }
    }
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
            branch_node_masks.push((
                nibbles.to_vec(),
                masks.hash_mask.get(),
                masks.tree_mask.get(),
            ));
        }

        let mut storages = Vec::new();
        for (hashed_address, storage_proof) in &proof.storages {
            let mut subtree = Vec::new();
            for (nibbles, node_bytes) in storage_proof.subtree.iter() {
                subtree.push((nibbles.to_vec(), node_bytes.to_vec()));
            }

            let mut storage_masks = Vec::new();
            for (nibbles, masks) in &storage_proof.branch_node_masks {
                storage_masks.push((
                    nibbles.to_vec(),
                    masks.hash_mask.get(),
                    masks.tree_mask.get(),
                ));
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
            account_subtree_map.insert(
                Nibbles::from_nibbles(nibbles_bytes.clone()),
                Bytes::from(node_bytes.clone()),
            );
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
                subtree_map.insert(
                    Nibbles::from_nibbles(nibbles_bytes.clone()),
                    Bytes::from(node_bytes.clone()),
                );
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

/// Commit to the witness payload and the cache-miss target set it is supposed to cover.
pub fn partial_witness_commitment(
    parent_state_root: B256,
    cache_miss_targets: &StateTargetSet,
    witness: &PartialExecutionWitness,
) -> B256 {
    fn encode_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
        out.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
        out.extend_from_slice(bytes);
    }

    let mut preimage = Vec::new();
    preimage.extend_from_slice(b"PartialWitnessCommitment/v1");
    preimage.extend_from_slice(b"parent_state_root");
    preimage.extend_from_slice(parent_state_root.as_slice());
    encode_state_targets(&mut preimage, b"cache_miss_targets", cache_miss_targets);

    preimage.extend_from_slice(b"state_mpt_multiproof");
    encode_bytes(&mut preimage, witness.state.mpt_multiproof_bytes());

    let mut codes: Vec<_> =
        witness.codes.iter().map(|code| (keccak256(code.as_ref()), code)).collect();
    codes.sort_by_key(|(code_hash, _)| *code_hash);
    preimage.extend_from_slice(b"code_preimages");
    preimage.extend_from_slice(&(codes.len() as u64).to_be_bytes());
    for (code_hash, code) in codes {
        preimage.extend_from_slice(code_hash.as_slice());
        encode_bytes(&mut preimage, code.as_ref());
    }

    let mut keys = witness.keys.clone();
    keys.sort_by(|a, b| a.as_ref().cmp(b.as_ref()));
    preimage.extend_from_slice(b"key_preimages");
    preimage.extend_from_slice(&(keys.len() as u64).to_be_bytes());
    for key in keys {
        encode_bytes(&mut preimage, key.as_ref());
    }

    let mut headers: Vec<_> =
        witness.headers.iter().map(|header| (keccak256(header.as_ref()), header)).collect();
    headers.sort_by_key(|(header_hash, _)| *header_hash);
    preimage.extend_from_slice(b"ancestor_headers");
    preimage.extend_from_slice(&(headers.len() as u64).to_be_bytes());
    for (header_hash, header) in headers {
        preimage.extend_from_slice(header_hash.as_slice());
        encode_bytes(&mut preimage, header.as_ref());
    }

    keccak256(preimage)
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
    /// Deterministic identifier for the cache policy used to compute the target split.
    pub cache_policy_id: B256,
    /// Cache state the validator must already share before executing this block.
    pub prev_cache_anchor: CacheAnchor,
    /// Cache state the validator must reach after successful block execution.
    pub next_cache_anchor: CacheAnchor,
    /// Metadata describing the cache eviction policy (e.g. "LastNBlocks(60, 30)").
    pub cache_policy_metadata: String,
    /// Canonical cache-missed target set that the sidecar claims to cover.
    pub cache_miss_targets: StateTargetSet,
    /// Commitment binding miss targets to the carried proof/value/header witness payload.
    pub witness_commitment: B256,
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

    fn empty_stats() -> WitnessResult {
        WitnessResult {
            total_size_bytes: 0,
            account_proof_bytes: 0,
            storage_proof_bytes: 0,
            bytecode_bytes: 0,
            account_proof_nodes: 0,
            storage_proof_nodes: 0,
            target_accounts: 0,
            target_storage_slots: 0,
            computation_time_ms: None,
        }
    }

    fn test_anchor(block_number: u64, block_hash: B256, cache_policy_id: B256) -> CacheAnchor {
        CacheAnchor { block_number, block_hash, cache_policy_id, cache_root: keccak256(block_hash) }
    }

    fn test_sidecar(
        parent_hash: B256,
        block_hash: B256,
        cache_policy_id: B256,
        miss_manifest: WitnessTargets,
    ) -> PartialStatelessSidecar {
        let cache_miss_targets = StateTargetSet::from(&miss_manifest);
        let witness = PartialExecutionWitness {
            state: PartialExecutionWitnessState::MptMultiProof(vec![]),
            codes: vec![],
            keys: vec![],
            headers: vec![],
        };
        let witness_commitment =
            partial_witness_commitment(B256::repeat_byte(0x22), &cache_miss_targets, &witness);
        PartialStatelessSidecar {
            parent_hash,
            parent_state_root: B256::repeat_byte(0x22),
            block_hash,
            block_number: 100,
            cache_block: 99,
            cache_policy_id,
            prev_cache_anchor: test_anchor(99, parent_hash, cache_policy_id),
            next_cache_anchor: test_anchor(100, block_hash, cache_policy_id),
            cache_policy_metadata: "LastNBlocks(60, 30)".to_string(),
            cache_miss_targets,
            witness_commitment,
            miss_manifest,
            witness,
            stats: empty_stats(),
        }
    }

    #[test]
    fn sidecar_fingerprint_checks_accept_matching_context_miss_and_next_anchor() {
        let parent_hash = B256::repeat_byte(0xaa);
        let block_hash = B256::repeat_byte(0xbb);
        let cache_policy_id = last_n_blocks_cache_policy_id(60, 30);
        let missed_account = Address::repeat_byte(0x01);
        let miss_manifest = WitnessTargets {
            missed_accounts: vec![missed_account],
            missed_storage: vec![],
            missed_code_hashes: vec![],
        };
        let sidecar = test_sidecar(parent_hash, block_hash, cache_policy_id, miss_manifest.clone());
        let expected_miss = StateTargetSet::from(&miss_manifest);

        check_sidecar_context(&sidecar, &sidecar.prev_cache_anchor)
            .expect("matching previous cache context");
        check_sidecar_miss_targets(&sidecar, &expected_miss).expect("matching miss-only targets");
        check_next_cache_anchor(&sidecar, &sidecar.next_cache_anchor)
            .expect("matching next cache anchor");
    }

    #[test]
    fn sidecar_miss_targets_reject_cache_hit_target() {
        let parent_hash = B256::repeat_byte(0xaa);
        let block_hash = B256::repeat_byte(0xbb);
        let cache_policy_id = last_n_blocks_cache_policy_id(60, 30);
        let sidecar = test_sidecar(
            parent_hash,
            block_hash,
            cache_policy_id,
            WitnessTargets {
                missed_accounts: vec![Address::repeat_byte(0x01)],
                missed_storage: vec![],
                missed_code_hashes: vec![],
            },
        );

        let err = check_sidecar_miss_targets(&sidecar, &StateTargetSet::default())
            .expect_err("cache-hit target must not be included as a miss");

        assert!(matches!(err, SidecarCheckError::MissTargetsMismatch { .. }));
    }

    #[test]
    fn sidecar_self_consistency_rejects_miss_manifest_mismatch() {
        let parent_hash = B256::repeat_byte(0xaa);
        let block_hash = B256::repeat_byte(0xbb);
        let cache_policy_id = last_n_blocks_cache_policy_id(60, 30);
        let mut sidecar = test_sidecar(
            parent_hash,
            block_hash,
            cache_policy_id,
            WitnessTargets {
                missed_accounts: vec![Address::repeat_byte(0x01)],
                missed_storage: vec![],
                missed_code_hashes: vec![],
            },
        );
        sidecar.cache_miss_targets = StateTargetSet::default();

        let err = check_sidecar_self_consistency(&sidecar)
            .expect_err("declared miss targets must match witness manifest");

        assert!(matches!(err, SidecarCheckError::MissManifestMismatch { .. }));
    }

    #[test]
    fn sidecar_self_consistency_rejects_witness_commitment_mismatch() {
        let parent_hash = B256::repeat_byte(0xaa);
        let block_hash = B256::repeat_byte(0xbb);
        let cache_policy_id = last_n_blocks_cache_policy_id(60, 30);
        let miss_manifest = WitnessTargets {
            missed_accounts: vec![Address::repeat_byte(0x01)],
            missed_storage: vec![],
            missed_code_hashes: vec![],
        };
        let mut sidecar =
            test_sidecar(parent_hash, block_hash, cache_policy_id, miss_manifest.clone());
        sidecar.witness_commitment = B256::repeat_byte(0xee);

        let err = check_sidecar_self_consistency(&sidecar)
            .expect_err("witness payload must match witness commitment");

        assert!(matches!(err, SidecarCheckError::WitnessCommitmentMismatch { .. }));
    }

    #[test]
    fn sidecar_context_rejects_prev_anchor_mismatch() {
        let parent_hash = B256::repeat_byte(0xaa);
        let block_hash = B256::repeat_byte(0xbb);
        let cache_policy_id = last_n_blocks_cache_policy_id(60, 30);
        let sidecar = test_sidecar(
            parent_hash,
            block_hash,
            cache_policy_id,
            WitnessTargets {
                missed_accounts: vec![],
                missed_storage: vec![],
                missed_code_hashes: vec![],
            },
        );
        let mut local_prev_anchor = sidecar.prev_cache_anchor;
        local_prev_anchor.cache_root = B256::repeat_byte(0xcc);

        let err = check_sidecar_context(&sidecar, &local_prev_anchor)
            .expect_err("different previous cache root must fail");

        assert!(matches!(err, SidecarCheckError::PrevCacheAnchorMismatch { .. }));
    }

    #[test]
    fn sidecar_context_rejects_internal_anchor_mismatch() {
        let parent_hash = B256::repeat_byte(0xaa);
        let block_hash = B256::repeat_byte(0xbb);
        let cache_policy_id = last_n_blocks_cache_policy_id(60, 30);
        let mut sidecar = test_sidecar(
            parent_hash,
            block_hash,
            cache_policy_id,
            WitnessTargets {
                missed_accounts: vec![],
                missed_storage: vec![],
                missed_code_hashes: vec![],
            },
        );
        sidecar.prev_cache_anchor.block_hash = B256::repeat_byte(0xdd);

        let err = check_sidecar_context(&sidecar, &sidecar.prev_cache_anchor)
            .expect_err("prev anchor must bind to parent hash");

        assert!(matches!(err, SidecarCheckError::SidecarContextMismatch { .. }));
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
            cpu_time_ms: None,
            major_page_faults: None,
            minor_page_faults: None,
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
            cpu_time_ms: None,
            major_page_faults: None,
            minor_page_faults: None,
        };

        let reduction = WitnessReductionStats::new(&partial, &full);
        assert_eq!(reduction.total_reduction_ratio, Some(0.6));
        assert_eq!(reduction.mpt_reduction_ratio, Some(0.5));
        assert_eq!(reduction.bytecode_reduction_ratio, Some(0.75));
    }
}
