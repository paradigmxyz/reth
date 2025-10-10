//! MDBX cursor implementations for external storage
//!
//! This module provides efficient cursors that merge versioned data from multiple blocks.
//! The cursors use a streaming merge approach that processes data in a single pass.
//!
use std::marker::PhantomData;

use alloy_primitives::{B256, U256};
use reth_db_api::cursor::{DbCursorRO, DbDupCursorRO};
use reth_db_api::table::{DupSort, Table};
use reth_primitives_traits::Account;
use reth_trie::{BranchNodeCompact, Nibbles, StoredNibbles};

use super::tables;
use crate::mdbx::{HashedStorageSubKey, StorageBranchSubKey};
use crate::{
    mdbx::MaybeDeleted,
    storage::{
        OpProofsHashedCursor, OpProofsStorageError, OpProofsStorageResult, OpProofsTrieCursor,
    },
};

/// Cursor over account trie branches
///
/// Uses streaming merge: iterates through sorted data once, merging versions as encountered.
#[derive(Debug, Clone)]
pub struct BlockNumberVersionedCursor<T: Table + DupSort, Cursor> {
    _table: PhantomData<T>,
    cursor: Cursor,
    max_block_number: u64,
}

impl<
        V,
        T: Table<Value = MaybeDeleted<V>> + DupSort<SubKey = u64>,
        Cursor: DbCursorRO<T> + DbDupCursorRO<T>,
    > BlockNumberVersionedCursor<T, Cursor>
{
    pub(crate) fn new(cursor: Cursor, max_block_number: u64) -> Self {
        Self { cursor, max_block_number, _table: PhantomData }
    }

    fn get_latest_key_value(
        &mut self,
        target_path: T::Key,
    ) -> OpProofsStorageResult<Option<(T::Key, T::Value)>> {
        // Position cursor at key after the requested version
        self.cursor
            .seek_by_key_subkey(target_path, self.max_block_number)
            .map_err(|e| OpProofsStorageError::Other(e.into()))?;

        // Seek to the previous dup value to find the latest value, or None if no values exist at this height
        let prev = self.cursor.prev_dup().map_err(|e| OpProofsStorageError::Other(e.into()))?;

        return Ok(prev);
    }

    fn get_next_key_value(
        &mut self,
        target_path: T::Key,
    ) -> OpProofsStorageResult<Option<(T::Key, V)>> {
        let mut target_path = target_path;
        loop {
            // seek to the next key
            if self
                .cursor
                .seek_by_key_subkey(target_path, u64::MAX)
                .map_err(|e| OpProofsStorageError::Other(e.into()))?
                .is_none()
            {
                // there are no keys after this key
                return Ok(None);
            };

            let Some((key, _)) =
                self.cursor.current().map_err(|e| OpProofsStorageError::Other(e.into()))?
            else {
                // there are no keys after this key
                return Ok(None);
            };

            let Some((latest_key, latest_value)) = self
                .get_latest_key_value(key.clone())
                .map_err(|e| OpProofsStorageError::Other(e.into()))?
            else {
                // end of trie
                return Ok(None);
            };

            // ensure latest_value is not the same key (should never happen)
            assert_ne!(&latest_key, &key, "latest_value is the same key");

            // if non-deleted, return the latest value
            if let Some(latest_value) = latest_value.0 {
                return Ok(Some((latest_key, latest_value)));
            }

            // if the node was deleted, continue to the next key
            target_path = latest_key;
        }
    }
}

impl<
        Cursor: DbCursorRO<tables::ExternalAccountBranches>
            + DbDupCursorRO<tables::ExternalAccountBranches>
            + Send
            + Sync,
    > OpProofsTrieCursor for BlockNumberVersionedCursor<tables::ExternalAccountBranches, Cursor>
{
    fn seek_exact(
        &mut self,
        target_path: Nibbles,
    ) -> OpProofsStorageResult<Option<(Nibbles, BranchNodeCompact)>> {
        Ok(self.get_latest_key_value(StoredNibbles(target_path))?.and_then(|entry| {
            if let Some(val) = entry.1 .0
                && entry.0 .0 == target_path
            {
                Some((entry.0 .0, val))
            } else {
                None
            }
        }))
    }

    fn seek(
        &mut self,
        target_path: Nibbles,
    ) -> OpProofsStorageResult<Option<(Nibbles, BranchNodeCompact)>> {
        Ok(self.get_next_key_value(StoredNibbles(target_path))?.map(|entry| (entry.0 .0, entry.1)))
    }

    fn next(&mut self) -> OpProofsStorageResult<Option<(Nibbles, BranchNodeCompact)>> {
        let Some(current) = self.current()? else {
            return self.seek(Nibbles::default());
        };

        Ok(self.get_next_key_value(StoredNibbles(current))?.map(|entry| (entry.0 .0, entry.1)))
    }

    fn current(&mut self) -> OpProofsStorageResult<Option<Nibbles>> {
        Ok(self
            .cursor
            .current()
            .map_err(|e| OpProofsStorageError::Other(e.into()))?
            .map(|entry| entry.0 .0))
    }
}

/// Cursor over storage trie branches
#[derive(Debug)]
pub struct MdbxOpProofsStorageTrieCursor<T: Table + DupSort, Cursor> {
    hashed_address: B256,
    cursor: BlockNumberVersionedCursor<T, Cursor>,
}

impl<
        V,
        T: Table<Value = MaybeDeleted<V>> + DupSort<SubKey = u64>,
        Cursor: DbCursorRO<T> + DbDupCursorRO<T>,
    > MdbxOpProofsStorageTrieCursor<T, Cursor>
{
    pub(crate) fn new(cursor: Cursor, hashed_address: B256, max_block_number: u64) -> Self {
        Self { hashed_address, cursor: BlockNumberVersionedCursor::new(cursor, max_block_number) }
    }
}

impl<
        Cursor: DbCursorRO<tables::ExternalStorageBranches>
            + DbDupCursorRO<tables::ExternalStorageBranches>
            + Send
            + Sync,
    > OpProofsTrieCursor
    for MdbxOpProofsStorageTrieCursor<tables::ExternalStorageBranches, Cursor>
{
    fn seek_exact(
        &mut self,
        target_path: Nibbles,
    ) -> OpProofsStorageResult<Option<(Nibbles, BranchNodeCompact)>> {
        let subkey = StorageBranchSubKey::new(self.hashed_address, StoredNibbles(target_path));

        Ok(self.cursor.get_latest_key_value(subkey.clone())?.and_then(|entry| {
            if let Some(val) = entry.1 .0
                && entry.0 == subkey
            {
                Some((entry.0.path.0.clone(), val))
            } else {
                None
            }
        }))
    }

    fn seek(
        &mut self,
        target_path: Nibbles,
    ) -> OpProofsStorageResult<Option<(Nibbles, BranchNodeCompact)>> {
        let subkey = StorageBranchSubKey::new(self.hashed_address, StoredNibbles(target_path));

        Ok(self.cursor.get_next_key_value(subkey)?.and_then(|(subkey, value)| {
            (subkey.hashed_address == subkey.hashed_address).then(|| (subkey.path.0.clone(), value))
        }))
    }

    fn next(&mut self) -> OpProofsStorageResult<Option<(Nibbles, BranchNodeCompact)>> {
        let Some(current) = self.current()? else {
            return self.seek(Nibbles::default());
        };

        let subkey = StorageBranchSubKey::new(self.hashed_address, StoredNibbles(current));

        Ok(self.cursor.get_next_key_value(subkey)?.and_then(|(subkey, value)| {
            (subkey.hashed_address == subkey.hashed_address).then(|| (subkey.path.0.clone(), value))
        }))
    }

    fn current(&mut self) -> OpProofsStorageResult<Option<Nibbles>> {
        Ok(self
            .cursor
            .cursor
            .current()
            .map_err(|e| OpProofsStorageError::Other(e.into()))?
            .map(|entry| entry.0.path.0))
    }
}

/// Cursor over account trie branches
#[derive(Debug)]
pub struct MdbxAccountCursor<Cursor> {
    cursor: BlockNumberVersionedCursor<tables::ExternalHashedAccounts, Cursor>,
}

impl<
        Cursor: DbCursorRO<tables::ExternalHashedAccounts>
            + DbDupCursorRO<tables::ExternalHashedAccounts>
            + Send
            + Sync,
    > MdbxAccountCursor<Cursor>
{
    pub(crate) fn new(cursor: Cursor, block_number: u64) -> Self {
        Self { cursor: BlockNumberVersionedCursor::new(cursor, block_number) }
    }
}

impl<
        Cursor: DbCursorRO<tables::ExternalHashedAccounts>
            + DbDupCursorRO<tables::ExternalHashedAccounts>
            + Send
            + Sync,
    > OpProofsHashedCursor for MdbxAccountCursor<Cursor>
{
    type Value = Account;

    fn seek(&mut self, target_path: B256) -> OpProofsStorageResult<Option<(B256, Account)>> {
        Ok(self.cursor.get_latest_key_value(target_path)?.and_then(|entry| {
            if let Some(val) = entry.1 .0
                && entry.0 == target_path
            {
                Some((entry.0, val))
            } else {
                None
            }
        }))
    }

    fn next(&mut self) -> OpProofsStorageResult<Option<(B256, Account)>> {
        let Some((current, _)) = self.cursor.cursor.current()? else {
            return self.seek(B256::default());
        };

        Ok(self.cursor.get_next_key_value(current)?)
    }
}

/// Cursor over storage trie branches
#[derive(Debug)]
pub struct MdbxStorageCursor<Cursor> {
    cursor: BlockNumberVersionedCursor<tables::ExternalHashedStorages, Cursor>,
    hashed_address: B256,
}

impl<
        Cursor: DbCursorRO<tables::ExternalHashedStorages>
            + DbDupCursorRO<tables::ExternalHashedStorages>
            + Send
            + Sync,
    > MdbxStorageCursor<Cursor>
{
    pub(crate) fn new(cursor: Cursor, block_number: u64, hashed_address: B256) -> Self {
        Self { cursor: BlockNumberVersionedCursor::new(cursor, block_number), hashed_address }
    }
}

impl<
        Cursor: DbCursorRO<tables::ExternalHashedStorages>
            + DbDupCursorRO<tables::ExternalHashedStorages>
            + Send
            + Sync,
    > OpProofsHashedCursor for MdbxStorageCursor<Cursor>
{
    type Value = U256;

    fn seek(&mut self, target_path: B256) -> OpProofsStorageResult<Option<(B256, U256)>> {
        let subkey = HashedStorageSubKey {
            hashed_address: self.hashed_address,
            hashed_storage_key: target_path,
        };

        Ok(self.cursor.get_latest_key_value(subkey.clone())?.and_then(
            |(key, maybe_deleted_val)| {
                if let Some(val) = maybe_deleted_val.0
                    && key == subkey
                {
                    Some((key.hashed_storage_key, U256::from_be_slice(val.as_slice())))
                } else {
                    None
                }
            },
        ))
    }

    fn next(&mut self) -> OpProofsStorageResult<Option<(B256, U256)>> {
        let Some((current, _)) = self.cursor.cursor.current()? else {
            return self.seek(B256::default());
        };

        Ok(self.cursor.get_next_key_value(current)?.and_then(|(subkey, value)| {
            (subkey.hashed_address == subkey.hashed_address)
                .then(|| (subkey.hashed_storage_key, U256::from_be_slice(value.as_slice())))
        }))
    }
}
