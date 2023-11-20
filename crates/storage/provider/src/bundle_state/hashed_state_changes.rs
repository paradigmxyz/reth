use reth_db::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
    tables,
    transaction::{DbTx, DbTxMut},
    DatabaseError,
};
use reth_primitives::{Account, StorageEntry, B256, U256};
use reth_trie::hashed_cursor::HashedPostState;
use std::collections::BTreeMap;

/// A change to the state of the world.
#[derive(Debug, Default)]
pub struct HashedStateChanges(pub HashedPostState);

impl HashedStateChanges {
    /// Write the bundle state to the database.
    pub fn write_to_db<TX: DbTxMut + DbTx>(self, tx: &TX) -> Result<(), DatabaseError> {
        // Collect hashed account changes.
        let mut hashed_accounts = BTreeMap::<B256, Option<Account>>::default();
        for hashed_address in self.0.destroyed_accounts {
            hashed_accounts.insert(hashed_address, None);
        }
        for (hashed_address, account) in self.0.accounts {
            hashed_accounts.insert(hashed_address, Some(account));
        }

        // Write hashed account updates.
        let mut hashed_accounts_cursor = tx.cursor_write::<tables::HashedAccount>()?;
        for (hashed_address, account) in hashed_accounts {
            if let Some(account) = account {
                hashed_accounts_cursor.upsert(hashed_address, account)?;
            } else if hashed_accounts_cursor.seek_exact(hashed_address)?.is_some() {
                hashed_accounts_cursor.delete_current()?;
            }
        }

        // Collect hashed storage changes.
        let mut hashed_storages = BTreeMap::<B256, (bool, BTreeMap<B256, U256>)>::default();
        for (hashed_address, storage) in self.0.storages {
            let entry = hashed_storages.entry(hashed_address).or_default();
            if storage.wiped {
                entry.0 |= storage.wiped;
            }
            for (hashed_slot, value) in storage.non_zero_valued_storage {
                entry.1.insert(hashed_slot, value);
            }
            for slot in storage.zero_valued_slots {
                entry.1.insert(slot, U256::ZERO);
            }
        }

        // Write hashed storage changes.
        let mut hashed_storage_cursor = tx.cursor_dup_write::<tables::HashedStorage>()?;
        for (hashed_address, (wiped, storage)) in hashed_storages {
            if wiped && hashed_storage_cursor.seek_exact(hashed_address)?.is_some() {
                hashed_storage_cursor.delete_current_duplicates()?;
            }

            for (hashed_slot, value) in storage {
                let entry = StorageEntry { key: hashed_slot, value };
                if let Some(db_entry) =
                    hashed_storage_cursor.seek_by_key_subkey(hashed_address, entry.key)?
                {
                    if db_entry.key == entry.key {
                        hashed_storage_cursor.delete_current()?;
                    }
                }

                if entry.value != U256::ZERO {
                    hashed_storage_cursor.upsert(hashed_address, entry)?;
                }
            }
        }

        Ok(())
    }
}
