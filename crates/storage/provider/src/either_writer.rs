//! Generic reader and writer abstractions for interacting with either database tables or static
//! files.

use std::{
    collections::BTreeSet,
    marker::PhantomData,
    ops::{Range, RangeInclusive},
};

#[cfg(all(unix, feature = "rocksdb"))]
use crate::providers::rocksdb::RocksDBBatch;
use crate::{
    providers::{StaticFileProvider, StaticFileProviderRWRefMut},
    StaticFileProviderFactory,
};
use alloy_primitives::{map::HashMap, Address, BlockNumber, TxHash, TxNumber, B256};

use crate::providers::{compute_history_rank, needs_prev_shard_check, HistoryInfo};
use rayon::slice::ParallelSliceMut;
use reth_db::{
    cursor::{DbCursorRO, DbDupCursorRW},
    models::AccountBeforeTx,
    static_file::TransactionSenderMask,
    table::Value,
    transaction::{CursorMutTy, CursorTy, DbTx, DbTxMut, DupCursorMutTy, DupCursorTy},
};
use reth_db_api::{
    cursor::DbCursorRW,
    models::{storage_sharded_key::StorageShardedKey, ShardedKey},
    tables,
    tables::BlockNumberList,
};
use reth_errors::ProviderError;
use reth_node_types::NodePrimitives;
use reth_primitives_traits::ReceiptTy;
use reth_static_file_types::StaticFileSegment;
use reth_storage_api::{ChangeSetReader, DBProvider, NodePrimitivesProvider, StorageSettingsCache};
use reth_storage_errors::provider::ProviderResult;
use strum::{Display, EnumIs};

/// Type alias for [`EitherReader`] constructors.
type EitherReaderTy<'a, P, T> =
    EitherReader<'a, CursorTy<<P as DBProvider>::Tx, T>, <P as NodePrimitivesProvider>::Primitives>;

/// Type alias for [`EitherReader`] constructors.
type DupEitherReaderTy<'a, P, T> = EitherReader<
    'a,
    DupCursorTy<<P as DBProvider>::Tx, T>,
    <P as NodePrimitivesProvider>::Primitives,
>;

/// Type alias for dup [`EitherWriter`] constructors.
type DupEitherWriterTy<'a, P, T> = EitherWriter<
    'a,
    DupCursorMutTy<<P as DBProvider>::Tx, T>,
    <P as NodePrimitivesProvider>::Primitives,
>;

/// Type alias for [`EitherWriter`] constructors.
type EitherWriterTy<'a, P, T> = EitherWriter<
    'a,
    CursorMutTy<<P as DBProvider>::Tx, T>,
    <P as NodePrimitivesProvider>::Primitives,
>;

/// Helper type for `RocksDB` batch argument in writer constructors.
///
/// When `rocksdb` feature is enabled, this is a real `RocksDB` batch.
/// Otherwise, it's `()` (unit type) to allow the same API without feature gates.
#[cfg(all(unix, feature = "rocksdb"))]
pub type RocksBatchArg<'a> = crate::providers::rocksdb::RocksDBBatch<'a>;
/// Helper type for `RocksDB` batch argument in writer constructors.
///
/// When `rocksdb` feature is enabled, this is a real `RocksDB` batch.
/// Otherwise, it's `()` (unit type) to allow the same API without feature gates.
#[cfg(not(all(unix, feature = "rocksdb")))]
pub type RocksBatchArg<'a> = ();

/// The raw `RocksDB` batch type returned by [`EitherWriter::into_raw_rocksdb_batch`].
#[cfg(all(unix, feature = "rocksdb"))]
pub type RawRocksDBBatch = rocksdb::WriteBatchWithTransaction<true>;
/// The raw `RocksDB` batch type returned by [`EitherWriter::into_raw_rocksdb_batch`].
#[cfg(not(all(unix, feature = "rocksdb")))]
pub type RawRocksDBBatch = ();

/// Helper type for `RocksDB` transaction reference argument in reader constructors.
///
/// When `rocksdb` feature is enabled, this is a reference to a `RocksDB` transaction.
/// Otherwise, it's `()` (unit type) to allow the same API without feature gates.
#[cfg(all(unix, feature = "rocksdb"))]
pub type RocksTxRefArg<'a> = &'a crate::providers::rocksdb::RocksTx<'a>;
/// Helper type for `RocksDB` transaction reference argument in reader constructors.
///
/// When `rocksdb` feature is enabled, this is a reference to a `RocksDB` transaction.
/// Otherwise, it's `()` (unit type) to allow the same API without feature gates.
#[cfg(not(all(unix, feature = "rocksdb")))]
pub type RocksTxRefArg<'a> = ();

/// Represents a destination for writing data, either to database, static files, or `RocksDB`.
#[derive(Debug, Display)]
pub enum EitherWriter<'a, CURSOR, N> {
    /// Write to database table via cursor
    Database(CURSOR),
    /// Write to static file
    StaticFile(StaticFileProviderRWRefMut<'a, N>),
    /// Write to `RocksDB` using a write-only batch (historical tables).
    #[cfg(all(unix, feature = "rocksdb"))]
    RocksDB(RocksDBBatch<'a>),
}

impl<'a> EitherWriter<'a, (), ()> {
    /// Creates a new [`EitherWriter`] for receipts based on storage settings and prune modes.
    pub fn new_receipts<P>(
        provider: &'a P,
        block_number: BlockNumber,
    ) -> ProviderResult<EitherWriterTy<'a, P, tables::Receipts<ReceiptTy<P::Primitives>>>>
    where
        P: DBProvider + NodePrimitivesProvider + StorageSettingsCache + StaticFileProviderFactory,
        P::Tx: DbTxMut,
        ReceiptTy<P::Primitives>: Value,
    {
        if Self::receipts_destination(provider).is_static_file() {
            Ok(EitherWriter::StaticFile(
                provider.get_static_file_writer(block_number, StaticFileSegment::Receipts)?,
            ))
        } else {
            Ok(EitherWriter::Database(
                provider.tx_ref().cursor_write::<tables::Receipts<ReceiptTy<P::Primitives>>>()?,
            ))
        }
    }

    /// Creates a new [`EitherWriter`] for senders based on storage settings.
    pub fn new_senders<P>(
        provider: &'a P,
        block_number: BlockNumber,
    ) -> ProviderResult<EitherWriterTy<'a, P, tables::TransactionSenders>>
    where
        P: DBProvider + NodePrimitivesProvider + StorageSettingsCache + StaticFileProviderFactory,
        P::Tx: DbTxMut,
    {
        if EitherWriterDestination::senders(provider).is_static_file() {
            Ok(EitherWriter::StaticFile(
                provider
                    .get_static_file_writer(block_number, StaticFileSegment::TransactionSenders)?,
            ))
        } else {
            Ok(EitherWriter::Database(
                provider.tx_ref().cursor_write::<tables::TransactionSenders>()?,
            ))
        }
    }

    /// Creates a new [`EitherWriter`] for account changesets based on storage settings and prune
    /// modes.
    pub fn new_account_changesets<P>(
        provider: &'a P,
        block_number: BlockNumber,
    ) -> ProviderResult<DupEitherWriterTy<'a, P, tables::AccountChangeSets>>
    where
        P: DBProvider + NodePrimitivesProvider + StorageSettingsCache + StaticFileProviderFactory,
        P::Tx: DbTxMut,
    {
        if provider.cached_storage_settings().account_changesets_in_static_files {
            Ok(EitherWriter::StaticFile(
                provider
                    .get_static_file_writer(block_number, StaticFileSegment::AccountChangeSets)?,
            ))
        } else {
            Ok(EitherWriter::Database(
                provider.tx_ref().cursor_dup_write::<tables::AccountChangeSets>()?,
            ))
        }
    }

    /// Returns the destination for writing receipts.
    ///
    /// The rules are as follows:
    /// - If the node should not always write receipts to static files, and any receipt pruning is
    ///   enabled, write to the database.
    /// - If the node should always write receipts to static files, but receipt log filter pruning
    ///   is enabled, write to the database.
    /// - Otherwise, write to static files.
    pub fn receipts_destination<P: DBProvider + StorageSettingsCache>(
        provider: &P,
    ) -> EitherWriterDestination {
        let receipts_in_static_files = provider.cached_storage_settings().receipts_in_static_files;
        let prune_modes = provider.prune_modes_ref();

        if !receipts_in_static_files && prune_modes.has_receipts_pruning() ||
            // TODO: support writing receipts to static files with log filter pruning enabled
            receipts_in_static_files && !prune_modes.receipts_log_filter.is_empty()
        {
            EitherWriterDestination::Database
        } else {
            EitherWriterDestination::StaticFile
        }
    }

    /// Returns the destination for writing account changesets.
    ///
    /// This determines the destination based solely on storage settings.
    pub fn account_changesets_destination<P: DBProvider + StorageSettingsCache>(
        provider: &P,
    ) -> EitherWriterDestination {
        if provider.cached_storage_settings().account_changesets_in_static_files {
            EitherWriterDestination::StaticFile
        } else {
            EitherWriterDestination::Database
        }
    }

    /// Creates a new [`EitherWriter`] for storages history based on storage settings.
    pub fn new_storages_history<P>(
        provider: &P,
        _rocksdb_batch: RocksBatchArg<'a>,
    ) -> ProviderResult<EitherWriterTy<'a, P, tables::StoragesHistory>>
    where
        P: DBProvider + NodePrimitivesProvider + StorageSettingsCache,
        P::Tx: DbTxMut,
    {
        #[cfg(all(unix, feature = "rocksdb"))]
        if provider.cached_storage_settings().storages_history_in_rocksdb {
            return Ok(EitherWriter::RocksDB(_rocksdb_batch));
        }

        Ok(EitherWriter::Database(provider.tx_ref().cursor_write::<tables::StoragesHistory>()?))
    }

    /// Creates a new [`EitherWriter`] for transaction hash numbers based on storage settings.
    pub fn new_transaction_hash_numbers<P>(
        provider: &P,
        _rocksdb_batch: RocksBatchArg<'a>,
    ) -> ProviderResult<EitherWriterTy<'a, P, tables::TransactionHashNumbers>>
    where
        P: DBProvider + NodePrimitivesProvider + StorageSettingsCache,
        P::Tx: DbTxMut,
    {
        #[cfg(all(unix, feature = "rocksdb"))]
        if provider.cached_storage_settings().transaction_hash_numbers_in_rocksdb {
            return Ok(EitherWriter::RocksDB(_rocksdb_batch));
        }

        Ok(EitherWriter::Database(
            provider.tx_ref().cursor_write::<tables::TransactionHashNumbers>()?,
        ))
    }

    /// Creates a new [`EitherWriter`] for account history based on storage settings.
    pub fn new_accounts_history<P>(
        provider: &P,
        _rocksdb_batch: RocksBatchArg<'a>,
    ) -> ProviderResult<EitherWriterTy<'a, P, tables::AccountsHistory>>
    where
        P: DBProvider + NodePrimitivesProvider + StorageSettingsCache,
        P::Tx: DbTxMut,
    {
        #[cfg(all(unix, feature = "rocksdb"))]
        if provider.cached_storage_settings().account_history_in_rocksdb {
            return Ok(EitherWriter::RocksDB(_rocksdb_batch));
        }

        Ok(EitherWriter::Database(provider.tx_ref().cursor_write::<tables::AccountsHistory>()?))
    }
}

impl<'a, CURSOR, N: NodePrimitives> EitherWriter<'a, CURSOR, N> {
    /// Extracts the raw `RocksDB` write batch from this writer, if it contains one.
    ///
    /// Returns `Some(WriteBatchWithTransaction)` for [`Self::RocksDB`] variant,
    /// `None` for other variants.
    ///
    /// This is used to defer `RocksDB` commits to the provider level, ensuring all
    /// storage commits (MDBX, static files, `RocksDB`) happen atomically in a single place.
    #[cfg(all(unix, feature = "rocksdb"))]
    pub fn into_raw_rocksdb_batch(self) -> Option<rocksdb::WriteBatchWithTransaction<true>> {
        match self {
            Self::Database(_) | Self::StaticFile(..) => None,
            Self::RocksDB(batch) => Some(batch.into_inner()),
        }
    }

    /// Extracts the raw `RocksDB` write batch from this writer, if it contains one.
    ///
    /// Without the `rocksdb` feature, this always returns `None`.
    #[cfg(not(all(unix, feature = "rocksdb")))]
    pub fn into_raw_rocksdb_batch(self) -> Option<RawRocksDBBatch> {
        match self {
            Self::Database(_) | Self::StaticFile(..) => None,
        }
    }

    /// Increment the block number.
    ///
    /// Relevant only for [`Self::StaticFile`]. It is a no-op for [`Self::Database`].
    pub fn increment_block(&mut self, expected_block_number: BlockNumber) -> ProviderResult<()> {
        match self {
            Self::Database(_) => Ok(()),
            Self::StaticFile(writer) => writer.increment_block(expected_block_number),
            #[cfg(all(unix, feature = "rocksdb"))]
            Self::RocksDB(_) => Err(ProviderError::UnsupportedProvider),
        }
    }

    /// Ensures that the writer is positioned at the specified block number.
    ///
    /// If the writer is positioned at a greater block number than the specified one, the writer
    /// will NOT be unwound and the error will be returned.
    ///
    /// Relevant only for [`Self::StaticFile`]. It is a no-op for [`Self::Database`].
    pub fn ensure_at_block(&mut self, block_number: BlockNumber) -> ProviderResult<()> {
        match self {
            Self::Database(_) => Ok(()),
            Self::StaticFile(writer) => writer.ensure_at_block(block_number),
            #[cfg(all(unix, feature = "rocksdb"))]
            Self::RocksDB(_) => Err(ProviderError::UnsupportedProvider),
        }
    }
}

impl<'a, CURSOR, N: NodePrimitives> EitherWriter<'a, CURSOR, N>
where
    N::Receipt: Value,
    CURSOR: DbCursorRW<tables::Receipts<N::Receipt>>,
{
    /// Append a transaction receipt.
    pub fn append_receipt(&mut self, tx_num: TxNumber, receipt: &N::Receipt) -> ProviderResult<()> {
        match self {
            Self::Database(cursor) => Ok(cursor.append(tx_num, receipt)?),
            Self::StaticFile(writer) => writer.append_receipt(tx_num, receipt),
            #[cfg(all(unix, feature = "rocksdb"))]
            Self::RocksDB(_) => Err(ProviderError::UnsupportedProvider),
        }
    }
}

impl<'a, CURSOR, N: NodePrimitives> EitherWriter<'a, CURSOR, N>
where
    CURSOR: DbCursorRW<tables::TransactionSenders>,
{
    /// Append a transaction sender to the destination
    pub fn append_sender(&mut self, tx_num: TxNumber, sender: &Address) -> ProviderResult<()> {
        match self {
            Self::Database(cursor) => Ok(cursor.append(tx_num, sender)?),
            Self::StaticFile(writer) => writer.append_transaction_sender(tx_num, sender),
            #[cfg(all(unix, feature = "rocksdb"))]
            Self::RocksDB(_) => Err(ProviderError::UnsupportedProvider),
        }
    }

    /// Append transaction senders to the destination
    pub fn append_senders<I>(&mut self, senders: I) -> ProviderResult<()>
    where
        I: Iterator<Item = (TxNumber, Address)>,
    {
        match self {
            Self::Database(cursor) => {
                for (tx_num, sender) in senders {
                    cursor.append(tx_num, &sender)?;
                }
                Ok(())
            }
            Self::StaticFile(writer) => writer.append_transaction_senders(senders),
            #[cfg(all(unix, feature = "rocksdb"))]
            Self::RocksDB(_) => Err(ProviderError::UnsupportedProvider),
        }
    }

    /// Removes all transaction senders above the given transaction number, and stops at the given
    /// block number.
    pub fn prune_senders(
        &mut self,
        unwind_tx_from: TxNumber,
        block: BlockNumber,
    ) -> ProviderResult<()>
    where
        CURSOR: DbCursorRO<tables::TransactionSenders>,
    {
        match self {
            Self::Database(cursor) => {
                let mut walker = cursor.walk_range(unwind_tx_from..)?;
                while walker.next().transpose()?.is_some() {
                    walker.delete_current()?;
                }
            }
            Self::StaticFile(writer) => {
                let static_file_transaction_sender_num = writer
                    .reader()
                    .get_highest_static_file_tx(StaticFileSegment::TransactionSenders);

                let to_delete = static_file_transaction_sender_num
                    .map(|static_num| (static_num + 1).saturating_sub(unwind_tx_from))
                    .unwrap_or_default();

                writer.prune_transaction_senders(to_delete, block)?;
            }
            #[cfg(all(unix, feature = "rocksdb"))]
            Self::RocksDB(_) => return Err(ProviderError::UnsupportedProvider),
        }

        Ok(())
    }
}

impl<'a, CURSOR, N: NodePrimitives> EitherWriter<'a, CURSOR, N>
where
    CURSOR: DbCursorRW<tables::TransactionHashNumbers> + DbCursorRO<tables::TransactionHashNumbers>,
{
    /// Puts a transaction hash number mapping.
    ///
    /// When `append_only` is true, uses `cursor.append()` which is significantly faster
    /// but requires entries to be inserted in order and the table to be empty.
    /// When false, uses `cursor.upsert()` which handles arbitrary insertion order and duplicates.
    pub fn put_transaction_hash_number(
        &mut self,
        hash: TxHash,
        tx_num: TxNumber,
        append_only: bool,
    ) -> ProviderResult<()> {
        match self {
            Self::Database(cursor) => {
                if append_only {
                    Ok(cursor.append(hash, &tx_num)?)
                } else {
                    Ok(cursor.upsert(hash, &tx_num)?)
                }
            }
            Self::StaticFile(..) => Err(ProviderError::UnsupportedProvider),
            #[cfg(all(unix, feature = "rocksdb"))]
            Self::RocksDB(batch) => batch.put::<tables::TransactionHashNumbers>(hash, &tx_num),
        }
    }

    /// Deletes a transaction hash number mapping.
    pub fn delete_transaction_hash_number(&mut self, hash: TxHash) -> ProviderResult<()> {
        match self {
            Self::Database(cursor) => {
                if cursor.seek_exact(hash)?.is_some() {
                    cursor.delete_current()?;
                }
                Ok(())
            }
            Self::StaticFile(..) => Err(ProviderError::UnsupportedProvider),
            #[cfg(all(unix, feature = "rocksdb"))]
            Self::RocksDB(batch) => batch.delete::<tables::TransactionHashNumbers>(hash),
        }
    }
}

impl<'a, CURSOR, N: NodePrimitives> EitherWriter<'a, CURSOR, N>
where
    CURSOR: DbCursorRW<tables::StoragesHistory> + DbCursorRO<tables::StoragesHistory>,
{
    /// Puts a storage history entry.
    pub fn put_storage_history(
        &mut self,
        key: StorageShardedKey,
        value: &BlockNumberList,
    ) -> ProviderResult<()> {
        match self {
            Self::Database(cursor) => Ok(cursor.upsert(key, value)?),
            Self::StaticFile(..) => Err(ProviderError::UnsupportedProvider),
            #[cfg(all(unix, feature = "rocksdb"))]
            Self::RocksDB(batch) => batch.put::<tables::StoragesHistory>(key, value),
        }
    }

    /// Deletes a storage history entry.
    pub fn delete_storage_history(&mut self, key: StorageShardedKey) -> ProviderResult<()> {
        match self {
            Self::Database(cursor) => {
                if cursor.seek_exact(key)?.is_some() {
                    cursor.delete_current()?;
                }
                Ok(())
            }
            Self::StaticFile(..) => Err(ProviderError::UnsupportedProvider),
            #[cfg(all(unix, feature = "rocksdb"))]
            Self::RocksDB(batch) => batch.delete::<tables::StoragesHistory>(key),
        }
    }
}

impl<'a, CURSOR, N: NodePrimitives> EitherWriter<'a, CURSOR, N>
where
    CURSOR: DbCursorRW<tables::AccountsHistory> + DbCursorRO<tables::AccountsHistory>,
{
    /// Puts an account history entry.
    pub fn put_account_history(
        &mut self,
        key: ShardedKey<Address>,
        value: &BlockNumberList,
    ) -> ProviderResult<()> {
        match self {
            Self::Database(cursor) => Ok(cursor.upsert(key, value)?),
            Self::StaticFile(..) => Err(ProviderError::UnsupportedProvider),
            #[cfg(all(unix, feature = "rocksdb"))]
            Self::RocksDB(batch) => batch.put::<tables::AccountsHistory>(key, value),
        }
    }

    /// Deletes an account history entry.
    pub fn delete_account_history(&mut self, key: ShardedKey<Address>) -> ProviderResult<()> {
        match self {
            Self::Database(cursor) => {
                if cursor.seek_exact(key)?.is_some() {
                    cursor.delete_current()?;
                }
                Ok(())
            }
            Self::StaticFile(..) => Err(ProviderError::UnsupportedProvider),
            #[cfg(all(unix, feature = "rocksdb"))]
            Self::RocksDB(batch) => batch.delete::<tables::AccountsHistory>(key),
        }
    }
}

impl<'a, CURSOR, N: NodePrimitives> EitherWriter<'a, CURSOR, N>
where
    CURSOR: DbDupCursorRW<tables::AccountChangeSets>,
{
    /// Append account changeset for a block.
    ///
    /// NOTE: This _sorts_ the changesets by address before appending
    pub fn append_account_changeset(
        &mut self,
        block_number: BlockNumber,
        mut changeset: Vec<AccountBeforeTx>,
    ) -> ProviderResult<()> {
        // First sort the changesets
        changeset.par_sort_by_key(|a| a.address);
        match self {
            Self::Database(cursor) => {
                for change in changeset {
                    cursor.append_dup(block_number, change)?;
                }
            }
            Self::StaticFile(writer) => {
                writer.append_account_changeset(changeset, block_number)?;
            }
            #[cfg(all(unix, feature = "rocksdb"))]
            Self::RocksDB(_) => return Err(ProviderError::UnsupportedProvider),
        }

        Ok(())
    }
}

/// Represents a source for reading data, either from database, static files, or `RocksDB`.
///
/// Note: The `StaticFile` variant holds `PhantomData<&'a ()>` to ensure the lifetime `'a`
/// is used even when the `rocksdb` feature is disabled (where `RocksDB` variant is absent).
#[derive(Debug, Display)]
pub enum EitherReader<'a, CURSOR, N> {
    /// Read from database table via cursor
    Database(CURSOR),
    /// Read from static file
    StaticFile(StaticFileProvider<N>, PhantomData<&'a ()>),
    /// Read from `RocksDB` transaction
    #[cfg(all(unix, feature = "rocksdb"))]
    RocksDB(&'a crate::providers::rocksdb::RocksTx<'a>),
}

impl<'a> EitherReader<'a, (), ()> {
    /// Creates a new [`EitherReader`] for senders based on storage settings.
    pub fn new_senders<P>(
        provider: &P,
    ) -> ProviderResult<EitherReaderTy<'a, P, tables::TransactionSenders>>
    where
        P: DBProvider + NodePrimitivesProvider + StorageSettingsCache + StaticFileProviderFactory,
        P::Tx: DbTx,
    {
        if EitherWriterDestination::senders(provider).is_static_file() {
            Ok(EitherReader::StaticFile(provider.static_file_provider(), PhantomData))
        } else {
            Ok(EitherReader::Database(
                provider.tx_ref().cursor_read::<tables::TransactionSenders>()?,
            ))
        }
    }

    /// Creates a new [`EitherReader`] for storages history based on storage settings.
    pub fn new_storages_history<P>(
        provider: &P,
        _rocksdb_tx: RocksTxRefArg<'a>,
    ) -> ProviderResult<EitherReaderTy<'a, P, tables::StoragesHistory>>
    where
        P: DBProvider + NodePrimitivesProvider + StorageSettingsCache,
        P::Tx: DbTx,
    {
        #[cfg(all(unix, feature = "rocksdb"))]
        if provider.cached_storage_settings().storages_history_in_rocksdb {
            return Ok(EitherReader::RocksDB(_rocksdb_tx));
        }

        Ok(EitherReader::Database(provider.tx_ref().cursor_read::<tables::StoragesHistory>()?))
    }

    /// Creates a new [`EitherReader`] for transaction hash numbers based on storage settings.
    pub fn new_transaction_hash_numbers<P>(
        provider: &P,
        _rocksdb_tx: RocksTxRefArg<'a>,
    ) -> ProviderResult<EitherReaderTy<'a, P, tables::TransactionHashNumbers>>
    where
        P: DBProvider + NodePrimitivesProvider + StorageSettingsCache,
        P::Tx: DbTx,
    {
        #[cfg(all(unix, feature = "rocksdb"))]
        if provider.cached_storage_settings().transaction_hash_numbers_in_rocksdb {
            return Ok(EitherReader::RocksDB(_rocksdb_tx));
        }

        Ok(EitherReader::Database(
            provider.tx_ref().cursor_read::<tables::TransactionHashNumbers>()?,
        ))
    }

    /// Creates a new [`EitherReader`] for account history based on storage settings.
    pub fn new_accounts_history<P>(
        provider: &P,
        _rocksdb_tx: RocksTxRefArg<'a>,
    ) -> ProviderResult<EitherReaderTy<'a, P, tables::AccountsHistory>>
    where
        P: DBProvider + NodePrimitivesProvider + StorageSettingsCache,
        P::Tx: DbTx,
    {
        #[cfg(all(unix, feature = "rocksdb"))]
        if provider.cached_storage_settings().account_history_in_rocksdb {
            return Ok(EitherReader::RocksDB(_rocksdb_tx));
        }

        Ok(EitherReader::Database(provider.tx_ref().cursor_read::<tables::AccountsHistory>()?))
    }

    /// Creates a new [`EitherReader`] for account changesets based on storage settings.
    pub fn new_account_changesets<P>(
        provider: &P,
    ) -> ProviderResult<DupEitherReaderTy<'a, P, tables::AccountChangeSets>>
    where
        P: DBProvider + NodePrimitivesProvider + StorageSettingsCache + StaticFileProviderFactory,
        P::Tx: DbTx,
    {
        if EitherWriterDestination::account_changesets(provider).is_static_file() {
            Ok(EitherReader::StaticFile(provider.static_file_provider(), PhantomData))
        } else {
            Ok(EitherReader::Database(
                provider.tx_ref().cursor_dup_read::<tables::AccountChangeSets>()?,
            ))
        }
    }
}

impl<CURSOR, N: NodePrimitives> EitherReader<'_, CURSOR, N>
where
    CURSOR: DbCursorRO<tables::TransactionSenders>,
{
    /// Fetches the senders for a range of transactions.
    pub fn senders_by_tx_range(
        &mut self,
        range: Range<TxNumber>,
    ) -> ProviderResult<HashMap<TxNumber, Address>> {
        match self {
            Self::Database(cursor) => cursor
                .walk_range(range)?
                .map(|result| result.map_err(ProviderError::from))
                .collect::<ProviderResult<HashMap<_, _>>>(),
            Self::StaticFile(provider, _) => range
                .clone()
                .zip(provider.fetch_range_iter(
                    StaticFileSegment::TransactionSenders,
                    range,
                    |cursor, number| cursor.get_one::<TransactionSenderMask>(number.into()),
                )?)
                .filter_map(|(tx_num, sender)| {
                    let result = sender.transpose()?;
                    Some(result.map(|sender| (tx_num, sender)))
                })
                .collect::<ProviderResult<HashMap<_, _>>>(),
            #[cfg(all(unix, feature = "rocksdb"))]
            Self::RocksDB(_) => Err(ProviderError::UnsupportedProvider),
        }
    }
}

impl<CURSOR, N: NodePrimitives> EitherReader<'_, CURSOR, N>
where
    CURSOR: DbCursorRO<tables::TransactionHashNumbers>,
{
    /// Gets a transaction number by its hash.
    pub fn get_transaction_hash_number(
        &mut self,
        hash: TxHash,
    ) -> ProviderResult<Option<TxNumber>> {
        match self {
            Self::Database(cursor) => Ok(cursor.seek_exact(hash)?.map(|(_, v)| v)),
            Self::StaticFile(..) => Err(ProviderError::UnsupportedProvider),
            #[cfg(all(unix, feature = "rocksdb"))]
            Self::RocksDB(tx) => tx.get::<tables::TransactionHashNumbers>(hash),
        }
    }
}

impl<CURSOR, N: NodePrimitives> EitherReader<'_, CURSOR, N>
where
    CURSOR: DbCursorRO<tables::StoragesHistory>,
{
    /// Gets a storage history entry.
    pub fn get_storage_history(
        &mut self,
        key: StorageShardedKey,
    ) -> ProviderResult<Option<BlockNumberList>> {
        match self {
            Self::Database(cursor) => Ok(cursor.seek_exact(key)?.map(|(_, v)| v)),
            Self::StaticFile(..) => Err(ProviderError::UnsupportedProvider),
            #[cfg(all(unix, feature = "rocksdb"))]
            Self::RocksDB(tx) => tx.get::<tables::StoragesHistory>(key),
        }
    }

    /// Lookup storage history and return [`HistoryInfo`] directly.
    ///
    /// Uses the rank/select logic to efficiently find the first block >= target
    /// where the storage slot was modified.
    pub fn storage_history_info(
        &mut self,
        address: Address,
        storage_key: B256,
        block_number: BlockNumber,
        lowest_available_block_number: Option<BlockNumber>,
    ) -> ProviderResult<HistoryInfo> {
        match self {
            Self::Database(cursor) => {
                // Lookup the history chunk in the history index. If the key does not appear in the
                // index, the first chunk for the next key will be returned so we filter out chunks
                // that have a different key.
                let key = StorageShardedKey::new(address, storage_key, block_number);
                if let Some(chunk) = cursor
                    .seek(key)?
                    .filter(|(k, _)| k.address == address && k.sharded_key.key == storage_key)
                    .map(|x| x.1)
                {
                    let (rank, found_block) = compute_history_rank(&chunk, block_number);

                    // Check if this is before the first write by looking at the previous shard.
                    let is_before_first_write =
                        needs_prev_shard_check(rank, found_block, block_number) &&
                            cursor.prev()?.is_none_or(|(k, _)| {
                                k.address != address || k.sharded_key.key != storage_key
                            });

                    Ok(HistoryInfo::from_lookup(
                        found_block,
                        is_before_first_write,
                        lowest_available_block_number,
                    ))
                } else if lowest_available_block_number.is_some() {
                    // The key may have been written, but due to pruning we may not have changesets
                    // and history, so we need to make a plain state lookup.
                    Ok(HistoryInfo::MaybeInPlainState)
                } else {
                    // The key has not been written to at all.
                    Ok(HistoryInfo::NotYetWritten)
                }
            }
            Self::StaticFile(..) => Err(ProviderError::UnsupportedProvider),
            #[cfg(all(unix, feature = "rocksdb"))]
            Self::RocksDB(tx) => tx.storage_history_info(
                address,
                storage_key,
                block_number,
                lowest_available_block_number,
            ),
        }
    }
}

impl<CURSOR, N: NodePrimitives> EitherReader<'_, CURSOR, N>
where
    CURSOR: DbCursorRO<tables::AccountsHistory>,
{
    /// Gets an account history entry.
    pub fn get_account_history(
        &mut self,
        key: ShardedKey<Address>,
    ) -> ProviderResult<Option<BlockNumberList>> {
        match self {
            Self::Database(cursor) => Ok(cursor.seek_exact(key)?.map(|(_, v)| v)),
            Self::StaticFile(..) => Err(ProviderError::UnsupportedProvider),
            #[cfg(all(unix, feature = "rocksdb"))]
            Self::RocksDB(tx) => tx.get::<tables::AccountsHistory>(key),
        }
    }

    /// Lookup account history and return [`HistoryInfo`] directly.
    ///
    /// Uses the rank/select logic to efficiently find the first block >= target
    /// where the account was modified.
    pub fn account_history_info(
        &mut self,
        address: Address,
        block_number: BlockNumber,
        lowest_available_block_number: Option<BlockNumber>,
    ) -> ProviderResult<HistoryInfo> {
        match self {
            Self::Database(cursor) => {
                // Lookup the history chunk in the history index. If the key does not appear in the
                // index, the first chunk for the next key will be returned so we filter out chunks
                // that have a different key.
                let key = ShardedKey::new(address, block_number);
                if let Some(chunk) =
                    cursor.seek(key)?.filter(|(k, _)| k.key == address).map(|x| x.1)
                {
                    let (rank, found_block) = compute_history_rank(&chunk, block_number);

                    // Check if this is before the first write by looking at the previous shard.
                    let is_before_first_write =
                        needs_prev_shard_check(rank, found_block, block_number) &&
                            cursor.prev()?.is_none_or(|(k, _)| k.key != address);

                    Ok(HistoryInfo::from_lookup(
                        found_block,
                        is_before_first_write,
                        lowest_available_block_number,
                    ))
                } else if lowest_available_block_number.is_some() {
                    // The key may have been written, but due to pruning we may not have changesets
                    // and history, so we need to make a plain state lookup.
                    Ok(HistoryInfo::MaybeInPlainState)
                } else {
                    // The key has not been written to at all.
                    Ok(HistoryInfo::NotYetWritten)
                }
            }
            Self::StaticFile(..) => Err(ProviderError::UnsupportedProvider),
            #[cfg(all(unix, feature = "rocksdb"))]
            Self::RocksDB(tx) => {
                tx.account_history_info(address, block_number, lowest_available_block_number)
            }
        }
    }
}

impl<CURSOR, N: NodePrimitives> EitherReader<'_, CURSOR, N>
where
    CURSOR: DbCursorRO<tables::AccountChangeSets>,
{
    /// Iterate over account changesets and return all account address that were changed.
    pub fn changed_accounts_with_range(
        &mut self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<BTreeSet<Address>> {
        match self {
            Self::StaticFile(provider, _) => {
                let highest_static_block =
                    provider.get_highest_static_file_block(StaticFileSegment::AccountChangeSets);

                let Some(highest) = highest_static_block else {
                    return Err(ProviderError::MissingHighestStaticFileBlock(
                        StaticFileSegment::AccountChangeSets,
                    ))
                };

                let start = *range.start();
                let static_end = (*range.end()).min(highest + 1);

                let mut changed_accounts = BTreeSet::default();
                if start <= static_end {
                    for block in start..=static_end {
                        let block_changesets = provider.account_block_changeset(block)?;
                        for changeset in block_changesets {
                            changed_accounts.insert(changeset.address);
                        }
                    }
                }

                Ok(changed_accounts)
            }
            Self::Database(provider) => provider
                .walk_range(range)?
                .map(|entry| {
                    entry.map(|(_, account_before)| account_before.address).map_err(Into::into)
                })
                .collect(),
            #[cfg(all(unix, feature = "rocksdb"))]
            Self::RocksDB(_) => Err(ProviderError::UnsupportedProvider),
        }
    }
}

/// Destination for writing data.
#[derive(Debug, EnumIs)]
pub enum EitherWriterDestination {
    /// Write to database table
    Database,
    /// Write to static file
    StaticFile,
    /// Write to `RocksDB`
    RocksDB,
}

impl EitherWriterDestination {
    /// Returns the destination for writing senders based on storage settings.
    pub fn senders<P>(provider: &P) -> Self
    where
        P: StorageSettingsCache,
    {
        // Write senders to static files only if they're explicitly enabled
        if provider.cached_storage_settings().transaction_senders_in_static_files {
            Self::StaticFile
        } else {
            Self::Database
        }
    }

    /// Returns the destination for writing account changesets based on storage settings.
    pub fn account_changesets<P>(provider: &P) -> Self
    where
        P: StorageSettingsCache,
    {
        // Write account changesets to static files only if they're explicitly enabled
        if provider.cached_storage_settings().account_changesets_in_static_files {
            Self::StaticFile
        } else {
            Self::Database
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::test_utils::create_test_provider_factory;

    use super::*;
    use alloy_primitives::Address;
    use reth_storage_api::{DatabaseProviderFactory, StorageSettings};

    #[test]
    fn test_reader_senders_by_tx_range() {
        let factory = create_test_provider_factory();

        // Insert senders only from 1 to 4, but we will query from 0 to 5.
        let senders = [
            (1, Address::random()),
            (2, Address::random()),
            (3, Address::random()),
            (4, Address::random()),
        ];

        for transaction_senders_in_static_files in [false, true] {
            factory.set_storage_settings_cache(
                StorageSettings::legacy()
                    .with_transaction_senders_in_static_files(transaction_senders_in_static_files),
            );

            let provider = factory.database_provider_rw().unwrap();
            let mut writer = EitherWriter::new_senders(&provider, 0).unwrap();
            if transaction_senders_in_static_files {
                assert!(matches!(writer, EitherWriter::StaticFile(_)));
            } else {
                assert!(matches!(writer, EitherWriter::Database(_)));
            }

            writer.increment_block(0).unwrap();
            writer.append_senders(senders.iter().copied()).unwrap();
            drop(writer);
            provider.commit().unwrap();

            let provider = factory.database_provider_ro().unwrap();
            let mut reader = EitherReader::new_senders(&provider).unwrap();
            if transaction_senders_in_static_files {
                assert!(matches!(reader, EitherReader::StaticFile(_, _)));
            } else {
                assert!(matches!(reader, EitherReader::Database(_)));
            }

            assert_eq!(
                reader.senders_by_tx_range(0..6).unwrap(),
                senders.iter().copied().collect::<HashMap<_, _>>(),
                "{reader}"
            );
        }
    }
}

#[cfg(all(test, unix, feature = "rocksdb"))]
mod rocksdb_tests {
    use super::*;
    use crate::{
        providers::rocksdb::{RocksDBBuilder, RocksDBProvider},
        test_utils::create_test_provider_factory,
        RocksDBProviderFactory,
    };
    use alloy_primitives::{Address, B256};
    use reth_db_api::{
        models::{storage_sharded_key::StorageShardedKey, IntegerList, ShardedKey},
        tables,
    };
    use reth_storage_api::{DatabaseProviderFactory, StorageSettings};
    use tempfile::TempDir;

    fn create_rocksdb_provider() -> (TempDir, RocksDBProvider) {
        let temp_dir = TempDir::new().unwrap();
        let provider = RocksDBBuilder::new(temp_dir.path())
            .with_table::<tables::TransactionHashNumbers>()
            .with_table::<tables::StoragesHistory>()
            .with_table::<tables::AccountsHistory>()
            .build()
            .unwrap();
        (temp_dir, provider)
    }

    /// Test that `EitherWriter::new_transaction_hash_numbers` creates a `RocksDB` writer
    /// when the storage setting is enabled, and that put operations followed by commit
    /// persist the data to `RocksDB`.
    #[test]
    fn test_either_writer_transaction_hash_numbers_with_rocksdb() {
        let factory = create_test_provider_factory();

        // Enable RocksDB for transaction hash numbers
        factory.set_storage_settings_cache(
            StorageSettings::legacy().with_transaction_hash_numbers_in_rocksdb(true),
        );

        let hash1 = B256::from([1u8; 32]);
        let hash2 = B256::from([2u8; 32]);
        let tx_num1 = 100u64;
        let tx_num2 = 200u64;

        // Get the RocksDB batch from the provider
        let rocksdb = factory.rocksdb_provider();
        let batch = rocksdb.batch();

        // Create EitherWriter with RocksDB
        let provider = factory.database_provider_rw().unwrap();
        let mut writer = EitherWriter::new_transaction_hash_numbers(&provider, batch).unwrap();

        // Verify we got a RocksDB writer
        assert!(matches!(writer, EitherWriter::RocksDB(_)));

        // Write transaction hash numbers (append_only=false since we're using RocksDB)
        writer.put_transaction_hash_number(hash1, tx_num1, false).unwrap();
        writer.put_transaction_hash_number(hash2, tx_num2, false).unwrap();

        // Extract the batch and register with provider for commit
        if let Some(batch) = writer.into_raw_rocksdb_batch() {
            provider.set_pending_rocksdb_batch(batch);
        }

        // Commit via provider - this commits RocksDB batch too
        provider.commit().unwrap();

        // Verify data was written to RocksDB
        let rocksdb = factory.rocksdb_provider();
        assert_eq!(rocksdb.get::<tables::TransactionHashNumbers>(hash1).unwrap(), Some(tx_num1));
        assert_eq!(rocksdb.get::<tables::TransactionHashNumbers>(hash2).unwrap(), Some(tx_num2));
    }

    /// Test that `EitherWriter::delete_transaction_hash_number` works with `RocksDB`.
    #[test]
    fn test_either_writer_delete_transaction_hash_number_with_rocksdb() {
        let factory = create_test_provider_factory();

        // Enable RocksDB for transaction hash numbers
        factory.set_storage_settings_cache(
            StorageSettings::legacy().with_transaction_hash_numbers_in_rocksdb(true),
        );

        let hash = B256::from([1u8; 32]);
        let tx_num = 100u64;

        // First, write a value directly to RocksDB
        let rocksdb = factory.rocksdb_provider();
        rocksdb.put::<tables::TransactionHashNumbers>(hash, &tx_num).unwrap();
        assert_eq!(rocksdb.get::<tables::TransactionHashNumbers>(hash).unwrap(), Some(tx_num));

        // Now delete using EitherWriter
        let batch = rocksdb.batch();
        let provider = factory.database_provider_rw().unwrap();
        let mut writer = EitherWriter::new_transaction_hash_numbers(&provider, batch).unwrap();
        writer.delete_transaction_hash_number(hash).unwrap();

        // Extract the batch and commit via provider
        if let Some(batch) = writer.into_raw_rocksdb_batch() {
            provider.set_pending_rocksdb_batch(batch);
        }
        provider.commit().unwrap();

        // Verify deletion
        let rocksdb = factory.rocksdb_provider();
        assert_eq!(rocksdb.get::<tables::TransactionHashNumbers>(hash).unwrap(), None);
    }

    #[test]
    fn test_rocksdb_batch_transaction_hash_numbers() {
        let (_temp_dir, provider) = create_rocksdb_provider();

        let hash1 = B256::from([1u8; 32]);
        let hash2 = B256::from([2u8; 32]);
        let tx_num1 = 100u64;
        let tx_num2 = 200u64;

        // Write via RocksDBBatch (same as EitherWriter::RocksDB would use internally)
        let mut batch = provider.batch();
        batch.put::<tables::TransactionHashNumbers>(hash1, &tx_num1).unwrap();
        batch.put::<tables::TransactionHashNumbers>(hash2, &tx_num2).unwrap();
        batch.commit().unwrap();

        // Read via RocksTx (same as EitherReader::RocksDB would use internally)
        let tx = provider.tx();
        assert_eq!(tx.get::<tables::TransactionHashNumbers>(hash1).unwrap(), Some(tx_num1));
        assert_eq!(tx.get::<tables::TransactionHashNumbers>(hash2).unwrap(), Some(tx_num2));

        // Test missing key
        let missing_hash = B256::from([99u8; 32]);
        assert_eq!(tx.get::<tables::TransactionHashNumbers>(missing_hash).unwrap(), None);
    }

    #[test]
    fn test_rocksdb_batch_storage_history() {
        let (_temp_dir, provider) = create_rocksdb_provider();

        let address = Address::random();
        let storage_key = B256::from([1u8; 32]);
        let key = StorageShardedKey::new(address, storage_key, 1000);
        let value = IntegerList::new([1, 5, 10, 50]).unwrap();

        // Write via RocksDBBatch
        let mut batch = provider.batch();
        batch.put::<tables::StoragesHistory>(key.clone(), &value).unwrap();
        batch.commit().unwrap();

        // Read via RocksTx
        let tx = provider.tx();
        let result = tx.get::<tables::StoragesHistory>(key).unwrap();
        assert_eq!(result, Some(value));

        // Test missing key
        let missing_key = StorageShardedKey::new(Address::random(), B256::ZERO, 0);
        assert_eq!(tx.get::<tables::StoragesHistory>(missing_key).unwrap(), None);
    }

    #[test]
    fn test_rocksdb_batch_account_history() {
        let (_temp_dir, provider) = create_rocksdb_provider();

        let address = Address::random();
        let key = ShardedKey::new(address, 1000);
        let value = IntegerList::new([1, 10, 100, 500]).unwrap();

        // Write via RocksDBBatch
        let mut batch = provider.batch();
        batch.put::<tables::AccountsHistory>(key.clone(), &value).unwrap();
        batch.commit().unwrap();

        // Read via RocksTx
        let tx = provider.tx();
        let result = tx.get::<tables::AccountsHistory>(key).unwrap();
        assert_eq!(result, Some(value));

        // Test missing key
        let missing_key = ShardedKey::new(Address::random(), 0);
        assert_eq!(tx.get::<tables::AccountsHistory>(missing_key).unwrap(), None);
    }

    #[test]
    fn test_rocksdb_batch_delete_transaction_hash_number() {
        let (_temp_dir, provider) = create_rocksdb_provider();

        let hash = B256::from([1u8; 32]);
        let tx_num = 100u64;

        // First write
        provider.put::<tables::TransactionHashNumbers>(hash, &tx_num).unwrap();
        assert_eq!(provider.get::<tables::TransactionHashNumbers>(hash).unwrap(), Some(tx_num));

        // Delete via RocksDBBatch
        let mut batch = provider.batch();
        batch.delete::<tables::TransactionHashNumbers>(hash).unwrap();
        batch.commit().unwrap();

        // Verify deletion
        assert_eq!(provider.get::<tables::TransactionHashNumbers>(hash).unwrap(), None);
    }

    #[test]
    fn test_rocksdb_batch_delete_storage_history() {
        let (_temp_dir, provider) = create_rocksdb_provider();

        let address = Address::random();
        let storage_key = B256::from([1u8; 32]);
        let key = StorageShardedKey::new(address, storage_key, 1000);
        let value = IntegerList::new([1, 5, 10]).unwrap();

        // First write
        provider.put::<tables::StoragesHistory>(key.clone(), &value).unwrap();
        assert!(provider.get::<tables::StoragesHistory>(key.clone()).unwrap().is_some());

        // Delete via RocksDBBatch
        let mut batch = provider.batch();
        batch.delete::<tables::StoragesHistory>(key.clone()).unwrap();
        batch.commit().unwrap();

        // Verify deletion
        assert_eq!(provider.get::<tables::StoragesHistory>(key).unwrap(), None);
    }

    #[test]
    fn test_rocksdb_batch_delete_account_history() {
        let (_temp_dir, provider) = create_rocksdb_provider();

        let address = Address::random();
        let key = ShardedKey::new(address, 1000);
        let value = IntegerList::new([1, 10, 100]).unwrap();

        // First write
        provider.put::<tables::AccountsHistory>(key.clone(), &value).unwrap();
        assert!(provider.get::<tables::AccountsHistory>(key.clone()).unwrap().is_some());

        // Delete via RocksDBBatch
        let mut batch = provider.batch();
        batch.delete::<tables::AccountsHistory>(key.clone()).unwrap();
        batch.commit().unwrap();

        // Verify deletion
        assert_eq!(provider.get::<tables::AccountsHistory>(key).unwrap(), None);
    }

    /// Test that `RocksDB` commits happen at `provider.commit()` level, not at writer level.
    ///
    /// This ensures all storage commits (MDBX, static files, `RocksDB`) happen atomically
    /// in a single place, making it easier to reason about commit ordering and consistency.
    #[test]
    fn test_rocksdb_commits_at_provider_level() {
        let factory = create_test_provider_factory();

        // Enable RocksDB for transaction hash numbers
        factory.set_storage_settings_cache(
            StorageSettings::legacy().with_transaction_hash_numbers_in_rocksdb(true),
        );

        let hash1 = B256::from([1u8; 32]);
        let hash2 = B256::from([2u8; 32]);
        let tx_num1 = 100u64;
        let tx_num2 = 200u64;

        // Get the RocksDB batch from the provider
        let rocksdb = factory.rocksdb_provider();
        let batch = rocksdb.batch();

        // Create provider and EitherWriter
        let provider = factory.database_provider_rw().unwrap();
        let mut writer = EitherWriter::new_transaction_hash_numbers(&provider, batch).unwrap();

        // Write transaction hash numbers (append_only=false since we're using RocksDB)
        writer.put_transaction_hash_number(hash1, tx_num1, false).unwrap();
        writer.put_transaction_hash_number(hash2, tx_num2, false).unwrap();

        // Extract the raw batch from the writer and register it with the provider
        let raw_batch = writer.into_raw_rocksdb_batch();
        if let Some(batch) = raw_batch {
            provider.set_pending_rocksdb_batch(batch);
        }

        // Data should NOT be visible yet (batch not committed)
        let rocksdb = factory.rocksdb_provider();
        assert_eq!(
            rocksdb.get::<tables::TransactionHashNumbers>(hash1).unwrap(),
            None,
            "Data should not be visible before provider.commit()"
        );

        // Commit the provider - this should commit both MDBX and RocksDB
        provider.commit().unwrap();

        // Now data should be visible in RocksDB
        let rocksdb = factory.rocksdb_provider();
        assert_eq!(
            rocksdb.get::<tables::TransactionHashNumbers>(hash1).unwrap(),
            Some(tx_num1),
            "Data should be visible after provider.commit()"
        );
        assert_eq!(
            rocksdb.get::<tables::TransactionHashNumbers>(hash2).unwrap(),
            Some(tx_num2),
            "Data should be visible after provider.commit()"
        );
    }
}
