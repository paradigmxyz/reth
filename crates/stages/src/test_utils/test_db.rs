use reth_db::{
    common::KeyValue,
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO},
    mdbx::{
        test_utils::{create_test_db, create_test_db_with_path},
        tx::Tx,
        Env, EnvKind, WriteMap, RW,
    },
    models::{AccountBeforeTx, StoredBlockBodyIndices},
    table::Table,
    tables,
    transaction::{DbTx, DbTxMut},
    DatabaseError as DbError,
};
use reth_primitives::{
    keccak256, Account, Address, BlockNumber, SealedBlock, SealedHeader, StorageEntry, H256,
    MAINNET, U256,
};
use reth_provider::{DatabaseProviderRW, ShareableDatabase};
use std::{
    borrow::Borrow,
    collections::BTreeMap,
    ops::RangeInclusive,
    path::{Path, PathBuf},
    sync::Arc,
};

/// The [TestTransaction] is used as an internal
/// database for testing stage implementation.
///
/// ```rust,ignore
/// let tx = TestTransaction::default();
/// stage.execute(&mut tx.container(), input);
/// ```
#[derive(Debug)]
pub struct TestTransaction {
    /// WriteMap DB
    pub tx: Arc<Env<WriteMap>>,
    pub path: Option<PathBuf>,
    factory: ShareableDatabase<Arc<Env<WriteMap>>>,
}

impl Default for TestTransaction {
    /// Create a new instance of [TestTransaction]
    fn default() -> Self {
        let tx = create_test_db::<WriteMap>(EnvKind::RW);
        Self { tx: tx.clone(), path: None, factory: ShareableDatabase::new(tx, MAINNET.clone()) }
    }
}

impl TestTransaction {
    pub fn new(path: &Path) -> Self {
        let tx = create_test_db::<WriteMap>(EnvKind::RW);
        Self {
            tx: tx.clone(),
            path: Some(path.to_path_buf()),
            factory: ShareableDatabase::new(tx, MAINNET.clone()),
        }
    }

    /// Return a database wrapped in [DatabaseProviderRW].
    pub fn inner(&self) -> DatabaseProviderRW<'_, Arc<Env<WriteMap>>> {
        self.factory.provider_rw().expect("failed to create db container")
    }

    /// Get a pointer to an internal database.
    pub fn inner_raw(&self) -> Arc<Env<WriteMap>> {
        self.tx.clone()
    }

    /// Invoke a callback with transaction committing it afterwards
    pub fn commit<F>(&self, f: F) -> Result<(), DbError>
    where
        F: FnOnce(&mut Tx<'_, RW, WriteMap>) -> Result<(), DbError>,
    {
        let mut tx = self.inner();
        f(tx.tx_mut())?;
        tx.commit().expect("failed to commit");
        Ok(())
    }

    /// Invoke a callback with a read transaction
    pub fn query<F, R>(&self, f: F) -> Result<R, DbError>
    where
        F: FnOnce(&Tx<'_, RW, WriteMap>) -> Result<R, DbError>,
    {
        f(self.inner().tx_ref())
    }

    /// Check if the table is empty
    pub fn table_is_empty<T: Table>(&self) -> Result<bool, DbError> {
        self.query(|tx| {
            let last = tx.cursor_read::<T>()?.last()?;
            Ok(last.is_none())
        })
    }

    /// Return full table as Vec
    pub fn table<T: Table>(&self) -> Result<Vec<KeyValue<T>>, DbError>
    where
        T::Key: Default + Ord,
    {
        self.query(|tx| {
            tx.cursor_read::<T>()?
                .walk(Some(T::Key::default()))?
                .collect::<Result<Vec<_>, DbError>>()
        })
    }

    /// Map a collection of values and store them in the database.
    /// This function commits the transaction before exiting.
    ///
    /// ```rust,ignore
    /// let tx = TestTransaction::default();
    /// tx.map_put::<Table, _, _>(&items, |item| item)?;
    /// ```
    #[allow(dead_code)]
    pub fn map_put<T, S, F>(&self, values: &[S], mut map: F) -> Result<(), DbError>
    where
        T: Table,
        S: Clone,
        F: FnMut(&S) -> (T::Key, T::Value),
    {
        self.commit(|tx| {
            values.iter().try_for_each(|src| {
                let (k, v) = map(src);
                tx.put::<T>(k, v)
            })
        })
    }

    /// Transform a collection of values using a callback and store
    /// them in the database. The callback additionally accepts the
    /// optional last element that was stored.
    /// This function commits the transaction before exiting.
    ///
    /// ```rust,ignore
    /// let tx = TestTransaction::default();
    /// tx.transform_append::<Table, _, _>(&items, |prev, item| prev.unwrap_or_default() + item)?;
    /// ```
    #[allow(dead_code)]
    pub fn transform_append<T, S, F>(&self, values: &[S], mut transform: F) -> Result<(), DbError>
    where
        T: Table,
        <T as Table>::Value: Clone,
        S: Clone,
        F: FnMut(&Option<<T as Table>::Value>, &S) -> (T::Key, T::Value),
    {
        self.commit(|tx| {
            let mut cursor = tx.cursor_write::<T>()?;
            let mut last = cursor.last()?.map(|(_, v)| v);
            values.iter().try_for_each(|src| {
                let (k, v) = transform(&last, src);
                last = Some(v.clone());
                cursor.append(k, v)
            })
        })
    }

    /// Check that there is no table entry above a given
    /// number by [Table::Key]
    pub fn ensure_no_entry_above<T, F>(&self, num: u64, mut selector: F) -> Result<(), DbError>
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
    ) -> Result<(), DbError>
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
    fn insert_header(tx: &mut Tx<'_, RW, WriteMap>, header: &SealedHeader) -> Result<(), DbError> {
        tx.put::<tables::CanonicalHeaders>(header.number, header.hash())?;
        tx.put::<tables::HeaderNumbers>(header.hash(), header.number)?;
        tx.put::<tables::Headers>(header.number, header.clone().unseal())
    }

    /// Insert ordered collection of [SealedHeader] into the corresponding tables
    /// that are supposed to be populated by the headers stage.
    pub fn insert_headers<'a, I>(&self, headers: I) -> Result<(), DbError>
    where
        I: Iterator<Item = &'a SealedHeader>,
    {
        self.commit(|tx| headers.into_iter().try_for_each(|header| Self::insert_header(tx, header)))
    }

    /// Inserts total difficulty of headers into the corresponding tables.
    ///
    /// Superset functionality of [TestTransaction::insert_headers].
    pub(crate) fn insert_headers_with_td<'a, I>(&self, headers: I) -> Result<(), DbError>
    where
        I: Iterator<Item = &'a SealedHeader>,
    {
        self.commit(|tx| {
            let mut td = U256::ZERO;
            headers.into_iter().try_for_each(|header| {
                Self::insert_header(tx, header)?;
                td += header.difficulty;
                tx.put::<tables::HeaderTD>(header.number, td.into())
            })
        })
    }

    /// Insert ordered collection of [SealedBlock] into corresponding tables.
    /// Superset functionality of [TestTransaction::insert_headers].
    ///
    /// Assumes that there's a single transition for each transaction (i.e. no block rewards).
    pub fn insert_blocks<'a, I>(&self, blocks: I, tx_offset: Option<u64>) -> Result<(), DbError>
    where
        I: Iterator<Item = &'a SealedBlock>,
    {
        self.commit(|tx| {
            let mut next_tx_num = tx_offset.unwrap_or_default();

            blocks.into_iter().try_for_each(|block| {
                Self::insert_header(tx, &block.header)?;
                // Insert into body tables.
                tx.put::<tables::BlockBodyIndices>(
                    block.number,
                    StoredBlockBodyIndices {
                        first_tx_num: next_tx_num,
                        tx_count: block.body.len() as u64,
                    },
                )?;
                block.body.iter().try_for_each(|body_tx| {
                    tx.put::<tables::Transactions>(next_tx_num, body_tx.clone().into())?;
                    next_tx_num += 1;
                    Ok(())
                })
            })
        })
    }

    /// Insert collection of ([Address], [Account]) into corresponding tables.
    pub fn insert_accounts_and_storages<I, S>(&self, accounts: I) -> Result<(), DbError>
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

    /// Insert collection of Vec<([Address], [Account], Vec<[StorageEntry]>)> into
    /// corresponding tables.
    pub fn insert_transitions<I>(
        &self,
        transitions: I,
        transition_offset: Option<u64>,
    ) -> Result<(), DbError>
    where
        I: IntoIterator<Item = Vec<(Address, Account, Vec<StorageEntry>)>>,
    {
        let offset = transition_offset.unwrap_or_default();
        self.commit(|tx| {
            transitions.into_iter().enumerate().try_for_each(|(transition_id, changes)| {
                changes.into_iter().try_for_each(|(address, old_account, old_storage)| {
                    let tid = offset + transition_id as u64;
                    // Insert into account changeset.
                    tx.put::<tables::AccountChangeSet>(
                        tid,
                        AccountBeforeTx { address, info: Some(old_account) },
                    )?;

                    let tid_address = (tid, address).into();

                    // Insert into storage changeset.
                    old_storage.into_iter().try_for_each(|entry| {
                        tx.put::<tables::StorageChangeSet>(tid_address, entry)
                    })
                })
            })
        })
    }
}
