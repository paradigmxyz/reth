use reth_db::{
    common::KeyValue,
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO},
    database::Database,
    models::{AccountBeforeTx, StoredBlockBodyIndices},
    table::{Table, TableRow},
    tables,
    test_utils::{create_test_rw_db, create_test_rw_db_with_path, TempDatabase},
    transaction::{DbTx, DbTxMut},
    DatabaseEnv, DatabaseError as DbError,
};
use reth_interfaces::{provider::ProviderResult, test_utils::generators::ChangeSet, RethResult};
use reth_primitives::{
    keccak256, Account, Address, BlockNumber, Receipt, SealedBlock, SealedHeader, StorageEntry,
    TxHash, TxNumber, B256, MAINNET, U256,
};
use reth_provider::{DatabaseProviderRO, DatabaseProviderRW, HistoryWriter, ProviderFactory};
use std::{
    borrow::Borrow,
    collections::BTreeMap,
    ops::{Deref, RangeInclusive},
    path::{Path, PathBuf},
    sync::Arc,
};

/// Test database that is used for testing stage implementations.
#[derive(Debug)]
pub struct TestStageDB {
    pub factory: ProviderFactory<Arc<TempDatabase<DatabaseEnv>>>,
}

impl Default for TestStageDB {
    /// Create a new instance of [TestStageDB]
    fn default() -> Self {
        Self { factory: ProviderFactory::new(create_test_rw_db(), MAINNET.clone()) }
    }
}

impl TestStageDB {
    pub fn new(path: &Path) -> Self {
        Self { factory: ProviderFactory::new(create_test_rw_db_with_path(path), MAINNET.clone()) }
    }

    /// Invoke a callback with transaction committing it afterwards
    pub fn commit<F>(&self, f: F) -> ProviderResult<()>
    where
        F: FnOnce(&<DatabaseEnv as Database>::TXMut) -> ProviderResult<()>,
    {
        let mut tx = self.factory.provider_rw()?;
        f(tx.tx_ref())?;
        tx.commit().expect("failed to commit");
        Ok(())
    }

    /// Invoke a callback with a read transaction
    pub fn query<F, Ok>(&self, f: F) -> ProviderResult<Ok>
    where
        F: FnOnce(&<DatabaseEnv as Database>::TX) -> ProviderResult<Ok>,
    {
        f(self.factory.provider()?.tx_ref())
    }

    /// Check if the table is empty
    pub fn table_is_empty<T: Table>(&self) -> ProviderResult<bool> {
        self.query(|tx| {
            let last = tx.cursor_read::<T>()?.last()?;
            Ok(last.is_none())
        })
    }

    /// Return full table as Vec
    pub fn table<T: Table>(&self) -> ProviderResult<Vec<KeyValue<T>>>
    where
        T::Key: Default + Ord,
    {
        self.query(|tx| {
            Ok(tx
                .cursor_read::<T>()?
                .walk(Some(T::Key::default()))?
                .collect::<Result<Vec<_>, DbError>>()?)
        })
    }

    /// Check that there is no table entry above a given
    /// number by [Table::Key]
    pub fn ensure_no_entry_above<T, F>(&self, num: u64, mut selector: F) -> ProviderResult<()>
    where
        T: Table,
        F: FnMut(T::Key) -> BlockNumber,
    {
        self.query(|tx| {
            let mut cursor = tx.cursor_read::<T>()?;
            if let Some((key, _)) = cursor.last()? {
                assert!(selector(key) <= num);
            }
            Ok(())
        })
    }

    /// Check that there is no table entry above a given
    /// number by [Table::Value]
    pub fn ensure_no_entry_above_by_value<T, F>(
        &self,
        num: u64,
        mut selector: F,
    ) -> ProviderResult<()>
    where
        T: Table,
        F: FnMut(T::Value) -> BlockNumber,
    {
        self.query(|tx| {
            let mut cursor = tx.cursor_read::<T>()?;
            let mut rev_walker = cursor.walk_back(None)?;
            while let Some((_, value)) = rev_walker.next().transpose()? {
                assert!(selector(value) <= num);
            }
            Ok(())
        })
    }

    /// Inserts a single [SealedHeader] into the corresponding tables of the headers stage.
    fn insert_header<TX: DbTxMut + DbTx>(tx: &TX, header: &SealedHeader) -> Result<(), DbError> {
        tx.put::<tables::CanonicalHeaders>(header.number, header.hash())?;
        tx.put::<tables::HeaderNumbers>(header.hash(), header.number)?;
        tx.put::<tables::Headers>(header.number, header.clone().unseal())
    }

    /// Insert ordered collection of [SealedHeader] into the corresponding tables
    /// that are supposed to be populated by the headers stage.
    pub fn insert_headers<'a, I>(&self, headers: I) -> ProviderResult<()>
    where
        I: Iterator<Item = &'a SealedHeader>,
    {
        self.commit(|tx| {
            Ok(headers.into_iter().try_for_each(|header| Self::insert_header(tx, header))?)
        })
    }

    /// Inserts total difficulty of headers into the corresponding tables.
    ///
    /// Superset functionality of [TestStageDB::insert_headers].
    pub fn insert_headers_with_td<'a, I>(&self, headers: I) -> ProviderResult<()>
    where
        I: Iterator<Item = &'a SealedHeader>,
    {
        self.commit(|tx| {
            let mut td = U256::ZERO;
            headers.into_iter().try_for_each(|header| {
                Self::insert_header(tx, header)?;
                td += header.difficulty;
                Ok(tx.put::<tables::HeaderTD>(header.number, td.into())?)
            })
        })
    }

    /// Insert ordered collection of [SealedBlock] into corresponding tables.
    /// Superset functionality of [TestStageDB::insert_headers].
    ///
    /// Assumes that there's a single transition for each transaction (i.e. no block rewards).
    pub fn insert_blocks<'a, I>(&self, blocks: I, tx_offset: Option<u64>) -> ProviderResult<()>
    where
        I: Iterator<Item = &'a SealedBlock>,
    {
        self.commit(|tx| {
            let mut next_tx_num = tx_offset.unwrap_or_default();

            blocks.into_iter().try_for_each(|block| {
                Self::insert_header(tx, &block.header)?;
                // Insert into body tables.
                let block_body_indices = StoredBlockBodyIndices {
                    first_tx_num: next_tx_num,
                    tx_count: block.body.len() as u64,
                };

                if !block.body.is_empty() {
                    tx.put::<tables::TransactionBlock>(
                        block_body_indices.last_tx_num(),
                        block.number,
                    )?;
                }
                tx.put::<tables::BlockBodyIndices>(block.number, block_body_indices)?;

                block.body.iter().try_for_each(|body_tx| {
                    tx.put::<tables::Transactions>(next_tx_num, body_tx.clone().into())?;
                    next_tx_num += 1;
                    Ok(())
                })
            })
        })
    }

    pub fn insert_tx_hash_numbers<I>(&self, tx_hash_numbers: I) -> ProviderResult<()>
    where
        I: IntoIterator<Item = (TxHash, TxNumber)>,
    {
        self.commit(|tx| {
            tx_hash_numbers.into_iter().try_for_each(|(tx_hash, tx_num)| {
                // Insert into tx hash numbers table.
                Ok(tx.put::<tables::TxHashNumber>(tx_hash, tx_num)?)
            })
        })
    }

    /// Insert collection of ([TxNumber], [Receipt]) into the corresponding table.
    pub fn insert_receipts<I>(&self, receipts: I) -> ProviderResult<()>
    where
        I: IntoIterator<Item = (TxNumber, Receipt)>,
    {
        self.commit(|tx| {
            receipts.into_iter().try_for_each(|(tx_num, receipt)| {
                // Insert into receipts table.
                Ok(tx.put::<tables::Receipts>(tx_num, receipt)?)
            })
        })
    }

    pub fn insert_transaction_senders<I>(&self, transaction_senders: I) -> ProviderResult<()>
    where
        I: IntoIterator<Item = (TxNumber, Address)>,
    {
        self.commit(|tx| {
            transaction_senders.into_iter().try_for_each(|(tx_num, sender)| {
                // Insert into receipts table.
                Ok(tx.put::<tables::TxSenders>(tx_num, sender)?)
            })
        })
    }

    /// Insert collection of ([Address], [Account]) into corresponding tables.
    pub fn insert_accounts_and_storages<I, S>(&self, accounts: I) -> ProviderResult<()>
    where
        I: IntoIterator<Item = (Address, (Account, S))>,
        S: IntoIterator<Item = StorageEntry>,
    {
        self.commit(|tx| {
            accounts.into_iter().try_for_each(|(address, (account, storage))| {
                let hashed_address = keccak256(address);

                // Insert into account tables.
                tx.put::<tables::PlainAccountState>(address, account)?;
                tx.put::<tables::HashedAccount>(hashed_address, account)?;

                // Insert into storage tables.
                storage.into_iter().filter(|e| e.value != U256::ZERO).try_for_each(|entry| {
                    let hashed_entry = StorageEntry { key: keccak256(entry.key), ..entry };

                    let mut cursor = tx.cursor_dup_write::<tables::PlainStorageState>()?;
                    if let Some(e) = cursor
                        .seek_by_key_subkey(address, entry.key)?
                        .filter(|e| e.key == entry.key)
                    {
                        cursor.delete_current()?;
                    }
                    cursor.upsert(address, entry)?;

                    let mut cursor = tx.cursor_dup_write::<tables::HashedStorage>()?;
                    if let Some(e) = cursor
                        .seek_by_key_subkey(hashed_address, hashed_entry.key)?
                        .filter(|e| e.key == hashed_entry.key)
                    {
                        cursor.delete_current()?;
                    }
                    cursor.upsert(hashed_address, hashed_entry)?;

                    Ok(())
                })
            })
        })
    }

    /// Insert collection of [ChangeSet] into corresponding tables.
    pub fn insert_changesets<I>(
        &self,
        changesets: I,
        block_offset: Option<u64>,
    ) -> ProviderResult<()>
    where
        I: IntoIterator<Item = ChangeSet>,
    {
        let offset = block_offset.unwrap_or_default();
        self.commit(|tx| {
            changesets.into_iter().enumerate().try_for_each(|(block, changeset)| {
                changeset.into_iter().try_for_each(|(address, old_account, old_storage)| {
                    let block = offset + block as u64;
                    // Insert into account changeset.
                    tx.put::<tables::AccountChangeSet>(
                        block,
                        AccountBeforeTx { address, info: Some(old_account) },
                    )?;

                    let block_address = (block, address).into();

                    // Insert into storage changeset.
                    old_storage.into_iter().try_for_each(|entry| {
                        Ok(tx.put::<tables::StorageChangeSet>(block_address, entry)?)
                    })
                })
            })
        })
    }

    pub fn insert_history<I>(&self, changesets: I, block_offset: Option<u64>) -> ProviderResult<()>
    where
        I: IntoIterator<Item = ChangeSet>,
    {
        let mut accounts = BTreeMap::<Address, Vec<u64>>::new();
        let mut storages = BTreeMap::<(Address, B256), Vec<u64>>::new();

        for (block, changeset) in changesets.into_iter().enumerate() {
            for (address, _, storage_entries) in changeset {
                accounts.entry(address).or_default().push(block as u64);
                for storage_entry in storage_entries {
                    storages.entry((address, storage_entry.key)).or_default().push(block as u64);
                }
            }
        }

        let provider_rw = self.factory.provider_rw()?;
        provider_rw.insert_account_history_index(accounts)?;
        provider_rw.insert_storage_history_index(storages)?;
        provider_rw.commit()?;

        Ok(())
    }
}
