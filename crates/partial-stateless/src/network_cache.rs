//! NetworkStateCache: the protocol-level cache representing state that all
//! validators are assumed to hold.
//!
//! Completely separate from reth's internal `ExecutionCache`.

use crate::{
    accessed_state::BlockAccessedState,
    policy::{AccountData, CachePolicy},
};
use alloy_primitives::{Address, Bytes, B256, U256};
use std::collections::{HashMap, VecDeque};
use tracing::{debug, info};

/// An entry in the network state cache, tracking access metadata.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CachedEntry<T> {
    pub value: T,
    pub first_accessed_block: u64,
    pub last_accessed_block: u64,
    pub access_count: u32,
}

impl<T> CachedEntry<T> {
    fn new(value: T, block: u64) -> Self {
        Self { value, first_accessed_block: block, last_accessed_block: block, access_count: 1 }
    }

    fn touch(&mut self, block: u64) {
        self.last_accessed_block = block;
        self.access_count += 1;
    }
}

/// Statistics for a single cache update operation.
#[derive(Debug, Clone, Default)]
pub struct UpdateStats {
    /// Number of new account entries added.
    pub accounts_added: usize,
    /// Number of existing account entries refreshed (access time updated).
    pub accounts_refreshed: usize,
    /// Number of account entries evicted by policy.
    pub accounts_evicted: usize,
    /// Number of new storage entries added.
    pub storage_added: usize,
    /// Number of existing storage entries refreshed.
    pub storage_refreshed: usize,
    /// Number of storage entries evicted by policy.
    pub storage_evicted: usize,
    /// Number of new code entries added.
    pub codes_added: usize,
    /// Number of code entries evicted.
    pub codes_evicted: usize,
}

/// Snapshot of the cache state at a point in time.
#[derive(Debug, Clone, Default)]
pub struct CacheSnapshot {
    pub total_accounts: usize,
    pub total_storage_slots: usize,
    pub total_codes: usize,
    pub current_block: u64,
}

/// Network-level state cache.
///
/// Represents the state that all validators in the network are assumed to hold.
/// When a new block arrives, state that is NOT in this cache requires a witness
/// (Merkle proof) to be transmitted as a sidecar.
pub struct NetworkStateCache {
    /// Cached accounts: address → (AccountData, metadata)
    accounts: HashMap<Address, CachedEntry<AccountData>>,
    /// Cached storage: (address, slot) → (value, metadata)
    storage: HashMap<(Address, B256), CachedEntry<U256>>,
    /// Cached bytecodes: code_hash → (bytes, metadata)
    codes: HashMap<B256, CachedEntry<Bytes>>,
    /// Eviction policy for accounts (can differ from storage policy).
    account_policy: Box<dyn CachePolicy>,
    /// Eviction policy for storage & codes (can differ from account policy).
    storage_policy: Box<dyn CachePolicy>,
    /// Current block number.
    current_block: u64,
    /// Per-block undo records (oldest→newest) enabling rollback on reorg.
    /// Retained only for the unfinalized window; pruned below the finalized block.
    undo_log: VecDeque<BlockCacheUndo>,
}

impl NetworkStateCache {
    /// Create a new cache with separate policies for accounts and storage/codes.
    pub fn new(
        account_policy: Box<dyn CachePolicy>,
        storage_policy: Box<dyn CachePolicy>,
    ) -> Self {
        Self {
            accounts: HashMap::new(),
            storage: HashMap::new(),
            codes: HashMap::new(),
            account_policy,
            storage_policy,
            current_block: 0,
            undo_log: VecDeque::new(),
        }
    }

    /// Create a new cache with the same policy applied to both accounts and storage/codes.
    pub fn with_uniform_policy(policy_fn: impl Fn() -> Box<dyn CachePolicy>) -> Self {
        Self::new(policy_fn(), policy_fn())
    }

    /// Restore a cache from previously persisted state.
    pub fn restore(
        accounts: HashMap<Address, CachedEntry<AccountData>>,
        storage: HashMap<(Address, B256), CachedEntry<U256>>,
        codes: HashMap<B256, CachedEntry<Bytes>>,
        current_block: u64,
        account_policy: Box<dyn CachePolicy>,
        storage_policy: Box<dyn CachePolicy>,
    ) -> Self {
        // Undo history is not persisted: a freshly restored cache has no rollback
        // history, so a reorg deeper than what arrives after restart triggers a
        // cold reset (see `reset`). This is safe — only accuracy of the affected
        // blocks degrades until the cache warms again.
        Self {
            accounts,
            storage,
            codes,
            account_policy,
            storage_policy,
            current_block,
            undo_log: VecDeque::new(),
        }
    }

    /// Process a new block's execution results.
    ///
    /// 1. Inserts/refreshes accessed state entries.
    /// 2. Applies eviction policies.
    /// 3. Returns update statistics.
    pub fn on_block_executed(
        &mut self,
        block_number: u64,
        accessed: &BlockAccessedState,
    ) -> UpdateStats {
        // Capture the cache's pre-block state so this block can be rolled back on
        // reorg. For every key we touch or evict we record its prior value:
        // `Some(entry)` = existed before, `None` = absent before.
        let mut undo = BlockCacheUndo::new(block_number, self.current_block);
        self.current_block = block_number;
        let mut stats = UpdateStats::default();

        // --- Insert/refresh accounts ---
        for (address, account_data) in &accessed.accounts {
            undo.accounts_before
                .entry(*address)
                .or_insert_with(|| self.accounts.get(address).cloned());
            match self.accounts.get_mut(address) {
                Some(entry) => {
                    entry.value = account_data.clone();
                    entry.touch(block_number);
                    stats.accounts_refreshed += 1;
                }
                None => {
                    self.accounts
                        .insert(*address, CachedEntry::new(account_data.clone(), block_number));
                    stats.accounts_added += 1;
                }
            }
        }

        // --- Insert/refresh storage ---
        for ((address, slot), value) in &accessed.storage {
            undo.storage_before
                .entry((*address, *slot))
                .or_insert_with(|| self.storage.get(&(*address, *slot)).cloned());
            match self.storage.get_mut(&(*address, *slot)) {
                Some(entry) => {
                    entry.value = *value;
                    entry.touch(block_number);
                    stats.storage_refreshed += 1;
                }
                None => {
                    self.storage.insert((*address, *slot), CachedEntry::new(*value, block_number));
                    stats.storage_added += 1;
                }
            }
        }

        // --- Insert/refresh codes ---
        for (code_hash, bytecode) in &accessed.codes {
            undo.codes_before
                .entry(*code_hash)
                .or_insert_with(|| self.codes.get(code_hash).cloned());
            match self.codes.get_mut(code_hash) {
                Some(entry) => {
                    entry.touch(block_number);
                }
                None => {
                    self.codes
                        .insert(*code_hash, CachedEntry::new(bytecode.clone(), block_number));
                    stats.codes_added += 1;
                }
            }
        }

        // --- Apply eviction policies ---
        // Snapshot the maps before eviction so we can record exactly which entries
        // the policy removes (their values are gone afterwards). The snapshot is
        // transient — only the removed entries are kept, in the undo record.
        let accounts_pre_evict = self.accounts.clone();
        let storage_pre_evict = self.storage.clone();
        let codes_pre_evict = self.codes.clone();

        self.account_policy.evict_accounts(&mut self.accounts, block_number);
        self.storage_policy.evict_storage(&mut self.storage, &mut self.codes, block_number);

        for (address, entry) in &accounts_pre_evict {
            if !self.accounts.contains_key(address) {
                undo.accounts_before.entry(*address).or_insert_with(|| Some(entry.clone()));
            }
        }
        for (key, entry) in &storage_pre_evict {
            if !self.storage.contains_key(key) {
                undo.storage_before.entry(*key).or_insert_with(|| Some(entry.clone()));
            }
        }
        for (code_hash, entry) in &codes_pre_evict {
            if !self.codes.contains_key(code_hash) {
                undo.codes_before.entry(*code_hash).or_insert_with(|| Some(entry.clone()));
            }
        }

        stats.accounts_evicted = accounts_pre_evict.len().saturating_sub(self.accounts.len());
        stats.storage_evicted = storage_pre_evict.len().saturating_sub(self.storage.len());
        stats.codes_evicted = codes_pre_evict.len().saturating_sub(self.codes.len());

        self.undo_log.push_back(undo);

        debug!(
            target: "partial_stateless::cache",
            block = block_number,
            accounts_total = self.accounts.len(),
            storage_total = self.storage.len(),
            codes_total = self.codes.len(),
            ?stats,
            "Cache updated"
        );

        stats
    }

    /// Check if an account is in the cache.
    pub fn contains_account(&self, address: &Address) -> bool {
        self.accounts.contains_key(address)
    }

    /// Check if a storage slot is in the cache.
    pub fn contains_storage(&self, address: &Address, slot: &B256) -> bool {
        self.storage.contains_key(&(*address, *slot))
    }

    /// Check if a bytecode is in the cache.
    pub fn contains_code(&self, code_hash: &B256) -> bool {
        self.codes.contains_key(code_hash)
    }

    /// Get current cache snapshot (sizes).
    pub fn snapshot(&self) -> CacheSnapshot {
        CacheSnapshot {
            total_accounts: self.accounts.len(),
            total_storage_slots: self.storage.len(),
            total_codes: self.codes.len(),
            current_block: self.current_block,
        }
    }

    /// Compute which state from `accessed` is NOT in the cache (= needs witness).
    ///
    /// This represents what a builder would need to include in the witness sidecar.
    pub fn compute_miss(
        &self,
        accessed: &BlockAccessedState,
    ) -> MissResult {
        let mut missed_accounts: Vec<Address> = Vec::new();
        let mut missed_storage: Vec<(Address, B256)> = Vec::new();
        let mut missed_codes: Vec<B256> = Vec::new();

        for address in accessed.accounts.keys() {
            if !self.accounts.contains_key(address) {
                missed_accounts.push(*address);
            }
        }

        for (address, slot) in accessed.storage.keys() {
            if !self.storage.contains_key(&(*address, *slot)) {
                missed_storage.push((*address, *slot));
            }
        }

        for code_hash in accessed.codes.keys() {
            if !self.codes.contains_key(code_hash) {
                missed_codes.push(*code_hash);
            }
        }

        let total_accessed = accessed.total_keys();
        let total_missed = missed_accounts.len() + missed_storage.len() + missed_codes.len();
        let miss_ratio = if total_accessed > 0 {
            total_missed as f64 / total_accessed as f64
        } else {
            0.0
        };

        MissResult {
            missed_accounts,
            missed_storage,
            missed_codes,
            total_accessed,
            total_missed,
            miss_ratio,
        }
    }

    /// Get a reference to the accounts map (for persistence/inspection).
    pub fn accounts(&self) -> &HashMap<Address, CachedEntry<AccountData>> {
        &self.accounts
    }

    /// Get a reference to the storage map (for persistence/inspection).
    pub fn storage(&self) -> &HashMap<(Address, B256), CachedEntry<U256>> {
        &self.storage
    }

    /// Get a reference to the codes map (for persistence/inspection).
    pub fn codes(&self) -> &HashMap<B256, CachedEntry<Bytes>> {
        &self.codes
    }

    /// Current block number.
    pub fn current_block(&self) -> u64 {
        self.current_block
    }

    /// Estimated memory usage in bytes.
    pub fn estimated_memory_bytes(&self) -> usize {
        // Rough estimates:
        // Account entry: 20 (address) + 8 (nonce) + 32 (balance) + 32 (code_hash) + 20 (metadata) ≈ 112
        // Storage entry: 20 (address) + 32 (slot) + 32 (value) + 20 (metadata) ≈ 104
        // Code entry: 32 (hash) + avg ~8KB (bytecode) + 20 (metadata)
        let accounts_size = self.accounts.len() * 112;
        let storage_size = self.storage.len() * 104;
        let codes_size: usize = self.codes.values().map(|e| 52 + e.value.len()).sum();
        accounts_size + storage_size + codes_size
    }

    /// Roll back the most recently applied block, restoring the cache to its exact
    /// state before that block (including values overwritten by refresh and entries
    /// removed by eviction).
    ///
    /// `block_number` must equal the newest undo record (the most recently applied
    /// block); otherwise [`CacheError::RollbackMismatch`] is returned and the caller
    /// should cold-reset via [`reset`](Self::reset). Reorgs revert blocks newest→oldest,
    /// matching the undo stack order.
    pub fn rollback_block(&mut self, block_number: u64) -> Result<(), CacheError> {
        match self.undo_log.back() {
            Some(undo) if undo.block_number == block_number => {}
            other => {
                return Err(CacheError::RollbackMismatch {
                    requested: block_number,
                    found: other.map(|u| u.block_number),
                })
            }
        }

        let undo = self.undo_log.pop_back().expect("checked non-empty above");
        for (address, before) in undo.accounts_before {
            match before {
                Some(entry) => {
                    self.accounts.insert(address, entry);
                }
                None => {
                    self.accounts.remove(&address);
                }
            }
        }
        for (key, before) in undo.storage_before {
            match before {
                Some(entry) => {
                    self.storage.insert(key, entry);
                }
                None => {
                    self.storage.remove(&key);
                }
            }
        }
        for (code_hash, before) in undo.codes_before {
            match before {
                Some(entry) => {
                    self.codes.insert(code_hash, entry);
                }
                None => {
                    self.codes.remove(&code_hash);
                }
            }
        }
        self.current_block = undo.previous_block;
        Ok(())
    }

    /// Drop undo records at or below `finalized_block`. Reorgs never cross a finalized
    /// block, so these records can never be needed again — this bounds undo-log memory
    /// to the unfinalized window.
    pub fn prune_undo_below(&mut self, finalized_block: u64) {
        while let Some(front) = self.undo_log.front() {
            if front.block_number <= finalized_block {
                self.undo_log.pop_front();
            } else {
                break;
            }
        }
    }

    /// Clear the entire cache and undo history (cold reset). Used to recover when a
    /// reorg is deeper than the retained undo history and cannot be rolled back; the
    /// cache is then rebuilt from the new canonical chain.
    pub fn reset(&mut self) {
        self.accounts.clear();
        self.storage.clear();
        self.codes.clear();
        self.undo_log.clear();
        self.current_block = 0;
    }
}

/// Result of computing cache misses for a block.
#[derive(Debug, Clone)]
pub struct MissResult {
    /// Account addresses not in cache.
    pub missed_accounts: Vec<Address>,
    /// Storage slots not in cache.
    pub missed_storage: Vec<(Address, B256)>,
    /// Bytecodes not in cache.
    pub missed_codes: Vec<B256>,
    /// Total state keys accessed in the block.
    pub total_accessed: usize,
    /// Total state keys missed (not in cache).
    pub total_missed: usize,
    /// Miss ratio (0.0 = all cached, 1.0 = nothing cached).
    pub miss_ratio: f64,
}

impl MissResult {
    /// Log a summary of the miss result.
    pub fn log_summary(&self, block_number: u64) {
        info!(
            target: "partial_stateless::miss",
            block = block_number,
            total_accessed = self.total_accessed,
            total_missed = self.total_missed,
            miss_ratio = format!("{:.2}%", self.miss_ratio * 100.0),
            missed_accounts = self.missed_accounts.len(),
            missed_storage = self.missed_storage.len(),
            missed_codes = self.missed_codes.len(),
            "Witness requirement computed"
        );
    }
}

/// Undo record for a single applied block, enabling rollback on reorg.
///
/// Each `*_before` map stores the value of a touched or evicted key *before* the
/// block was applied: `Some(entry)` = the key existed (restore it on rollback),
/// `None` = the key was absent (remove it on rollback).
#[derive(Debug, Clone)]
struct BlockCacheUndo {
    /// The block this record can undo.
    block_number: u64,
    /// Cache `current_block` before this block was applied (restored on rollback).
    previous_block: u64,
    accounts_before: HashMap<Address, Option<CachedEntry<AccountData>>>,
    storage_before: HashMap<(Address, B256), Option<CachedEntry<U256>>>,
    codes_before: HashMap<B256, Option<CachedEntry<Bytes>>>,
}

impl BlockCacheUndo {
    fn new(block_number: u64, previous_block: u64) -> Self {
        Self {
            block_number,
            previous_block,
            accounts_before: HashMap::new(),
            storage_before: HashMap::new(),
            codes_before: HashMap::new(),
        }
    }
}

/// Errors returned by reorg-related cache operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheError {
    /// Rollback was requested for a block that is not the newest undo record.
    /// `found` is the newest retained undo block (or `None` if no history remains).
    RollbackMismatch { requested: u64, found: Option<u64> },
}

impl std::fmt::Display for CacheError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CacheError::RollbackMismatch { requested, found } => write!(
                f,
                "cache rollback mismatch: requested block {requested}, newest undo record is {found:?}"
            ),
        }
    }
}

impl std::error::Error for CacheError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::LastNBlocksPolicy;

    fn make_cache(account_window: u64, storage_window: u64) -> NetworkStateCache {
        NetworkStateCache::new(
            Box::new(LastNBlocksPolicy::new(account_window)),
            Box::new(LastNBlocksPolicy::new(storage_window)),
        )
    }

    #[test]
    fn test_basic_insert_and_lookup() {
        let mut cache = make_cache(10, 10);
        let addr = Address::repeat_byte(0x01);
        let slot = B256::repeat_byte(0x02);

        let mut accessed = BlockAccessedState::default();
        accessed.accounts.insert(
            addr,
            AccountData { nonce: 1, balance: U256::from(1000), code_hash: None },
        );
        accessed.storage.insert((addr, slot), U256::from(42));

        cache.on_block_executed(100, &accessed);

        assert!(cache.contains_account(&addr));
        assert!(cache.contains_storage(&addr, &slot));
        assert!(!cache.contains_account(&Address::repeat_byte(0xFF)));
    }

    #[test]
    fn test_eviction_after_window() {
        let mut cache = make_cache(5, 3);
        let addr = Address::repeat_byte(0x01);
        let slot = B256::repeat_byte(0x02);

        // Block 10: insert both
        let mut accessed = BlockAccessedState::default();
        accessed.accounts.insert(
            addr,
            AccountData { nonce: 1, balance: U256::from(100), code_hash: None },
        );
        accessed.storage.insert((addr, slot), U256::from(1));
        cache.on_block_executed(10, &accessed);

        // Block 13: storage window=3, cutoff=10, so block 10 entry is still at boundary
        cache.on_block_executed(13, &BlockAccessedState::default());
        assert!(cache.contains_account(&addr));
        assert!(cache.contains_storage(&addr, &slot));

        // Block 14: storage cutoff=11, evicts the slot (last accessed at 10)
        cache.on_block_executed(14, &BlockAccessedState::default());
        assert!(cache.contains_account(&addr)); // account window=5, cutoff=9
        assert!(!cache.contains_storage(&addr, &slot)); // evicted!

        // Block 16: account cutoff=11, evicts the account
        cache.on_block_executed(16, &BlockAccessedState::default());
        assert!(!cache.contains_account(&addr));
    }

    #[test]
    fn test_miss_computation() {
        let mut cache = make_cache(10, 10);
        let addr_cached = Address::repeat_byte(0x01);
        let addr_missed = Address::repeat_byte(0x02);
        let slot_cached = B256::repeat_byte(0xAA);
        let slot_missed = B256::repeat_byte(0xBB);

        // Pre-populate cache
        let mut pre = BlockAccessedState::default();
        pre.accounts.insert(
            addr_cached,
            AccountData { nonce: 1, balance: U256::from(100), code_hash: None },
        );
        pre.storage.insert((addr_cached, slot_cached), U256::from(1));
        cache.on_block_executed(100, &pre);

        // New block accesses both cached and uncached state
        let mut new_block = BlockAccessedState::default();
        new_block.accounts.insert(
            addr_cached,
            AccountData { nonce: 1, balance: U256::from(100), code_hash: None },
        );
        new_block.accounts.insert(
            addr_missed,
            AccountData { nonce: 0, balance: U256::ZERO, code_hash: None },
        );
        new_block.storage.insert((addr_cached, slot_cached), U256::from(1));
        new_block.storage.insert((addr_cached, slot_missed), U256::from(2));

        let miss = cache.compute_miss(&new_block);

        // addr_cached is in cache, addr_missed is not
        assert_eq!(miss.missed_accounts.len(), 1);
        assert_eq!(miss.missed_accounts[0], addr_missed);

        // slot_cached is in cache, slot_missed is not
        assert_eq!(miss.missed_storage.len(), 1);
        assert_eq!(miss.missed_storage[0], (addr_cached, slot_missed));

        // 4 total keys, 2 missed
        assert_eq!(miss.total_accessed, 4);
        assert_eq!(miss.total_missed, 2);
        assert!((miss.miss_ratio - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_refresh_extends_lifetime() {
        let mut cache = make_cache(10, 5);
        let addr = Address::repeat_byte(0x01);
        let slot = B256::repeat_byte(0x02);

        // Block 10: insert storage
        let mut accessed = BlockAccessedState::default();
        accessed.storage.insert((addr, slot), U256::from(1));
        cache.on_block_executed(10, &accessed);

        // Block 14: re-access the same slot (refreshes last_accessed_block to 14)
        let mut accessed2 = BlockAccessedState::default();
        accessed2.storage.insert((addr, slot), U256::from(2));
        cache.on_block_executed(14, &accessed2);

        // Block 18: storage cutoff=13. Since last_accessed=14, it should be retained.
        cache.on_block_executed(18, &BlockAccessedState::default());
        assert!(cache.contains_storage(&addr, &slot));

        // Block 20: storage cutoff=15. Since last_accessed=14, now evicted.
        cache.on_block_executed(20, &BlockAccessedState::default());
        assert!(!cache.contains_storage(&addr, &slot));
    }

    fn account(nonce: u64, balance: u64) -> AccountData {
        AccountData { nonce, balance: U256::from(balance), code_hash: None }
    }

    #[test]
    fn test_rollback_removes_newly_inserted() {
        let mut cache = make_cache(10, 10);
        let addr = Address::repeat_byte(0x01);
        let mut accessed = BlockAccessedState::default();
        accessed.accounts.insert(addr, account(1, 1));
        cache.on_block_executed(100, &accessed);
        assert!(cache.contains_account(&addr));

        cache.rollback_block(100).unwrap();
        assert!(!cache.contains_account(&addr));
        assert_eq!(cache.current_block(), 0);
    }

    #[test]
    fn test_rollback_restores_refreshed_value() {
        let mut cache = make_cache(10, 10);
        let addr = Address::repeat_byte(0x01);

        let mut b1 = BlockAccessedState::default();
        b1.accounts.insert(addr, account(1, 100));
        cache.on_block_executed(10, &b1);

        let mut b2 = BlockAccessedState::default();
        b2.accounts.insert(addr, account(2, 200));
        cache.on_block_executed(11, &b2);

        cache.rollback_block(11).unwrap();
        let entry = cache.accounts().get(&addr).expect("account still cached");
        assert_eq!(entry.value.nonce, 1);
        assert_eq!(entry.value.balance, U256::from(100));
        assert_eq!(entry.last_accessed_block, 10);
        assert_eq!(cache.current_block(), 10);
    }

    #[test]
    fn test_rollback_restores_evicted_entry() {
        // account window = 3: entry inserted at block 10 is evicted by block 14 (cutoff 11).
        let mut cache = make_cache(3, 3);
        let addr = Address::repeat_byte(0x01);
        let mut b = BlockAccessedState::default();
        b.accounts.insert(addr, account(1, 1));
        cache.on_block_executed(10, &b);

        cache.on_block_executed(14, &BlockAccessedState::default());
        assert!(!cache.contains_account(&addr), "should be evicted at block 14");

        cache.rollback_block(14).unwrap();
        assert!(cache.contains_account(&addr), "rollback must restore the evicted entry");
    }

    #[test]
    fn test_rollback_matches_prior_state_invariant() {
        let mut cache = make_cache(10, 10);
        let a1 = Address::repeat_byte(0x01);
        let a2 = Address::repeat_byte(0x02);

        let mut b1 = BlockAccessedState::default();
        b1.accounts.insert(a1, account(1, 1));
        cache.on_block_executed(10, &b1);
        let snap_after_10 = cache.snapshot();

        let mut b2 = BlockAccessedState::default();
        b2.accounts.insert(a2, account(1, 2));
        cache.on_block_executed(11, &b2);

        cache.rollback_block(11).unwrap();
        let rolled_back = cache.snapshot();
        assert_eq!(rolled_back.total_accounts, snap_after_10.total_accounts);
        assert_eq!(rolled_back.current_block, snap_after_10.current_block);
        assert!(cache.contains_account(&a1));
        assert!(!cache.contains_account(&a2));
    }

    #[test]
    fn test_rollback_mismatch_is_rejected() {
        let mut cache = make_cache(10, 10);
        let mut b = BlockAccessedState::default();
        b.accounts.insert(Address::repeat_byte(0x01), account(1, 1));
        cache.on_block_executed(10, &b);
        // Newest undo is block 10; requesting any other block must fail.
        assert!(cache.rollback_block(9).is_err());
        assert!(cache.rollback_block(11).is_err());
    }

    #[test]
    fn test_prune_below_finalized_then_rollback_rejected() {
        let mut cache = make_cache(10, 10);
        let mut b = BlockAccessedState::default();
        b.accounts.insert(Address::repeat_byte(0x01), account(1, 1));
        cache.on_block_executed(10, &b);

        cache.prune_undo_below(10);
        // Undo for block 10 was pruned (finalized) → it can no longer be rolled back.
        assert!(cache.rollback_block(10).is_err());
    }

    #[test]
    fn test_reset_clears_everything() {
        let mut cache = make_cache(10, 10);
        let mut b = BlockAccessedState::default();
        b.accounts.insert(Address::repeat_byte(0x01), account(1, 1));
        b.storage.insert((Address::repeat_byte(0x01), B256::repeat_byte(0x02)), U256::from(5));
        cache.on_block_executed(10, &b);

        cache.reset();
        assert_eq!(cache.snapshot().total_accounts, 0);
        assert_eq!(cache.snapshot().total_storage_slots, 0);
        assert_eq!(cache.current_block(), 0);
    }
}
