//! Bootstrap both partial-stateless cache layers from an untrusted peer snapshot.
//!
//! The flat value cache is authenticated by a consensus-trusted [`CacheAnchor`]. Cached account
//! and storage paths are authenticated independently by a state multiproof against the canonical
//! state root at that anchor. Both checks must succeed before either cache is returned.

use crate::{
    network_cache::NetworkStateCache,
    persistence::CacheState,
    policy::{AccountData, CachePolicy},
    sidecar::{CacheAnchor, SerializableMultiProof},
    trie_cache::{PartialTrieNodeCache, TrieCacheValidationError},
    witness_check::{
        multiproof_to_v2_or_request, verify_and_trim_redundant_suffixes, TrieProofTarget,
        TrieTransitionError,
    },
};
use alloy_primitives::{keccak256, Address, B256, U256};
use reth_primitives_traits::Account;
use reth_trie_common::{MultiProof, MultiProofTargets};
use std::collections::{BTreeMap, BTreeSet};

const MAX_PROOF_RETRIES: usize = 128;

/// Resource limits applied before expensive snapshot verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapLimits {
    /// Maximum number of flat account entries.
    pub max_accounts: usize,
    /// Maximum number of flat storage entries.
    pub max_storage_slots: usize,
    /// Maximum number of bytecode entries.
    pub max_codes: usize,
    /// Maximum bincode-serialized state multiproof size.
    pub max_state_proof_bytes: usize,
    /// Maximum aggregate bytecode payload size.
    pub max_code_bytes: usize,
}

impl Default for BootstrapLimits {
    fn default() -> Self {
        Self {
            max_accounts: 100_000,
            max_storage_slots: 300_000,
            max_codes: 20_000,
            max_state_proof_bytes: 64 * 1024 * 1024,
            max_code_bytes: 64 * 1024 * 1024,
        }
    }
}

/// An untrusted flat-cache snapshot plus authenticated paths for every cached state entry.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CacheSnapshotPackage {
    /// Serialized accounts, storage, bytecodes, and current block.
    pub state: CacheState,
    /// Claimed cache context. The receiver compares this with a trusted anchor.
    pub anchor: CacheAnchor,
    /// State multiproof covering all cached accounts and storage slots.
    pub proof: SerializableMultiProof,
}

impl CacheSnapshotPackage {
    /// Assemble a package from a live cache and a provider-generated multiproof.
    pub fn from_cache(cache: &NetworkStateCache, anchor: CacheAnchor, proof: &MultiProof) -> Self {
        Self {
            state: CacheState::from_cache(cache),
            anchor,
            proof: SerializableMultiProof::from_multiproof(proof),
        }
    }
}

/// The two cache layers restored atomically at the same finalized block.
pub struct RestoredBootstrapState {
    /// Authenticated flat values restored with the caller-provided policies.
    pub value_cache: NetworkStateCache,
    /// Authenticated trie paths retained for exactly the restored flat state entries.
    pub trie_cache: PartialTrieNodeCache,
}

/// Produce the complete leaf target set required by a joint cache snapshot.
pub fn bootstrap_proof_targets(cache: &NetworkStateCache) -> MultiProofTargets {
    let mut targets = MultiProofTargets::with_capacity(cache.accounts().len());
    for address in cache.accounts().keys() {
        targets.entry(keccak256(address)).or_default();
    }
    for (address, slot) in cache.storage().keys() {
        targets.entry(keccak256(address)).or_default().insert(keccak256(slot));
    }
    targets
}

/// Build and locally validate a package, expanding structural proof targets when required.
///
/// The callback receives hashed multiproof targets and should query state at `state_root`.
pub fn build_snapshot_package(
    cache: &NetworkStateCache,
    anchor: CacheAnchor,
    state_root: B256,
    mut proof_provider: impl FnMut(MultiProofTargets) -> Result<MultiProof, String>,
) -> Result<CacheSnapshotPackage, BootstrapError> {
    if cache.current_block() != anchor.block_number {
        return Err(BootstrapError::StateBlockMismatch {
            state_block: cache.current_block(),
            anchor_block: anchor.block_number,
        });
    }
    let actual_cache_root = cache.cache_root();
    if actual_cache_root != anchor.cache_root {
        return Err(BootstrapError::CacheRootMismatch {
            expected: anchor.cache_root,
            actual: actual_cache_root,
        });
    }

    let mut targets = bootstrap_proof_targets(cache);
    if targets.is_empty() {
        let pkg = CacheSnapshotPackage::from_cache(cache, anchor, &MultiProof::default());
        check_package_limits(&pkg, &BootstrapLimits::default())?;
        return Ok(pkg)
    }

    let mut retries = 0usize;
    loop {
        let proof = proof_provider(targets.clone()).map_err(BootstrapError::ProofProvider)?;
        verify_cache_values_against_proof(cache, &proof, state_root)?;
        match restore_trie_cache(cache, proof.clone(), state_root) {
            Ok(_) => {
                let pkg = CacheSnapshotPackage::from_cache(cache, anchor, &proof);
                check_package_limits(&pkg, &BootstrapLimits::default())?;
                return Ok(pkg)
            }
            Err(BootstrapError::AdditionalProofTargetsRequired(required)) => {
                let before = targets.chunking_length();
                for target in required {
                    match target {
                        TrieProofTarget::Account(hashed_address) => {
                            targets.entry(hashed_address).or_default();
                        }
                        TrieProofTarget::Storage { hashed_address, hashed_slot } => {
                            targets.entry(hashed_address).or_default().insert(hashed_slot);
                        }
                    }
                }
                retries += 1;
                let after = targets.chunking_length();
                if after == before {
                    return Err(BootstrapError::ProofRetryNoProgress { retries })
                }
                if retries > MAX_PROOF_RETRIES {
                    return Err(BootstrapError::ProofRetryLimit {
                        retries,
                        limit: MAX_PROOF_RETRIES,
                    })
                }
            }
            Err(err) => return Err(err),
        }
    }
}

/// Verify a snapshot with conservative default resource limits.
///
/// `expected_anchor` and `expected_state_root` must be consensus-trusted values for the same
/// canonical block. The supplied policies must match the policy identifiers in the anchor.
pub fn verify_and_restore(
    pkg: CacheSnapshotPackage,
    expected_anchor: &CacheAnchor,
    expected_state_root: B256,
    account_policy: Box<dyn CachePolicy>,
    storage_policy: Box<dyn CachePolicy>,
) -> Result<RestoredBootstrapState, BootstrapError> {
    verify_and_restore_with_limits(
        pkg,
        expected_anchor,
        expected_state_root,
        account_policy,
        storage_policy,
        &BootstrapLimits::default(),
    )
}

/// Verify an untrusted snapshot and atomically restore both coordinated cache layers.
///
/// `expected_anchor` and `expected_state_root` must be consensus-trusted values for the same
/// canonical block. The supplied policies must match the policy identifiers in the anchor.
pub fn verify_and_restore_with_limits(
    pkg: CacheSnapshotPackage,
    expected_anchor: &CacheAnchor,
    expected_state_root: B256,
    account_policy: Box<dyn CachePolicy>,
    storage_policy: Box<dyn CachePolicy>,
    limits: &BootstrapLimits,
) -> Result<RestoredBootstrapState, BootstrapError> {
    check_package_limits(&pkg, limits)?;

    if pkg.anchor != *expected_anchor {
        return Err(BootstrapError::AnchorMismatch {
            expected: Box::new(*expected_anchor),
            actual: Box::new(pkg.anchor),
        });
    }
    if pkg.state.current_block != pkg.anchor.block_number {
        return Err(BootstrapError::StateBlockMismatch {
            state_block: pkg.state.current_block,
            anchor_block: pkg.anchor.block_number,
        });
    }

    let proof = pkg.proof.try_to_multiproof().map_err(BootstrapError::ProofVerification)?;
    let cache = pkg.state.into_cache(account_policy, storage_policy);

    for (code_hash, entry) in cache.codes() {
        let computed = keccak256(&entry.value);
        if computed != *code_hash {
            return Err(BootstrapError::BytecodePreimageMismatch {
                code_hash: *code_hash,
                computed,
            });
        }
    }

    let actual_root = cache.cache_root();
    if actual_root != expected_anchor.cache_root {
        return Err(BootstrapError::CacheRootMismatch {
            expected: expected_anchor.cache_root,
            actual: actual_root,
        });
    }

    verify_cache_values_against_proof(&cache, &proof, expected_state_root)?;
    let trie_cache = restore_trie_cache(&cache, proof, expected_state_root)?;
    Ok(RestoredBootstrapState { value_cache: cache, trie_cache })
}

fn check_package_limits(
    pkg: &CacheSnapshotPackage,
    limits: &BootstrapLimits,
) -> Result<(), BootstrapError> {
    ensure_limit("accounts", pkg.state.accounts.len(), limits.max_accounts)?;
    ensure_limit("storage slots", pkg.state.storage.len(), limits.max_storage_slots)?;
    ensure_limit("codes", pkg.state.codes.len(), limits.max_codes)?;
    ensure_limit(
        "state proof bytes",
        bincode::serialized_size(&pkg.proof)
            .map_err(|err| BootstrapError::ProofSerialization(err.to_string()))?
            .try_into()
            .unwrap_or(usize::MAX),
        limits.max_state_proof_bytes,
    )?;
    ensure_limit(
        "bytecode bytes",
        pkg.state
            .codes
            .iter()
            .fold(0usize, |total, (_, entry)| total.saturating_add(entry.value.len())),
        limits.max_code_bytes,
    )?;
    Ok(())
}

fn ensure_limit(label: &'static str, actual: usize, cap: usize) -> Result<(), BootstrapError> {
    if actual > cap {
        return Err(BootstrapError::LimitExceeded { label, actual, cap })
    }
    Ok(())
}

fn grouped_cache_targets(cache: &NetworkStateCache) -> BTreeMap<Address, BTreeSet<B256>> {
    let mut grouped = BTreeMap::new();
    for address in cache.accounts().keys() {
        grouped.entry(*address).or_insert_with(BTreeSet::new);
    }
    for (address, slot) in cache.storage().keys() {
        grouped.entry(*address).or_insert_with(BTreeSet::new).insert(*slot);
    }
    grouped
}

fn account_matches(cached: &AccountData, proven: Option<Account>) -> bool {
    match proven {
        Some(proven) => {
            cached.nonce == proven.nonce &&
                cached.balance == proven.balance &&
                cached.code_hash == proven.bytecode_hash
        }
        None => cached.nonce == 0 && cached.balance.is_zero() && cached.code_hash.is_none(),
    }
}

fn verify_cache_values_against_proof(
    cache: &NetworkStateCache,
    proof: &MultiProof,
    state_root: B256,
) -> Result<(), BootstrapError> {
    for (address, slots) in grouped_cache_targets(cache) {
        let slots = slots.into_iter().collect::<Vec<_>>();
        let account_proof = proof.account_proof(address, &slots).map_err(|err| {
            BootstrapError::ProofVerification(format!(
                "failed to extract proof for account {address}: {err}"
            ))
        })?;
        let account_proof =
            verify_and_trim_redundant_suffixes(account_proof, state_root).map_err(|err| {
                BootstrapError::ProofVerification(format!(
                    "invalid account or storage proof for {address}: {err}"
                ))
            })?;

        if let Some(cached) = cache.accounts().get(&address) &&
            !account_matches(&cached.value, account_proof.info)
        {
            return Err(BootstrapError::AccountValueMismatch { address })
        }

        for storage_proof in account_proof.storage_proofs {
            let expected = cache
                .storage()
                .get(&(address, storage_proof.key))
                .ok_or(BootstrapError::UnexpectedStorageProof { address, slot: storage_proof.key })?
                .value;
            if storage_proof.value != expected {
                return Err(BootstrapError::StorageValueMismatch {
                    address,
                    slot: storage_proof.key,
                    expected,
                    actual: storage_proof.value,
                })
            }
        }
    }
    Ok(())
}

fn restore_trie_cache(
    cache: &NetworkStateCache,
    proof: MultiProof,
    expected_state_root: B256,
) -> Result<PartialTrieNodeCache, BootstrapError> {
    if cache.accounts().is_empty() && cache.storage().is_empty() {
        return Ok(PartialTrieNodeCache::new())
    }

    let decoded = match multiproof_to_v2_or_request(proof) {
        Ok(decoded) => decoded,
        Err(TrieTransitionError::ProofRequired(targets)) => {
            return Err(BootstrapError::AdditionalProofTargetsRequired(targets))
        }
        Err(TrieTransitionError::Failed(err)) => return Err(BootstrapError::TrieRestore(err)),
    };
    match PartialTrieNodeCache::restore_from_decoded_multiproof(decoded, expected_state_root, cache)
    {
        Ok(cache) => Ok(cache),
        Err(TrieCacheValidationError::StateRootMismatch { expected, actual }) => {
            Err(BootstrapError::StateRootMismatch { expected, actual })
        }
        Err(err) => Err(BootstrapError::TrieCacheInvariant(err.to_string())),
    }
}

/// Reasons a cache snapshot could not be built or jointly restored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootstrapError {
    AnchorMismatch { expected: Box<CacheAnchor>, actual: Box<CacheAnchor> },
    StateBlockMismatch { state_block: u64, anchor_block: u64 },
    CacheRootMismatch { expected: B256, actual: B256 },
    BytecodePreimageMismatch { code_hash: B256, computed: B256 },
    LimitExceeded { label: &'static str, actual: usize, cap: usize },
    ProofSerialization(String),
    ProofProvider(String),
    ProofVerification(String),
    AccountValueMismatch { address: Address },
    UnexpectedStorageProof { address: Address, slot: B256 },
    StorageValueMismatch { address: Address, slot: B256, expected: U256, actual: U256 },
    AdditionalProofTargetsRequired(Vec<TrieProofTarget>),
    ProofRetryNoProgress { retries: usize },
    ProofRetryLimit { retries: usize, limit: usize },
    TrieRestore(String),
    StateRootMismatch { expected: B256, actual: B256 },
    TrieCacheInvariant(String),
}

impl std::fmt::Display for BootstrapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AnchorMismatch { expected, actual } => write!(
                f,
                "bootstrap anchor mismatch: expected {expected:?}, peer claimed {actual:?}"
            ),
            Self::StateBlockMismatch { state_block, anchor_block } => write!(
                f,
                "bootstrap state-block mismatch: state at block {state_block}, anchor at {anchor_block}"
            ),
            Self::CacheRootMismatch { expected, actual } => write!(
                f,
                "bootstrap cache root mismatch: expected {expected}, recomputed {actual}"
            ),
            Self::BytecodePreimageMismatch { code_hash, computed } => write!(
                f,
                "bootstrap bytecode preimage mismatch: key {code_hash}, keccak256(code) = {computed}"
            ),
            Self::LimitExceeded { label, actual, cap } => {
                write!(f, "bootstrap {label} exceeds limit: actual={actual}, cap={cap}")
            }
            Self::ProofSerialization(err) => {
                write!(f, "failed to measure bootstrap state proof: {err}")
            }
            Self::ProofProvider(err) => write!(f, "bootstrap proof provider failed: {err}"),
            Self::ProofVerification(err) => write!(f, "bootstrap proof verification failed: {err}"),
            Self::AccountValueMismatch { address } => {
                write!(f, "cached account does not match state proof: {address}")
            }
            Self::UnexpectedStorageProof { address, slot } => write!(
                f,
                "bootstrap proof contains unexpected storage target: address={address}, slot={slot}"
            ),
            Self::StorageValueMismatch { address, slot, expected, actual } => write!(
                f,
                "cached storage does not match state proof: address={address}, slot={slot}, expected={expected}, actual={actual}"
            ),
            Self::AdditionalProofTargetsRequired(targets) => write!(
                f,
                "bootstrap trie requires {} additional structural proof targets",
                targets.len()
            ),
            Self::ProofRetryNoProgress { retries } => {
                write!(f, "bootstrap proof retry made no progress after {retries} retries")
            }
            Self::ProofRetryLimit { retries, limit } => write!(
                f,
                "bootstrap proof retry limit exceeded: retries={retries}, limit={limit}"
            ),
            Self::TrieRestore(err) => write!(f, "bootstrap trie restore failed: {err}"),
            Self::StateRootMismatch { expected, actual } => write!(
                f,
                "bootstrap state root mismatch: expected={expected}, actual={actual}"
            ),
            Self::TrieCacheInvariant(err) => {
                write!(f, "bootstrap trie-cache invariant failed: {err}")
            }
        }
    }
}

impl std::error::Error for BootstrapError {}
