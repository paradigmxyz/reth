use crate::{providers::StaticFileProviderRWRefMut, DatabaseProviderRW};
use itertools::Itertools;
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
    tables,
    transaction::{DbTx, DbTxMut},
    Database,
};
use reth_errors::{ProviderError, ProviderResult};
use reth_primitives::{BlockNumber, StorageEntry, U256};
use reth_storage_api::ReceiptWriter;
use reth_storage_errors::writer::StorageWriterError;
use reth_trie::HashedPostStateSorted;
use static_file::StaticFileWriter;

mod database;
mod static_file;
use database::DatabaseWriter;

enum StorageType<C = (), S = ()> {
    Database(C),
    StaticFile(S),
}

/// [`StorageWriter`] is responsible for managing the writing to either database, static file or
/// both.
#[derive(Debug)]
pub struct StorageWriter<'a, 'b, DB: Database> {
    database_writer: Option<&'a DatabaseProviderRW<DB>>,
    static_file_writer: Option<StaticFileProviderRWRefMut<'b>>,
}

impl<'a, 'b, DB: Database> StorageWriter<'a, 'b, DB> {
    /// Creates a new instance of [`StorageWriter`].
    ///
    /// # Parameters
    /// - `database_writer`: An optional reference to a database writer.
    /// - `static_file_writer`: An optional mutable reference to a static file writer.
    pub const fn new(
        database_writer: Option<&'a DatabaseProviderRW<DB>>,
        static_file_writer: Option<StaticFileProviderRWRefMut<'b>>,
    ) -> Self {
        Self { database_writer, static_file_writer }
    }

    /// Creates a new instance of [`StorageWriter`] from a database writer.
    pub const fn from_database_writer(database_writer: &'a DatabaseProviderRW<DB>) -> Self {
        Self::new(Some(database_writer), None)
    }

    /// Creates a new instance of [`StorageWriter`] from a static file writer.
    pub const fn from_static_file_writer(
        static_file_writer: StaticFileProviderRWRefMut<'b>,
    ) -> Self {
        Self::new(None, Some(static_file_writer))
    }

    /// Returns a reference to the database writer.
    ///
    /// # Panics
    /// If the database writer is not set.
    fn database_writer(&self) -> &DatabaseProviderRW<DB> {
        self.database_writer.as_ref().expect("should exist")
    }

    /// Returns a mutable reference to the static file writer.
    ///
    /// # Panics
    /// If the static file writer is not set.
    fn static_file_writer(&mut self) -> &mut StaticFileProviderRWRefMut<'b> {
        self.static_file_writer.as_mut().expect("should exist")
    }

    /// Ensures that the database writer is set.
    ///
    /// # Returns
    /// - `Ok(())` if the database writer is set.
    /// - `Err(StorageWriterError::MissingDatabaseWriter)` if the database writer is not set.
    const fn ensure_database_writer(&self) -> Result<(), StorageWriterError> {
        if self.database_writer.is_none() {
            return Err(StorageWriterError::MissingDatabaseWriter)
        }
        Ok(())
    }

    /// Ensures that the static file writer is set.
    ///
    /// # Returns
    /// - `Ok(())` if the static file writer is set.
    /// - `Err(StorageWriterError::MissingStaticFileWriter)` if the static file writer is not set.
    const fn ensure_static_file_writer(&self) -> Result<(), StorageWriterError> {
        if self.static_file_writer.is_none() {
            return Err(StorageWriterError::MissingStaticFileWriter)
        }
        Ok(())
    }

    /// Writes the hashed state changes to the database
    pub fn write_hashed_state(&self, hashed_state: &HashedPostStateSorted) -> ProviderResult<()> {
        self.ensure_database_writer()?;

        // Write hashed account updates.
        let mut hashed_accounts_cursor =
            self.database_writer().tx_ref().cursor_write::<tables::HashedAccounts>()?;
        for (hashed_address, account) in hashed_state.accounts().accounts_sorted() {
            if let Some(account) = account {
                hashed_accounts_cursor.upsert(hashed_address, account)?;
            } else if hashed_accounts_cursor.seek_exact(hashed_address)?.is_some() {
                hashed_accounts_cursor.delete_current()?;
            }
        }

        // Write hashed storage changes.
        let sorted_storages = hashed_state.account_storages().iter().sorted_by_key(|(key, _)| *key);
        let mut hashed_storage_cursor =
            self.database_writer().tx_ref().cursor_dup_write::<tables::HashedStorages>()?;
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

    /// Appends receipts block by block.
    ///
    /// ATTENTION: If called from [`StorageWriter`] without a static file producer, it will always
    /// write them to database. Otherwise, it will look into the pruning configuration to decide.
    ///
    /// # Parameters
    /// - `initial_block_number`: The starting block number.
    /// - `blocks`: An iterator over blocks, each block having a vector of optional receipts. If
    ///   `receipt` is `None`, it has been pruned.
    pub fn append_receipts_from_blocks(
        mut self,
        initial_block_number: BlockNumber,
        blocks: impl Iterator<Item = Vec<Option<reth_primitives::Receipt>>>,
    ) -> ProviderResult<()> {
        self.ensure_database_writer()?;
        let mut bodies_cursor =
            self.database_writer().tx_ref().cursor_read::<tables::BlockBodyIndices>()?;

        // We write receipts to database in two situations:
        // * If we are in live sync. In this case, `StorageWriter` is built without a static file
        //   writer.
        // * If there is any kind of receipt pruning
        let mut storage_type = if self.static_file_writer.is_none() ||
            self.database_writer().prune_modes_ref().has_receipts_pruning()
        {
            StorageType::Database(
                self.database_writer().tx_ref().cursor_write::<tables::Receipts>()?,
            )
        } else {
            self.ensure_static_file_writer()?;
            StorageType::StaticFile(self.static_file_writer())
        };

        for (idx, receipts) in blocks.enumerate() {
            let block_number = initial_block_number + idx as u64;

            let first_tx_index = bodies_cursor
                .seek_exact(block_number)?
                .map(|(_, indices)| indices.first_tx_num())
                .ok_or_else(|| ProviderError::BlockBodyIndicesNotFound(block_number))?;

            match &mut storage_type {
                StorageType::Database(cursor) => {
                    DatabaseWriter(cursor).append_block_receipts(
                        first_tx_index,
                        block_number,
                        receipts,
                    )?;
                }
                StorageType::StaticFile(sf) => {
                    StaticFileWriter(*sf).append_block_receipts(
                        first_tx_index,
                        block_number,
                        receipts,
                    )?;
                }
            };
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
        let storage_writer = StorageWriter::new(Some(&provider_rw), None);
        assert_eq!(storage_writer.write_hashed_state(&hashed_state.into_sorted()), Ok(()));
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
