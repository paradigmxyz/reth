//! Witness computation for cache-missed state.
//!
//! Converts `MissResult` into `MultiProofTargets` and provides helpers
//! for measuring witness (Merkle proof) size.

use crate::network_cache::MissResult;
use crate::WitnessTargets;
use alloy_primitives::{keccak256, Address};
use reth_trie_common::{MultiProof, MultiProofTargets};

/// Result of witness computation for a single block.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WitnessResult {
    /// Total size of witness in bytes (account trie nodes + storage trie nodes + bytecode bytes).
    pub total_size_bytes: usize,
    /// Size of account proof nodes in bytes.
    pub account_proof_bytes: usize,
    /// Size of storage proof nodes in bytes.
    pub storage_proof_bytes: usize,
    /// Size of missed contract bytecodes in bytes.
    pub bytecode_bytes: usize,
    /// Number of account proof trie nodes.
    pub account_proof_nodes: usize,
    /// Number of storage proof trie nodes (across all accounts).
    pub storage_proof_nodes: usize,
    /// Number of unique accounts in the proof targets.
    pub target_accounts: usize,
    /// Number of unique storage slots in the proof targets.
    pub target_storage_slots: usize,
    /// Time taken to compute the multiproof (if measured).
    pub computation_time_ms: Option<u64>,
}

/// Convert a `MissResult` into `MultiProofTargets` suitable for `StateProofProvider::multiproof()`.
///
/// This hashes addresses and storage slots with keccak256, which is what reth's
/// trie infrastructure expects.
pub fn miss_to_proof_targets(miss: &MissResult) -> MultiProofTargets {
    let mut targets = MultiProofTargets::with_capacity(miss.missed_accounts.len());

    // Add all missed accounts (even those without storage misses)
    for address in &miss.missed_accounts {
        let hashed_address = keccak256(address);
        targets.entry(hashed_address).or_default();
    }

    // Add missed storage slots grouped by account
    for (address, slot) in &miss.missed_storage {
        let hashed_address = keccak256(address);
        let hashed_slot = keccak256(slot);
        targets.entry(hashed_address).or_default().insert(hashed_slot);
    }

    targets
}

/// Measure the total byte size of a `MultiProof`, adding the size of any missed bytecodes.
///
/// This counts the raw bytes of all trie nodes in the proof (account + storage subtrees)
/// and sums them with the provided bytecode bytes.
pub fn measure_multiproof_size(proof: &MultiProof, missed_bytecode_bytes: usize) -> WitnessResult {
    // Account proof size
    let mut account_proof_bytes = 0usize;
    let mut account_proof_nodes = 0usize;
    for node_bytes in proof.account_subtree.values() {
        account_proof_bytes += node_bytes.len();
        account_proof_nodes += 1;
    }

    // Storage proof sizes
    let mut storage_proof_bytes = 0usize;
    let mut storage_proof_nodes = 0usize;
    for storage_mp in proof.storages.values() {
        for node_bytes in storage_mp.subtree.values() {
            storage_proof_bytes += node_bytes.len();
            storage_proof_nodes += 1;
        }
    }

    let total_size_bytes = account_proof_bytes + storage_proof_bytes + missed_bytecode_bytes;

    // Count targets
    let target_accounts = proof.storages.len().max(
        // account_subtree doesn't directly tell us account count,
        // but storages map has one entry per targeted account
        proof.storages.len(),
    );
    let target_storage_slots: usize = proof
        .storages
        .values()
        .map(|s| {
            // Estimate slots from number of leaf nodes in storage proof
            // (this is approximate, actual slot count comes from targets)
            s.subtree.len()
        })
        .sum();

    WitnessResult {
        total_size_bytes,
        account_proof_bytes,
        storage_proof_bytes,
        bytecode_bytes: missed_bytecode_bytes,
        account_proof_nodes,
        storage_proof_nodes,
        target_accounts,
        target_storage_slots,
        computation_time_ms: None,
    }
}

/// Measure witness result from a `MissResult` and targets (before proof computation).
/// This provides target counts without actual proof size.
pub fn witness_targets_summary(miss: &MissResult) -> WitnessTargetsSummary {
    // Group storage misses by account
    let mut storage_by_account: std::collections::HashMap<Address, usize> =
        std::collections::HashMap::new();
    for (address, _slot) in &miss.missed_storage {
        *storage_by_account.entry(*address).or_default() += 1;
    }

    WitnessTargetsSummary {
        missed_accounts: miss.missed_accounts.len(),
        missed_storage_slots: miss.missed_storage.len(),
        missed_codes: miss.missed_codes.len(),
        accounts_with_storage: storage_by_account.len(),
        max_slots_per_account: storage_by_account.values().copied().max().unwrap_or(0),
    }
}

/// Summary of witness targets (before proof computation).
#[derive(Debug, Clone)]
pub struct WitnessTargetsSummary {
    /// Number of accounts that need witness.
    pub missed_accounts: usize,
    /// Number of storage slots that need witness.
    pub missed_storage_slots: usize,
    /// Number of code entries that need witness (codes are not part of trie proof).
    pub missed_codes: usize,
    /// Number of unique accounts that have storage misses.
    pub accounts_with_storage: usize,
    /// Maximum number of missed slots for a single account.
    pub max_slots_per_account: usize,
}

/// Builds raw `WitnessTargets` (for Sidecar data payload) and hashed `MultiProofTargets` (for Trie Provider)
/// in a single pass from `MissResult`.
pub fn build_sidecar_targets(miss: &MissResult) -> (WitnessTargets, MultiProofTargets) {
    let mut multiproof_targets = MultiProofTargets::with_capacity(miss.missed_accounts.len());

    // 1. Convert missed accounts to WitnessTargets & hashed multiproof targets
    let missed_accounts = miss.missed_accounts.clone();
    for address in &missed_accounts {
        let hashed_address = keccak256(address);
        multiproof_targets.entry(hashed_address).or_default();
    }

    // 2. Convert missed storage to WitnessTargets & hashed multiproof targets
    let missed_storage = miss.missed_storage.clone();
    for (address, slot) in &missed_storage {
        let hashed_address = keccak256(address);
        let hashed_slot = keccak256(slot);
        multiproof_targets.entry(hashed_address).or_default().insert(hashed_slot);
    }

    // 3. Convert missed codes to WitnessTargets
    let missed_code_hashes = miss.missed_codes.clone();

    let raw_targets = WitnessTargets {
        missed_accounts,
        missed_storage,
        missed_code_hashes,
    };

    (raw_targets, multiproof_targets)
}

