//! MDBX cursor implementations for external storage
//!
//! This module provides efficient cursors that merge versioned data from multiple blocks.
//! The cursors use a streaming merge approach that processes data in a single pass.

use crate::storage::{
    ExternalHashedCursor, ExternalStorageError, ExternalStorageResult, ExternalTrieCursor,
};
use alloy_primitives::{B256, U256};
use reth_db::{DatabaseEnv, DatabaseError};
use reth_db_api::{
    cursor::{DbCursorRO, DbDupCursorRO},
    transaction::DbTx,
};
use reth_primitives_traits::Account;
use reth_trie::{BranchNodeCompact, Nibbles, StoredNibbles};
use std::sync::Arc;

use super::{codec, models, tables};

/// Cursor over account trie branches
///
/// Uses streaming merge: iterates through sorted data once, merging versions as encountered.
pub struct MdbxTrieCursor<Cursor> {
    cursor: Cursor,
    hashed_address: Option<B256>,
    max_block_number: u64,
}

impl<
        Cursor: DbCursorRO<tables::ExternalAccountBranches>
            + DbDupCursorRO<tables::ExternalAccountBranches>,
    > MdbxTrieCursor<Cursor>
{
    pub(crate) fn new(cursor: Cursor, hashed_address: Option<B256>, max_block_number: u64) -> Self {
        Self { cursor, hashed_address, max_block_number }
    }

    fn get_latest_key_value(
        &mut self,
        target_path: Nibbles,
    ) -> ExternalStorageResult<Option<(Nibbles, BranchNodeCompact)>> {
        // Find all versions of this path
        self.cursor
            .seek(StoredNibbles(target_path))
            .map_err(|e| ExternalStorageError::Other(e.into()))?;

        loop {
            let current = self.cursor.prev().map_err(|e| ExternalStorageError::Other(e.into()))?;

            let Some(current) = current else {
                return Ok(None);
            };

            if current.

            return current.1 .0;
        }

        return Ok(self.cursor.prev().map_err(|e| ExternalStorageError::Other(e.into()))?);
    }

    fn get_next_key_value(
        &mut self,
        target_path: Nibbles,
    ) -> ExternalStorageResult<Option<(Nibbles, BranchNodeCompact)>> {
        let next_key =
            self.cursor.next_no_dup().map_err(|e| ExternalStorageError::Other(e.into()))?;

        if let Some(next_key) = next_key {
            return self.get_latest_key_value(next_key.0);
        }

        return Ok(None);
    }
}

impl<Cursor: DbCursorRO<tables::ExternalAccountBranches>> ExternalTrieCursor
    for MdbxTrieCursor<Cursor>
{
    fn seek_exact(
        &mut self,
        target_path: Nibbles,
    ) -> ExternalStorageResult<Option<(Nibbles, BranchNodeCompact)>> {
        self.get_latest_key_value(target_path).and_then(|entry| {
            if entry.0 .0 == target_path {
                Some((entry.0, entry.1))
            } else {
                None
            }
        })
    }

    fn seek(
        &mut self,
        target_path: Nibbles,
    ) -> ExternalStorageResult<Option<(Nibbles, BranchNodeCompact)>> {
        // Initialize iterator at target position
        self.ensure_iter_state()?;
        let state = self.iter_state.as_mut().unwrap();

        // Seek to the target path
        let seek_key = models::BlockPath(0, target_path.into());
        state.current =
            state.cursor.seek(seek_key).map_err(|e| ExternalStorageError::Other(e.into()))?;
        state.last_emitted = None;

        // Get next merged entry (which will be >= target)
        self.advance_to_next_merged()
    }

    fn next(&mut self) -> ExternalStorageResult<Option<(Nibbles, BranchNodeCompact)>> {
        self.advance_to_next_merged()
    }

    fn current(&mut self) -> ExternalStorageResult<Option<Nibbles>> {
        Ok(self.iter_state.as_ref().and_then(|s| s.last_emitted.clone()))
    }
}

/// Cursor over hashed accounts
pub struct MdbxAccountHashedCursor {
    db: Arc<DatabaseEnv>,
    max_block_number: u64,
    iter_state: Option<AccountIterState>,
}

struct AccountIterState {
    tx: <DatabaseEnv as reth_db_api::database::Database>::TX,
    cursor:
        <DatabaseEnv as reth_db_api::database::Database>::Cursor<tables::ExternalHashedAccounts>,
    last_emitted: Option<B256>,
    current: Option<(crate::models::BlockNumberHashedAddress, codec::MaybeDeleted<Account>)>,
}

impl MdbxAccountHashedCursor {
    pub(crate) fn new(db: Arc<DatabaseEnv>, max_block_number: u64) -> Self {
        Self { db, max_block_number, iter_state: None }
    }

    fn ensure_iter_state(&mut self) -> ExternalStorageResult<()> {
        if self.iter_state.is_some() {
            return Ok(());
        }

        let tx = self.db.tx().map_err(|e| ExternalStorageError::Other(e.into()))?;
        let cursor = tx
            .cursor_read::<tables::ExternalHashedAccounts>()
            .map_err(|e| ExternalStorageError::Other(e.into()))?;

        self.iter_state = Some(AccountIterState { tx, cursor, last_emitted: None, current: None });

        Ok(())
    }

    fn advance_to_next_merged(&mut self) -> ExternalStorageResult<Option<(B256, Account)>> {
        self.ensure_iter_state()?;
        let state = self.iter_state.as_mut().unwrap();

        if state.current.is_none() {
            state.current = if let Some(last) = state.last_emitted {
                let next_key = crate::models::BlockNumberHashedAddress::new(0, last);
                state.cursor.seek(next_key).map_err(|e| ExternalStorageError::Other(e.into()))?
            } else {
                state.cursor.first().map_err(|e| ExternalStorageError::Other(e.into()))?
            };
        }

        let mut current_address: Option<B256> = None;
        let mut latest_account: Option<Account> = None;

        while let Some((key, value)) = state.current.take() {
            let address = key.hashed_address();
            let block_num = key.block_number();

            if block_num > self.max_block_number {
                state.current =
                    state.cursor.next().map_err(|e| ExternalStorageError::Other(e.into()))?;
                if let Some((next_key, _)) = &state.current {
                    if Some(next_key.hashed_address()) != current_address
                        && current_address.is_some()
                    {
                        break;
                    }
                }
                continue;
            }

            if current_address.is_some() && Some(address) != current_address {
                break;
            }

            if current_address.is_none() {
                current_address = Some(address);
            }

            if Some(address) == state.last_emitted {
                state.current =
                    state.cursor.next().map_err(|e| ExternalStorageError::Other(e.into()))?;
                continue;
            }

            match value.into() {
                Some(account) => latest_account = Some(account),
                None => latest_account = None,
            }

            state.current =
                state.cursor.next().map_err(|e| ExternalStorageError::Other(e.into()))?;
        }

        if let (Some(address), Some(account)) = (current_address, latest_account) {
            state.last_emitted = Some(address);
            Ok(Some((address, account)))
        } else {
            Ok(None)
        }
    }
}

impl ExternalHashedCursor for MdbxAccountHashedCursor {
    type Value = Account;

    fn seek(&mut self, key: B256) -> ExternalStorageResult<Option<(B256, Self::Value)>> {
        self.ensure_iter_state()?;
        let state = self.iter_state.as_mut().unwrap();

        let seek_key = crate::models::BlockNumberHashedAddress::new(0, key);
        state.current =
            state.cursor.seek(seek_key).map_err(|e| ExternalStorageError::Other(e.into()))?;
        state.last_emitted = None;

        self.advance_to_next_merged()
    }

    fn next(&mut self) -> ExternalStorageResult<Option<(B256, Self::Value)>> {
        self.advance_to_next_merged()
    }

    fn is_storage_empty(&mut self) -> ExternalStorageResult<bool> {
        Ok(self.seek(B256::ZERO)?.is_none())
    }
}

/// Cursor over hashed storage values
pub struct MdbxStorageCursor {
    db: Arc<DatabaseEnv>,
    hashed_address: B256,
    max_block_number: u64,
    iter_state: Option<StorageIterState>,
}

struct StorageIterState {
    tx: <DatabaseEnv as reth_db_api::database::Database>::TX,
    cursor:
        <DatabaseEnv as reth_db_api::database::Database>::CursorDup<tables::ExternalHashedStorages>,
    last_emitted: Option<B256>,
    current:
        Option<(crate::models::BlockNumberHashedAddress, reth_primitives_traits::StorageEntry)>,
}

impl MdbxStorageCursor {
    pub(crate) fn new(db: Arc<DatabaseEnv>, hashed_address: B256, max_block_number: u64) -> Self {
        Self { db, hashed_address, max_block_number, iter_state: None }
    }

    fn ensure_iter_state(&mut self) -> ExternalStorageResult<()> {
        if self.iter_state.is_some() {
            return Ok(());
        }

        let tx = self.db.tx().map_err(|e| ExternalStorageError::Other(e.into()))?;
        let cursor = tx
            .cursor_dup_read::<tables::ExternalHashedStorages>()
            .map_err(|e| ExternalStorageError::Other(e.into()))?;

        self.iter_state = Some(StorageIterState { tx, cursor, last_emitted: None, current: None });

        Ok(())
    }

    fn advance_to_next_merged(&mut self) -> ExternalStorageResult<Option<(B256, U256)>> {
        self.ensure_iter_state()?;
        let state = self.iter_state.as_mut().unwrap();

        if state.current.is_none() {
            state.current = if let Some(last) = state.last_emitted {
                let next_key = crate::models::BlockNumberHashedAddress::new(0, self.hashed_address);
                state.cursor.seek(next_key).map_err(|e| ExternalStorageError::Other(e.into()))?
            } else {
                let start_key =
                    crate::models::BlockNumberHashedAddress::new(0, self.hashed_address);
                state.cursor.seek(start_key).map_err(|e| ExternalStorageError::Other(e.into()))?
            };
        }

        let mut current_storage_key: Option<B256> = None;
        let mut latest_value: Option<U256> = None;

        while let Some((key, entry)) = state.current.take() {
            // Only process entries for our address
            if key.hashed_address() != self.hashed_address {
                break;
            }

            let storage_key = entry.key;
            let block_num = key.block_number();

            if block_num > self.max_block_number {
                state.current =
                    state.cursor.next().map_err(|e| ExternalStorageError::Other(e.into()))?;
                if let Some((next_key, next_entry)) = &state.current {
                    if next_key.hashed_address() != self.hashed_address {
                        break;
                    }
                    if Some(next_entry.key) != current_storage_key && current_storage_key.is_some()
                    {
                        break;
                    }
                }
                continue;
            }

            if current_storage_key.is_some() && Some(storage_key) != current_storage_key {
                break;
            }

            if current_storage_key.is_none() {
                current_storage_key = Some(storage_key);
            }

            if Some(storage_key) == state.last_emitted {
                state.current =
                    state.cursor.next().map_err(|e| ExternalStorageError::Other(e.into()))?;
                continue;
            }

            // Update latest value (filter zero values)
            if entry.value.is_zero() {
                latest_value = None;
            } else {
                latest_value = Some(entry.value);
            }

            state.current =
                state.cursor.next().map_err(|e| ExternalStorageError::Other(e.into()))?;
        }

        if let (Some(storage_key), Some(value)) = (current_storage_key, latest_value) {
            state.last_emitted = Some(storage_key);
            Ok(Some((storage_key, value)))
        } else {
            Ok(None)
        }
    }
}

impl ExternalHashedCursor for MdbxStorageCursor {
    type Value = U256;

    fn seek(&mut self, key: B256) -> ExternalStorageResult<Option<(B256, Self::Value)>> {
        self.ensure_iter_state()?;
        let state = self.iter_state.as_mut().unwrap();

        let seek_key = crate::models::BlockNumberHashedAddress::new(0, self.hashed_address);
        state.current =
            state.cursor.seek(seek_key).map_err(|e| ExternalStorageError::Other(e.into()))?;
        state.last_emitted = None;

        // Skip to the target storage key
        while let Some((_, entry)) = &state.current {
            if entry.key >= key {
                break;
            }
            state.current =
                state.cursor.next().map_err(|e| ExternalStorageError::Other(e.into()))?;
        }

        self.advance_to_next_merged()
    }

    fn next(&mut self) -> ExternalStorageResult<Option<(B256, Self::Value)>> {
        self.advance_to_next_merged()
    }

    fn is_storage_empty(&mut self) -> ExternalStorageResult<bool> {
        Ok(self.seek(B256::ZERO)?.is_none())
    }
}
