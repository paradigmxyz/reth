//! Bootstrap a cold node into a warm cache from an untrusted peer snapshot.
//!
//! A partial-stateless sidecar carries only cache *misses*, so it assumes the
//! validator already holds the shared network-level cache. A freshly started
//! node has an empty cache and cannot validate blocks from misses alone: it must
//! first warm its cache to the network state before entering partial mode.
//!
//! Rather than replaying `[H - W, H]` block-by-block, the node accepts a
//! [`CacheSnapshotPackage`] from an untrusted peer and verifies it against a
//! `cache_root` commitment that comes from consensus (the trust anchor). This is
//! the same shape as EIP-4844 blob sidecars: the data travels untrusted, but a
//! consensus-committed commitment lets the node accept it without trusting the
//! sender. A forged snapshot fails the `cache_root` re-computation and is
//! rejected, so peer honesty is never required.

use crate::{
    network_cache::NetworkStateCache, persistence::CacheState, policy::CachePolicy,
    sidecar::CacheAnchor,
};
use alloy_primitives::{keccak256, B256};

/// An untrusted peer's cache snapshot plus the anchor it claims the snapshot
/// commits to. Verified by [`verify_and_restore`] before being trusted.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CacheSnapshotPackage {
    /// Serialized cache contents (accounts/storage/codes/current_block).
    pub state: CacheState,
    /// The `(block, policy, cache_root)` context the peer claims for `state`.
    pub anchor: CacheAnchor,
}

impl CacheSnapshotPackage {
    /// Build a package from a live cache and the anchor binding it. Used by the
    /// serving peer and by tests; the receiving node only ever consumes packages.
    pub fn from_cache(cache: &NetworkStateCache, anchor: CacheAnchor) -> Self {
        Self { state: CacheState::from_cache(cache), anchor }
    }
}

/// Verify an untrusted snapshot against a consensus-trusted anchor and, on
/// success, return a warm [`NetworkStateCache`] ready for partial mode.
///
/// `expected_anchor` is the trust root: it must originate from consensus, not
/// from the peer. Checks run cheapest-first so a malformed package is rejected
/// before the O(n) hashing work.
///
/// `account_policy`/`storage_policy` must match `expected_anchor.cache_policy_id`;
/// this isn't checked (`cache_root` omits the policy), so a mismatch verifies but
/// diverges later. Derive both from the same source.
pub fn verify_and_restore(
    pkg: CacheSnapshotPackage,
    expected_anchor: &CacheAnchor,
    account_policy: Box<dyn CachePolicy>,
    storage_policy: Box<dyn CachePolicy>,
) -> Result<NetworkStateCache, BootstrapError> {
    // 1a. The peer's claimed context must equal the consensus anchor. This
    // rejects a valid snapshot taken at a different block/policy window.
    if pkg.anchor != *expected_anchor {
        return Err(BootstrapError::AnchorMismatch {
            expected: Box::new(*expected_anchor),
            actual: Box::new(pkg.anchor),
        });
    }

    // 1b. `cache_root` does not commit `current_block`, so bind it explicitly.
    if pkg.state.current_block != pkg.anchor.block_number {
        return Err(BootstrapError::StateBlockMismatch {
            state_block: pkg.state.current_block,
            anchor_block: pkg.anchor.block_number,
        });
    }

    // 2. Reconstruct the cache. Undo history is empty, so bootstrap must anchor
    // to a finalized block (a deeper reorg cannot be rolled back post-restore).
    let cache = pkg.state.into_cache(account_policy, storage_policy);

    // 3. The heart of the check: recompute `cache_root` over the restored
    // contents and compare against the consensus commitment. Any tampered
    // account/storage value or `last_accessed_block` diverges here, which is
    // where trust in the peer is discharged.
    let actual_root = cache.cache_root();
    if actual_root != expected_anchor.cache_root {
        return Err(BootstrapError::CacheRootMismatch {
            expected: expected_anchor.cache_root,
            actual: actual_root,
        });
    }

    // 4. `cache_root` commits code entries by `code_hash` only, not by raw
    // bytecode, so a peer could ship garbage bytes under a valid hash and pass
    // step 3. Verify each preimage directly.
    //
    // NOTE: if `cache_root` is later changed to bind bytecode values, step 3
    // already catches this and step 4 becomes redundant and can be removed.
    for (code_hash, entry) in cache.codes() {
        let computed = keccak256(&entry.value);
        if computed != *code_hash {
            return Err(BootstrapError::BytecodePreimageMismatch {
                code_hash: *code_hash,
                computed,
            });
        }
    }

    Ok(cache)
}

/// Reasons a [`CacheSnapshotPackage`] failed verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootstrapError {
    /// The peer's claimed anchor did not match the consensus anchor.
    // Boxed because `CacheAnchor` is large relative to the other variants.
    AnchorMismatch { expected: Box<CacheAnchor>, actual: Box<CacheAnchor> },
    /// `state.current_block` disagreed with `anchor.block_number`.
    StateBlockMismatch { state_block: u64, anchor_block: u64 },
    /// The recomputed `cache_root` did not match the committed root.
    CacheRootMismatch { expected: B256, actual: B256 },
    /// A code entry's bytes did not hash to its `code_hash` key.
    BytecodePreimageMismatch { code_hash: B256, computed: B256 },
}

impl std::fmt::Display for BootstrapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BootstrapError::AnchorMismatch { expected, actual } => write!(
                f,
                "bootstrap anchor mismatch: expected {expected:?}, peer claimed {actual:?}"
            ),
            BootstrapError::StateBlockMismatch { state_block, anchor_block } => write!(
                f,
                "bootstrap state-block mismatch: state at block {state_block}, anchor at {anchor_block}"
            ),
            BootstrapError::CacheRootMismatch { expected, actual } => write!(
                f,
                "bootstrap cache_root mismatch: expected {expected}, recomputed {actual}"
            ),
            BootstrapError::BytecodePreimageMismatch { code_hash, computed } => write!(
                f,
                "bootstrap bytecode preimage mismatch: key {code_hash}, keccak256(code) = {computed}"
            ),
        }
    }
}

impl std::error::Error for BootstrapError {}
