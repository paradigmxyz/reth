use crate::{prefix_set::PrefixSet, Nibbles};

use super::{HashedAccountCursor, HashedCursorFactory, HashedStorageCursor};
use reth_db::{
    cursor::{DbCursorRO, DbDupCursorRO},
    tables,
    transaction::{DbTx, DbTxGAT},
};
use reth_primitives::{Account, StorageEntry, H256, U256};
use std::collections::{BTreeMap, HashMap};

/// The post state account storage with hashed slots.
#[derive(Debug, Default, Clone, Eq, PartialEq)]
pub struct HashedStorage {
    /// Whether the storage was wiped or not.
    pub wiped: bool,
    /// Hashed storage slots.
    pub storage: BTreeMap<H256, U256>,
}

/// The post state with hashed addresses as keys.
#[derive(Debug, Default, Clone, Eq, PartialEq)]
pub struct HashedPostState {
    /// Map of hashed addresses to account info.
    pub accounts: BTreeMap<H256, Option<Account>>,
    /// Map of hashed addresses to hashed storage.
    pub storages: BTreeMap<H256, HashedStorage>,
}

impl HashedPostState {
    /// Construct prefix sets from hashed post state.
    pub fn construct_prefix_sets(&self) -> (PrefixSet, HashMap<H256, PrefixSet>) {
        // Initialize prefix sets.
        let mut account_prefix_set = PrefixSet::default();
        let mut storage_prefix_set: HashMap<H256, PrefixSet> = HashMap::default();

        for hashed_address in self.accounts.keys() {
            account_prefix_set.insert(Nibbles::unpack(hashed_address));
        }

        for (hashed_address, hashed_storage) in self.storages.iter() {
            account_prefix_set.insert(Nibbles::unpack(hashed_address));
            for hashed_slot in hashed_storage.storage.keys() {
                storage_prefix_set
                    .entry(*hashed_address)
                    .or_default()
                    .insert(Nibbles::unpack(hashed_slot));
            }
        }

        (account_prefix_set, storage_prefix_set)
    }
}

/// The hashed cursor factory for the post state.
pub struct HashedPostStateFactory<'a, TX> {
    tx: &'a TX,
    post_state: &'a HashedPostState,
}

impl<'a, TX> HashedPostStateFactory<'a, TX> {
    /// Create a new factory.
    pub fn new(tx: &'a TX, post_state: &'a HashedPostState) -> Self {
        Self { tx, post_state }
    }
}

impl<'a, TX: DbTx<'a>> HashedCursorFactory<'a> for HashedPostStateFactory<'a, TX> {
    type AccountCursor<'tx> = HashedPostStateAccountCursor<'a, <TX as DbTxGAT<'tx>>::Cursor<tables::HashedAccount>> where Self: 'tx;
    type StorageCursor<'tx> = HashedPostStateStorageCursor<'a, <TX as DbTxGAT<'tx>>::DupCursor<tables::HashedStorage>> where Self: 'tx;

    fn hashed_account_cursor(&self) -> Result<Self::AccountCursor<'_>, reth_db::Error> {
        let cursor = self.tx.cursor_read::<tables::HashedAccount>()?;
        Ok(HashedPostStateAccountCursor { post_state: self.post_state, cursor, last_account: None })
    }

    fn hashed_storage_cursor(&self) -> Result<Self::StorageCursor<'_>, reth_db::Error> {
        let cursor = self.tx.cursor_dup_read::<tables::HashedStorage>()?;
        Ok(HashedPostStateStorageCursor {
            post_state: self.post_state,
            cursor,
            account: None,
            last_slot: None,
        })
    }
}

/// The cursor to iterate over post state hashed accounts and corresponding database entries.
/// It will always give precedence to the data from the post state.
#[derive(Debug, Clone)]
pub struct HashedPostStateAccountCursor<'a, C> {
    post_state: &'a HashedPostState,
    cursor: C,
    last_account: Option<H256>,
}

impl<'a, 'tx, C> HashedPostStateAccountCursor<'a, C>
where
    C: DbCursorRO<'tx, tables::HashedAccount>,
{
    fn was_account_cleared(&self, account: &H256) -> bool {
        matches!(self.post_state.accounts.get(account), Some(None))
    }

    fn next_account(
        &self,
        post_state_item: Option<(H256, Account)>,
        db_item: Option<(H256, Account)>,
    ) -> Result<Option<(H256, Account)>, reth_db::Error> {
        let result = match (post_state_item, db_item) {
            // If both are not empty, return the smallest of the two
            (Some((post_state_address, post_state_account)), Some((db_address, db_account))) => {
                if post_state_address < db_address {
                    Some((post_state_address, post_state_account))
                } else {
                    Some((db_address, db_account))
                }
            }
            // If the database is empty, return the post state entry
            (Some((post_state_address, post_state_account)), None) => {
                Some((post_state_address, post_state_account))
            }
            // If the post state is empty, return the database entry
            (None, Some((db_address, db_account))) => Some((db_address, db_account)),
            // If both are empty, return None
            (None, None) => None,
        };
        Ok(result)
    }
}

impl<'a, 'tx, C> HashedAccountCursor for HashedPostStateAccountCursor<'a, C>
where
    C: DbCursorRO<'tx, tables::HashedAccount>,
{
    fn seek(&mut self, key: H256) -> Result<Option<(H256, Account)>, reth_db::Error> {
        self.last_account = None;

        // Attempt to find the account in poststate.
        let post_state_item = self
            .post_state
            .accounts
            .iter()
            .skip_while(|(k, v)| k < &&key || v.is_none())
            .next()
            .map(|(address, info)| (*address, info.unwrap().clone()));
        if let Some((address, account)) = post_state_item {
            // It's an exact match, return the account from post state without looking up in the
            // database.
            if address == key {
                self.last_account = Some(address);
                return Ok(Some((address, account)))
            }
        }

        // It's not an exact match, reposition to the first greater or equal account that wasn't
        // cleared.
        let mut db_item = self.cursor.seek(key)?;
        while db_item
            .as_ref()
            .map(|(address, _)| self.was_account_cleared(address))
            .unwrap_or_default()
        {
            db_item = self.cursor.next()?;
        }

        let result = self.next_account(post_state_item, db_item)?;
        self.last_account = result.as_ref().map(|(address, _)| *address);
        Ok(result)
    }

    fn next(&mut self) -> Result<Option<(H256, Account)>, reth_db::Error> {
        let last_account = match self.last_account.as_ref() {
            Some(account) => account,
            None => return Ok(None), // no previous entry was found
        };

        // If post state was given precedence, move the cursor forward.
        let mut db_item = self.cursor.current()?;
        while db_item
            .as_ref()
            .map(|(address, _)| address <= last_account || self.was_account_cleared(address))
            .unwrap_or_default()
        {
            db_item = self.cursor.next()?;
        }

        let post_state_item = self
            .post_state
            .accounts
            .iter()
            .skip_while(|(k, v)| k <= &last_account || v.is_none())
            .next()
            .map(|(address, info)| (*address, info.unwrap().clone()));
        let result = self.next_account(post_state_item, db_item)?;
        self.last_account = result.as_ref().map(|(address, _)| *address);
        Ok(result)
    }
}

/// The cursor to iterate over post state hashed storages and corresponding database entries.
/// It will always give precedence to the data from the post state.
#[derive(Debug, Clone)]
pub struct HashedPostStateStorageCursor<'a, C> {
    post_state: &'a HashedPostState,
    cursor: C,
    account: Option<H256>,
    last_slot: Option<H256>,
}

impl<'a, C> HashedPostStateStorageCursor<'a, C> {
    fn was_storage_wiped(&self, account: &H256) -> bool {
        match self.post_state.storages.get(account) {
            Some(storage) => storage.wiped,
            None => false,
        }
    }

    fn next_slot(
        &self,
        post_state_item: Option<(&H256, &U256)>,
        db_item: Option<StorageEntry>,
    ) -> Result<Option<StorageEntry>, reth_db::Error> {
        let result = match (post_state_item, db_item) {
            // If both are not empty, return the smallest of the two
            (Some((post_state_slot, post_state_value)), Some(db_entry)) => {
                if post_state_slot < &&db_entry.key {
                    Some(StorageEntry { key: *post_state_slot, value: *post_state_value })
                } else {
                    Some(db_entry)
                }
            }
            // If the database is empty, return the post state entry
            (Some((post_state_slot, post_state_value)), None) => {
                Some(StorageEntry { key: *post_state_slot, value: *post_state_value })
            }
            // If the post state is empty, return the database entry
            (None, Some(db_entry)) => Some(db_entry),
            // If both are empty, return None
            (None, None) => None,
        };
        Ok(result)
    }
}

impl<'a, 'tx, C> HashedStorageCursor for HashedPostStateStorageCursor<'a, C>
where
    C: DbCursorRO<'tx, tables::HashedStorage> + DbDupCursorRO<'tx, tables::HashedStorage>,
{
    fn seek(&mut self, key: H256, subkey: H256) -> Result<Option<StorageEntry>, reth_db::Error> {
        self.last_slot = None;
        self.account = Some(key);

        // Attempt to find the account's storage in poststate.
        let post_state_item = self
            .post_state
            .storages
            .get(&key)
            .map(|storage| storage.storage.iter().skip_while(|(slot, _)| slot <= &&subkey))
            .and_then(|mut iter| iter.next());
        if let Some((slot, value)) = post_state_item {
            // It's an exact match, return the storage slot from post state without looking up in
            // the database.
            if slot == &subkey {
                self.last_slot = Some(*slot);
                return Ok(Some(StorageEntry { key: *slot, value: *value }))
            }
        }

        // It's not an exact match, reposition to the first greater or equal account.
        let db_item = if self.was_storage_wiped(&key) {
            None
        } else {
            self.cursor.seek_by_key_subkey(key, subkey)?
        };

        let result = self.next_slot(post_state_item, db_item)?;
        self.last_slot = result.as_ref().map(|entry| entry.key);
        Ok(result)
    }

    fn next(&mut self) -> Result<Option<StorageEntry>, reth_db::Error> {
        let account = self.account.expect("`seek` must be called first");

        let last_slot = match self.last_slot.as_ref() {
            Some(account) => account,
            None => return Ok(None), // no previous entry was found
        };

        let db_item = if self.was_storage_wiped(&account) {
            None
        } else {
            // If post state was given precedence, move the cursor forward.
            let mut db_item = self.cursor.seek_by_key_subkey(account, *last_slot)?;

            // If the entry was already returned, move to the next.
            if db_item.as_ref().map(|entry| &entry.key == last_slot).unwrap_or_default() {
                db_item = self.cursor.next_dup_val()?;
            }

            db_item
        };

        let post_state_item = self
            .post_state
            .storages
            .get(&account)
            .map(|storage| storage.storage.iter().skip_while(|(slot, _)| slot <= &&last_slot))
            .and_then(|mut iter| iter.next());
        let result = self.next_slot(post_state_item, db_item)?;
        self.last_slot = result.as_ref().map(|entry| entry.key);
        Ok(result)
    }
}
