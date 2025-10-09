//! Table definitions for MDBX external storage
//!
//! This module defines all database tables used to store external proofs data.
//! Each table is completely independent from Reth's main database tables.

use super::{codec::BlockNumberHash, models::MetadataKey};
use crate::models::IntegerList;
use alloy_primitives::B256;
use reth_db_api::table::{DupSort, Table};
use reth_trie_common::{StoredNibbles, StoredNibblesSubKey};

// ============================================================================
// Main Data Tables (store actual trie/state data)
// ============================================================================

/// Account trie branches by path and block (DupSort)
///
/// Key: path (StoredNibbles)
/// SubKey: block_number (u64)
/// Value: MaybeDeleted<BranchNodeCompact> (empty bytes = deleted, non-empty = present)
///
/// This structure allows efficient iteration by path, with block versioning as secondary sort
#[derive(Debug, Clone)]
pub struct ExternalAccountBranches;

impl Table for ExternalAccountBranches {
    const NAME: &'static str = "ExternalAccountBranches";
    const DUPSORT: bool = true;

    type Key = StoredNibbles;
    type Value = super::codec::MaybeDeleted<reth_trie::BranchNodeCompact>;
}

impl DupSort for ExternalAccountBranches {
    type SubKey = u64; // block_number
}

/// Storage trie branches by address, path, and block (DupSort)
///
/// Key: hashed_address (B256)
/// SubKey: (path, block_number) - StorageBranchSubKey
/// Value: MaybeDeleted<BranchNodeCompact> (empty bytes = deleted, non-empty = present)
///
/// This structure allows efficient iteration by address and path, with block versioning
#[derive(Debug, Clone)]
pub struct ExternalStorageBranches;

impl Table for ExternalStorageBranches {
    const NAME: &'static str = "ExternalStorageBranches";
    const DUPSORT: bool = true;

    type Key = B256;
    type Value = super::codec::MaybeDeleted<reth_trie::BranchNodeCompact>;
}

impl DupSort for ExternalStorageBranches {
    type SubKey = super::models::StorageBranchSubKey; // (path, block_number)
}

/// Hashed accounts by address and block (DupSort)
///
/// Key: hashed_address (B256)
/// SubKey: block_number (u64)
/// Value: MaybeDeleted<Account> (empty bytes = deleted, non-empty = present)
///
/// This structure allows efficient iteration by address, with block versioning as secondary sort
#[derive(Debug, Clone)]
pub struct ExternalHashedAccounts;

impl Table for ExternalHashedAccounts {
    const NAME: &'static str = "ExternalHashedAccounts";
    const DUPSORT: bool = true;

    type Key = B256;
    type Value = super::codec::MaybeDeleted<reth_primitives_traits::Account>;
}

impl DupSort for ExternalHashedAccounts {
    type SubKey = u64; // block_number
}

/// Hashed storage values by address, storage_key, and block (DupSort)
///
/// Key: hashed_address (B256)
/// SubKey: (storage_key, block_number) - HashedStorageSubKey
/// Value: StorageEntry (zero values are NOT stored)
///
/// This structure allows efficient iteration by address and storage_key, with block versioning
#[derive(Debug, Clone)]
pub struct ExternalHashedStorages;

impl Table for ExternalHashedStorages {
    const NAME: &'static str = "ExternalHashedStorages";
    const DUPSORT: bool = true;

    type Key = B256;
    type Value = reth_primitives_traits::StorageEntry;
}

impl DupSort for ExternalHashedStorages {
    type SubKey = super::models::HashedStorageSubKey; // (storage_key, block_number)
}

// ============================================================================
// Index Tables (for efficient pruning and reorg operations)
// ============================================================================

/// Index: Account trie path → list of block numbers that modified it
///
/// This allows us to efficiently find all blocks that need to be cleaned up
/// when pruning or handling reorgs for a specific account branch path.
///
/// Key: path (StoredNibbles)
/// Value: BlockNumberList (compressed list of block numbers)
#[derive(Debug, Clone)]
pub struct ExternalAccountBranchesIndex;

impl Table for ExternalAccountBranchesIndex {
    const NAME: &'static str = "ExternalAccountBranchesIndex";
    const DUPSORT: bool = false;

    type Key = StoredNibbles;
    type Value = IntegerList;
}

/// Index: (address, path) → list of block numbers that modified it
///
/// This allows efficient cleanup of storage branches during pruning/reorgs.
///
/// Key: hashed_address
/// SubKey: path
/// Value: BlockNumberList
#[derive(Debug, Clone)]
pub struct ExternalStorageBranchesIndex;

impl Table for ExternalStorageBranchesIndex {
    const NAME: &'static str = "ExternalStorageBranchesIndex";
    const DUPSORT: bool = true;

    type Key = B256;
    type Value = IntegerList;
}

impl DupSort for ExternalStorageBranchesIndex {
    type SubKey = StoredNibblesSubKey;
}

/// Index: hashed_address → list of block numbers that modified it
///
/// This allows efficient cleanup of account data during pruning/reorgs.
///
/// Key: hashed_address
/// Value: BlockNumberList
#[derive(Debug, Clone)]
pub struct ExternalHashedAccountsIndex;

impl Table for ExternalHashedAccountsIndex {
    const NAME: &'static str = "ExternalHashedAccountsIndex";
    const DUPSORT: bool = false;

    type Key = B256;
    type Value = IntegerList;
}

/// Index: (address, storage_key) → list of block numbers that modified it
///
/// This allows efficient cleanup of storage values during pruning/reorgs.
///
/// Key: hashed_address
/// SubKey: storage_key (B256)
/// Value: BlockNumberList (IntegerList)
#[derive(Debug, Clone)]
pub struct ExternalHashedStoragesIndex;

impl Table for ExternalHashedStoragesIndex {
    const NAME: &'static str = "ExternalHashedStoragesIndex";
    const DUPSORT: bool = true;

    type Key = B256;
    type Value = IntegerList;
}

impl DupSort for ExternalHashedStoragesIndex {
    type SubKey = B256;
}

// ============================================================================
// Metadata Table
// ============================================================================

/// Metadata (earliest/latest block tracking)
///
/// Key: MetadataKey (EarliestBlock or LatestBlock)
/// Value: BlockNumberHash (block_number + block_hash)
#[derive(Debug, Clone)]
pub struct ExternalBlockMetadata;

impl Table for ExternalBlockMetadata {
    const NAME: &'static str = "ExternalBlockMetadata";
    const DUPSORT: bool = false;

    type Key = MetadataKey;
    type Value = BlockNumberHash;
}

// ============================================================================
// Table Set for Database Initialization
// ============================================================================

/// All external storage tables as an enum
///
/// This is used to initialize the database with all required tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tables {
    /// Account trie branches
    ExternalAccountBranches,
    /// Storage trie branches  
    ExternalStorageBranches,
    /// Hashed accounts
    ExternalHashedAccounts,
    /// Hashed storage values
    ExternalHashedStorages,
    /// Account branches index
    ExternalAccountBranchesIndex,
    /// Storage branches index
    ExternalStorageBranchesIndex,
    /// Hashed accounts index
    ExternalHashedAccountsIndex,
    /// Hashed storages index
    ExternalHashedStoragesIndex,
    /// Block metadata
    ExternalBlockMetadata,
}

impl Tables {
    /// All external tables
    pub const ALL: &'static [Self] = &[
        Self::ExternalAccountBranches,
        Self::ExternalStorageBranches,
        Self::ExternalHashedAccounts,
        Self::ExternalHashedStorages,
        Self::ExternalAccountBranchesIndex,
        Self::ExternalStorageBranchesIndex,
        Self::ExternalHashedAccountsIndex,
        Self::ExternalHashedStoragesIndex,
        Self::ExternalBlockMetadata,
    ];

    /// Get the table name
    pub const fn name(&self) -> &'static str {
        match self {
            Self::ExternalAccountBranches => ExternalAccountBranches::NAME,
            Self::ExternalStorageBranches => ExternalStorageBranches::NAME,
            Self::ExternalHashedAccounts => ExternalHashedAccounts::NAME,
            Self::ExternalHashedStorages => ExternalHashedStorages::NAME,
            Self::ExternalAccountBranchesIndex => ExternalAccountBranchesIndex::NAME,
            Self::ExternalStorageBranchesIndex => ExternalStorageBranchesIndex::NAME,
            Self::ExternalHashedAccountsIndex => ExternalHashedAccountsIndex::NAME,
            Self::ExternalHashedStoragesIndex => ExternalHashedStoragesIndex::NAME,
            Self::ExternalBlockMetadata => ExternalBlockMetadata::NAME,
        }
    }

    /// Check if the table is a DUPSORT table
    pub const fn is_dupsort(&self) -> bool {
        match self {
            Self::ExternalStorageBranches
            | Self::ExternalStorageBranchesIndex
            | Self::ExternalHashedStorages
            | Self::ExternalHashedStoragesIndex => true,
            _ => false,
        }
    }
}

impl reth_db_api::table::TableInfo for Tables {
    fn name(&self) -> &'static str {
        self.name()
    }

    fn is_dupsort(&self) -> bool {
        self.is_dupsort()
    }
}

impl reth_db_api::TableSet for Tables {
    fn tables() -> Box<dyn Iterator<Item = Box<dyn reth_db_api::table::TableInfo>>> {
        Box::new(
            Self::ALL
                .iter()
                .map(|table| Box::new(*table) as Box<dyn reth_db_api::table::TableInfo>),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_table_names_unique() {
        use std::collections::HashSet;
        let names: HashSet<_> = Tables::ALL.iter().collect();
        assert_eq!(names.len(), Tables::ALL.len(), "Table names must be unique");
    }
}
