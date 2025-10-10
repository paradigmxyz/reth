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
    mdbx::VersionedValue,
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
        T: Table<Value = VersionedValue<V>> + DupSort<SubKey = u64>,
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
        // Try to position cursor at or after max_block_number
        // If found, the cursor is positioned at a value with block_number >= max_block_number
        let seek_result = self
            .cursor
            .seek_by_key_subkey(target_path.clone(), self.max_block_number)
            .map_err(|e| OpProofsStorageError::Other(e.into()))?;

        if seek_result.is_some() {
            // Found a value >= max_block_number, go back one to get the latest value <= max_block_number
            return self.cursor.prev_dup().map_err(|e| OpProofsStorageError::Other(e.into()));
        }

        // No value >= max_block_number exists
        // This means all values for this key have block_number < max_block_number
        // So we want the last (highest block_number) value for this key

        // First check if the key exists at all
        if self
            .cursor
            .seek_exact(target_path.clone())
            .map_err(|e| OpProofsStorageError::Other(e.into()))?
            .is_none()
        {
            // Key doesn't exist
            return Ok(None);
        }

        // Key exists, find the last dup entry (which will have the highest block_number < max_block_number)
        let mut last = None;
        while let Some(kv) =
            self.cursor.next_dup().map_err(|e| OpProofsStorageError::Other(e.into()))?
        {
            last = Some(kv);
        }

        // If last is None, it means there's only one entry (the one from seek_exact)
        if last.is_none() {
            // Go back to get that first entry
            self.cursor.seek_exact(target_path).map_err(|e| OpProofsStorageError::Other(e.into()))
        } else {
            Ok(last)
        }
    }

    fn get_next_key_value(
        &mut self,
        target_path: T::Key,
    ) -> OpProofsStorageResult<Option<(T::Key, V)>> {
        // Seek to the next key >= target_path
        let Some((mut key, _)) =
            self.cursor.seek(target_path).map_err(|e| OpProofsStorageError::Other(e.into()))?
        else {
            // No keys >= target_path exist
            return Ok(None);
        };

        loop {
            // Now get the latest value for this key that's <= max_block_number
            let latest = self
                .get_latest_key_value(key.clone())
                .map_err(|e| OpProofsStorageError::Other(e.into()))?;

            if let Some((latest_key, latest_value)) = latest {
                // if non-deleted, return the latest value (extract from VersionedValue)
                if let Some(latest_value) = latest_value.value.0 {
                    return Ok(Some((latest_key, latest_value)));
                }
                // Value was deleted, continue to next key
            }

            // No valid value for this key, or value was deleted - move to next key
            // Re-position cursor at current key first (get_latest_key_value may have moved it)
            if self
                .cursor
                .seek_exact(key.clone())
                .map_err(|e| OpProofsStorageError::Other(e.into()))?
                .is_none()
            {
                // Key disappeared, shouldn't happen
                return Ok(None);
            }

            // Now get the next distinct key
            let Some((next_key, _)) =
                self.cursor.next_no_dup().map_err(|e| OpProofsStorageError::Other(e.into()))?
            else {
                // No more keys
                return Ok(None);
            };
            key = next_key;
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
            if let Some(val) = entry.1.value.0
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
        if self.current()?.is_none() {
            return self.seek(Nibbles::default());
        }

        // Move to next distinct key first (skip past current key)
        let Some((next_key, _)) =
            self.cursor.next_no_dup().map_err(|e| OpProofsStorageError::Other(e.into()))?
        else {
            return Ok(None);
        };

        // Now find the latest valid value for that key
        Ok(self.get_next_key_value(next_key)?.map(|entry| (entry.0 .0, entry.1)))
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
        T: Table<Value = VersionedValue<V>> + DupSort<SubKey = u64>,
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
            if let Some(val) = entry.1.value.0
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
            (subkey.hashed_address == self.hashed_address).then(|| (subkey.path.0.clone(), value))
        }))
    }

    fn next(&mut self) -> OpProofsStorageResult<Option<(Nibbles, BranchNodeCompact)>> {
        if self.current()?.is_none() {
            return self.seek(Nibbles::default());
        }

        // Move to next distinct key first (skip past current key)
        let Some((next_key, _)) =
            self.cursor.cursor.next_no_dup().map_err(|e| OpProofsStorageError::Other(e.into()))?
        else {
            return Ok(None);
        };

        // Check if still in same address
        if next_key.hashed_address != self.hashed_address {
            return Ok(None);
        }

        // Now find the latest valid value for that key
        Ok(self.cursor.get_next_key_value(next_key.clone())?.and_then(|(subkey, value)| {
            (subkey.hashed_address == self.hashed_address).then(|| (subkey.path.0.clone(), value))
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
        // Find first entry >= target_path (get_next_key_value returns non-deleted values)
        Ok(self.cursor.get_next_key_value(target_path)?)
    }

    fn next(&mut self) -> OpProofsStorageResult<Option<(B256, Account)>> {
        if self.cursor.cursor.current()?.is_none() {
            return self.seek(B256::default());
        }

        // Move to next distinct key first (skip past current key)
        let Some((next_key, _)) =
            self.cursor.cursor.next_no_dup().map_err(|e| OpProofsStorageError::Other(e.into()))?
        else {
            return Ok(None);
        };

        Ok(self.cursor.get_next_key_value(next_key)?)
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

        // Find first entry >= target_path for this address (get_next_key_value returns non-deleted values)
        Ok(self.cursor.get_next_key_value(subkey)?.and_then(|(key, val)| {
            if key.hashed_address == self.hashed_address {
                Some((key.hashed_storage_key, U256::from_be_slice(val.as_slice())))
            } else {
                None
            }
        }))
    }

    fn next(&mut self) -> OpProofsStorageResult<Option<(B256, U256)>> {
        if self.cursor.cursor.current()?.is_none() {
            return self.seek(B256::default());
        }

        // Move to next distinct key first (skip past current key)
        let Some((next_key, _)) =
            self.cursor.cursor.next_no_dup().map_err(|e| OpProofsStorageError::Other(e.into()))?
        else {
            return Ok(None);
        };

        // Check if still in same address
        if next_key.hashed_address != self.hashed_address {
            return Ok(None);
        }

        Ok(self.cursor.get_next_key_value(next_key)?.and_then(|(subkey, value)| {
            (subkey.hashed_address == self.hashed_address)
                .then(|| (subkey.hashed_storage_key, U256::from_be_slice(value.as_slice())))
        }))
    }
}
