//! Witness computation for cache-missed state.
//!
//! Converts `MissResult` into `MultiProofTargets` and provides helpers
//! for measuring witness (Merkle proof) size.

use crate::network_cache::MissResult;
use alloy_primitives::{keccak256, Address};
use reth_trie_common::{MultiProof, MultiProofTargets};

/// Result of witness computation for a single block.
#[derive(Debug, Clone)]
pub struct WitnessResult {
    /// Total size of witness in bytes (account trie nodes + storage trie nodes).
    pub total_size_bytes: usize,
    /// Size of account proof nodes in bytes.
    pub account_proof_bytes: usize,
    /// Size of storage proof nodes in bytes.
    pub storage_proof_bytes: usize,
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

/// Measure the total byte size of a `MultiProof`.
///
/// This counts the raw bytes of all trie nodes in the proof (account + storage subtrees).
pub fn measure_multiproof_size(proof: &MultiProof) -> WitnessResult {
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

    let total_size_bytes = account_proof_bytes + storage_proof_bytes;

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
