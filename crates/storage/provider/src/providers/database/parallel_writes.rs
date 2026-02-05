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
use reth_trie_db::{DatabaseStorageTrieCursor, StorageTrieOpCounts};
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
/// Source of the arena hint value after applying floor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ArenaHintSource {
    /// Raw estimate was used (no floor applied)
    #[default]
    Estimated,
    /// Floor was applied (estimate was below minimum)
    Floored,
}

impl ArenaHintSource {
    /// Returns the source as a static string for metrics labels.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Estimated => "estimated",
            Self::Floored => "floored",
        }
    }

    /// Returns numeric representation for gauge (0=estimated, 1=floored)
    pub const fn as_f64(&self) -> f64 {
        match self {
            Self::Estimated => 0.0,
            Self::Floored => 1.0,
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
    /// Minimum arena size
    pub const MIN_ARENA_PAGES: usize = 8;
    /// Default minimum for bytecodes (low usage, typically 0)
    pub const DEFAULT_BYTECODES_PAGES: usize = 12;

    /// Default hints based on observed production usage patterns.
    ///
    /// These values target ~95% of observed batch demand to minimize
    /// fallback frequency. Derived from metrics:
    /// - StoragesTrie: 9,244 pages/batch → 8,800 (95%)
    /// - AccountsTrie: 7,656 pages/batch → 7,300 (95%)
    /// - HashedStorages: 4,200 pages/batch → 4,000 (95%)
    /// - PlainStorageState: 3,776 pages/batch → 3,600 (95%)
    /// - HashedAccounts: 3,544 pages/batch → 3,400 (95%)
    /// - PlainAccountState: 3,120 pages/batch → 3,000 (95%)
    /// - Bytecodes: 0 pages (fine as-is)
    pub const fn default_hints() -> Self {
        Self {
            plain_accounts: 3000,
            bytecodes: Self::DEFAULT_BYTECODES_PAGES,
            plain_storage: 3600,
            hashed_accounts: 3400,
            hashed_storages: 4000,
            account_trie: 7300,
            storage_trie: 8800,
            details: ArenaHintDetails {
                plain_accounts: ArenaHintDetail {
                    estimated: 3000,
                    used: 3000,
                    source: ArenaHintSource::Estimated,
                },
                bytecodes: ArenaHintDetail {
                    estimated: Self::DEFAULT_BYTECODES_PAGES,
                    used: Self::DEFAULT_BYTECODES_PAGES,
                    source: ArenaHintSource::Estimated,
                },
                plain_storage: ArenaHintDetail {
                    estimated: 3600,
                    used: 3600,
                    source: ArenaHintSource::Estimated,
                },
                hashed_accounts: ArenaHintDetail {
                    estimated: 3400,
                    used: 3400,
                    source: ArenaHintSource::Estimated,
                },
                hashed_storages: ArenaHintDetail {
                    estimated: 4000,
                    used: 4000,
                    source: ArenaHintSource::Estimated,
                },
                account_trie: ArenaHintDetail {
                    estimated: 7300,
                    used: 7300,
                    source: ArenaHintSource::Estimated,
                },
                storage_trie: ArenaHintDetail {
                    estimated: 8800,
                    used: 8800,
                    source: ArenaHintSource::Estimated,
                },
            },
        }
    }

    /// Estimate arena sizes from state data.
    ///
    /// Uses pages-per-entry ratios derived from production metrics.
    /// Ratios target ~20% headroom over observed actual usage.
    ///
    /// # Pages-per-entry ratios (with headroom to reduce arena refills)
    /// - PlainAccountState: 1.47 × 1.2 = 1.76
    /// - PlainStorageState: 1.50 × 1.33 = 2.0 (bumped from 1.80)
    /// - HashedAccounts: 1.55 × 1.2 = 1.86
    /// - HashedStorages: 1.63 × 1.35 = 2.2 (bumped from 1.96)
    /// - AccountsTrie: 0.94 × 1.2 = 1.13
    /// - StoragesTrie: 1.45 × 1.52 = 2.2 (bumped from 1.74)
    /// - Bytecodes: 1.67 × 1.2 = 2.00
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

        // Actual: 1.47 * 1.2 = 1.76 pages/account
        let plain_accounts_detail =
            Self::calc_with_ratio(num_accounts, 1.76, default.plain_accounts);
        // Actual: 1.67 * 1.2 = 2.0 pages/contract
        let bytecodes_detail =
            Self::calc_with_ratio(num_contracts, 2.0, Self::DEFAULT_BYTECODES_PAGES);
        // Actual: 1.50 * 1.33 = 2.0 pages/slot (bumped from 1.80 to reduce refills)
        let plain_storage_detail = Self::calc_with_ratio(num_storage, 2.0, default.plain_storage);
        // Actual: 1.55 * 1.2 = 1.86 pages/account
        let hashed_accounts_detail =
            Self::calc_with_ratio(num_accounts, 1.86, default.hashed_accounts);
        // Actual: 1.63 * 1.35 = 2.2 pages/slot (bumped from 1.96 to reduce refills)
        let hashed_storages_detail =
            Self::calc_with_ratio(num_storage, 2.2, default.hashed_storages);

        // Trie tables
        // Actual: 0.94 * 1.2 = 1.13 pages/node
        let account_trie_detail =
            Self::calc_with_ratio(num_account_trie_nodes, 1.13, default.account_trie);
        // Actual: 1.45 * 1.52 = 2.2 pages/node (bumped from 1.74 to reduce refills on large blocks)
        let storage_trie_detail =
            Self::calc_with_ratio(num_storage_trie_nodes, 2.2, default.storage_trie);

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

    /// Calculate arena hint using floating-point pages-per-entry ratio.
    ///
    /// Formula: `ceil(entries * ratio) + MIN_ARENA_PAGES`
    /// - Floating point allows precise 20% headroom targeting
    /// - Ceiling ensures we never underestimate
    /// - +MIN_ARENA_PAGES for minimum working set
    fn calc_with_ratio(entries: usize, ratio: f64, floor: usize) -> ArenaHintDetail {
        let raw_estimate = if entries == 0 {
            Self::MIN_ARENA_PAGES
        } else {
            let pages = (entries as f64 * ratio).ceil() as usize;
            pages.saturating_add(Self::MIN_ARENA_PAGES)
        };

        Self::apply_bounds(raw_estimate, floor)
    }

    /// Calculate arena hint for trie tables where each node may require multiple pages.
    /// (Kept for backwards compatibility, delegates to calc_linear_with_floor)
    #[allow(dead_code)]
    fn calc_trie_with_floor(nodes: usize, pages_per_node: usize, floor: usize) -> ArenaHintDetail {
        let raw_estimate = if nodes == 0 {
            Self::MIN_ARENA_PAGES
        } else {
            nodes.saturating_mul(pages_per_node).saturating_add(8)
        };

        Self::apply_bounds(raw_estimate, floor)
    }

    /// Apply floor bound to a raw estimate.
    fn apply_bounds(raw_estimate: usize, floor: usize) -> ArenaHintDetail {
        let (used, source) = if raw_estimate < floor {
            (floor, ArenaHintSource::Floored)
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

    /// Estimate arena sizes with optional dynamic floors from recent usage history.
    ///
    /// When `dynamic_floors` is `Some`, these floors override the static defaults
    /// for tables where the dynamic floor is higher. This adapts to recent workload
    /// patterns, reducing waste on small blocks and refills on large blocks.
    ///
    /// # Arguments
    /// * `num_accounts` - Number of account changes
    /// * `num_storage` - Number of storage slot changes
    /// * `num_contracts` - Number of new contracts
    /// * `num_account_trie_nodes` - Number of account trie node updates
    /// * `num_storage_trie_nodes` - Number of storage trie node updates
    /// * `dynamic_floors` - Optional P95-based floors from [`ArenaUsageTracker`]
    pub fn estimate_with_dynamic_floors(
        num_accounts: usize,
        num_storage: usize,
        num_contracts: usize,
        num_account_trie_nodes: usize,
        num_storage_trie_nodes: usize,
        dynamic_floors: Option<&ArenaUsageSnapshot>,
    ) -> Self {
        let default = Self::default_hints();

        // Use dynamic floors if available, otherwise fall back to static defaults
        let (
            plain_accounts_floor,
            plain_storage_floor,
            bytecodes_floor,
            hashed_accounts_floor,
            hashed_storages_floor,
            account_trie_floor,
            storage_trie_floor,
        ) = if let Some(dynamic) = dynamic_floors {
            // Take the max of static and dynamic floors for each table
            (
                default.plain_accounts.max(dynamic.plain_accounts),
                default.plain_storage.max(dynamic.plain_storage),
                Self::DEFAULT_BYTECODES_PAGES.max(dynamic.bytecodes),
                default.hashed_accounts.max(dynamic.hashed_accounts),
                default.hashed_storages.max(dynamic.hashed_storages),
                default.account_trie.max(dynamic.account_trie),
                default.storage_trie.max(dynamic.storage_trie),
            )
        } else {
            (
                default.plain_accounts,
                default.plain_storage,
                Self::DEFAULT_BYTECODES_PAGES,
                default.hashed_accounts,
                default.hashed_storages,
                default.account_trie,
                default.storage_trie,
            )
        };

        // Calculate with potentially elevated floors
        // Ratios must match those in estimate() to ensure consistency
        let plain_accounts_detail = Self::calc_with_ratio(num_accounts, 1.76, plain_accounts_floor);
        let bytecodes_detail = Self::calc_with_ratio(num_contracts, 2.0, bytecodes_floor);
        // 1.50 * 1.33 = 2.0 pages/slot (bumped from 1.80 to reduce refills)
        let plain_storage_detail = Self::calc_with_ratio(num_storage, 2.0, plain_storage_floor);
        let hashed_accounts_detail =
            Self::calc_with_ratio(num_accounts, 1.86, hashed_accounts_floor);
        // 1.63 * 1.35 = 2.2 pages/slot (bumped from 1.96 to reduce refills)
        let hashed_storages_detail = Self::calc_with_ratio(num_storage, 2.2, hashed_storages_floor);
        let account_trie_detail =
            Self::calc_with_ratio(num_account_trie_nodes, 1.13, account_trie_floor);
        // 1.45 * 1.52 = 2.2 pages/node (bumped from 1.74 to reduce refills on large blocks)
        let storage_trie_detail =
            Self::calc_with_ratio(num_storage_trie_nodes, 2.2, storage_trie_floor);

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
}

/// Snapshot of actual pages used per table after a batch commits.
///
/// This captures the real page consumption (allocated - unused) for each table,
/// enabling dynamic floor calculation based on recent workload patterns.
#[derive(Debug, Default, Clone, Copy)]
pub struct ArenaUsageSnapshot {
    /// Pages used by PlainAccountState
    pub plain_accounts: usize,
    /// Pages used by PlainStorageState
    pub plain_storage: usize,
    /// Pages used by Bytecodes
    pub bytecodes: usize,
    /// Pages used by HashedAccounts
    pub hashed_accounts: usize,
    /// Pages used by HashedStorages
    pub hashed_storages: usize,
    /// Pages used by AccountsTrie
    pub account_trie: usize,
    /// Pages used by StoragesTrie
    pub storage_trie: usize,
}

impl ArenaUsageSnapshot {
    /// Creates a snapshot from allocated and unused page counts.
    ///
    /// The actual usage is `allocated - unused` for each table.
    pub fn from_allocation_stats(hints: &ArenaHints, unused: &ArenaUsageSnapshot) -> Self {
        Self {
            plain_accounts: hints.plain_accounts.saturating_sub(unused.plain_accounts),
            plain_storage: hints.plain_storage.saturating_sub(unused.plain_storage),
            bytecodes: hints.bytecodes.saturating_sub(unused.bytecodes),
            hashed_accounts: hints.hashed_accounts.saturating_sub(unused.hashed_accounts),
            hashed_storages: hints.hashed_storages.saturating_sub(unused.hashed_storages),
            account_trie: hints.account_trie.saturating_sub(unused.account_trie),
            storage_trie: hints.storage_trie.saturating_sub(unused.storage_trie),
        }
    }

    /// Returns total pages across all tables.
    pub const fn total(&self) -> usize {
        self.plain_accounts +
            self.plain_storage +
            self.bytecodes +
            self.hashed_accounts +
            self.hashed_storages +
            self.account_trie +
            self.storage_trie
    }
}

/// Tracks rolling history of arena usage to compute dynamic floors.
///
/// Instead of using static floors that waste pages on small blocks and
/// cause refills on large blocks, this tracker maintains a rolling window
/// of actual page usage and computes percentile-based dynamic floors.
#[derive(Debug)]
pub struct ArenaUsageTracker {
    /// Rolling window of actual pages used per table.
    history: std::collections::VecDeque<ArenaUsageSnapshot>,
    /// Maximum number of batches to track.
    max_history: usize,
}

impl Default for ArenaUsageTracker {
    fn default() -> Self {
        Self::new(Self::DEFAULT_HISTORY_SIZE)
    }
}

impl ArenaUsageTracker {
    /// Default number of batches to track for rolling average.
    pub const DEFAULT_HISTORY_SIZE: usize = 32;

    /// Minimum number of samples before using dynamic floors.
    pub const MIN_SAMPLES_FOR_DYNAMIC: usize = 8;

    /// Creates a new tracker with the specified history size.
    pub fn new(max_history: usize) -> Self {
        Self { history: std::collections::VecDeque::with_capacity(max_history), max_history }
    }

    /// Records a usage snapshot from the latest batch.
    ///
    /// Call this after each batch commits with the actual pages used.
    pub fn record(&mut self, snapshot: ArenaUsageSnapshot) {
        if self.history.len() >= self.max_history {
            self.history.pop_front();
        }
        self.history.push_back(snapshot);
    }

    /// Returns the number of recorded samples.
    pub fn sample_count(&self) -> usize {
        self.history.len()
    }

    /// Returns true if enough samples exist for dynamic floor calculation.
    pub fn has_enough_samples(&self) -> bool {
        self.history.len() >= Self::MIN_SAMPLES_FOR_DYNAMIC
    }

    /// Computes dynamic floors based on the 95th percentile of recent usage.
    ///
    /// Returns `None` if not enough history is available yet.
    /// When `None`, callers should fall back to static defaults.
    pub fn get_dynamic_floors(&self) -> Option<ArenaUsageSnapshot> {
        if !self.has_enough_samples() {
            return None;
        }

        Some(ArenaUsageSnapshot {
            plain_accounts: self.percentile_95(|s| s.plain_accounts),
            plain_storage: self.percentile_95(|s| s.plain_storage),
            bytecodes: self.percentile_95(|s| s.bytecodes),
            hashed_accounts: self.percentile_95(|s| s.hashed_accounts),
            hashed_storages: self.percentile_95(|s| s.hashed_storages),
            account_trie: self.percentile_95(|s| s.account_trie),
            storage_trie: self.percentile_95(|s| s.storage_trie),
        })
    }

    /// Computes the 95th percentile for a given field extractor.
    fn percentile_95<F>(&self, extractor: F) -> usize
    where
        F: Fn(&ArenaUsageSnapshot) -> usize,
    {
        let mut values: Vec<usize> = self.history.iter().map(&extractor).collect();
        values.sort_unstable();

        if values.is_empty() {
            return 0;
        }

        // P95 index: for N samples, index = ceil(0.95 * N) - 1
        let idx = ((values.len() as f64 * 0.95).ceil() as usize).saturating_sub(1);
        let idx = idx.min(values.len() - 1);
        values[idx]
    }

    /// Clears all recorded history.
    pub fn clear(&mut self) {
        self.history.clear();
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

        self.tx_ref().prefault_arena_for_table::<tables::PlainAccountState>()?;
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

        self.tx_ref().prefault_arena_for_table::<tables::Bytecodes>()?;
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

        self.tx_ref().prefault_arena_for_table::<tables::PlainStorageState>()?;
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

        self.tx_ref().prefault_arena_for_table::<tables::HashedAccounts>()?;
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

        self.tx_ref().prefault_arena_for_table::<tables::HashedStorages>()?;
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

        self.tx_ref().prefault_arena_for_table::<tables::AccountsTrie>()?;
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
    /// Returns a tuple of (entries modified, operation counts, duration).
    pub fn write_storage_trie_only(
        &self,
        tries: &[(B256, &StorageTrieUpdatesSorted)],
    ) -> ProviderResult<(usize, StorageTrieOpCounts, Duration)> {
        let start = Instant::now();
        let mut num_entries = 0;
        let mut total_op_counts = StorageTrieOpCounts::default();

        self.tx_ref().prefault_arena_for_table::<tables::StoragesTrie>()?;
        let mut cursor = self.tx_ref().cursor_dup_write::<tables::StoragesTrie>()?;
        for (hashed_address, storage_trie_updates) in tries {
            let mut db_storage_trie_cursor =
                DatabaseStorageTrieCursor::new(cursor, *hashed_address);
            let (entries, op_counts) =
                db_storage_trie_cursor.write_storage_trie_updates_sorted(storage_trie_updates)?;
            num_entries += entries;
            total_op_counts.seek_count += op_counts.seek_count;
            total_op_counts.delete_count += op_counts.delete_count;
            total_op_counts.upsert_count += op_counts.upsert_count;
            cursor = db_storage_trie_cursor.cursor;
        }

        Ok((num_entries, total_op_counts, start.elapsed()))
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
        // StoragesTrie: ratio 2.2 (bumped from 1.74 for large blocks)
        // With 10000 nodes: ceil(10000*2.2)+8 = 22008 > floor(8800), so Estimated
        let hints = ArenaHints::estimate(0, 0, 0, 0, 10000);
        assert_eq!(hints.details.storage_trie.estimated, 22008);
        assert_eq!(hints.storage_trie, 22008);
        assert_eq!(hints.details.storage_trie.source, ArenaHintSource::Estimated);

        // With 4000 nodes: ceil(4000*2.2)+8 = 8808 < floor(8800), so estimated (just above floor)
        let hints = ArenaHints::estimate(0, 0, 0, 0, 4000);
        assert_eq!(hints.details.storage_trie.estimated, 8808);
        assert_eq!(hints.storage_trie, 8808);
        assert_eq!(hints.details.storage_trie.source, ArenaHintSource::Estimated);

        // AccountsTrie: ratio 1.13 (actual 0.94 * 1.2)
        // With 10000 nodes: ceil(10000*1.13)+8 = 11308 > floor(7300), so Estimated
        let hints = ArenaHints::estimate(0, 0, 0, 10000, 0);
        assert_eq!(hints.details.account_trie.estimated, 11308);
        assert_eq!(hints.account_trie, 11308);
        assert_eq!(hints.details.account_trie.source, ArenaHintSource::Estimated);

        // With 5000 nodes: ceil(5000*1.13)+8 = 5658 < floor(7300), so floored
        let hints = ArenaHints::estimate(0, 0, 0, 5000, 0);
        assert_eq!(hints.details.account_trie.estimated, 5658);
        assert_eq!(hints.account_trie, 7300);
        assert_eq!(hints.details.account_trie.source, ArenaHintSource::Floored);
    }

    #[test]
    fn test_arena_hints_estimate_data_tables() {
        // HashedStorages: ratio 2.2 (bumped from 1.96)
        // 5000 storage: ceil(5000*2.2)+8 = 11008 > floor(4000), so Estimated
        let hints = ArenaHints::estimate(0, 5000, 0, 0, 0);
        assert_eq!(hints.details.hashed_storages.estimated, 11008);
        assert_eq!(hints.hashed_storages, 11008);
        assert_eq!(hints.details.hashed_storages.source, ArenaHintSource::Estimated);

        // 1000 storage: ceil(1000*2.2)+8 = 2208 < floor(4000), so floored
        let hints = ArenaHints::estimate(0, 1000, 0, 0, 0);
        assert_eq!(hints.details.hashed_storages.estimated, 2208);
        assert_eq!(hints.hashed_storages, 4000);
        assert_eq!(hints.details.hashed_storages.source, ArenaHintSource::Floored);

        // PlainAccountState: ratio 1.76 (actual 1.47 * 1.2)
        // 3000 accounts: ceil(3000*1.76)+8 = 5288 > floor(3000), so Estimated
        let hints = ArenaHints::estimate(3000, 0, 0, 0, 0);
        assert_eq!(hints.details.plain_accounts.estimated, 5288);
        assert_eq!(hints.plain_accounts, 5288);
        assert_eq!(hints.details.plain_accounts.source, ArenaHintSource::Estimated);
    }

    #[test]
    fn test_arena_hints_zero_entries() {
        let hints = ArenaHints::estimate(0, 0, 0, 0, 0);
        // Zero entries should use MIN_ARENA_PAGES for estimate, but floor applies
        assert_eq!(hints.details.storage_trie.estimated, ArenaHints::MIN_ARENA_PAGES);
        assert_eq!(hints.storage_trie, 8800); // floor from default_hints
    }

    #[test]
    fn test_arena_usage_snapshot_default() {
        let snapshot = ArenaUsageSnapshot::default();
        assert_eq!(snapshot.plain_accounts, 0);
        assert_eq!(snapshot.plain_storage, 0);
        assert_eq!(snapshot.bytecodes, 0);
        assert_eq!(snapshot.hashed_accounts, 0);
        assert_eq!(snapshot.hashed_storages, 0);
        assert_eq!(snapshot.account_trie, 0);
        assert_eq!(snapshot.storage_trie, 0);
        assert_eq!(snapshot.total(), 0);
    }

    #[test]
    fn test_arena_usage_snapshot_from_allocation_stats() {
        let hints = ArenaHints {
            plain_accounts: 1000,
            plain_storage: 2000,
            bytecodes: 100,
            hashed_accounts: 1500,
            hashed_storages: 2500,
            account_trie: 3000,
            storage_trie: 4000,
            details: ArenaHintDetails::default(),
        };

        let unused = ArenaUsageSnapshot {
            plain_accounts: 200,
            plain_storage: 500,
            bytecodes: 50,
            hashed_accounts: 300,
            hashed_storages: 600,
            account_trie: 700,
            storage_trie: 800,
        };

        let actual = ArenaUsageSnapshot::from_allocation_stats(&hints, &unused);
        assert_eq!(actual.plain_accounts, 800);
        assert_eq!(actual.plain_storage, 1500);
        assert_eq!(actual.bytecodes, 50);
        assert_eq!(actual.hashed_accounts, 1200);
        assert_eq!(actual.hashed_storages, 1900);
        assert_eq!(actual.account_trie, 2300);
        assert_eq!(actual.storage_trie, 3200);
    }

    #[test]
    fn test_arena_usage_tracker_default() {
        let tracker = ArenaUsageTracker::default();
        assert_eq!(tracker.sample_count(), 0);
        assert!(!tracker.has_enough_samples());
        assert!(tracker.get_dynamic_floors().is_none());
    }

    #[test]
    fn test_arena_usage_tracker_record_and_sample_count() {
        let mut tracker = ArenaUsageTracker::new(10);
        assert_eq!(tracker.sample_count(), 0);

        tracker.record(ArenaUsageSnapshot { plain_accounts: 100, ..Default::default() });
        assert_eq!(tracker.sample_count(), 1);

        tracker.record(ArenaUsageSnapshot { plain_accounts: 200, ..Default::default() });
        assert_eq!(tracker.sample_count(), 2);
    }

    #[test]
    fn test_arena_usage_tracker_respects_max_history() {
        let mut tracker = ArenaUsageTracker::new(3);

        for i in 0..5 {
            tracker.record(ArenaUsageSnapshot { plain_accounts: i * 100, ..Default::default() });
        }

        // Should have capped at 3
        assert_eq!(tracker.sample_count(), 3);
    }

    #[test]
    fn test_arena_usage_tracker_needs_minimum_samples() {
        let mut tracker = ArenaUsageTracker::new(32);

        // Add fewer than MIN_SAMPLES_FOR_DYNAMIC
        for i in 0..(ArenaUsageTracker::MIN_SAMPLES_FOR_DYNAMIC - 1) {
            tracker.record(ArenaUsageSnapshot { plain_accounts: i * 100, ..Default::default() });
        }

        assert!(!tracker.has_enough_samples());
        assert!(tracker.get_dynamic_floors().is_none());

        // Add one more to reach minimum
        tracker.record(ArenaUsageSnapshot { plain_accounts: 1000, ..Default::default() });
        assert!(tracker.has_enough_samples());
        assert!(tracker.get_dynamic_floors().is_some());
    }

    #[test]
    fn test_arena_usage_tracker_percentile_95() {
        let mut tracker = ArenaUsageTracker::new(32);

        // Add 20 samples with values 100, 200, 300, ..., 2000
        for i in 1..=20 {
            tracker.record(ArenaUsageSnapshot { plain_accounts: i * 100, ..Default::default() });
        }

        let floors = tracker.get_dynamic_floors().unwrap();
        // P95 of 20 samples: index = ceil(20 * 0.95) - 1 = 19 - 1 = 18 (0-indexed)
        // Values sorted: [100, 200, ..., 2000], value at index 18 = 1900
        assert_eq!(floors.plain_accounts, 1900);
    }

    #[test]
    fn test_arena_usage_tracker_clear() {
        let mut tracker = ArenaUsageTracker::new(32);

        for i in 0..10 {
            tracker.record(ArenaUsageSnapshot { plain_accounts: i * 100, ..Default::default() });
        }
        assert_eq!(tracker.sample_count(), 10);

        tracker.clear();
        assert_eq!(tracker.sample_count(), 0);
        assert!(!tracker.has_enough_samples());
    }

    #[test]
    fn test_arena_hints_estimate_with_dynamic_floors_none() {
        // When dynamic_floors is None, should behave exactly like estimate()
        let hints_regular = ArenaHints::estimate(1000, 2000, 10, 500, 600);
        let hints_dynamic =
            ArenaHints::estimate_with_dynamic_floors(1000, 2000, 10, 500, 600, None);

        assert_eq!(hints_regular.plain_accounts, hints_dynamic.plain_accounts);
        assert_eq!(hints_regular.plain_storage, hints_dynamic.plain_storage);
        assert_eq!(hints_regular.bytecodes, hints_dynamic.bytecodes);
        assert_eq!(hints_regular.hashed_accounts, hints_dynamic.hashed_accounts);
        assert_eq!(hints_regular.hashed_storages, hints_dynamic.hashed_storages);
        assert_eq!(hints_regular.account_trie, hints_dynamic.account_trie);
        assert_eq!(hints_regular.storage_trie, hints_dynamic.storage_trie);
    }

    #[test]
    fn test_arena_hints_estimate_with_dynamic_floors_higher() {
        // Dynamic floor higher than static should be used
        let dynamic_floors = ArenaUsageSnapshot {
            plain_accounts: 5000,  // Higher than static 3000
            plain_storage: 5000,   // Higher than static 3600
            bytecodes: 100,        // Higher than static 12
            hashed_accounts: 5000, // Higher than static 3400
            hashed_storages: 6000, // Higher than static 4000
            account_trie: 10000,   // Higher than static 7300
            storage_trie: 12000,   // Higher than static 8800
        };

        // Use small counts so estimate would be below floor
        let hints =
            ArenaHints::estimate_with_dynamic_floors(100, 100, 1, 100, 100, Some(&dynamic_floors));

        // All should be floored to dynamic values (which are higher than static)
        assert_eq!(hints.plain_accounts, 5000);
        assert_eq!(hints.plain_storage, 5000);
        assert_eq!(hints.bytecodes, 100);
        assert_eq!(hints.hashed_accounts, 5000);
        assert_eq!(hints.hashed_storages, 6000);
        assert_eq!(hints.account_trie, 10000);
        assert_eq!(hints.storage_trie, 12000);
    }

    #[test]
    fn test_arena_hints_estimate_with_dynamic_floors_lower() {
        // Dynamic floor lower than static should use static
        let dynamic_floors = ArenaUsageSnapshot {
            plain_accounts: 1000,  // Lower than static 3000
            plain_storage: 1000,   // Lower than static 3600
            bytecodes: 5,          // Lower than static 12
            hashed_accounts: 1000, // Lower than static 3400
            hashed_storages: 2000, // Lower than static 4000
            account_trie: 5000,    // Lower than static 7300
            storage_trie: 6000,    // Lower than static 8800
        };

        // Use small counts so estimate would be below floor
        let hints =
            ArenaHints::estimate_with_dynamic_floors(100, 100, 1, 100, 100, Some(&dynamic_floors));

        // Should fall back to static floors (higher)
        assert_eq!(hints.plain_accounts, 3000);
        assert_eq!(hints.plain_storage, 3600);
        assert_eq!(hints.bytecodes, 12);
        assert_eq!(hints.hashed_accounts, 3400);
        assert_eq!(hints.hashed_storages, 4000);
        assert_eq!(hints.account_trie, 7300);
        assert_eq!(hints.storage_trie, 8800);
    }
}
