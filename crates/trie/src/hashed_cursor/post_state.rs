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
pub struct HashedPostStateCursorFactory<'a, 'b, TX> {
    tx: &'a TX,
    post_state: &'b HashedPostState,
}

impl<'a, 'b, TX> HashedPostStateCursorFactory<'a, 'b, TX> {
    /// Create a new factory.
    pub fn new(tx: &'a TX, post_state: &'b HashedPostState) -> Self {
        Self { tx, post_state }
    }
}

impl<'a, 'b, 'tx, TX: DbTx<'tx>> HashedCursorFactory<'a>
    for HashedPostStateCursorFactory<'a, 'b, TX>
where
    'a: 'b,
{
    type AccountCursor = HashedPostStateAccountCursor<'b, <TX as DbTxGAT<'a>>::Cursor<tables::HashedAccount>> where Self: 'a ;
    type StorageCursor = HashedPostStateStorageCursor<'b, <TX as DbTxGAT<'a>>::DupCursor<tables::HashedStorage>> where Self: 'a;

    fn hashed_account_cursor(&'a self) -> Result<Self::AccountCursor, reth_db::Error> {
        let cursor = self.tx.cursor_read::<tables::HashedAccount>()?;
        Ok(HashedPostStateAccountCursor { post_state: self.post_state, cursor, last_account: None })
    }

    fn hashed_storage_cursor(&'a self) -> Result<Self::StorageCursor, reth_db::Error> {
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
pub struct HashedPostStateAccountCursor<'b, C> {
    cursor: C,
    post_state: &'b HashedPostState,
    last_account: Option<H256>,
}

impl<'b, 'tx, C> HashedPostStateAccountCursor<'b, C>
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
            // Post state is given precedence if keys are equal
            (Some((post_state_address, post_state_account)), Some((db_address, db_account))) => {
                if post_state_address <= db_address {
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

impl<'b, 'tx, C> HashedAccountCursor for HashedPostStateAccountCursor<'b, C>
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
            .find_map(|(k, v)| v.filter(|_| k >= &key).map(|v| (*k, v)));
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
            .find(|(k, v)| k > &last_account && v.is_some())
            .map(|(address, info)| (*address, info.unwrap()));
        let result = self.next_account(post_state_item, db_item)?;
        self.last_account = result.as_ref().map(|(address, _)| *address);
        Ok(result)
    }
}

/// The cursor to iterate over post state hashed storages and corresponding database entries.
/// It will always give precedence to the data from the post state.
#[derive(Debug, Clone)]
pub struct HashedPostStateStorageCursor<'b, C> {
    post_state: &'b HashedPostState,
    cursor: C,
    account: Option<H256>,
    last_slot: Option<H256>,
}

impl<'b, C> HashedPostStateStorageCursor<'b, C> {
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
            // Post state is given precedence if keys are equal
            (Some((post_state_slot, post_state_value)), Some(db_entry)) => {
                if post_state_slot <= &db_entry.key {
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

impl<'b, 'tx, C> HashedStorageCursor for HashedPostStateStorageCursor<'b, C>
where
    C: DbCursorRO<'tx, tables::HashedStorage> + DbDupCursorRO<'tx, tables::HashedStorage>,
{
    fn is_empty(&mut self, key: H256) -> Result<bool, reth_db::Error> {
        let is_empty = match self.post_state.storages.get(&key) {
            Some(storage) => storage.wiped && storage.storage.is_empty(),
            None => self.cursor.seek_exact(key)?.is_none(),
        };
        Ok(is_empty)
    }

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
            .map(|storage| storage.storage.iter().skip_while(|(slot, _)| slot <= &last_slot))
            .and_then(|mut iter| iter.next());
        let result = self.next_slot(post_state_item, db_item)?;
        self.last_slot = result.as_ref().map(|entry| entry.key);
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use reth_db::{database::Database, mdbx::test_utils::create_test_rw_db, transaction::DbTxMut};

    fn assert_account_cursor_order<'a, 'b>(
        factory: &'a impl HashedCursorFactory<'b>,
        mut expected: impl Iterator<Item = (H256, Account)>,
    ) where
        'a: 'b,
    {
        let mut cursor = factory.hashed_account_cursor().unwrap();

        let first_account = cursor.seek(H256::default()).unwrap();
        assert_eq!(first_account, expected.next());

        for expected in expected {
            let next_cursor_account = cursor.next().unwrap();
            assert_eq!(next_cursor_account, Some(expected));
        }

        assert!(cursor.next().unwrap().is_none());
    }

    fn assert_storage_cursor_order<'a, 'b>(
        factory: &'a impl HashedCursorFactory<'b>,
        expected: impl Iterator<Item = (H256, BTreeMap<H256, U256>)>,
    ) where
        'a: 'b,
    {
        let mut cursor = factory.hashed_storage_cursor().unwrap();

        for (account, storage) in expected {
            let mut expected_storage = storage.into_iter();

            let first_storage = cursor.seek(account, H256::default()).unwrap();
            assert_eq!(first_storage.map(|e| (e.key, e.value)), expected_storage.next());

            for expected_entry in expected_storage {
                let next_cursor_storage = cursor.next().unwrap();
                assert_eq!(next_cursor_storage.map(|e| (e.key, e.value)), Some(expected_entry));
            }

            assert!(cursor.next().unwrap().is_none());
        }
    }

    #[test]
    fn post_state_only_accounts() {
        let accounts =
            Vec::from_iter((1..11).map(|key| (H256::from_low_u64_be(key), Account::default())));
        let post_state = HashedPostState {
            accounts: BTreeMap::from_iter(
                accounts.iter().map(|(key, account)| (*key, Some(*account))),
            ),
            storages: Default::default(),
        };

        let db = create_test_rw_db();
        let tx = db.tx().unwrap();

        let factory = HashedPostStateCursorFactory::new(&tx, &post_state);
        assert_account_cursor_order(&factory, accounts.into_iter());
    }

    #[test]
    fn db_only_accounts() {
        let accounts =
            Vec::from_iter((1..11).map(|key| (H256::from_low_u64_be(key), Account::default())));

        let db = create_test_rw_db();
        db.update(|tx| {
            for (key, account) in accounts.iter() {
                tx.put::<tables::HashedAccount>(*key, *account).unwrap();
            }
        })
        .unwrap();

        let tx = db.tx().unwrap();
        let post_state = HashedPostState::default();
        let factory = HashedPostStateCursorFactory::new(&tx, &post_state);
        assert_account_cursor_order(&factory, accounts.into_iter());
    }

    #[test]
    fn account_cursor_correct_order() {
        // odd keys are in post state, even keys are in db
        let accounts =
            Vec::from_iter((1..111).map(|key| (H256::from_low_u64_be(key), Account::default())));

        let db = create_test_rw_db();
        db.update(|tx| {
            for (key, account) in accounts.iter().filter(|x| x.0.to_low_u64_be() % 2 == 0) {
                tx.put::<tables::HashedAccount>(*key, *account).unwrap();
            }
        })
        .unwrap();

        let post_state = HashedPostState {
            accounts: BTreeMap::from_iter(
                accounts
                    .iter()
                    .filter(|x| x.0.to_low_u64_be() % 2 != 0)
                    .map(|(key, account)| (*key, Some(*account))),
            ),
            storages: Default::default(),
        };

        let tx = db.tx().unwrap();
        let factory = HashedPostStateCursorFactory::new(&tx, &post_state);
        assert_account_cursor_order(&factory, accounts.into_iter());
    }

    #[test]
    fn removed_accounts_are_omitted() {
        // odd keys are in post state, even keys are in db
        let accounts =
            Vec::from_iter((1..111).map(|key| (H256::from_low_u64_be(key), Account::default())));
        // accounts 5, 9, 11 should be considered removed from post state
        let removed_keys = Vec::from_iter([5, 9, 11].into_iter().map(H256::from_low_u64_be));

        let db = create_test_rw_db();
        db.update(|tx| {
            for (key, account) in accounts.iter().filter(|x| x.0.to_low_u64_be() % 2 == 0) {
                tx.put::<tables::HashedAccount>(*key, *account).unwrap();
            }
        })
        .unwrap();

        let post_state = HashedPostState {
            accounts: BTreeMap::from_iter(
                accounts.iter().filter(|x| x.0.to_low_u64_be() % 2 != 0).map(|(key, account)| {
                    (*key, if removed_keys.contains(key) { None } else { Some(*account) })
                }),
            ),
            storages: Default::default(),
        };

        let tx = db.tx().unwrap();
        let factory = HashedPostStateCursorFactory::new(&tx, &post_state);
        let expected = accounts.into_iter().filter(|x| !removed_keys.contains(&x.0));
        assert_account_cursor_order(&factory, expected);
    }

    #[test]
    fn post_state_accounts_take_precedence() {
        let accounts =
            Vec::from_iter((1..10).map(|key| {
                (H256::from_low_u64_be(key), Account { nonce: key, ..Default::default() })
            }));

        let db = create_test_rw_db();
        db.update(|tx| {
            for (key, _) in accounts.iter() {
                // insert zero value accounts to the database
                tx.put::<tables::HashedAccount>(*key, Account::default()).unwrap();
            }
        })
        .unwrap();

        let post_state = HashedPostState {
            accounts: BTreeMap::from_iter(
                accounts.iter().map(|(key, account)| (*key, Some(*account))),
            ),
            storages: Default::default(),
        };

        let tx = db.tx().unwrap();
        let factory = HashedPostStateCursorFactory::new(&tx, &post_state);
        assert_account_cursor_order(&factory, accounts.into_iter());
    }

    #[test]
    fn fuzz_hashed_account_cursor() {
        proptest!(ProptestConfig::with_cases(10), |(db_accounts: BTreeMap<H256, Account>, post_state_accounts: BTreeMap<H256, Option<Account>>)| {
                let db = create_test_rw_db();
                db.update(|tx| {
                    for (key, account) in db_accounts.iter() {
                        tx.put::<tables::HashedAccount>(*key, *account).unwrap();
                    }
                })
                .unwrap();

                let post_state = HashedPostState {
                    accounts: BTreeMap::from_iter(
                        post_state_accounts.iter().map(|(key, account)| (*key, *account)),
                    ),
                    storages: Default::default(),
                };

                let mut expected = db_accounts;
                // overwrite or remove accounts from the expected result
                for (key, account) in post_state_accounts.iter() {
                    if let Some(account) = account {
                        expected.insert(*key, *account);
                    } else {
                        expected.remove(key);
                    }
                }

                let tx = db.tx().unwrap();
                let factory = HashedPostStateCursorFactory::new(&tx, &post_state);
                assert_account_cursor_order(&factory, expected.into_iter());
            }
        );
    }

    #[test]
    fn storage_is_empty() {
        let address = H256::random();
        let db = create_test_rw_db();

        // empty from the get go
        {
            let post_state = HashedPostState::default();
            let tx = db.tx().unwrap();
            let factory = HashedPostStateCursorFactory::new(&tx, &post_state);
            let mut cursor = factory.hashed_storage_cursor().unwrap();
            assert!(cursor.is_empty(address).unwrap());
        }

        let db_storage =
            BTreeMap::from_iter((0..10).map(|key| (H256::from_low_u64_be(key), U256::from(key))));
        db.update(|tx| {
            for (slot, value) in db_storage.iter() {
                // insert zero value accounts to the database
                tx.put::<tables::HashedStorage>(
                    address,
                    StorageEntry { key: *slot, value: *value },
                )
                .unwrap();
            }
        })
        .unwrap();

        // not empty
        {
            let post_state = HashedPostState::default();
            let tx = db.tx().unwrap();
            let factory = HashedPostStateCursorFactory::new(&tx, &post_state);
            let mut cursor = factory.hashed_storage_cursor().unwrap();
            assert!(!cursor.is_empty(address).unwrap());
        }

        // wiped storage, must be empty
        {
            let post_state = HashedPostState {
                accounts: BTreeMap::default(),
                storages: BTreeMap::from_iter([(
                    address,
                    HashedStorage { wiped: true, ..Default::default() },
                )]),
            };
            let tx = db.tx().unwrap();
            let factory = HashedPostStateCursorFactory::new(&tx, &post_state);
            let mut cursor = factory.hashed_storage_cursor().unwrap();
            assert!(cursor.is_empty(address).unwrap());
        }

        // wiped storage, but post state has entries
        {
            let post_state = HashedPostState {
                accounts: BTreeMap::default(),
                storages: BTreeMap::from_iter([(
                    address,
                    HashedStorage {
                        wiped: true,
                        storage: BTreeMap::from_iter([(H256::random(), U256::ZERO)]),
                    },
                )]),
            };
            let tx = db.tx().unwrap();
            let factory = HashedPostStateCursorFactory::new(&tx, &post_state);
            let mut cursor = factory.hashed_storage_cursor().unwrap();
            assert!(!cursor.is_empty(address).unwrap());
        }
    }

    #[test]
    fn storage_cursor_correct_order() {
        let address = H256::random();
        let db_storage =
            BTreeMap::from_iter((0..10).map(|key| (H256::from_low_u64_be(key), U256::from(key))));
        let post_state_storage =
            BTreeMap::from_iter((10..20).map(|key| (H256::from_low_u64_be(key), U256::from(key))));

        let db = create_test_rw_db();
        db.update(|tx| {
            for (slot, value) in db_storage.iter() {
                // insert zero value accounts to the database
                tx.put::<tables::HashedStorage>(
                    address,
                    StorageEntry { key: *slot, value: *value },
                )
                .unwrap();
            }
        })
        .unwrap();

        let post_state = HashedPostState {
            accounts: Default::default(),
            storages: BTreeMap::from([(
                address,
                HashedStorage { wiped: false, storage: post_state_storage.clone() },
            )]),
        };

        let tx = db.tx().unwrap();
        let factory = HashedPostStateCursorFactory::new(&tx, &post_state);
        let expected =
            [(address, db_storage.into_iter().chain(post_state_storage.into_iter()).collect())]
                .into_iter();
        assert_storage_cursor_order(&factory, expected);
    }

    #[test]
    fn wiped_storage_is_discarded() {
        let address = H256::random();
        let db_storage =
            BTreeMap::from_iter((0..10).map(|key| (H256::from_low_u64_be(key), U256::from(key))));
        let post_state_storage =
            BTreeMap::from_iter((10..20).map(|key| (H256::from_low_u64_be(key), U256::from(key))));

        let db = create_test_rw_db();
        db.update(|tx| {
            for (slot, _) in db_storage {
                // insert zero value accounts to the database
                tx.put::<tables::HashedStorage>(
                    address,
                    StorageEntry { key: slot, value: U256::ZERO },
                )
                .unwrap();
            }
        })
        .unwrap();

        let post_state = HashedPostState {
            accounts: Default::default(),
            storages: BTreeMap::from([(
                address,
                HashedStorage { wiped: true, storage: post_state_storage.clone() },
            )]),
        };

        let tx = db.tx().unwrap();
        let factory = HashedPostStateCursorFactory::new(&tx, &post_state);
        let expected = [(address, post_state_storage)].into_iter();
        assert_storage_cursor_order(&factory, expected);
    }

    #[test]
    fn post_state_storages_take_precedence() {
        let address = H256::random();
        let storage =
            BTreeMap::from_iter((1..10).map(|key| (H256::from_low_u64_be(key), U256::from(key))));

        let db = create_test_rw_db();
        db.update(|tx| {
            for (slot, _) in storage.iter() {
                // insert zero value accounts to the database
                tx.put::<tables::HashedStorage>(
                    address,
                    StorageEntry { key: *slot, value: U256::ZERO },
                )
                .unwrap();
            }
        })
        .unwrap();

        let post_state = HashedPostState {
            accounts: Default::default(),
            storages: BTreeMap::from([(
                address,
                HashedStorage { wiped: false, storage: storage.clone() },
            )]),
        };

        let tx = db.tx().unwrap();
        let factory = HashedPostStateCursorFactory::new(&tx, &post_state);
        let expected = [(address, storage)].into_iter();
        assert_storage_cursor_order(&factory, expected);
    }

    #[test]
    fn fuzz_hashed_storage_cursor() {
        proptest!(ProptestConfig::with_cases(10),
            |(
                db_storages: BTreeMap<H256, BTreeMap<H256, U256>>,
                post_state_storages: BTreeMap<H256, (bool, BTreeMap<H256, U256>)>
            )|
        {
            let db = create_test_rw_db();
            db.update(|tx| {
                for (address, storage) in db_storages.iter() {
                    for (slot, value) in storage {
                        let entry = StorageEntry { key: *slot, value: *value };
                        tx.put::<tables::HashedStorage>(*address, entry).unwrap();
                    }
                }
            })
            .unwrap();

            let post_state = HashedPostState {
                accounts: Default::default(),
                storages: BTreeMap::from_iter(post_state_storages.iter().map(
                    |(address, (wiped, storage))| {
                        (*address, HashedStorage { wiped: *wiped, storage: storage.clone() })
                    },
                )),
            };

            let mut expected = db_storages;
            // overwrite or remove accounts from the expected result
            for (key, (wiped, storage)) in post_state_storages {
                let entry = expected.entry(key).or_default();
                if wiped {
                    entry.clear();
                }
                entry.extend(storage);
            }

            let tx = db.tx().unwrap();
            let factory = HashedPostStateCursorFactory::new(&tx, &post_state);
            assert_storage_cursor_order(&factory, expected.into_iter());
        });
    }
}
