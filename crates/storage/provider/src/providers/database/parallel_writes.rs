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
}

impl ArenaHints {
    /// Maximum pages per arena to prevent memory bloat
    pub const MAX_ARENA_PAGES: usize = 512;
    /// Minimum arena size
    pub const MIN_ARENA_PAGES: usize = 8;

    /// Estimate arena sizes from state data.
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
        Self {
            // Target ~120 pages based on 187 hits/s peak (+20% headroom)
            plain_accounts: Self::calc(num_accounts, 25),
            // Bytecodes are large but few per block
            bytecodes: Self::calc(num_contracts, 1).max(Self::MIN_ARENA_PAGES),
            // Target ~150 pages based on 211 hits/s peak (+20% headroom)
            plain_storage: Self::calc(num_storage, 20),
            // Target ~120 pages based on 205 hits/s peak (+20% headroom)
            hashed_accounts: Self::calc(num_accounts, 25),
            // Target ~150 pages based on 236 hits/s peak (+20% headroom)
            hashed_storages: Self::calc(num_storage, 20),
            // Target ~560 pages based on 467 hits/s peak (+20% headroom)
            account_trie: Self::calc(num_account_trie_nodes, 8),
            // Target ~500 pages based on 485 hits/s peak (+20% headroom)
            storage_trie: Self::calc(num_storage_trie_nodes, 10),
        }
    }

    fn calc(entries: usize, entries_per_page: usize) -> usize {
        if entries == 0 {
            return Self::MIN_ARENA_PAGES;
        }
        let base = entries.div_ceil(entries_per_page);
        // 2x for B+ tree traversal overhead + buffer
        let with_overhead = base.saturating_mul(2).saturating_add(8);
        with_overhead.clamp(Self::MIN_ARENA_PAGES, Self::MAX_ARENA_PAGES)
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
}
