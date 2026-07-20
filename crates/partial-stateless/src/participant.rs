//! Shared participant interface for partial statelessness.
//!
//! Partial statelessness only works if every participant agrees, byte-for-byte, on
//! which accessed state is *cold* (needs a witness) for a block. A caching-aware block
//! builder must compute the same miss set the validator will compute, otherwise the
//! witness it ships does not cover what the validator is missing.
//!
//! [`ParticipantCache`] is that shared contract. Both the builder and the validator run
//! an identical implementation, so the miss set — and therefore the miss rate — is a
//! deterministic function of the cache policy and the canonical block stream alone.
//!
//! Two caches implement it:
//! - [`NetworkStateCache`](crate::network_cache::NetworkStateCache): the *value* membership view
//!   (which nonce/balance/storage/code the witness must carry).
//! - [`PartialTrieNodeCache`](crate::trie_cache::PartialTrieNodeCache): the *trie-node* membership
//!   view (which MPT nodes, by nibble path, the witness must carry). The optimization objective —
//!   cold trie-node count at a fixed memory budget — is measured through this implementation.

use crate::{
    accessed_state::BlockAccessedState,
    network_cache::{MissResult, NetworkStateCache},
};
use alloy_primitives::{Address, B256};

/// Contract shared by every partial-stateless participant (block builders and validators).
///
/// An implementation answers a single question consistently across the network: given the
/// state a block accesses, what is cold (must be supplied by a witness) versus warm (served
/// from this cache)? Because every participant runs the same implementation against the same
/// canonical chain, they all derive the identical miss set, and hence the identical miss rate.
pub trait ParticipantCache {
    /// Whether this account is warm (served from cache, no witness required).
    fn contains_account(&self, address: &Address) -> bool;

    /// Whether this storage slot is warm.
    fn contains_storage(&self, address: &Address, slot: &B256) -> bool;

    /// Whether this bytecode is warm.
    fn contains_code(&self, code_hash: &B256) -> bool;

    /// Compute the canonical cold (miss) set for a block's accessed state.
    ///
    /// This is the set a caching-aware builder must cover with a witness, and the set a
    /// validator expects the witness to cover — they must be equal for verification to pass.
    fn compute_miss(&self, accessed: &BlockAccessedState) -> MissResult;

    /// Deterministic commitment to the current warm set.
    ///
    /// Two participants whose caches hold the same warm set (under the same policy) produce
    /// the same `cache_root`; it is what a [`CacheAnchor`](crate::sidecar::CacheAnchor) binds.
    fn cache_root(&self) -> B256;

    /// Convenience: fraction of accessed keys that are cold for `accessed` (0.0..=1.0).
    ///
    /// Provided in terms of [`Self::compute_miss`] so every implementation reports the miss
    /// rate the same way.
    fn miss_rate(&self, accessed: &BlockAccessedState) -> f64 {
        self.compute_miss(accessed).miss_ratio
    }
}

/// The flat value cache is the *value* membership view of the shared contract: a key is warm
/// iff its concrete value (nonce/balance/storage/code) is currently held.
impl ParticipantCache for NetworkStateCache {
    fn contains_account(&self, address: &Address) -> bool {
        NetworkStateCache::contains_account(self, address)
    }

    fn contains_storage(&self, address: &Address, slot: &B256) -> bool {
        NetworkStateCache::contains_storage(self, address, slot)
    }

    fn contains_code(&self, code_hash: &B256) -> bool {
        NetworkStateCache::contains_code(self, code_hash)
    }

    fn compute_miss(&self, accessed: &BlockAccessedState) -> MissResult {
        NetworkStateCache::compute_miss(self, accessed)
    }

    fn cache_root(&self) -> B256 {
        NetworkStateCache::cache_root(self)
    }
}
