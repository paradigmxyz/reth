use super::{HashedCursor, HashedCursorFactory, HashedStorageCursor};
use crate::{
    forward_cursor::ForwardInMemoryCursor, HashedAccountsSorted, HashedPostStateSorted,
    HashedStorageSorted,
};
use alloy_primitives::{B256, U256};
use reth_primitives::Account;
use reth_storage_errors::db::DatabaseError;
use std::collections::HashSet;

/// The hashed cursor factory for the post state.
#[derive(Clone, Debug)]
pub struct HashedPostStateCursorFactory<'a, CF> {
    cursor_factory: CF,
    post_state: &'a HashedPostStateSorted,
}

impl<'a, CF> HashedPostStateCursorFactory<'a, CF> {
    /// Create a new factory.
    pub const fn new(cursor_factory: CF, post_state: &'a HashedPostStateSorted) -> Self {
        Self { cursor_factory, post_state }
    }
}

impl<'a, CF: HashedCursorFactory> HashedCursorFactory for HashedPostStateCursorFactory<'a, CF> {
    type AccountCursor = HashedPostStateAccountCursor<'a, CF::AccountCursor>;
    type StorageCursor = HashedPostStateStorageCursor<'a, CF::StorageCursor>;

    fn hashed_account_cursor(&self) -> Result<Self::AccountCursor, DatabaseError> {
        let cursor = self.cursor_factory.hashed_account_cursor()?;
        Ok(HashedPostStateAccountCursor::new(cursor, &self.post_state.accounts))
    }

    fn hashed_storage_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageCursor, DatabaseError> {
        let cursor = self.cursor_factory.hashed_storage_cursor(hashed_address)?;
        Ok(HashedPostStateStorageCursor::new(cursor, self.post_state.storages.get(&hashed_address)))
    }
}

/// The cursor to iterate over post state hashed accounts and corresponding database entries.
/// It will always give precedence to the data from the hashed post state.
#[derive(Debug)]
pub struct HashedPostStateAccountCursor<'a, C> {
    /// The database cursor.
    cursor: C,
    /// Forward-only in-memory cursor over accounts.
    post_state_cursor: ForwardInMemoryCursor<'a, B256, Account>,
    /// Reference to the collection of account keys that were destroyed.
    destroyed_accounts: &'a HashSet<B256>,
    /// The last hashed account that was returned by the cursor.
    /// De facto, this is a current cursor position.
    last_account: Option<B256>,
}

impl<'a, C> HashedPostStateAccountCursor<'a, C>
where
    C: HashedCursor<Value = Account>,
{
    /// Create new instance of [`HashedPostStateAccountCursor`].
    pub const fn new(cursor: C, post_state_accounts: &'a HashedAccountsSorted) -> Self {
        let post_state_cursor = ForwardInMemoryCursor::new(&post_state_accounts.accounts);
        let destroyed_accounts = &post_state_accounts.destroyed_accounts;
        Self { cursor, post_state_cursor, destroyed_accounts, last_account: None }
    }

    /// Returns `true` if the account has been destroyed.
    /// This check is used for evicting account keys from the state trie.
    ///
    /// This function only checks the post state, not the database, because the latter does not
    /// store destroyed accounts.
    fn is_account_cleared(&self, account: &B256) -> bool {
        self.destroyed_accounts.contains(account)
    }

    fn seek_inner(&mut self, key: B256) -> Result<Option<(B256, Account)>, DatabaseError> {
        // Take the next account from the post state with the key greater than or equal to the
        // sought key.
        let post_state_entry = self.post_state_cursor.seek(&key);

        // It's an exact match, return the account from post state without looking up in the
        // database.
        if post_state_entry.map_or(false, |entry| entry.0 == key) {
            return Ok(post_state_entry)
        }

        // It's not an exact match, reposition to the first greater or equal account that wasn't
        // cleared.
        let mut db_entry = self.cursor.seek(key)?;
        while db_entry.as_ref().map_or(false, |(address, _)| self.is_account_cleared(address)) {
            db_entry = self.cursor.next()?;
        }

        // Compare two entries and return the lowest.
        Ok(Self::compare_entries(post_state_entry, db_entry))
    }

    fn next_inner(&mut self, last_account: B256) -> Result<Option<(B256, Account)>, DatabaseError> {
        // Take the next account from the post state with the key greater than the last sought key.
        let post_state_entry = self.post_state_cursor.first_after(&last_account);

        // If post state was given precedence or account was cleared, move the cursor forward.
        let mut db_entry = self.cursor.seek(last_account)?;
        while db_entry.as_ref().map_or(false, |(address, _)| {
            address <= &last_account || self.is_account_cleared(address)
        }) {
            db_entry = self.cursor.next()?;
        }

        // Compare two entries and return the lowest.
        Ok(Self::compare_entries(post_state_entry, db_entry))
    }

    /// Return the account with the lowest hashed account key.
    ///
    /// Given the next post state and database entries, return the smallest of the two.
    /// If the account keys are the same, the post state entry is given precedence.
    fn compare_entries(
        post_state_item: Option<(B256, Account)>,
        db_item: Option<(B256, Account)>,
    ) -> Option<(B256, Account)> {
        if let Some((post_state_entry, db_entry)) = post_state_item.zip(db_item) {
            // If both are not empty, return the smallest of the two
            // Post state is given precedence if keys are equal
            Some(if post_state_entry.0 <= db_entry.0 { post_state_entry } else { db_entry })
        } else {
            // Return either non-empty entry
            db_item.or(post_state_item)
        }
    }
}

impl<C> HashedCursor for HashedPostStateAccountCursor<'_, C>
where
    C: HashedCursor<Value = Account>,
{
    type Value = Account;

    /// Seek the next entry for a given hashed account key.
    ///
    /// If the post state contains the exact match for the key, return it.
    /// Otherwise, retrieve the next entries that are greater than or equal to the key from the
    /// database and the post state. The two entries are compared and the lowest is returned.
    ///
    /// The returned account key is memoized and the cursor remains positioned at that key until
    /// [`HashedCursor::seek`] or [`HashedCursor::next`] are called.
    fn seek(&mut self, key: B256) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        // Find the closes account.
        let entry = self.seek_inner(key)?;
        self.last_account = entry.as_ref().map(|entry| entry.0);
        Ok(entry)
    }

    /// Retrieve the next entry from the cursor.
    ///
    /// If the cursor is positioned at the entry, return the entry with next greater key.
    /// Returns [None] if the previous memoized or the next greater entries are missing.
    ///
    /// NOTE: This function will not return any entry unless [`HashedCursor::seek`] has been
    /// called.
    fn next(&mut self) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        let next = match self.last_account {
            Some(account) => {
                let entry = self.next_inner(account)?;
                self.last_account = entry.as_ref().map(|entry| entry.0);
                entry
            }
            // no previous entry was found
            None => None,
        };
        Ok(next)
    }
}

/// The cursor to iterate over post state hashed storages and corresponding database entries.
/// It will always give precedence to the data from the post state.
#[derive(Debug)]
pub struct HashedPostStateStorageCursor<'a, C> {
    /// The database cursor.
    cursor: C,
    /// Forward-only in-memory cursor over non zero-valued account storage slots.
    post_state_cursor: Option<ForwardInMemoryCursor<'a, B256, U256>>,
    /// Reference to the collection of storage slot keys that were cleared.
    cleared_slots: Option<&'a HashSet<B256>>,
    /// Flag indicating whether database storage was wiped.
    storage_wiped: bool,
    /// The last slot that has been returned by the cursor.
    /// De facto, this is the cursor's position for the given account key.
    last_slot: Option<B256>,
}

impl<'a, C> HashedPostStateStorageCursor<'a, C>
where
    C: HashedStorageCursor<Value = U256>,
{
    /// Create new instance of [`HashedPostStateStorageCursor`] for the given hashed address.
    pub fn new(cursor: C, post_state_storage: Option<&'a HashedStorageSorted>) -> Self {
        let post_state_cursor =
            post_state_storage.map(|s| ForwardInMemoryCursor::new(&s.non_zero_valued_slots));
        let cleared_slots = post_state_storage.map(|s| &s.zero_valued_slots);
        let storage_wiped = post_state_storage.map_or(false, |s| s.wiped);
        Self { cursor, post_state_cursor, cleared_slots, storage_wiped, last_slot: None }
    }

    /// Check if the slot was zeroed out in the post state.
    /// The database is not checked since it already has no zero-valued slots.
    fn is_slot_zero_valued(&self, slot: &B256) -> bool {
        self.cleared_slots.map_or(false, |s| s.contains(slot))
    }

    /// Find the storage entry in post state or database that's greater or equal to provided subkey.
    fn seek_inner(&mut self, subkey: B256) -> Result<Option<(B256, U256)>, DatabaseError> {
        // Attempt to find the account's storage in post state.
        let post_state_entry = self.post_state_cursor.as_mut().and_then(|c| c.seek(&subkey));

        // If database storage was wiped or it's an exact match,
        // return the storage slot from post state without looking up in the database.
        if self.storage_wiped || post_state_entry.map_or(false, |entry| entry.0 == subkey) {
            return Ok(post_state_entry)
        }

        // It's not an exact match and storage was not wiped,
        // reposition to the first greater or equal account.
        let mut db_entry = self.cursor.seek(subkey)?;
        while db_entry.as_ref().map_or(false, |entry| self.is_slot_zero_valued(&entry.0)) {
            db_entry = self.cursor.next()?;
        }

        // Compare two entries and return the lowest.
        Ok(Self::compare_entries(post_state_entry, db_entry))
    }

    /// Find the storage entry that is right after current cursor position.
    fn next_inner(&mut self, last_slot: B256) -> Result<Option<(B256, U256)>, DatabaseError> {
        // Attempt to find the account's storage in post state.
        let post_state_entry =
            self.post_state_cursor.as_mut().and_then(|c| c.first_after(&last_slot));

        // Return post state entry immediately if database was wiped.
        if self.storage_wiped {
            return Ok(post_state_entry)
        }

        // If post state was given precedence, move the cursor forward.
        // If the entry was already returned or is zero-valued, move to the next.
        let mut db_entry = self.cursor.seek(last_slot)?;
        while db_entry
            .as_ref()
            .map_or(false, |entry| entry.0 == last_slot || self.is_slot_zero_valued(&entry.0))
        {
            db_entry = self.cursor.next()?;
        }

        // Compare two entries and return the lowest.
        Ok(Self::compare_entries(post_state_entry, db_entry))
    }

    /// Return the storage entry with the lowest hashed storage key (hashed slot).
    ///
    /// Given the next post state and database entries, return the smallest of the two.
    /// If the storage keys are the same, the post state entry is given precedence.
    fn compare_entries(
        post_state_item: Option<(B256, U256)>,
        db_item: Option<(B256, U256)>,
    ) -> Option<(B256, U256)> {
        if let Some((post_state_entry, db_entry)) = post_state_item.zip(db_item) {
            // If both are not empty, return the smallest of the two
            // Post state is given precedence if keys are equal
            Some(if post_state_entry.0 <= db_entry.0 { post_state_entry } else { db_entry })
        } else {
            // Return either non-empty entry
            db_item.or(post_state_item)
        }
    }
}

impl<C> HashedCursor for HashedPostStateStorageCursor<'_, C>
where
    C: HashedStorageCursor<Value = U256>,
{
    type Value = U256;

    /// Seek the next account storage entry for a given hashed key pair.
    fn seek(&mut self, subkey: B256) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        let entry = self.seek_inner(subkey)?;
        self.last_slot = entry.as_ref().map(|entry| entry.0);
        Ok(entry)
    }

    /// Return the next account storage entry for the current account key.
    fn next(&mut self) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        let next = match self.last_slot {
            Some(last_slot) => {
                let entry = self.next_inner(last_slot)?;
                self.last_slot = entry.as_ref().map(|entry| entry.0);
                entry
            }
            // no previous entry was found
            None => None,
        };
        Ok(next)
    }
}

impl<C> HashedStorageCursor for HashedPostStateStorageCursor<'_, C>
where
    C: HashedStorageCursor<Value = U256>,
{
    /// Returns `true` if the account has no storage entries.
    ///
    /// This function should be called before attempting to call [`HashedCursor::seek`] or
    /// [`HashedCursor::next`].
    fn is_storage_empty(&mut self) -> Result<bool, DatabaseError> {
        let is_empty = match &self.post_state_cursor {
            Some(cursor) => {
                // If the storage has been wiped at any point
                self.storage_wiped &&
                    // and the current storage does not contain any non-zero values
                    cursor.is_empty()
            }
            None => self.cursor.is_storage_empty()?,
        };
        Ok(is_empty)
    }
}
