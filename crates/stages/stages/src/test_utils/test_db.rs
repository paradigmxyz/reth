use alloy_primitives::{keccak256, Address, BlockNumber, TxHash, TxNumber, B256};
use reth_chainspec::MAINNET;
use reth_db::{
    test_utils::{
        create_test_rocksdb_dir, create_test_rw_db, create_test_rw_db_with_path,
        create_test_static_files_dir,
    },
    DatabaseEnv,
};
use reth_db_api::{
    common::KeyValue,
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO},
    database::Database,
    models::{AccountBeforeTx, StorageBeforeTx, StoredBlockBodyIndices},
    table::Table,
    tables,
    transaction::{DbTx, DbTxMut},
    DatabaseError as DbError,
};
use reth_ethereum_primitives::{Block, EthPrimitives, Receipt};
use reth_primitives_traits::{Account, SealedBlock, SealedHeader, StorageEntry};
use reth_provider::{
    providers::{
        RocksDBProvider, StaticFileProvider, StaticFileProviderRWRefMut, StaticFileWriter,
    },
    test_utils::MockNodeTypesWithDB,
    DatabaseProviderFactory, EitherWriter, HistoryWriter, ProviderError, ProviderFactory,
    RocksBatchArg, StaticFileProviderFactory, StatsReader,
};
use reth_static_file_types::StaticFileSegment;
use reth_storage_errors::provider::ProviderResult;
use reth_testing_utils::generators::ChangeSet;
use std::{collections::BTreeMap, fmt::Debug, path::Path};
use tempfile::TempDir;

/// Test database that is used for testing stage implementations.
#[derive(Debug)]
pub struct TestStageDB {
    pub factory: ProviderFactory<MockNodeTypesWithDB>,
    pub temp_static_files_dir: TempDir,
    pub temp_rocksdb_dir: TempDir,
}

impl Default for TestStageDB {
    /// Create a new instance of [`TestStageDB`]
    fn default() -> Self {
        let (static_dir, static_dir_path) = create_test_static_files_dir();
        let (rocksdb_dir, rocksdb_dir_path) = create_test_rocksdb_dir();
        Self {
            temp_static_files_dir: static_dir,
            temp_rocksdb_dir: rocksdb_dir,
            factory: ProviderFactory::new(
                create_test_rw_db(),
                MAINNET.clone(),
                StaticFileProvider::read_write(static_dir_path).unwrap(),
                RocksDBProvider::builder(rocksdb_dir_path).with_default_tables().build().unwrap(),
                reth_tasks::Runtime::test(),
            )
            .expect("failed to create test provider factory"),
        }
    }
}

impl TestStageDB {
    pub fn new(path: &Path) -> Self {
        let (static_dir, static_dir_path) = create_test_static_files_dir();
        let (rocksdb_dir, rocksdb_dir_path) = create_test_rocksdb_dir();

        Self {
            temp_static_files_dir: static_dir,
            temp_rocksdb_dir: rocksdb_dir,
            factory: ProviderFactory::new(
                create_test_rw_db_with_path(path),
                MAINNET.clone(),
                StaticFileProvider::read_write(static_dir_path).unwrap(),
                RocksDBProvider::builder(rocksdb_dir_path).with_default_tables().build().unwrap(),
                reth_tasks::Runtime::test(),
            )
            .expect("failed to create test provider factory"),
        }
    }

    /// Invoke a callback with transaction committing it afterwards
    pub fn commit<F>(&self, f: F) -> ProviderResult<()>
    where
        F: FnOnce(&<DatabaseEnv as Database>::TXMut) -> ProviderResult<()>,
    {
        let tx = self.factory.provider_rw()?;
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

    /// Invoke a callback with a provider that can be used to create transactions or fetch from
    /// static files.
    pub fn query_with_provider<F, Ok>(&self, f: F) -> ProviderResult<Ok>
    where
        F: FnOnce(
            <ProviderFactory<MockNodeTypesWithDB> as DatabaseProviderFactory>::Provider,
        ) -> ProviderResult<Ok>,
    {
        f(self.factory.provider()?)
    }

    /// Invoke a callback with a writable provider, committing afterwards.
    pub fn commit_with_provider<F>(&self, f: F) -> ProviderResult<()>
    where
        F: FnOnce(
            &<ProviderFactory<MockNodeTypesWithDB> as DatabaseProviderFactory>::ProviderRW,
        ) -> ProviderResult<()>,
    {
        let provider = self.factory.provider_rw()?;
        f(&provider)?;
        provider.commit().expect("failed to commit");
        Ok(())
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

    /// Return the number of entries in the table or static file segment
    pub fn count_entries<T: Table>(&self) -> ProviderResult<usize> {
        self.factory.provider()?.count_entries::<T>()
    }

    /// Check that there is no table entry above a given
    /// number by [`Table::Key`]
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
    /// number by [`Table::Value`]
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

    /// Insert header to static file if `writer` exists, otherwise to DB.
    pub fn insert_header<TX: DbTx + DbTxMut>(
        writer: Option<&mut StaticFileProviderRWRefMut<'_, EthPrimitives>>,
        tx: &TX,
        header: &SealedHeader,
    ) -> ProviderResult<()> {
        if let Some(writer) = writer {
            // Backfill: some tests start at a forward block number, but static files require no
            // gaps.
            let segment_header = writer.user_header();
            if segment_header.block_end().is_none() && segment_header.expected_block_start() == 0 {
                for block_number in 0..header.number {
                    let mut prev = header.clone_header();
                    prev.number = block_number;
                    writer.append_header(&prev, &B256::ZERO)?;
                }
            }

            writer.append_header(header.header(), &header.hash())?;
        } else {
            tx.put::<tables::CanonicalHeaders>(header.number, header.hash())?;
            tx.put::<tables::Headers>(header.number, header.header().clone())?;
        }

        tx.put::<tables::HeaderNumbers>(header.hash(), header.number)?;
        Ok(())
    }

    fn insert_headers_inner<'a, I>(&self, headers: I) -> ProviderResult<()>
    where
        I: IntoIterator<Item = &'a SealedHeader>,
    {
        let provider = self.factory.static_file_provider();
        let mut writer = provider.latest_writer(StaticFileSegment::Headers)?;
        let tx = self.factory.provider_rw()?.into_tx();

        for header in headers {
            Self::insert_header(Some(&mut writer), &tx, header)?;
        }

        writer.commit()?;
        tx.commit()?;

        Ok(())
    }

    /// Insert ordered collection of [`SealedHeader`] into the corresponding static file and tables
    /// that are supposed to be populated by the headers stage.
    pub fn insert_headers<'a, I>(&self, headers: I) -> ProviderResult<()>
    where
        I: IntoIterator<Item = &'a SealedHeader>,
    {
        self.insert_headers_inner::<I>(headers)
    }

    /// Insert ordered collection of [`SealedBlock`] into corresponding tables.
    /// Superset functionality of [`TestStageDB::insert_headers`].
    ///
    /// If `tx_offset` is set to `None`, then transactions will be stored on static files, otherwise
    /// database.
    ///
    /// Assumes that there's a single transition for each transaction (i.e. no block rewards).
    pub fn insert_blocks<'a, I>(&self, blocks: I, storage_kind: StorageKind) -> ProviderResult<()>
    where
        I: IntoIterator<Item = &'a SealedBlock<Block>>,
    {
        let provider = self.factory.static_file_provider();

        let tx = self.factory.provider_rw().unwrap().into_tx();
        let mut next_tx_num = storage_kind.tx_offset();

        let blocks = blocks.into_iter().collect::<Vec<_>>();

        {
            let mut headers_writer = storage_kind
                .is_static()
                .then(|| provider.latest_writer(StaticFileSegment::Headers).unwrap());

            blocks.iter().try_for_each(|block| {
                Self::insert_header(headers_writer.as_mut(), &tx, block.sealed_header())
            })?;

            if let Some(mut writer) = headers_writer {
                writer.commit()?;
            }
        }

        {
            let mut txs_writer = storage_kind
                .is_static()
                .then(|| provider.latest_writer(StaticFileSegment::Transactions).unwrap());

            blocks.into_iter().try_for_each(|block| {
                // Insert into body tables.
                let block_body_indices = StoredBlockBodyIndices {
                    first_tx_num: next_tx_num,
                    tx_count: block.transaction_count() as u64,
                };

                if !block.body().transactions.is_empty() {
                    tx.put::<tables::TransactionBlocks>(
                        block_body_indices.last_tx_num(),
                        block.number,
                    )?;
                }
                tx.put::<tables::BlockBodyIndices>(block.number, block_body_indices)?;

                let res = block.body().transactions.iter().try_for_each(|body_tx| {
                    if let Some(txs_writer) = &mut txs_writer {
                        txs_writer.append_transaction(next_tx_num, body_tx)?;
                    } else {
                        tx.put::<tables::Transactions>(next_tx_num, body_tx.clone())?
                    }
                    next_tx_num += 1;
                    Ok::<(), ProviderError>(())
                });

                if let Some(txs_writer) = &mut txs_writer {
                    // Backfill: some tests start at a forward block number, but static files
                    // require no gaps.
                    let segment_header = txs_writer.user_header();
                    if segment_header.block_end().is_none() &&
                        segment_header.expected_block_start() == 0
                    {
                        for block in 0..block.number {
                            txs_writer.increment_block(block)?;
                        }
                    }
                    txs_writer.increment_block(block.number)?;
                }
                res
            })?;

            if let Some(txs_writer) = &mut txs_writer {
                txs_writer.commit()?;
            }
        }

        tx.commit()?;

        Ok(())
    }

    pub fn insert_tx_hash_numbers<I>(&self, tx_hash_numbers: I) -> ProviderResult<()>
    where
        I: IntoIterator<Item = (TxHash, TxNumber)>,
    {
        self.commit_with_provider(|provider| {
            provider.with_rocksdb_batch(|batch: RocksBatchArg<'_>| {
                let mut writer = EitherWriter::new_transaction_hash_numbers(provider, batch)?;
                for (tx_hash, tx_num) in tx_hash_numbers {
                    writer.put_transaction_hash_number(tx_hash, tx_num, false)?;
                }
                Ok(((), writer.into_raw_rocksdb_batch()))
            })
        })
    }

    /// Insert collection of ([`TxNumber`], [Receipt]) into the corresponding table.
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

    /// Insert collection of ([`TxNumber`], [Receipt]) organized by respective block numbers into
    /// the corresponding table or static file segment.
    pub fn insert_receipts_by_block<I, J>(
        &self,
        receipts: I,
        storage_kind: StorageKind,
    ) -> ProviderResult<()>
    where
        I: IntoIterator<Item = (BlockNumber, J)>,
        J: IntoIterator<Item = (TxNumber, Receipt)>,
    {
        match storage_kind {
            StorageKind::Database(_) => self.commit(|tx| {
                receipts.into_iter().try_for_each(|(_, receipts)| {
                    for (tx_num, receipt) in receipts {
                        tx.put::<tables::Receipts>(tx_num, receipt)?;
                    }
                    Ok(())
                })
            }),
            StorageKind::Static => {
                let provider = self.factory.static_file_provider();
                let mut writer = provider.latest_writer(StaticFileSegment::Receipts)?;
                let res = receipts.into_iter().try_for_each(|(block_num, receipts)| {
                    writer.increment_block(block_num)?;
                    writer.append_receipts(receipts.into_iter().map(Ok))?;
                    Ok(())
                });
                writer.commit_without_sync_all()?;
                res
            }
        }
    }

    pub fn insert_transaction_senders<I>(&self, transaction_senders: I) -> ProviderResult<()>
    where
        I: IntoIterator<Item = (TxNumber, Address)>,
    {
        self.commit(|tx| {
            transaction_senders.into_iter().try_for_each(|(tx_num, sender)| {
                // Insert into receipts table.
                Ok(tx.put::<tables::TransactionSenders>(tx_num, sender)?)
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
                tx.put::<tables::HashedAccounts>(hashed_address, account)?;

                // Insert into storage tables.
                storage.into_iter().filter(|e| !e.value.is_zero()).try_for_each(|entry| {
                    let hashed_entry = StorageEntry { key: keccak256(entry.key), ..entry };

                    let mut cursor = tx.cursor_dup_write::<tables::PlainStorageState>()?;
                    if cursor
                        .seek_by_key_subkey(address, entry.key)?
                        .is_some_and(|e| e.key == entry.key)
                    {
                        cursor.delete_current()?;
                    }
                    cursor.upsert(address, &entry)?;

                    let mut cursor = tx.cursor_dup_write::<tables::HashedStorages>()?;
                    if cursor
                        .seek_by_key_subkey(hashed_address, hashed_entry.key)?
                        .is_some_and(|e| e.key == hashed_entry.key)
                    {
                        cursor.delete_current()?;
                    }
                    cursor.upsert(hashed_address, &hashed_entry)?;

                    Ok(())
                })
            })
        })
    }

    /// Insert collection of [`ChangeSet`] into corresponding tables.
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
                    tx.put::<tables::AccountChangeSets>(
                        block,
                        AccountBeforeTx { address, info: Some(old_account) },
                    )?;

                    let block_address = (block, address).into();

                    // Insert into storage changeset.
                    old_storage.into_iter().try_for_each(|entry| {
                        Ok(tx.put::<tables::StorageChangeSets>(block_address, entry)?)
                    })
                })
            })
        })
    }

    /// Insert collection of [`ChangeSet`] into static files (account and storage changesets).
    pub fn insert_changesets_to_static_files<I>(
        &self,
        changesets: I,
        block_offset: Option<u64>,
    ) -> ProviderResult<()>
    where
        I: IntoIterator<Item = ChangeSet>,
    {
        let offset = block_offset.unwrap_or_default();
        let static_file_provider = self.factory.static_file_provider();

        let mut account_changeset_writer =
            static_file_provider.latest_writer(StaticFileSegment::AccountChangeSets)?;
        let mut storage_changeset_writer =
            static_file_provider.latest_writer(StaticFileSegment::StorageChangeSets)?;

        for (block, changeset) in changesets.into_iter().enumerate() {
            let block_number = offset + block as u64;

            let mut account_changesets = Vec::new();
            let mut storage_changesets = Vec::new();

            for (address, old_account, old_storage) in changeset {
                account_changesets.push(AccountBeforeTx { address, info: Some(old_account) });

                for entry in old_storage {
                    storage_changesets.push(StorageBeforeTx {
                        address,
                        key: entry.key,
                        value: entry.value,
                    });
                }
            }

            account_changeset_writer.append_account_changeset(account_changesets, block_number)?;
            storage_changeset_writer.append_storage_changeset(storage_changesets, block_number)?;
        }

        account_changeset_writer.commit()?;
        storage_changeset_writer.commit()?;

        Ok(())
    }

    pub fn insert_history<I>(&self, changesets: I, _block_offset: Option<u64>) -> ProviderResult<()>
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

/// Used to identify where to store data when setting up a test.
#[derive(Debug)]
pub enum StorageKind {
    Database(Option<u64>),
    Static,
}

impl StorageKind {
    #[expect(dead_code)]
    const fn is_database(&self) -> bool {
        matches!(self, Self::Database(_))
    }

    const fn is_static(&self) -> bool {
        matches!(self, Self::Static)
    }

    fn tx_offset(&self) -> u64 {
        if let Self::Database(offset) = self {
            return offset.unwrap_or_default();
        }
        0
    }
}
