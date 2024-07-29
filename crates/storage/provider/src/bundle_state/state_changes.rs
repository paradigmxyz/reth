use crate::DatabaseProviderRW;
use rayon::slice::ParallelSliceMut;
use reth_db::{tables, Database};
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
    transaction::DbTxMut,
};
use reth_primitives::{Bytecode, StorageEntry, U256};
use reth_storage_errors::db::DatabaseError;
use revm::db::states::{PlainStorageChangeset, StateChangeset};

/// A change to the state of the world.
#[derive(Debug, Default)]
pub struct StateChanges(pub StateChangeset);

impl From<StateChangeset> for StateChanges {
    fn from(revm: StateChangeset) -> Self {
        Self(revm)
    }
}

impl StateChanges {
    /// Write the bundle state to the database.
    pub fn write_to_db<DB>(mut self, provider: &DatabaseProviderRW<DB>) -> Result<(), DatabaseError>
    where
        DB: Database,
    {
        // sort all entries so they can be written to database in more performant way.
        // and take smaller memory footprint.
        self.0.accounts.par_sort_by_key(|a| a.0);
        self.0.storage.par_sort_by_key(|a| a.address);
        self.0.contracts.par_sort_by_key(|a| a.0);

        // Write new account state
        tracing::trace!(target: "provider::bundle_state", len = self.0.accounts.len(), "Writing new account state");
        let mut accounts_cursor = provider.tx_ref().cursor_write::<tables::PlainAccountState>()?;
        // write account to database.
        for (address, account) in self.0.accounts {
            if let Some(account) = account {
                tracing::trace!(target: "provider::bundle_state", ?address, "Updating plain state account");
                accounts_cursor.upsert(address, account.into())?;
            } else if accounts_cursor.seek_exact(address)?.is_some() {
                tracing::trace!(target: "provider::bundle_state", ?address, "Deleting plain state account");
                accounts_cursor.delete_current()?;
            }
        }

        // Write bytecode
        tracing::trace!(target: "provider::bundle_state", len = self.0.contracts.len(), "Writing bytecodes");
        let mut bytecodes_cursor = provider.tx_ref().cursor_write::<tables::Bytecodes>()?;
        for (hash, bytecode) in self.0.contracts {
            bytecodes_cursor.upsert(hash, Bytecode(bytecode))?;
        }

        // Write new storage state and wipe storage if needed.
        tracing::trace!(target: "provider::bundle_state", len = self.0.storage.len(), "Writing new storage state");
        let mut storages_cursor =
            provider.tx_ref().cursor_dup_write::<tables::PlainStorageState>()?;
        for PlainStorageChangeset { address, wipe_storage, storage } in self.0.storage {
            // Wiping of storage.
            if wipe_storage && storages_cursor.seek_exact(address)?.is_some() {
                storages_cursor.delete_current_duplicates()?;
            }
            // cast storages to B256.
            let mut storage = storage
                .into_iter()
                .map(|(k, value)| StorageEntry { key: k.into(), value })
                .collect::<Vec<_>>();
            // sort storage slots by key.
            storage.par_sort_unstable_by_key(|a| a.key);

            for entry in storage {
                tracing::trace!(target: "provider::bundle_state", ?address, ?entry.key, "Updating plain state storage");
                if let Some(db_entry) = storages_cursor.seek_by_key_subkey(address, entry.key)? {
                    if db_entry.key == entry.key {
                        storages_cursor.delete_current()?;
                    }
                }

                if entry.value != U256::ZERO {
                    storages_cursor.upsert(address, entry)?;
                }
            }
        }

        Ok(())
    }
}
