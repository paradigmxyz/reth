use super::{HashedCursor, HashedCursorFactory, HashedStorageCursor};
use crate::forward_cursor::ForwardInMemoryCursor;
use alloy_primitives::{B256, U256};
use reth_primitives_traits::Account;
use reth_storage_errors::db::DatabaseError;
use reth_trie_common::HashedPostStateSorted;

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

impl<'overlay, CF, T> HashedCursorFactory for HashedPostStateCursorFactory<CF, &'overlay T>
where
    CF: HashedCursorFactory,
    T: AsRef<HashedPostStateSorted>,
{
    type AccountCursor<'cursor>
        = HashedPostStateCursor<'overlay, CF::AccountCursor<'cursor>, Account>
    where
        Self: 'cursor;
    type StorageCursor<'cursor>
        = HashedPostStateCursor<'overlay, CF::StorageCursor<'cursor>, U256>
    where
        Self: 'cursor;

    fn hashed_account_cursor(&self) -> Result<Self::AccountCursor<'_>, DatabaseError> {
        let cursor = self.cursor_factory.hashed_account_cursor()?;
        Ok(HashedPostStateCursor::new(Some(cursor), Some(&self.post_state.as_ref().accounts)))
    }

    fn hashed_storage_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageCursor<'_>, DatabaseError> {
        static EMPTY_POST_STATES: Vec<(B256, Option<U256>)> = Vec::new();

        let post_state_storage = self.post_state.as_ref().storages.get(&hashed_address);
        let (storage_slots, wiped) = post_state_storage
            .map(|u| (u.storage_slots_ref(), u.is_wiped()))
            .unwrap_or((&EMPTY_POST_STATES, false));

        let cursor = if wiped {
            None
        } else {
            Some(self.cursor_factory.hashed_storage_cursor(hashed_address)?)
        };

        Ok(HashedPostStateCursor::new(cursor, Some(storage_slots)))
    }
}

/// A cursor to iterate over state updates and corresponding database entries.
/// It will always give precedence to the data from the trie updates.
#[derive(Debug)]
pub struct HashedPostStateCursor<'a, C, V> {
    /// The underlying cursor. If None then it is assumed there is no DB data.
    cursor: Option<C>,
    /// Forward-only in-memory cursor over underlying V.
    post_state_cursor: Option<ForwardInMemoryCursor<'a, B256, Option<V>>>,
    /// The last hashed account that was returned by the cursor.
    /// De facto, this is a current cursor position.
    last_key: Option<B256>,
}

impl<'a, C, V> HashedPostStateCursor<'a, C, V>
where
    C: HashedCursor<Value = V>,
    V: Copy + Clone + std::fmt::Debug,
{
    /// todo!()
    pub fn new(cursor: Option<C>, updates: Option<&'a [(B256, Option<V>)]>) -> Self {
        let post_state_cursor = updates.map(ForwardInMemoryCursor::new);
        Self { cursor, post_state_cursor, last_key: None }
    }

    fn seek_inner(&mut self, key: B256) -> Result<Option<(B256, V)>, DatabaseError> {
        // Seek in post state first
        let post_state_entry = self.post_state_cursor.as_mut().and_then(|c| c.seek(&key));

        // Fast path: exact match in post state with non-empty values
        if let Some((k, Some(value))) = post_state_entry &&
            k == key
        {
            return Ok(Some((k, value)))
        }

        let db_entry = self.cursor.as_mut().map(|c| c.seek(key)).transpose()?.flatten();
        self.resolve_next_entry(post_state_entry, db_entry)
    }

    fn next_inner(&mut self, last_key: B256) -> Result<Option<(B256, V)>, DatabaseError> {
        // Get next entry from post state after last_slot
        let post_state_entry =
            self.post_state_cursor.as_mut().and_then(|c| c.first_after(&last_key));

        // Seek to last_slot, then move past it
        let mut db_entry = self.cursor.as_mut().map(|c| c.seek(last_key)).transpose()?.flatten();
        if db_entry.as_ref().is_some_and(|(key, _)| *key == last_key) {
            db_entry = self.cursor.as_mut().map(|c| c.next()).transpose()?.flatten();
        }

        self.resolve_next_entry(post_state_entry, db_entry)
    }

    fn resolve_next_entry(
        &mut self,
        mut post_state_entry: Option<(B256, Option<V>)>,
        mut db_entry: Option<(B256, V)>,
    ) -> Result<Option<(B256, V)>, DatabaseError> {
        loop {
            match (post_state_entry, &db_entry) {
                // Post state has zero-valued slot before DB position
                (Some((post_key, None)), _)
                    if db_entry.as_ref().is_none_or(|(db_key, _)| post_key < *db_key) =>
                {
                    post_state_entry =
                        self.post_state_cursor.as_mut().and_then(|c| c.first_after(&post_key));
                }

                // Post state zeroed out exact DB entry
                (Some((post_key, None)), Some((db_key, _))) if post_key == *db_key => {
                    post_state_entry =
                        self.post_state_cursor.as_mut().and_then(|c| c.first_after(&post_key));
                    db_entry = self.cursor.as_mut().map(|c| c.next()).transpose()?.flatten();
                }

                // Post state has non-zero value before or at DB position
                (Some((post_key, Some(value))), _)
                    if db_entry.as_ref().is_none_or(|(db_key, _)| post_key <= *db_key) =>
                {
                    return Ok(Some((post_key, value)))
                }

                // All other cases:
                // - post state_key > db_key
                // - post state is exhausted
                // Return DB entry, if DB is also exhausted, this returns None
                _ => return Ok(db_entry),
            }
        }
    }
}

impl<C, V> HashedCursor for HashedPostStateCursor<'_, C, V>
where
    C: HashedCursor<Value = V>,
    V: Copy + Clone + std::fmt::Debug,
{
    type Value = V;

    fn seek(&mut self, key: B256) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        let entry = self.seek_inner(key)?;
        self.last_key = entry.as_ref().map(|entry| entry.0);
        Ok(entry)
    }

    fn next(&mut self) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        let next = match self.last_key {
            Some(last_key) => {
                let entry = self.next_inner(last_key)?;
                self.last_key = entry.as_ref().map(|entry| entry.0);
                entry
            }
            None => None,
        };
        Ok(next)
    }
}

impl<C, V> HashedStorageCursor for HashedPostStateCursor<'_, C, V>
where
    C: HashedStorageCursor<Value = V>,
    V: Copy + Clone + std::fmt::Debug,
{
    /// Returns `true` if the account has no storage entries.
    ///
    /// This function should be called before attempting to call [`HashedCursor::seek`] or
    /// [`HashedCursor::next`].
    fn is_storage_empty(&mut self) -> Result<bool, DatabaseError> {
        let is_empty = match &self.post_state_cursor {
            Some(cursor) => cursor.is_empty(),
            None => match &mut self.cursor {
                Some(c) => c.is_storage_empty()?,
                None => true, // No post state and no DB cursor means storage is empty
            },
        };
        Ok(is_empty)
    }
}
