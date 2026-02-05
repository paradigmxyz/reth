//! Parallel write methods for edge mode.
//!
//! This module provides fine-grained parallel write operations for reth's edge mode,
//! where each method writes to exactly ONE database table (DBI). This allows for
//! maximum parallelism when using subtxns.

use crate::{providers::NodeTypesForProvider, DatabaseProvider};
use alloy_primitives::{Address, B256};
use rayon::slice::ParallelSliceMut;
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_primitives_traits::{Account, Bytecode, StorageEntry};
use reth_storage_errors::provider::ProviderResult;
use reth_trie::{
    updates::StorageTrieUpdatesSorted, BranchNodeCompact, HashedStorageSorted, Nibbles,
    StoredNibbles,
};
use reth_trie_db::DatabaseStorageTrieCursor;
use revm_database::states::PlainStorageChangeset;
use revm_state::AccountInfo;
use std::time::{Duration, Instant};

/// Preprocessed data ready for parallel state writes.
/// All data is pre-sorted and storage entries are pre-converted.
#[derive(Debug, Default)]
pub struct PreparedStateWrites {
    /// Sorted account updates. `None` indicates account deletion.
    /// The inner type uses the reth Account type for direct DB writes.
    pub accounts: Vec<(Address, Option<Account>)>,
    /// Sorted contract bytecode updates (reth Bytecode wrapper type).
    pub contracts: Vec<(B256, Bytecode)>,
    /// Preprocessed storage writes with pre-sorted entries.
    pub storage: Vec<PreparedStorageWrite>,
}

/// Preprocessed storage write for a single address.
#[derive(Debug)]
pub struct PreparedStorageWrite {
    /// The address this storage belongs to.
    pub address: Address,
    /// Whether to wipe all existing storage for this address.
    pub wipe_storage: bool,
    /// Pre-sorted storage entries to write.
    pub storage: Vec<StorageEntry>,
}

/// Timings for individual parallel write operations.
#[derive(Debug, Default, Clone)]
pub struct ParallelWriteTimings {
    /// Duration of `PlainAccountState` writes.
    pub plain_accounts: Duration,
    /// Duration of `Bytecodes` writes.
    pub bytecodes: Duration,
    /// Duration of `PlainStorageState` writes.
    pub plain_storage: Duration,
    /// Duration of `HashedAccounts` writes.
    pub hashed_accounts: Duration,
    /// Duration of `HashedStorages` writes.
    pub hashed_storages: Duration,
    /// Duration of `AccountsTrie` writes.
    pub account_trie: Duration,
    /// Duration of `StoragesTrie` writes.
    pub storage_trie: Duration,
}

/// Hints for per-DBI arena allocation during parallel writes.
/// Source of the arena hint value after applying floor/cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ArenaHintSource {
    /// Raw estimate was used (no floor/cap applied)
    #[default]
    Estimated,
    /// Floor was applied (estimate was below minimum)
    Floored,
    /// Cap was applied (estimate exceeded maximum)
    Capped,
}

impl ArenaHintSource {
    /// Returns the source as a static string for metrics labels.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Estimated => "estimated",
            Self::Floored => "floored",
            Self::Capped => "capped",
        }
    }

    /// Returns numeric representation for gauge (0=estimated, 1=floored, 2=capped)
    pub const fn as_f64(&self) -> f64 {
        match self {
            Self::Estimated => 0.0,
            Self::Floored => 1.0,
            Self::Capped => 2.0,
        }
    }
}

/// Details about a single arena hint calculation.
#[derive(Debug, Clone, Copy, Default)]
pub struct ArenaHintDetail {
    /// Raw calculated estimate before floor/cap
    pub estimated: usize,
    /// Actual hint after applying floor/cap
    pub used: usize,
    /// Whether the value was estimated, floored, or capped
    pub source: ArenaHintSource,
}

impl ArenaHintDetail {
    /// Converts to the db-api's `ArenaHintEstimationStats` for metric recording.
    pub fn to_estimation_stats(&self) -> reth_db_api::transaction::ArenaHintEstimationStats {
        use reth_db_api::transaction::ArenaHintSource as DbApiSource;
        reth_db_api::transaction::ArenaHintEstimationStats {
            estimated: self.estimated,
            actual: self.used,
            source: match self.source {
                ArenaHintSource::Estimated => DbApiSource::Estimated,
                ArenaHintSource::Floored => DbApiSource::Floored,
                ArenaHintSource::Capped => DbApiSource::Capped,
            },
        }
    }
}

/// These hints inform the MDBX layer how to distribute freelist pages
/// proportionally among subtransactions.
#[derive(Debug, Default, Clone)]
pub struct ArenaHints {
    /// Estimated pages for PlainAccountState
    pub plain_accounts: usize,
    /// Estimated pages for Bytecodes
    pub bytecodes: usize,
    /// Estimated pages for PlainStorageState
    pub plain_storage: usize,
    /// Estimated pages for HashedAccounts
    pub hashed_accounts: usize,
    /// Estimated pages for HashedStorages
    pub hashed_storages: usize,
    /// Estimated pages for AccountsTrie
    pub account_trie: usize,
    /// Estimated pages for StoragesTrie
    pub storage_trie: usize,
    /// Per-table estimation details for metrics
    pub details: ArenaHintDetails,
}

/// Per-table arena hint estimation details.
#[derive(Debug, Default, Clone, Copy)]
pub struct ArenaHintDetails {
    /// PlainAccountState hint details
    pub plain_accounts: ArenaHintDetail,
    /// Bytecodes hint details
    pub bytecodes: ArenaHintDetail,
    /// PlainStorageState hint details
    pub plain_storage: ArenaHintDetail,
    /// HashedAccounts hint details
    pub hashed_accounts: ArenaHintDetail,
    /// HashedStorages hint details
    pub hashed_storages: ArenaHintDetail,
    /// AccountsTrie hint details
    pub account_trie: ArenaHintDetail,
    /// StoragesTrie hint details
    pub storage_trie: ArenaHintDetail,
}

impl ArenaHints {
    /// Maximum pages per arena to prevent memory bloat.
    ///
    /// Updated based on actual usage metrics (dev-joshie, 33 batches):
    /// - StoragesTrie: ~6,843 pages/batch
    /// - AccountsTrie: ~6,155 pages/batch
    /// - HashedStorages: ~3,166 pages/batch
    /// - PlainStorageState: ~2,925 pages/batch
    /// - HashedAccounts: ~2,718 pages/batch
    /// - PlainAccountState: ~2,502 pages/batch
    ///
    /// We set max to 7000 to accommodate the heaviest tables (trie tables)
    /// while still preventing unbounded memory growth.
    pub const MAX_ARENA_PAGES: usize = 7000;
    /// Minimum arena size
    pub const MIN_ARENA_PAGES: usize = 8;
    /// Default minimum for bytecodes (low usage, typically 0)
    pub const DEFAULT_BYTECODES_PAGES: usize = 12;

    /// Default hints based on observed production usage patterns.
    ///
    /// These values target ~80% of average batch demand to balance between
    /// over-allocation and fallback frequency. Derived from metrics showing
    /// (dev-joshie, 5 min window, ~33 batches):
    /// - StoragesTrie: ~6,843 pages/batch → 5500 (80%)
    /// - AccountsTrie: ~6,155 pages/batch → 5000 (81%)
    /// - HashedStorages: ~3,166 pages/batch → 2600 (82%)
    /// - PlainStorageState: ~2,925 pages/batch → 2400 (82%)
    /// - HashedAccounts: ~2,718 pages/batch → 2200 (81%)
    /// - PlainAccountState: ~2,502 pages/batch → 2000 (80%)
    /// - Bytecodes: 0 pages (fine as-is)
    pub const fn default_hints() -> Self {
        Self {
            plain_accounts: 2000,
            bytecodes: Self::DEFAULT_BYTECODES_PAGES,
            plain_storage: 2400,
            hashed_accounts: 2200,
            hashed_storages: 2600,
            account_trie: 5000,
            storage_trie: 5500,
            details: ArenaHintDetails {
                plain_accounts: ArenaHintDetail {
                    estimated: 2000,
                    used: 2000,
                    source: ArenaHintSource::Estimated,
                },
                bytecodes: ArenaHintDetail {
                    estimated: Self::DEFAULT_BYTECODES_PAGES,
                    used: Self::DEFAULT_BYTECODES_PAGES,
                    source: ArenaHintSource::Estimated,
                },
                plain_storage: ArenaHintDetail {
                    estimated: 2400,
                    used: 2400,
                    source: ArenaHintSource::Estimated,
                },
                hashed_accounts: ArenaHintDetail {
                    estimated: 2200,
                    used: 2200,
                    source: ArenaHintSource::Estimated,
                },
                hashed_storages: ArenaHintDetail {
                    estimated: 2600,
                    used: 2600,
                    source: ArenaHintSource::Estimated,
                },
                account_trie: ArenaHintDetail {
                    estimated: 5000,
                    used: 5000,
                    source: ArenaHintSource::Estimated,
                },
                storage_trie: ArenaHintDetail {
                    estimated: 5500,
                    used: 5500,
                    source: ArenaHintSource::Estimated,
                },
            },
        }
    }

    /// Estimate arena sizes from state data.
    ///
    /// The estimation uses fractional pages-per-entry ratios based on observed production data.
    /// Each table has different page requirements due to B-tree structure, key sizes, and
    /// value sizes.
    ///
    /// # Estimation Strategy
    ///
    /// For data tables (PlainAccountState, HashedAccounts, PlainStorageState, HashedStorages):
    /// - Multiple entries typically fit per 4KB page
    /// - We use `entries / entries_per_page` with a 2x safety factor
    ///
    /// For trie tables (AccountsTrie, StoragesTrie):
    /// - Each node update can cause page splits and parent updates up the tree
    /// - Tree depth is typically 4-8 levels, so one logical update can touch multiple pages
    /// - We use `nodes * pages_per_node` to account for this amplification
    ///
    /// # Arguments
    /// * `num_accounts` - Number of account changes
    /// * `num_storage` - Number of storage slot changes
    /// * `num_contracts` - Number of new contracts
    /// * `num_account_trie_nodes` - Number of account trie node updates
    /// * `num_storage_trie_nodes` - Number of storage trie node updates
    pub fn estimate(
        num_accounts: usize,
        num_storage: usize,
        num_contracts: usize,
        num_account_trie_nodes: usize,
        num_storage_trie_nodes: usize,
    ) -> Self {
        let default = Self::default_hints();

        // Data tables: multiple entries fit per page
        // Account record ~80 bytes, ~50 per 4KB page → use 25 with 2x safety
        let plain_accounts_detail = Self::calc_with_floor(num_accounts, 25, default.plain_accounts);
        // Bytecode entries are large but rare, use minimal estimate
        let bytecodes_detail =
            Self::calc_with_floor(num_contracts, 1, Self::DEFAULT_BYTECODES_PAGES);
        // Storage entry ~64 bytes, ~60 per page → use 30 with 2x safety
        let plain_storage_detail = Self::calc_with_floor(num_storage, 30, default.plain_storage);
        let hashed_accounts_detail =
            Self::calc_with_floor(num_accounts, 25, default.hashed_accounts);
        let hashed_storages_detail =
            Self::calc_with_floor(num_storage, 30, default.hashed_storages);

        // Trie tables: use multiplication-based estimation
        // Each trie node update can touch multiple pages due to tree structure
        let account_trie_detail =
            Self::calc_trie_with_floor(num_account_trie_nodes, 3, default.account_trie);
        let storage_trie_detail =
            Self::calc_trie_with_floor(num_storage_trie_nodes, 4, default.storage_trie);

        Self {
            plain_accounts: plain_accounts_detail.used,
            bytecodes: bytecodes_detail.used,
            plain_storage: plain_storage_detail.used,
            hashed_accounts: hashed_accounts_detail.used,
            hashed_storages: hashed_storages_detail.used,
            account_trie: account_trie_detail.used,
            storage_trie: storage_trie_detail.used,
            details: ArenaHintDetails {
                plain_accounts: plain_accounts_detail,
                bytecodes: bytecodes_detail,
                plain_storage: plain_storage_detail,
                hashed_accounts: hashed_accounts_detail,
                hashed_storages: hashed_storages_detail,
                account_trie: account_trie_detail,
                storage_trie: storage_trie_detail,
            },
        }
    }

    /// Calculate arena hint for data tables where multiple entries fit per page.
    ///
    /// Formula: `(entries / entries_per_page) * 2 + 8`
    /// - Division gives base page count
    /// - 2x multiplier for B-tree overhead and page splits
    /// - +8 for minimum working set
    fn calc_with_floor(entries: usize, entries_per_page: usize, floor: usize) -> ArenaHintDetail {
        let raw_estimate = if entries == 0 {
            Self::MIN_ARENA_PAGES
        } else {
            let base = entries.div_ceil(entries_per_page);
            base.saturating_mul(2).saturating_add(8)
        };

        Self::apply_bounds(raw_estimate, floor)
    }

    /// Calculate arena hint for trie tables where each node may require multiple pages.
    ///
    /// Formula: `nodes * pages_per_node + 8`
    /// - Multiplication accounts for tree depth and node splits
    /// - +8 for minimum working set
    ///
    /// Trie updates are expensive because:
    /// 1. Each node update may split into multiple nodes
    /// 2. Updates propagate up to parent nodes (tree depth typically 4-8)
    /// 3. StoragesTrie is a dupsort table with per-account subtries
    fn calc_trie_with_floor(nodes: usize, pages_per_node: usize, floor: usize) -> ArenaHintDetail {
        let raw_estimate = if nodes == 0 {
            Self::MIN_ARENA_PAGES
        } else {
            nodes.saturating_mul(pages_per_node).saturating_add(8)
        };

        Self::apply_bounds(raw_estimate, floor)
    }

    /// Apply floor and cap bounds to a raw estimate.
    fn apply_bounds(raw_estimate: usize, floor: usize) -> ArenaHintDetail {
        let (used, source) = if raw_estimate < floor {
            (floor, ArenaHintSource::Floored)
        } else if raw_estimate > Self::MAX_ARENA_PAGES {
            (Self::MAX_ARENA_PAGES, ArenaHintSource::Capped)
        } else {
            (raw_estimate, ArenaHintSource::Estimated)
        };

        ArenaHintDetail { estimated: raw_estimate, used, source }
    }

    /// Returns total estimated pages across all DBIs
    pub fn total(&self) -> usize {
        self.plain_accounts +
            self.bytecodes +
            self.plain_storage +
            self.hashed_accounts +
            self.hashed_storages +
            self.account_trie +
            self.storage_trie
    }
}

impl<TX: DbTxMut + DbTx + 'static, N: NodeTypesForProvider> DatabaseProvider<TX, N> {
    /// Preprocesses state data for parallel writes.
    ///
    /// This is called BEFORE spawning subtxns to:
    /// 1. Sort all data for efficient sequential writes
    /// 2. Pre-convert storage entries to the final format
    /// 3. Convert account types from revm to reth format
    ///
    /// The resulting [`PreparedStateWrites`] can be safely shared across threads.
    ///
    /// # Type Parameters
    /// * `A` - Account type that can be converted to reth Account (e.g., revm `AccountInfo`)
    /// # Arguments
    /// * `accounts` - Account updates from `StateChangeset` (revm `AccountInfo`)
    /// * `contracts` - Contract bytecode updates from `StateChangeset` (revm `Bytecode`)
    /// * `storage` - Storage updates per address
    pub fn prepare_state_writes_from_parts(
        &self,
        mut accounts: Vec<(Address, Option<AccountInfo>)>,
        mut contracts: Vec<(B256, revm_state::Bytecode)>,
        mut storage: Vec<PlainStorageChangeset>,
    ) -> PreparedStateWrites {
        // Sort all entries for more performant sequential writes
        accounts.par_sort_by_key(|a| a.0);
        contracts.par_sort_by_key(|a| a.0);
        storage.par_sort_by_key(|a| a.address);

        // Convert accounts from revm AccountInfo to reth Account type
        let accounts: Vec<(Address, Option<Account>)> = accounts
            .into_iter()
            .map(|(addr, acc): (Address, Option<AccountInfo>)| (addr, acc.map(Into::into)))
            .collect();

        // Convert contracts from revm Bytecode to reth Bytecode type
        let contracts: Vec<(B256, Bytecode)> =
            contracts.into_iter().map(|(hash, bc)| (hash, Bytecode(bc))).collect();

        // Prepare storage writes with pre-sorted entries
        let storage = storage
            .into_iter()
            .map(|PlainStorageChangeset { address, wipe_storage, storage }| {
                let mut entries: Vec<StorageEntry> = storage
                    .into_iter()
                    .map(|(k, value)| StorageEntry { key: k.into(), value })
                    .collect();
                entries.par_sort_unstable_by_key(|e| e.key);
                PreparedStorageWrite { address, wipe_storage, storage: entries }
            })
            .collect();

        PreparedStateWrites { accounts, contracts, storage }
    }

    /// Writes only `PlainAccountState` table.
    ///
    /// This method is designed for use in a subtxn, writing to exactly ONE DBI.
    /// Returns the duration of the write operation.
    pub fn write_plain_accounts_only(
        &self,
        accounts: &[(Address, Option<Account>)],
    ) -> ProviderResult<Duration> {
        let start = Instant::now();

        let mut cursor = self.tx_ref().cursor_write::<tables::PlainAccountState>()?;
        for (address, account) in accounts {
            if let Some(account) = account {
                cursor.upsert(*address, account)?;
            } else if cursor.seek_exact(*address)?.is_some() {
                cursor.delete_current()?;
            }
        }

        Ok(start.elapsed())
    }

    /// Writes only `Bytecodes` table.
    ///
    /// This method is designed for use in a subtxn, writing to exactly ONE DBI.
    /// Returns the duration of the write operation.
    pub fn write_bytecodes_only(&self, contracts: &[(B256, Bytecode)]) -> ProviderResult<Duration> {
        let start = Instant::now();

        let mut cursor = self.tx_ref().cursor_write::<tables::Bytecodes>()?;
        for (hash, bytecode) in contracts {
            cursor.upsert(*hash, bytecode)?;
        }

        Ok(start.elapsed())
    }

    /// Writes only `PlainStorageState` table.
    ///
    /// This method is designed for use in a subtxn, writing to exactly ONE DBI.
    /// Returns the duration of the write operation.
    pub fn write_plain_storage_only(
        &self,
        storage: &[PreparedStorageWrite],
    ) -> ProviderResult<Duration> {
        let start = Instant::now();

        let mut cursor = self.tx_ref().cursor_dup_write::<tables::PlainStorageState>()?;
        for PreparedStorageWrite { address, wipe_storage, storage } in storage {
            // Wipe storage if flagged
            if *wipe_storage && cursor.seek_exact(*address)?.is_some() {
                cursor.delete_current_duplicates()?;
            }

            for entry in storage {
                if let Some(db_entry) = cursor.seek_by_key_subkey(*address, entry.key)? &&
                    db_entry.key == entry.key
                {
                    cursor.delete_current()?;
                }

                if !entry.value.is_zero() {
                    cursor.upsert(*address, entry)?;
                }
            }
        }

        Ok(start.elapsed())
    }

    /// Writes only `HashedAccounts` table.
    ///
    /// This method is designed for use in a subtxn, writing to exactly ONE DBI.
    /// Expects accounts to be sorted by hashed address.
    /// Returns the duration of the write operation.
    pub fn write_hashed_accounts_only(
        &self,
        accounts: &[(B256, Option<Account>)],
    ) -> ProviderResult<Duration> {
        let start = Instant::now();

        let mut cursor = self.tx_ref().cursor_write::<tables::HashedAccounts>()?;
        for (hashed_address, account) in accounts {
            if let Some(account) = account {
                cursor.upsert(*hashed_address, account)?;
            } else if cursor.seek_exact(*hashed_address)?.is_some() {
                cursor.delete_current()?;
            }
        }

        Ok(start.elapsed())
    }

    /// Writes only `HashedStorages` table.
    ///
    /// This method is designed for use in a subtxn, writing to exactly ONE DBI.
    /// Expects storages to be sorted by hashed address.
    /// Returns the duration of the write operation.
    pub fn write_hashed_storages_only(
        &self,
        storages: &[(B256, &HashedStorageSorted)],
    ) -> ProviderResult<Duration> {
        let start = Instant::now();

        let mut cursor = self.tx_ref().cursor_dup_write::<tables::HashedStorages>()?;
        for (hashed_address, storage) in storages {
            // Wipe storage if flagged
            if storage.is_wiped() && cursor.seek_exact(*hashed_address)?.is_some() {
                cursor.delete_current_duplicates()?;
            }

            for (hashed_slot, value) in storage.storage_slots_ref() {
                let entry = StorageEntry { key: *hashed_slot, value: *value };

                if let Some(db_entry) = cursor.seek_by_key_subkey(*hashed_address, entry.key)? &&
                    db_entry.key == entry.key
                {
                    cursor.delete_current()?;
                }

                if !entry.value.is_zero() {
                    cursor.upsert(*hashed_address, &entry)?;
                }
            }
        }

        Ok(start.elapsed())
    }

    /// Writes only `AccountsTrie` table.
    ///
    /// This method is designed for use in a subtxn, writing to exactly ONE DBI.
    /// Expects account nodes to be sorted by nibbles.
    /// Returns a tuple of (entries modified, duration).
    pub fn write_account_trie_only(
        &self,
        nodes: &[(Nibbles, Option<BranchNodeCompact>)],
    ) -> ProviderResult<(usize, Duration)> {
        let start = Instant::now();
        let mut num_entries = 0;

        let mut cursor = self.tx_ref().cursor_write::<tables::AccountsTrie>()?;
        for (key, updated_node) in nodes {
            let nibbles = StoredNibbles(*key);
            match updated_node {
                Some(node) => {
                    if !nibbles.0.is_empty() {
                        num_entries += 1;
                        cursor.upsert(nibbles, node)?;
                    }
                }
                None => {
                    num_entries += 1;
                    if cursor.seek_exact(nibbles)?.is_some() {
                        cursor.delete_current()?;
                    }
                }
            }
        }

        Ok((num_entries, start.elapsed()))
    }

    /// Writes only `StoragesTrie` table.
    ///
    /// This method is designed for use in a subtxn, writing to exactly ONE DBI.
    /// Expects storage tries to be sorted by hashed address.
    /// Returns a tuple of (entries modified, duration).
    pub fn write_storage_trie_only(
        &self,
        tries: &[(B256, &StorageTrieUpdatesSorted)],
    ) -> ProviderResult<(usize, Duration)> {
        let start = Instant::now();
        let mut num_entries = 0;

        let mut cursor = self.tx_ref().cursor_dup_write::<tables::StoragesTrie>()?;
        for (hashed_address, storage_trie_updates) in tries {
            let mut db_storage_trie_cursor =
                DatabaseStorageTrieCursor::new(cursor, *hashed_address);
            num_entries +=
                db_storage_trie_cursor.write_storage_trie_updates_sorted(storage_trie_updates)?;
            cursor = db_storage_trie_cursor.cursor;
        }

        Ok((num_entries, start.elapsed()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::U256;

    #[test]
    fn test_prepared_state_writes_default() {
        let prepared = PreparedStateWrites::default();
        assert!(prepared.accounts.is_empty());
        assert!(prepared.contracts.is_empty());
        assert!(prepared.storage.is_empty());
    }

    #[test]
    fn test_parallel_write_timings_default() {
        let timings = ParallelWriteTimings::default();
        assert_eq!(timings.plain_accounts, Duration::ZERO);
        assert_eq!(timings.bytecodes, Duration::ZERO);
        assert_eq!(timings.plain_storage, Duration::ZERO);
        assert_eq!(timings.hashed_accounts, Duration::ZERO);
        assert_eq!(timings.hashed_storages, Duration::ZERO);
        assert_eq!(timings.account_trie, Duration::ZERO);
        assert_eq!(timings.storage_trie, Duration::ZERO);
    }

    #[test]
    fn test_prepared_storage_write() {
        let storage_write = PreparedStorageWrite {
            address: Address::ZERO,
            wipe_storage: true,
            storage: vec![StorageEntry { key: B256::ZERO, value: U256::from(42) }],
        };
        assert_eq!(storage_write.address, Address::ZERO);
        assert!(storage_write.wipe_storage);
        assert_eq!(storage_write.storage.len(), 1);
    }

    #[test]
    fn test_arena_hints_estimate_trie_scaling() {
        // Trie tables should scale with multiplication, not division
        // With 1400 storage trie nodes at 4 pages/node, expect 1400*4+8 = 5608 pages
        // This exceeds floor(5500) and is under MAX(6000), so we get Estimated
        let hints = ArenaHints::estimate(0, 0, 0, 0, 1400);
        assert_eq!(hints.details.storage_trie.estimated, 5608);
        assert_eq!(hints.storage_trie, 5608);
        assert_eq!(hints.details.storage_trie.source, ArenaHintSource::Estimated);

        // With 2000 nodes, expect ~8008 pages, but capped at MAX
        let hints = ArenaHints::estimate(0, 0, 0, 0, 2000);
        assert_eq!(hints.details.storage_trie.estimated, 8008);
        assert_eq!(hints.storage_trie, ArenaHints::MAX_ARENA_PAGES);
        assert_eq!(hints.details.storage_trie.source, ArenaHintSource::Capped);

        // With 1000 nodes: 1000*4+8 = 4008 < floor(5500), so floored
        let hints = ArenaHints::estimate(0, 0, 0, 0, 1000);
        assert_eq!(hints.details.storage_trie.estimated, 4008);
        assert_eq!(hints.storage_trie, 5500);
        assert_eq!(hints.details.storage_trie.source, ArenaHintSource::Floored);
    }

    #[test]
    fn test_arena_hints_estimate_data_tables() {
        // Data tables use division: 1000 storage / 30 entries_per_page = 34, * 2 + 8 = 76
        let hints = ArenaHints::estimate(0, 1000, 0, 0, 0);
        assert_eq!(hints.details.hashed_storages.estimated, 76);
        // But 76 < floor(2600), so floored
        assert_eq!(hints.hashed_storages, 2600);
        assert_eq!(hints.details.hashed_storages.source, ArenaHintSource::Floored);

        // Large storage count: 50000 / 30 = 1667, * 2 + 8 = 3342
        let hints = ArenaHints::estimate(0, 50000, 0, 0, 0);
        assert_eq!(hints.details.hashed_storages.estimated, 3342);
        assert_eq!(hints.hashed_storages, 3342);
        assert_eq!(hints.details.hashed_storages.source, ArenaHintSource::Estimated);
    }

    #[test]
    fn test_arena_hints_zero_entries() {
        let hints = ArenaHints::estimate(0, 0, 0, 0, 0);
        // Zero entries should use MIN_ARENA_PAGES for estimate, but floor applies
        assert_eq!(hints.details.storage_trie.estimated, ArenaHints::MIN_ARENA_PAGES);
        assert_eq!(hints.storage_trie, 5500); // floor from default_hints
    }
}
