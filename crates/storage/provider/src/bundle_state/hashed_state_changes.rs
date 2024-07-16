use crate::DatabaseProviderRW;
use itertools::Itertools;
use reth_db::{tables, Database};
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
    transaction::DbTxMut,
    DatabaseError,
};
use reth_primitives::{StorageEntry, U256};
use reth_trie::HashedPostStateSorted;

/// Changes to the hashed state.
#[derive(Debug)]
pub struct HashedStateChanges<'a>(pub &'a HashedPostStateSorted);

impl HashedStateChanges<'_> {
    /// Write the bundle state to the database.
    pub fn write_to_db<DB>(self, provider: &DatabaseProviderRW<DB>) -> Result<(), DatabaseError>
    where
        DB: Database,
    {
        // Write hashed account updates.
        let mut hashed_accounts_cursor =
            provider.tx_ref().cursor_write::<tables::HashedAccounts>()?;
        for (hashed_address, account) in self.0.accounts().accounts_sorted() {
            if let Some(account) = account {
                hashed_accounts_cursor.upsert(hashed_address, account)?;
            } else if hashed_accounts_cursor.seek_exact(hashed_address)?.is_some() {
                hashed_accounts_cursor.delete_current()?;
            }
        }

        // Write hashed storage changes.
        let sorted_storages = self.0.account_storages().iter().sorted_by_key(|(key, _)| *key);
        let mut hashed_storage_cursor =
            provider.tx_ref().cursor_dup_write::<tables::HashedStorages>()?;
        for (hashed_address, storage) in sorted_storages {
            if storage.is_wiped() && hashed_storage_cursor.seek_exact(*hashed_address)?.is_some() {
                hashed_storage_cursor.delete_current_duplicates()?;
            }

            for (hashed_slot, value) in storage.storage_slots_sorted() {
                let entry = StorageEntry { key: hashed_slot, value };
                if let Some(db_entry) =
                    hashed_storage_cursor.seek_by_key_subkey(*hashed_address, entry.key)?
                {
                    if db_entry.key == entry.key {
                        hashed_storage_cursor.delete_current()?;
                    }
                }

                if entry.value != U256::ZERO {
                    hashed_storage_cursor.upsert(*hashed_address, entry)?;
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
    use reth_db_api::transaction::DbTx;
    use reth_primitives::{keccak256, Account, Address, B256};
    use reth_trie::{HashedPostState, HashedStorage};

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
                provider_rw.tx_ref().cursor_write::<tables::HashedAccounts>().unwrap();
            let mut storage_cursor =
                provider_rw.tx_ref().cursor_write::<tables::HashedStorages>().unwrap();

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
        hashed_state.accounts.insert(destroyed_address_hashed, None);
        hashed_state.storages.insert(destroyed_address_hashed, HashedStorage::new(true));

        let provider_rw = provider_factory.provider_rw().unwrap();
        assert_eq!(
            HashedStateChanges(&hashed_state.into_sorted()).write_to_db(&provider_rw),
            Ok(())
        );
        provider_rw.commit().unwrap();

        let provider = provider_factory.provider().unwrap();
        assert_eq!(
            provider.tx_ref().get::<tables::HashedAccounts>(destroyed_address_hashed),
            Ok(None)
        );
        assert_eq!(
            provider
                .tx_ref()
                .cursor_read::<tables::HashedStorages>()
                .unwrap()
                .seek_by_key_subkey(destroyed_address_hashed, hashed_slot),
            Ok(None)
        );
    }
}
