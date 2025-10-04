use super::{HashedCursor, HashedCursorFactory, HashedStorageCursor};
use crate::forward_cursor::ForwardInMemoryCursor;
use alloy_primitives::{B256, U256};
use reth_primitives_traits::Account;
use reth_storage_errors::db::DatabaseError;
use reth_trie_common::{HashedAccountsSorted, HashedPostStateSorted, HashedStorageSorted};

/// The hashed cursor factory for the post state.
#[derive(Clone, Debug)]
pub struct HashedPostStateCursorFactory<CF, T> {
    cursor_factory: CF,
    post_state: T,
}

impl<CF, T> HashedPostStateCursorFactory<CF, T> {
    /// Create a new factory.
    pub const fn new(cursor_factory: CF, post_state: T) -> Self {
        Self { cursor_factory, post_state }
    }
}

impl<'a, CF, T> HashedCursorFactory for HashedPostStateCursorFactory<CF, &'a T>
where
    CF: HashedCursorFactory,
    T: AsRef<HashedPostStateSorted>,
{
    type AccountCursor = HashedPostStateAccountCursor<'a, CF::AccountCursor>;
    type StorageCursor = HashedPostStateStorageCursor<'a, CF::StorageCursor>;

    fn hashed_account_cursor(&self) -> Result<Self::AccountCursor, DatabaseError> {
        let cursor = self.cursor_factory.hashed_account_cursor()?;
        Ok(HashedPostStateAccountCursor::new(cursor, &self.post_state.as_ref().accounts))
    }

    fn hashed_storage_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageCursor, DatabaseError> {
        let cursor = self.cursor_factory.hashed_storage_cursor(hashed_address)?;
        Ok(HashedPostStateStorageCursor::new(
            cursor,
            self.post_state.as_ref().storages.get(&hashed_address),
        ))
    }
}

/// The cursor to iterate over post state hashed accounts and corresponding database entries.
/// It will always give precedence to the data from the hashed post state.
#[derive(Debug)]
pub struct HashedPostStateAccountCursor<'a, C> {
    /// The database cursor.
    cursor: C,
    /// Forward-only in-memory cursor over accounts. `None` values indicate destroyed accounts.
    post_state_cursor: ForwardInMemoryCursor<'a, B256, Option<Account>>,
    /// The last hashed account that was returned by the cursor.
    /// De facto, this is a current cursor position.
    last_account: Option<B256>,
}

impl<'a, C> HashedPostStateAccountCursor<'a, C>
where
    C: HashedCursor<Value = Account>,
{
    /// Create new instance of [`HashedPostStateAccountCursor`].
    pub fn new(cursor: C, post_state_accounts: &'a HashedAccountsSorted) -> Self {
        let post_state_cursor = ForwardInMemoryCursor::new(&post_state_accounts.accounts);
        Self { cursor, post_state_cursor, last_account: None }
    }

    fn seek_inner(&mut self, key: B256) -> Result<Option<(B256, Account)>, DatabaseError> {
        // Take the next account from the post state with the key greater than or equal to the
        // sought key. Skip over destroyed accounts.
        let mut post_state_entry = self.post_state_cursor.seek(&key);
        while post_state_entry.as_ref().is_some_and(|(_, account)| account.is_none()) {
            let address = post_state_entry.unwrap().0;
            post_state_entry = self.post_state_cursor.first_after(&address);
        }

        // It's an exact match with a non-destroyed account, return it.
        if let Some((address, account)) = post_state_entry {
            if address == key {
                return Ok(Some((address, account.unwrap())))
            }
        }

        // It's not an exact match, reposition to the first greater or equal account that wasn't
        // cleared.
        let mut db_entry = self.cursor.seek(key)?;
        while let Some((address, _)) = db_entry {
            // Check if this address is destroyed in post state
            let is_destroyed = post_state_entry
                .as_ref()
                .is_some_and(|(ps_addr, ps_account)| *ps_addr == address && ps_account.is_none());
            
            if is_destroyed {
                db_entry = self.cursor.next()?;
                // Also advance past this destroyed account in post state
                post_state_entry = self.post_state_cursor.first_after(&address);
                while post_state_entry.as_ref().is_some_and(|(_, account)| account.is_none()) {
                    let addr = post_state_entry.unwrap().0;
                    post_state_entry = self.post_state_cursor.first_after(&addr);
                }
            } else {
                break;
            }
        }

        // Compare two entries and return the lowest.
        Ok(Self::compare_entries(post_state_entry, db_entry))
    }

    fn next_inner(&mut self, last_account: B256) -> Result<Option<(B256, Account)>, DatabaseError> {
        // Take the next account from the post state with the key greater than the last sought key.
        // Skip over destroyed accounts.
        let mut post_state_entry = self.post_state_cursor.first_after(&last_account);
        while post_state_entry.as_ref().is_some_and(|(_, account)| account.is_none()) {
            let address = post_state_entry.unwrap().0;
            post_state_entry = self.post_state_cursor.first_after(&address);
        }

        // If post state was given precedence or account was cleared, move the cursor forward.
        let mut db_entry = self.cursor.seek(last_account)?;
        while let Some((address, _)) = db_entry {
            if address <= last_account {
                db_entry = self.cursor.next()?;
                continue;
            }
            
            // Check if this address is destroyed in post state
            let is_destroyed = post_state_entry
                .as_ref()
                .is_some_and(|(ps_addr, ps_account)| *ps_addr == address && ps_account.is_none());
            
            if is_destroyed {
                db_entry = self.cursor.next()?;
                // Also advance past this destroyed account in post state
                post_state_entry = self.post_state_cursor.first_after(&address);
                while post_state_entry.as_ref().is_some_and(|(_, account)| account.is_none()) {
                    let addr = post_state_entry.unwrap().0;
                    post_state_entry = self.post_state_cursor.first_after(&addr);
                }
            } else {
                break;
            }
        }

        // Compare two entries and return the lowest.
        Ok(Self::compare_entries(post_state_entry, db_entry))
    }

    /// Return the account with the lowest hashed account key.
    ///
    /// Given the next post state and database entries, return the smallest of the two.
    /// If the account keys are the same, the post state entry is given precedence.
    /// NOTE: Destroyed accounts (None values) are already filtered out before calling this.
    fn compare_entries(
        post_state_item: Option<(B256, Option<Account>)>,
        db_item: Option<(B256, Account)>,
    ) -> Option<(B256, Account)> {
        // Post state entries should already have None values filtered out
        let post_state_item = post_state_item.map(|(address, account)| {
            (address, account.expect("destroyed accounts should be filtered out"))
        });
        
        match (post_state_item, db_item) {
            (Some(post_state_entry), Some(db_entry)) => {
                // If both are not empty, return the smallest of the two
                // Post state is given precedence if keys are equal
                Some(if post_state_entry.0 <= db_entry.0 { post_state_entry } else { db_entry })
            }
            (Some(post_state_entry), None) => Some(post_state_entry),
            (None, Some(db_entry)) => Some(db_entry),
            (None, None) => None,
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
    /// Forward-only in-memory cursor over account storage slots. `U256::ZERO` indicates cleared slots.
    post_state_cursor: Option<ForwardInMemoryCursor<'a, B256, U256>>,
    /// Reference to the post state storage for checking emptiness.
    post_state_storage: Option<&'a [(B256, U256)]>,
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
            post_state_storage.map(|s| ForwardInMemoryCursor::new(&s.storage));
        let storage_ref = post_state_storage.map(|s| s.storage.as_slice());
        let storage_wiped = post_state_storage.is_some_and(|s| s.wiped);
        Self { cursor, post_state_cursor, post_state_storage: storage_ref, storage_wiped, last_slot: None }
    }

    /// Find the storage entry in post state or database that's greater or equal to provided subkey.
    fn seek_inner(&mut self, subkey: B256) -> Result<Option<(B256, U256)>, DatabaseError> {
        // Attempt to find the account's storage in post state.
        let post_state_entry = self.post_state_cursor.as_mut().and_then(|c| c.seek(&subkey));

        // If it's an exact match in post state and database storage was wiped
        if self.storage_wiped {
            if let Some((slot, value)) = post_state_entry {
                if slot == subkey {
                    return Ok(if value.is_zero() { None } else { Some((slot, value)) })
                }
            }
            return Ok(post_state_entry.filter(|(_, value)| !value.is_zero()))
        }

        // If the exact match in post state has a non-zero value, return it
        if let Some((slot, value)) = post_state_entry {
            if slot == subkey && !value.is_zero() {
                return Ok(Some((slot, value)))
            }
        }

        // It's not an exact non-zero match in post state, so we need to check the database.
        // Reposition to the first greater or equal slot that wasn't cleared.
        let mut db_entry = self.cursor.seek(subkey)?;
        while let Some((slot, _)) = db_entry {
            // Check if this slot is cleared in post state
            let is_cleared = post_state_entry
                .as_ref()
                .is_some_and(|(ps_slot, ps_value)| *ps_slot == slot && ps_value.is_zero());
            
            if is_cleared {
                db_entry = self.cursor.next()?;
            } else {
                break;
            }
        }

        // Compare two entries and return the lowest.
        Ok(Self::compare_entries(post_state_entry, db_entry))
    }

    /// Find the storage entry that is right after current cursor position.
    fn next_inner(&mut self, last_slot: B256) -> Result<Option<(B256, U256)>, DatabaseError> {
        // Attempt to find the account's storage in post state.
        let post_state_entry =
            self.post_state_cursor.as_mut().and_then(|c| c.first_after(&last_slot));

        // Return post state entry immediately if database was wiped (filtering out zeros).
        if self.storage_wiped {
            return Ok(post_state_entry.filter(|(_, value)| !value.is_zero()))
        }

        // If post state was given precedence, move the cursor forward.
        // If the entry was already returned or is zero-valued, move to the next.
        let mut db_entry = self.cursor.seek(last_slot)?;
        while let Some((slot, _)) = db_entry {
            if slot == last_slot {
                db_entry = self.cursor.next()?;
                continue;
            }
            
            // Check if this slot is cleared in post state
            let is_cleared = post_state_entry
                .as_ref()
                .is_some_and(|(ps_slot, ps_value)| *ps_slot == slot && ps_value.is_zero());
            
            if is_cleared {
                db_entry = self.cursor.next()?;
            } else {
                break;
            }
        }

        // Compare two entries and return the lowest.
        Ok(Self::compare_entries(post_state_entry, db_entry))
    }

    /// Return the storage entry with the lowest hashed storage key (hashed slot).
    ///
    /// Given the next post state and database entries, return the smallest of the two.
    /// If the storage keys are the same, the post state entry is given precedence.
    /// Zero-valued slots are filtered out.
    fn compare_entries(
        post_state_item: Option<(B256, U256)>,
        db_item: Option<(B256, U256)>,
    ) -> Option<(B256, U256)> {
        // Filter out zero-valued slots from post state
        let post_state_item = post_state_item.filter(|(_, value)| !value.is_zero());
        
        match (post_state_item, db_item) {
            (Some(post_state_entry), Some(db_entry)) => {
                // If both are not empty, return the smallest of the two
                // Post state is given precedence if keys are equal
                Some(if post_state_entry.0 <= db_entry.0 { post_state_entry } else { db_entry })
            }
            (Some(post_state_entry), None) => Some(post_state_entry),
            (None, Some(db_entry)) => Some(db_entry),
            (None, None) => None,
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
        let is_empty = match self.post_state_storage {
            Some(storage) => {
                // If the storage has been wiped at any point
                if self.storage_wiped {
                    // Storage is empty if all values are zero
                    !storage.iter().any(|(_, value)| !value.is_zero())
                } else {
                    // If not wiped, check if there are any non-zero values in post state
                    let has_non_zero_in_post_state = storage.iter().any(|(_, value)| !value.is_zero());
                    if has_non_zero_in_post_state {
                        false
                    } else {
                        // No non-zero values in post state, check database
                        self.cursor.is_storage_empty()?
                    }
                }
            }
            None => self.cursor.is_storage_empty()?,
        };
        Ok(is_empty)
    }
}
