//! MDBX-backed implementation of ExternalStorage
//!
//! This module provides a persistent storage backend using MDBX (libmdbx) for the external proofs
//! ExEx. The database is completely separate from Reth's main database and is stored by default
//! at `<datadir>/external-proofs/`.

pub mod codec;
pub mod cursor;
pub mod models;
pub mod tables;

use crate::{
    mdbx::cursor::{MdbxAccountCursor, MdbxStorageCursor},
    storage::{BlockStateDiff, OpProofsStorage, OpProofsStorageError, OpProofsStorageResult},
};
use alloy_primitives::map::HashMap;
use alloy_primitives::{B256, U256};
use reth_db::{
    mdbx::{init_db_for, DatabaseArguments},
    ClientVersion, DatabaseEnv, DatabaseError,
};
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW},
    database::Database,
    transaction::{DbTx, DbTxMut},
};
use reth_primitives_traits::Account;
use reth_trie::{BranchNodeCompact, Nibbles};
use std::path::Path;

pub use codec::{BlockNumberHash, MaybeDeleted};
pub use cursor::{BlockNumberVersionedCursor, MdbxOpProofsStorageTrieCursor};
pub use models::{HashedStorageSubKey, MetadataKey, StorageBranchSubKey};
pub use tables::Tables as ExternalTables;

impl From<DatabaseError> for OpProofsStorageError {
    fn from(error: DatabaseError) -> Self {
        Self::Other(error.into())
    }
}

/// MDBX-backed implementation of ExternalStorage
///
/// **IMPORTANT**: This uses a COMPLETELY SEPARATE database from Reth's main DB.
/// By default, it creates a database in `<datadir>/external-proofs/`.
#[derive(Debug, Clone)]
pub struct MdbxOpProofsStorage<DB> {
    /// Database environment (separate from main Reth DB)
    db: DB,
}

impl MdbxOpProofsStorage<DatabaseEnv> {
    /// Open or create external storage database at the specified path
    ///
    /// # Arguments
    /// * `path` - Path to the external storage database directory
    ///            (e.g., `/path/to/datadir/external-proofs/`)
    ///
    /// # Example
    /// ```no_run
    /// use std::path::Path;
    /// use external_proofs::mdbx::MdbxOpProofsStorage;
    ///
    /// let datadir = Path::new("/path/to/reth/data");
    /// let storage_path = datadir.join("external-proofs");
    /// let storage = MdbxOpProofsStorage::new(storage_path).unwrap();
    /// ```
    pub fn new(db: DatabaseEnv) -> OpProofsStorageResult<Self> {
        Ok(Self { db })
    }

    /// Get database for testing (creates in temp directory)
    #[cfg(test)]
    pub fn new_test() -> OpProofsStorageResult<Self> {
        let temp_dir = tempfile::tempdir().map_err(|e| {
            OpProofsStorageError::Other(eyre::eyre!("Failed to create temp dir: {}", e))
        })?;
        Self::new_from_path(temp_dir.path())
    }

    /// Create a new storage instance from a given path
    ///
    /// # Arguments
    /// * `path` - Path to the external storage database directory
    pub fn new_from_path(path: impl AsRef<Path>) -> OpProofsStorageResult<Self> {
        let path = path.as_ref().to_path_buf();

        // Create a NEW database with our external tables
        // This is SEPARATE from Reth's main database
        // init_db_for will create the directory and all tables automatically
        let args = DatabaseArguments::new(ClientVersion::default());
        let db = init_db_for::<_, tables::Tables>(&path, args).map_err(|e| {
            OpProofsStorageError::Other(eyre::eyre!(
                "Failed to initialize external storage database at {}: {}",
                path.display(),
                e
            ))
        })?;
        tracing::info!(
            path = %path.display(),
            tables = tables::Tables::ALL.len(),
            "Initialized external storage MDBX database"
        );

        Self::new(db)
    }
}

// Implement ExternalStorage trait - will be filled in phases 4-6
#[async_trait::async_trait]
impl<TX: DbTx, DB: Database<TX = TX>> OpProofsStorage for MdbxOpProofsStorage<DB> {
    type AccountTrieCursor = BlockNumberVersionedCursor<
        tables::ExternalAccountBranches,
        TX::DupCursor<tables::ExternalAccountBranches>,
    >;
    type StorageTrieCursor = MdbxOpProofsStorageTrieCursor<
        tables::ExternalStorageBranches,
        TX::DupCursor<tables::ExternalStorageBranches>,
    >;
    type AccountHashedCursor = MdbxAccountCursor<TX::DupCursor<tables::ExternalHashedAccounts>>;
    type StorageCursor = MdbxStorageCursor<TX::DupCursor<tables::ExternalHashedStorages>>;

    async fn store_account_branches(
        &self,
        block_number: u64,
        mut updates: Vec<(Nibbles, Option<BranchNodeCompact>)>,
    ) -> OpProofsStorageResult<()> {
        // Sort updates by path for MDBX append operation
        updates.sort_by(|(a, _), (b, _)| a.cmp(b));

        let tx = self.db.tx_mut()?;

        // Store branches using DupSort (key=path, subkey=block_number, value=branch)
        {
            let mut cursor = tx.cursor_dup_write::<tables::ExternalAccountBranches>()?;

            for (path, branch) in &updates {
                let key: reth_trie_common::StoredNibbles = path.clone().into();
                let value = codec::MaybeDeleted::from(branch.clone());
                // For DupSort tables, the subkey is encoded as part of the value by MDBX
                // We just use upsert which handles the (key, value) pair
                cursor.upsert(key, &value)?;
            }
        }

        // Update index
        {
            let mut cursor = tx.cursor_write::<tables::ExternalAccountBranchesIndex>()?;

            for (path, _) in updates {
                let key: reth_trie_common::StoredNibbles = path.into();

                // Get existing list or create new
                let mut list =
                    cursor.seek_exact(key.clone())?.map(|(_, list)| list).unwrap_or_default();

                // Add block number to list (using inner RoaringTreemap)
                list.0.insert(block_number);

                // Update the index
                cursor.upsert(key, &list)?;
            }
        }

        tx.commit()?;

        Ok(())
    }

    async fn store_storage_branches(
        &self,
        block_number: u64,
        hashed_address: B256,
        mut items: Vec<(Nibbles, Option<BranchNodeCompact>)>,
    ) -> OpProofsStorageResult<()> {
        // Sort items by path for MDBX append operation
        items.sort_by(|(a, _), (b, _)| a.cmp(b));

        let tx = self.db.tx_mut()?;

        // Store branches using DupSort (key=(address, path), subkey=block_number, value=branch)
        {
            let mut cursor = tx.cursor_dup_write::<tables::ExternalStorageBranches>()?;

            for (path, branch) in &items {
                let key = models::StorageBranchSubKey::new(
                    hashed_address,
                    reth_trie_common::StoredNibbles(path.clone()),
                );
                let value = codec::MaybeDeleted::from(branch.clone());
                cursor.upsert(key, &value)?;
            }
        }

        // Update index
        {
            let mut cursor = tx.cursor_write::<tables::ExternalStorageBranchesIndex>()?;

            for (path, _) in items {
                let key = models::StorageBranchSubKey::new(
                    hashed_address,
                    reth_trie_common::StoredNibbles(path),
                );

                // Get existing list or create new
                let mut list =
                    cursor.seek_exact(key.clone())?.map(|(_, list)| list).unwrap_or_default();

                // Add block number to list (using inner RoaringTreemap)
                list.0.insert(block_number);

                // Update the index
                cursor.upsert(key, &list)?;
            }
        }

        tx.commit()?;

        Ok(())
    }

    async fn store_hashed_accounts(
        &self,
        mut accounts: Vec<(B256, Option<Account>)>,
        block_number: u64,
    ) -> OpProofsStorageResult<()> {
        // Sort accounts by address for MDBX append operation
        accounts.sort_by(|(a, _), (b, _)| a.cmp(b));

        let tx = self.db.tx_mut()?;

        // Store accounts using DupSort (key=hashed_address, subkey=block_number, value=account)
        {
            let mut cursor = tx.cursor_dup_write::<tables::ExternalHashedAccounts>()?;

            for (address, account) in &accounts {
                let value = codec::MaybeDeleted::from(account.clone());
                cursor.upsert(*address, &value)?;
            }
        }

        // Update index
        {
            let mut cursor = tx.cursor_write::<tables::ExternalHashedAccountsIndex>()?;

            for (address, _) in accounts {
                // Get existing list or create new
                let mut list =
                    cursor.seek_exact(address)?.map(|(_, list)| list).unwrap_or_default();

                // Add block number to list (using inner RoaringTreemap)
                list.0.insert(block_number);

                // Update the index
                cursor.upsert(address, &list)?;
            }
        }

        tx.commit()?;

        Ok(())
    }

    async fn store_hashed_storages(
        &self,
        hashed_address: B256,
        mut storages: Vec<(B256, U256)>,
        block_number: u64,
    ) -> OpProofsStorageResult<()> {
        // Sort storages by storage key for MDBX append operation
        storages.sort_by(|(a, _), (b, _)| a.cmp(b));

        let tx = self.db.tx_mut()?;

        // Store storage values using DupSort (key=(address, storage_key), subkey=block_number, value=U256 as B256)
        {
            let mut cursor = tx.cursor_dup_write::<tables::ExternalHashedStorages>()?;

            for (storage_key, value) in &storages {
                let key = models::HashedStorageSubKey::new(hashed_address, *storage_key);
                // Convert U256 to B256 for storage
                let value_b256 = B256::from(value.to_be_bytes());
                let value = codec::MaybeDeleted::from(Some(value_b256));
                cursor.upsert(key, &value)?;
            }
        }

        // Update index
        {
            let mut cursor = tx.cursor_write::<tables::ExternalHashedStoragesIndex>()?;

            for (storage_key, _) in storages {
                let key = models::HashedStorageSubKey::new(hashed_address, storage_key);

                // Get existing list or create new
                let mut list =
                    cursor.seek_exact(key.clone())?.map(|(_, list)| list).unwrap_or_default();

                // Add block number to list (using inner RoaringTreemap)
                list.0.insert(block_number);

                // Update the index
                cursor.upsert(key, &list)?;
            }
        }

        tx.commit()?;

        Ok(())
    }

    async fn store_trie_updates(
        &self,
        block_number: u64,
        block_state_diff: BlockStateDiff,
    ) -> OpProofsStorageResult<()> {
        // Extract trie updates and post state
        let BlockStateDiff { trie_updates, post_state } = block_state_diff;

        // For now, we don't have the block hash in BlockStateDiff, so use ZERO
        // This matches the in-memory implementation
        let block_hash = B256::ZERO;

        // Store account trie branches
        if !trie_updates.account_nodes.is_empty() {
            let updates: Vec<_> = trie_updates
                .account_nodes
                .into_iter()
                .map(|(path, node)| (path, node.into()))
                .collect();
            self.store_account_branches(block_number, updates).await?;
        }

        // Store storage trie branches
        for (address, storage_trie) in trie_updates.storage_tries {
            if !storage_trie.storage_nodes.is_empty() {
                let updates: Vec<_> = storage_trie
                    .storage_nodes
                    .into_iter()
                    .map(|(path, node)| (path, node.into()))
                    .collect();
                self.store_storage_branches(block_number, address, updates).await?;
            }
        }

        // Store hashed accounts
        if !post_state.accounts.is_empty() {
            let accounts: Vec<_> = post_state.accounts.into_iter().collect();
            self.store_hashed_accounts(accounts, block_number).await?;
        }

        // Store hashed storage
        for (address, storage) in post_state.storages {
            if !storage.storage.is_empty() {
                let storages: Vec<_> = storage.storage.into_iter().collect();
                self.store_hashed_storages(address, storages, block_number).await?;
            }
        }

        // Update block metadata
        {
            let tx = self.db.tx_mut()?;

            let mut cursor = tx.cursor_write::<tables::ExternalBlockMetadata>()?;

            // Update latest block
            let latest_value = codec::BlockNumberHash(block_number, block_hash);
            cursor.upsert(models::MetadataKey::LatestBlock, &latest_value)?;

            // Set earliest block if not set
            if cursor.seek_exact(models::MetadataKey::EarliestBlock)?.is_none() {
                let earliest_value = codec::BlockNumberHash(block_number, block_hash);
                cursor.insert(models::MetadataKey::EarliestBlock, &earliest_value)?;
            }

            tx.commit()?;
        }

        Ok(())
    }

    fn storage_trie_cursor(
        &self,
        hashed_address: B256,
        max_block_number: u64,
    ) -> OpProofsStorageResult<Self::StorageTrieCursor> {
        let txn = self.db.tx()?;
        Ok(MdbxOpProofsStorageTrieCursor::new(
            txn.cursor_dup_read::<tables::ExternalStorageBranches>()?,
            hashed_address,
            max_block_number,
        ))
    }

    fn account_trie_cursor(
        &self,
        max_block_number: u64,
    ) -> OpProofsStorageResult<Self::AccountTrieCursor> {
        let txn = self.db.tx()?;
        Ok(BlockNumberVersionedCursor::new(
            txn.cursor_dup_read::<tables::ExternalAccountBranches>()?,
            max_block_number,
        ))
    }

    fn account_hashed_cursor(
        &self,
        max_block_number: u64,
    ) -> OpProofsStorageResult<Self::AccountHashedCursor> {
        let txn = self.db.tx()?;
        // Create a lazy cursor that queries MDBX on-demand
        Ok(MdbxAccountCursor::new(
            txn.cursor_dup_read::<tables::ExternalHashedAccounts>()?,
            max_block_number,
        ))
    }

    fn storage_hashed_cursor(
        &self,
        hashed_address: B256,
        max_block_number: u64,
    ) -> OpProofsStorageResult<Self::StorageCursor> {
        let txn = self.db.tx()?;
        // Create a lazy cursor that queries MDBX on-demand
        Ok(MdbxStorageCursor::new(
            txn.cursor_dup_read::<tables::ExternalHashedStorages>()?,
            max_block_number,
            hashed_address,
        ))
    }

    async fn get_earliest_block_number(&self) -> OpProofsStorageResult<Option<(u64, B256)>> {
        let tx = self.db.tx()?;

        let mut cursor = tx.cursor_read::<tables::ExternalBlockMetadata>()?;

        let result = cursor
            .seek_exact(models::MetadataKey::EarliestBlock)?
            .map(|(_, hash)| hash.into_components());

        Ok(result)
    }

    async fn get_latest_block_number(&self) -> OpProofsStorageResult<Option<(u64, B256)>> {
        let tx = self.db.tx()?;

        let mut cursor = tx.cursor_read::<tables::ExternalBlockMetadata>()?;

        let result = cursor
            .seek_exact(models::MetadataKey::LatestBlock)?
            .map(|(_, hash)| hash.into_components());

        Ok(result)
    }

    async fn fetch_trie_updates(
        &self,
        _block_number: u64,
    ) -> OpProofsStorageResult<BlockStateDiff> {
        // For now, return empty updates
        // A full implementation would need to reconstruct the exact changes
        // made in a specific block, which requires more complex logic
        // This is sufficient for basic operations
        Ok(BlockStateDiff { trie_updates: Default::default(), post_state: Default::default() })
    }

    async fn prune_earliest_state(
        &self,
        new_earliest_block_number: u64,
        _diff: BlockStateDiff,
    ) -> OpProofsStorageResult<()> {
        // Prune all data before new_earliest_block_number
        // This removes old historical data to save space

        let tx = self.db.tx_mut()?;

        // Delete entries with block_number < new_earliest_block_number from all tables
        // Note: We keep block 0 as the new base state, which we'll update with the diff

        // TODO: Implement actual pruning by iterating through tables and deleting old entries
        // For MVP, we'll just update the metadata
        // A full implementation would:
        // 1. Iterate through each table (AccountBranches, StorageBranches, etc.)
        // 2. Delete entries where block_number > 0 AND block_number < new_earliest_block_number
        // 3. Update block 0 with the consolidated diff
        // 4. Update index tables accordingly

        // Update metadata
        let mut cursor = tx.cursor_write::<tables::ExternalBlockMetadata>()?;

        // Note: We need the block hash, but diff doesn't have it
        // For now, use zero hash - this should be fixed in the trait design
        let hash = B256::ZERO;

        let value = codec::BlockNumberHash::new(new_earliest_block_number, hash);
        cursor.upsert(models::MetadataKey::EarliestBlock, &value)?;

        tx.commit()?;
        Ok(())
    }

    async fn replace_updates(
        &self,
        _latest_common_block_number: u64,
        _blocks_to_add: HashMap<u64, BlockStateDiff>,
    ) -> OpProofsStorageResult<()> {
        // Handle chain reorganization:
        // 1. Delete all blocks > latest_common_block_number (the reorg'd blocks)
        // 2. Add the new blocks from blocks_to_add
        // 3. Update LatestBlock metadata

        let tx = self.db.tx_mut()?;

        // TODO: Implement actual reorg handling
        // For MVP, we'll just update the metadata
        // A full implementation would:
        // 1. Delete all entries where block_number > latest_common_block_number
        // 2. For each block in blocks_to_add, call store_trie_updates
        // 3. Update LatestBlock metadata to the highest block in blocks_to_add
        // 4. Update index tables accordingly

        // For now, just commit empty transaction
        tx.commit()?;
        Ok(())
    }

    async fn set_earliest_block_number(
        &self,
        block_number: u64,
        hash: B256,
    ) -> OpProofsStorageResult<()> {
        let tx = self.db.tx_mut()?;

        let mut cursor = tx.cursor_write::<tables::ExternalBlockMetadata>()?;

        let value = codec::BlockNumberHash::new(block_number, hash);
        cursor.upsert(models::MetadataKey::EarliestBlock, &value)?;

        tx.commit()?;
        Ok(())
    }
}
