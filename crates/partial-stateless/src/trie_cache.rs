//! Partial MPT trie-node cache for the stateless validator.
//!
//! A stateless validator holds only flat state *values* in [`NetworkStateCache`]. To verify a
//! block's post-state root without touching a full database, it also needs the *trie nodes*
//! along the path to every account it writes. For accounts that were cache **misses**, those
//! nodes arrive in the builder's witness. For accounts that were cache **hits**, the witness
//! omits them — so the validator must have retained their trie path itself. That is what this
//! cache does.
//!
//! Nodes are keyed by their nibble **path** (not their hash), matching reth's
//! [`MultiProof::account_subtree`], so the cached nodes drop straight into a witness multiproof
//! and a [`SparseStateTrie`](reth_trie_sparse::SparseStateTrie) with no conversion.
//!
//! # Two layers (as specified for the v2 mechanism)
//!
//! - **Pinned (Layer 1)** — every account-trie node at path depth ≤ [`DEFAULT_PIN_NIBBLES`].
//!   Refreshed from each block's multiproof; the upper trie is small and touched by almost every
//!   block, so keeping it warm is cheap and high-value.
//! - **Deep (Layer 2)** — account-trie nodes at depth > pin depth, retained **only** for the
//!   accounts whose value currently lives in [`NetworkStateCache`]. Ref-counted so that nodes
//!   shared by several cached accounts survive until the last of them is evicted. The deep layer is
//!   added to (from the witness) when an account is first missed, and pruned in lockstep with
//!   value-cache eviction — the cache never diverges from the value cache it mirrors.
//!
//! [`NetworkStateCache`]: crate::network_cache::NetworkStateCache

use crate::{
    accessed_state::BlockAccessedState, network_cache::MissResult, participant::ParticipantCache,
};
use alloy_primitives::{keccak256, Address, Bytes, B256};
use reth_trie_common::{proof::ProofNodes, MultiProof, Nibbles};
use std::collections::HashMap;

/// Account-trie nibble-path depth up to which every node is pinned (Layer 1).
///
/// Depth is measured in nibbles: a path of length `d` sits `d` levels below the root. The top
/// five levels of the account trie are shared by essentially all traffic, so pinning them keeps
/// the common upper structure permanently warm.
pub const DEFAULT_PIN_NIBBLES: usize = 5;

/// Partial account-trie node cache (pinned top layer + ref-counted deep layer).
///
/// See the module docs for the layering rationale. All node bytes are RLP-encoded trie nodes as
/// produced by reth's proof machinery.
#[derive(Debug, Clone)]
pub struct PartialTrieNodeCache {
    /// Layer 1: account-trie nodes at path depth ≤ `pin_depth`, keyed by nibble path.
    pinned: HashMap<Nibbles, Bytes>,
    /// Layer 2: account-trie nodes at path depth > `pin_depth`, ref-counted by how many cached
    /// accounts' paths traverse them.
    deep: HashMap<Nibbles, DeepNode>,
    /// For each account currently mirrored from the value cache, the deep node paths it
    /// contributed. Used to decrement ref-counts when the account leaves the value cache.
    account_deep_paths: HashMap<Address, Vec<Nibbles>>,
    /// Depth threshold separating the pinned and deep layers.
    pin_depth: usize,
}

/// A deep-layer node with a reference count equal to the number of cached accounts whose path
/// passes through it.
#[derive(Debug, Clone)]
struct DeepNode {
    bytes: Bytes,
    refcount: u32,
}

impl Default for PartialTrieNodeCache {
    fn default() -> Self {
        Self::new()
    }
}

impl PartialTrieNodeCache {
    /// Create an empty cache with the default pin depth ([`DEFAULT_PIN_NIBBLES`]).
    pub fn new() -> Self {
        Self::with_pin_depth(DEFAULT_PIN_NIBBLES)
    }

    /// Create an empty cache with a custom pin depth.
    pub fn with_pin_depth(pin_depth: usize) -> Self {
        Self {
            pinned: HashMap::new(),
            deep: HashMap::new(),
            account_deep_paths: HashMap::new(),
            pin_depth,
        }
    }

    /// The pinned/deep depth threshold (in nibbles).
    pub fn pin_depth(&self) -> usize {
        self.pin_depth
    }

    /// Advance the cache after a block has executed.
    ///
    /// This is the single entry point the ExEx (builder or validator) calls each block; it keeps
    /// the trie-node cache a deterministic function of the value cache it mirrors:
    ///
    /// 1. Refresh the pinned top layer from this block's `multiproof`.
    /// 2. For each account that was a fresh miss this block and is now retained in the value cache,
    ///    capture its deep path from the witness.
    /// 3. Drop the deep paths of any account no longer held by the value cache.
    ///
    /// `is_still_cached` reports whether an account's value is currently in the value cache,
    /// letting the trie cache mirror eviction without any change to [`NetworkStateCache`].
    ///
    /// [`NetworkStateCache`]: crate::network_cache::NetworkStateCache
    pub fn on_block_executed<F>(
        &mut self,
        multiproof: &MultiProof,
        newly_missed_accounts: &[Address],
        is_still_cached: F,
    ) where
        F: Fn(&Address) -> bool,
    {
        self.refresh_pinned(multiproof);

        for address in newly_missed_accounts {
            if is_still_cached(address) && !self.account_deep_paths.contains_key(address) {
                self.insert_account_path(*address, multiproof);
            }
        }

        self.retain_accounts(is_still_cached);
    }

    /// Refresh the pinned layer (depth ≤ `pin_depth`) from a block's multiproof.
    ///
    /// Nodes touched this block are overwritten with their fresh bytes; untouched pinned nodes
    /// are left in place. Freshness on the current block is guaranteed at root-computation time,
    /// where the witness always takes precedence over cached nodes.
    pub fn refresh_pinned(&mut self, multiproof: &MultiProof) {
        for (path, bytes) in multiproof.account_subtree.iter() {
            if path.len() <= self.pin_depth {
                self.pinned.insert(*path, bytes.clone());
            }
        }
    }

    /// Capture the deep-layer nodes (depth > `pin_depth`) on the path to `address` from a
    /// multiproof that contains that account's proof (i.e. the block where it was missed).
    ///
    /// Idempotent per account: calling it again for an already-tracked account is a no-op, so
    /// ref-counts never double-count. (A warm account is never a miss, so this is not normally
    /// re-invoked while the account is tracked.)
    pub fn insert_account_path(&mut self, address: Address, multiproof: &MultiProof) {
        if self.account_deep_paths.contains_key(&address) {
            return;
        }

        let account_path = Nibbles::unpack(keccak256(address));
        let mut contributed = Vec::new();
        for (path, bytes) in multiproof.account_subtree.matching_nodes(&account_path) {
            if path.len() <= self.pin_depth {
                continue;
            }
            match self.deep.get_mut(&path) {
                Some(node) => node.refcount += 1,
                None => {
                    self.deep.insert(path, DeepNode { bytes, refcount: 1 });
                }
            }
            contributed.push(path);
        }

        // Track the account even if it contributed no deep nodes (its whole path fits in the
        // pinned layer) so membership queries and eviction bookkeeping stay consistent.
        self.account_deep_paths.insert(address, contributed);
    }

    /// Drop the deep paths of every tracked account for which `is_still_cached` is false.
    pub fn retain_accounts<F>(&mut self, is_still_cached: F)
    where
        F: Fn(&Address) -> bool,
    {
        let evicted: Vec<Address> = self
            .account_deep_paths
            .keys()
            .filter(|address| !is_still_cached(address))
            .copied()
            .collect();
        for address in evicted {
            self.evict_account(&address);
        }
    }

    /// Drop a single account's deep-path contribution, removing nodes whose ref-count hits zero.
    pub fn evict_account(&mut self, address: &Address) {
        let Some(paths) = self.account_deep_paths.remove(address) else {
            return;
        };
        for path in paths {
            if let Some(node) = self.deep.get_mut(&path) {
                node.refcount = node.refcount.saturating_sub(1);
                if node.refcount == 0 {
                    self.deep.remove(&path);
                }
            }
        }
    }

    /// Whether the full account-trie path to `address` is warm in this cache.
    ///
    /// True once the account has been captured from a witness and not yet evicted. Accounts
    /// whose leaf sits entirely within the pinned depth (extremely rare on mainnet) are also
    /// tracked with an empty deep contribution, so this stays correct.
    pub fn contains_account_path(&self, address: &Address) -> bool {
        self.account_deep_paths.contains_key(address)
    }

    /// Iterate all cached account-trie nodes as `(path, bytes)` — pinned then deep.
    ///
    /// This is the set merged into a witness multiproof for trustless root computation.
    pub fn account_proof_nodes(&self) -> impl Iterator<Item = (&Nibbles, &Bytes)> {
        self.pinned.iter().chain(self.deep.iter().map(|(path, node)| (path, &node.bytes)))
    }

    /// Total number of distinct trie nodes held (pinned + deep). This is the memory-budget /
    /// warm-node count the optimization objective is measured against.
    pub fn warm_node_count(&self) -> usize {
        self.pinned.len() + self.deep.len()
    }

    /// Number of pinned (Layer 1) nodes.
    pub fn pinned_node_count(&self) -> usize {
        self.pinned.len()
    }

    /// Number of deep (Layer 2) nodes.
    pub fn deep_node_count(&self) -> usize {
        self.deep.len()
    }

    /// Number of accounts whose deep path is currently mirrored.
    pub fn tracked_account_count(&self) -> usize {
        self.account_deep_paths.len()
    }

    /// Look up a cached account-trie node by its nibble path (pinned layer first, then deep).
    pub fn node_bytes(&self, path: &Nibbles) -> Option<&Bytes> {
        self.pinned.get(path).or_else(|| self.deep.get(path).map(|node| &node.bytes))
    }

    /// Drop from an account-trie multiproof every node this cache already holds **byte-identical**,
    /// returning the reduced proof. A cache-aware builder ships this smaller witness; the validator
    /// reconstructs the original with [`Self::reveal_known_nodes`], since its cache holds the dropped
    /// nodes verbatim. Only byte-identical nodes are dropped, so reconstruction is lossless even when
    /// the cache also holds a stale copy at some other path. Storage subtrees and branch masks are
    /// left untouched (the cache holds no storage-trie nodes).
    pub fn prune_known_nodes(&self, proof: &MultiProof) -> MultiProof {
        let mut account_subtree = ProofNodes::default();
        for (path, bytes) in proof.account_subtree.iter() {
            if self.node_bytes(path) == Some(bytes) {
                continue; // the validator already holds this node identically
            }
            account_subtree.insert(*path, bytes.clone());
        }
        MultiProof {
            account_subtree,
            branch_node_masks: proof.branch_node_masks.clone(),
            storages: proof.storages.clone(),
        }
    }

    /// Add every cached account-trie node whose path is absent from `proof` — the inverse of
    /// [`Self::prune_known_nodes`], reconstructing the full proof from a pruned witness + this cache.
    /// Nodes already present in `proof` win, so fresh witness nodes are never overwritten by a stale
    /// cached copy.
    pub fn reveal_known_nodes(&self, proof: &mut MultiProof) {
        for (path, bytes) in self.account_proof_nodes() {
            if !proof.account_subtree.contains_key(path) {
                proof.account_subtree.insert(*path, bytes.clone());
            }
        }
    }
}

/// The trie-node cache is the *trie-path* membership view of the shared contract: an account is
/// warm iff its account-trie path is held. Storage-trie and code nodes are not (yet) mirrored
/// here — they are carried by the value witness — so they always read as cold from this view.
///
/// Because of that, the only directly comparable figure today is the **account-path** miss
/// (`missed_accounts`): storage is counted as always-cold (inflating `miss_ratio` toward the
/// storage-heavy end) and code is excluded from the denominator entirely, since neither is an
/// account-trie node. Read `missed_accounts` for the account-trie coldness the objective targets;
/// the aggregate `miss_ratio` becomes meaningful once storage-trie node caching lands.
impl ParticipantCache for PartialTrieNodeCache {
    fn contains_account(&self, address: &Address) -> bool {
        self.contains_account_path(address)
    }

    fn contains_storage(&self, _address: &Address, _slot: &B256) -> bool {
        // Storage-trie node caching is a separate frontier; for now the storage path is always
        // supplied by the witness.
        false
    }

    fn contains_code(&self, _code_hash: &B256) -> bool {
        // Bytecode is not an MPT node; it is carried by the value witness, not this cache.
        false
    }

    fn compute_miss(&self, accessed: &BlockAccessedState) -> MissResult {
        let mut missed_accounts = Vec::new();
        for address in accessed.accounts.keys() {
            if !self.contains_account_path(address) {
                missed_accounts.push(*address);
            }
        }

        let mut missed_storage = Vec::new();
        for (address, slot) in accessed.storage.keys() {
            if !self.contains_storage(address, slot) {
                missed_storage.push((*address, *slot));
            }
        }

        // Codes are not trie nodes; the trie-node miss view does not account for them.
        let missed_codes = Vec::new();

        let total_accessed = accessed.accounts.len() + accessed.storage.len();
        let total_missed = missed_accounts.len() + missed_storage.len();
        let miss_ratio =
            if total_accessed > 0 { total_missed as f64 / total_accessed as f64 } else { 0.0 };

        MissResult {
            missed_accounts,
            missed_storage,
            missed_codes,
            total_accessed,
            total_missed,
            miss_ratio,
        }
    }

    // Commits to the node set currently held. The pinned layer is accumulate-only
    // (`refresh_pinned` never evicts), so this reflects recent block history, not solely the
    // present warm set; participants replaying the same chain from the same start still converge.
    fn cache_root(&self) -> B256 {
        let mut nodes: Vec<(Nibbles, &Bytes)> =
            self.account_proof_nodes().map(|(path, bytes)| (*path, bytes)).collect();
        nodes.sort_by(|(a, _), (b, _)| a.to_vec().cmp(&b.to_vec()));

        let mut preimage = Vec::new();
        preimage.extend_from_slice(b"PartialTrieNodeCacheRoot/v1");
        preimage.extend_from_slice(&(nodes.len() as u64).to_be_bytes());
        for (path, bytes) in nodes {
            let path_nibbles = path.to_vec();
            preimage.extend_from_slice(&(path_nibbles.len() as u64).to_be_bytes());
            preimage.extend_from_slice(&path_nibbles);
            preimage.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
            preimage.extend_from_slice(bytes);
        }
        keccak256(preimage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ParticipantCache;

    /// Build an account-trie multiproof that reveals the full path to `address`, placing one node
    /// at each of the given prefix lengths (plus the leaf at the account's full 64-nibble path).
    fn multiproof_for(address: Address, prefix_lens: &[usize]) -> MultiProof {
        let account_path = Nibbles::unpack(keccak256(address));
        let mut subtree = ProofNodes::default();
        for &len in prefix_lens {
            let path = account_path.slice(0..len);
            // Node bytes are opaque to the cache; tag them by depth so tests can assert identity.
            subtree.insert(path, Bytes::from(vec![len as u8]));
        }
        MultiProof {
            account_subtree: subtree,
            branch_node_masks: Default::default(),
            storages: Default::default(),
        }
    }

    #[test]
    fn pinned_layer_captures_only_shallow_nodes() {
        let addr = Address::repeat_byte(0x11);
        let mp = multiproof_for(addr, &[0, 2, 5, 6, 10, 64]);
        let mut cache = PartialTrieNodeCache::new();

        cache.refresh_pinned(&mp);

        // Depths 0,2,5 are pinned (≤ 5); 6,10,64 are not.
        assert_eq!(cache.pinned_node_count(), 3);
        assert_eq!(cache.deep_node_count(), 0);
    }

    #[test]
    fn deep_layer_captures_account_path_and_evicts_with_value() {
        let addr = Address::repeat_byte(0x22);
        let mp = multiproof_for(addr, &[0, 2, 5, 6, 10, 64]);
        let mut cache = PartialTrieNodeCache::new();

        // Account is missed and retained by the value cache.
        cache.on_block_executed(&mp, &[addr], |_| true);

        assert!(cache.contains_account_path(&addr));
        // Deep nodes: depths 6, 10, 64 → 3 nodes.
        assert_eq!(cache.deep_node_count(), 3);
        assert_eq!(cache.tracked_account_count(), 1);

        // Value cache evicts the account → deep path is dropped.
        cache.on_block_executed(&MultiProof::default(), &[], |_| false);
        assert!(!cache.contains_account_path(&addr));
        assert_eq!(cache.deep_node_count(), 0);
        assert_eq!(cache.tracked_account_count(), 0);
    }

    /// Deterministically find two distinct addresses whose hashed (account-trie) paths share a
    /// prefix strictly deeper than the pin depth, so a genuine deep node is shared between them.
    fn two_addresses_sharing_deep_prefix() -> (Address, Address, usize) {
        use std::collections::HashMap as Map;
        let prefix_len = DEFAULT_PIN_NIBBLES + 1;
        let mut seen: Map<Vec<u8>, Address> = Map::new();
        for n in 0u64..1_000_000 {
            let mut bytes = [0u8; 20];
            bytes[..8].copy_from_slice(&n.to_be_bytes());
            let addr = Address::from(bytes);
            let key = Nibbles::unpack(keccak256(addr)).slice(0..prefix_len).to_vec();
            if let Some(prev) = seen.insert(key, addr) {
                return (prev, addr, prefix_len);
            }
        }
        panic!("no shared {}-nibble prefix found", prefix_len);
    }

    #[test]
    fn shared_deep_nodes_are_refcounted() {
        // Two addresses whose hashed paths share a deep prefix both pin the node at that prefix;
        // it survives until the last of them is evicted.
        let (a, b, shared_len) = two_addresses_sharing_deep_prefix();
        let mut cache = PartialTrieNodeCache::new();
        // Each account reveals: the shared deep node + its own leaf.
        let mp_a = multiproof_for(a, &[shared_len, 64]);
        let mp_b = multiproof_for(b, &[shared_len, 64]);

        cache.insert_account_path(a, &mp_a);
        cache.insert_account_path(b, &mp_b);
        // shared node (refcount 2) + leaf_a + leaf_b.
        assert_eq!(cache.deep_node_count(), 3);

        // Evicting a drops leaf_a but keeps the shared node (still referenced by b).
        cache.evict_account(&a);
        assert!(cache.contains_account_path(&b));
        assert_eq!(cache.deep_node_count(), 2);

        // Evicting b drops the shared node too.
        cache.evict_account(&b);
        assert_eq!(cache.deep_node_count(), 0);
    }

    #[test]
    fn participant_miss_reflects_trie_path_membership() {
        let cached = Address::repeat_byte(0x33);
        let uncached = Address::repeat_byte(0x44);
        let mp = multiproof_for(cached, &[0, 2, 5, 6, 64]);
        let mut cache = PartialTrieNodeCache::new();
        cache.on_block_executed(&mp, &[cached], |_| true);

        let mut accessed = BlockAccessedState::default();
        accessed.accounts.insert(
            cached,
            crate::policy::AccountData {
                nonce: 0,
                balance: alloy_primitives::U256::ZERO,
                code_hash: None,
            },
        );
        accessed.accounts.insert(
            uncached,
            crate::policy::AccountData {
                nonce: 0,
                balance: alloy_primitives::U256::ZERO,
                code_hash: None,
            },
        );

        let miss = ParticipantCache::compute_miss(&cache, &accessed);
        assert_eq!(miss.missed_accounts, vec![uncached]);
        assert_eq!(miss.total_accessed, 2);
        assert_eq!(miss.total_missed, 1);
    }

    #[test]
    fn prune_then_reveal_round_trips_to_original() {
        let addr = Address::repeat_byte(0x66);
        // Cache holds this account's full path (missed once, retained by the value cache).
        let mut cache = PartialTrieNodeCache::new();
        cache.on_block_executed(&multiproof_for(addr, &[0, 2, 5, 6, 10, 64]), &[addr], |_| true);

        // A fresh proof for the same account: the builder prunes every node the cache holds.
        let fresh = multiproof_for(addr, &[0, 2, 5, 6, 10, 64]);
        let pruned = cache.prune_known_nodes(&fresh);
        assert!(pruned.account_subtree.len() < fresh.account_subtree.len());

        // The validator reconstructs pruned ∪ cache and recovers every original node byte-for-byte.
        let mut reconstructed = pruned;
        cache.reveal_known_nodes(&mut reconstructed);
        for (path, bytes) in fresh.account_subtree.iter() {
            assert_eq!(reconstructed.account_subtree.get(path), Some(bytes));
        }
    }

    #[test]
    fn cache_root_is_deterministic_and_content_bound() {
        let addr = Address::repeat_byte(0x55);
        let mp = multiproof_for(addr, &[0, 2, 6, 64]);

        let mut a = PartialTrieNodeCache::new();
        a.on_block_executed(&mp, &[addr], |_| true);
        let mut b = PartialTrieNodeCache::new();
        b.on_block_executed(&mp, &[addr], |_| true);
        assert_eq!(a.cache_root(), b.cache_root());

        let mut c = PartialTrieNodeCache::new();
        c.on_block_executed(&multiproof_for(addr, &[0, 2, 6]), &[addr], |_| true);
        assert_ne!(a.cache_root(), c.cache_root());
    }
}
