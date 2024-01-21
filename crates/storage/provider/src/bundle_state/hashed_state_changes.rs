use reth_db::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
    tables,
    transaction::{DbTx, DbTxMut},
    DatabaseError,
};
use reth_primitives::{Account, StorageEntry, B256, U256};
use reth_trie::HashedPostState;
use std::collections::BTreeMap;

/// Changes to the hashed state.
#[derive(Debug, Default)]
pub struct HashedStateChanges(pub HashedPostState);

impl HashedStateChanges {
    /// Write the bundle state to the database.
    pub fn write_to_db<TX: DbTxMut + DbTx>(self, tx: &TX) -> Result<(), DatabaseError> {
        // Collect hashed account changes.
        let mut hashed_accounts = BTreeMap::<B256, Option<Account>>::default();
        for (hashed_address, account) in self.0.accounts() {
            hashed_accounts.insert(hashed_address, account);
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
        for (hashed_address, storage) in self.0.storages() {
            let entry = hashed_storages.entry(*hashed_address).or_default();
            entry.0 |= storage.wiped();
            for (hashed_slot, value) in storage.storage_slots() {
                entry.1.insert(hashed_slot, value);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::create_test_provider_factory;
    use reth_primitives::{keccak256, Address};
    use reth_trie::HashedStorage;

    #[test]
    fn wiped_entries_are_removed() {
        let provider_factory = create_test_provider_factory();

        let addresses = (0..10).map(|_| Address::random()).collect::<Vec<_>>();
        let destroyed_address = *addresses.first().unwrap();
        let destroyed_address_hashed = keccak256(destroyed_address);
        let slot = B256::with_last_byte(1);
        let hashed_slot = keccak256(slot);
        {
            let provider_rw = provider_factory.provider_rw().unwrap();
            let mut accounts_cursor =
                provider_rw.tx_ref().cursor_write::<tables::HashedAccount>().unwrap();
            let mut storage_cursor =
                provider_rw.tx_ref().cursor_write::<tables::HashedStorage>().unwrap();

            for address in addresses {
                let hashed_address = keccak256(address);
                accounts_cursor
                    .insert(hashed_address, Account { nonce: 1, ..Default::default() })
                    .unwrap();
                storage_cursor
                    .insert(hashed_address, StorageEntry { key: hashed_slot, value: U256::from(1) })
                    .unwrap();
            }
            provider_rw.commit().unwrap();
        }

        let mut hashed_state = HashedPostState::default();
        hashed_state.insert_account(destroyed_address_hashed, None);
        hashed_state.insert_hashed_storage(destroyed_address_hashed, HashedStorage::new(true));

        let provider_rw = provider_factory.provider_rw().unwrap();
        assert_eq!(HashedStateChanges(hashed_state).write_to_db(provider_rw.tx_ref()), Ok(()));
        provider_rw.commit().unwrap();

        let provider = provider_factory.provider().unwrap();
        assert_eq!(
            provider.tx_ref().get::<tables::HashedAccount>(destroyed_address_hashed),
            Ok(None)
        );
        assert_eq!(
            provider
                .tx_ref()
                .cursor_read::<tables::HashedStorage>()
                .unwrap()
                .seek_by_key_subkey(destroyed_address_hashed, hashed_slot),
            Ok(None)
        );
    }
}
