//! Generic reader and writer abstractions for interacting with either database tables or static
//! files.

use std::{marker::PhantomData, ops::Range};

#[cfg(all(unix, feature = "rocksdb"))]
use crate::providers::rocksdb::RocksTx;
use crate::{
    providers::{StaticFileProvider, StaticFileProviderRWRefMut},
    StaticFileProviderFactory,
};
use alloy_primitives::{map::HashMap, Address, BlockNumber, TxNumber};
use reth_db::{
    cursor::DbCursorRO,
    static_file::TransactionSenderMask,
    table::Value,
    transaction::{CursorMutTy, CursorTy, DbTx, DbTxMut},
};
use reth_db_api::{cursor::DbCursorRW, tables};
use reth_errors::ProviderError;
use reth_node_types::NodePrimitives;
use reth_primitives_traits::ReceiptTy;
use reth_static_file_types::StaticFileSegment;
use reth_storage_api::{DBProvider, NodePrimitivesProvider, StorageSettingsCache};
use reth_storage_errors::provider::ProviderResult;
use strum::{Display, EnumIs};

/// Type alias for [`EitherReader`] constructors.
type EitherReaderTy<'a, P, T> =
    EitherReader<'a, CursorTy<<P as DBProvider>::Tx, T>, <P as NodePrimitivesProvider>::Primitives>;

/// Type alias for [`EitherWriter`] constructors.
type EitherWriterTy<'a, P, T> = EitherWriter<
    'a,
    CursorMutTy<<P as DBProvider>::Tx, T>,
    <P as NodePrimitivesProvider>::Primitives,
>;

// Helper type so constructors stay exported even when RocksDB feature is off.
#[cfg(all(unix, feature = "rocksdb"))]
type RocksTxArg<'a> = crate::providers::rocksdb::RocksTx<'a>;
#[cfg(not(all(unix, feature = "rocksdb")))]
type RocksTxArg<'a> = ();

#[cfg(all(unix, feature = "rocksdb"))]
type RocksTxRefArg<'a> = &'a crate::providers::rocksdb::RocksTx<'a>;
#[cfg(not(all(unix, feature = "rocksdb")))]
type RocksTxRefArg<'a> = ();

/// Represents a destination for writing data, either to database, static files, or `RocksDB`.
#[derive(Debug, Display)]
pub enum EitherWriter<'a, CURSOR, N> {
    /// Write to database table via cursor
    Database(CURSOR),
    /// Write to static file
    StaticFile(StaticFileProviderRWRefMut<'a, N>),
    /// Write to `RocksDB` transaction
    #[cfg(all(unix, feature = "rocksdb"))]
    RocksDB(RocksTx<'a>),
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

    /// Creates a new [`EitherWriter`] for storages history based on storage settings.
    pub fn new_storages_history<P>(
        provider: &P,
        rocksdb_tx: RocksTxArg<'a>,
    ) -> ProviderResult<EitherWriterTy<'a, P, tables::StoragesHistory>>
    where
        P: DBProvider + NodePrimitivesProvider + StorageSettingsCache,
        P::Tx: DbTxMut,
    {
        #[cfg(all(unix, feature = "rocksdb"))]
        if provider.cached_storage_settings().storages_history_in_rocksdb {
            return Ok(EitherWriter::RocksDB(rocksdb_tx));
        }
        #[cfg(not(all(unix, feature = "rocksdb")))]
        let _ = rocksdb_tx;

        Ok(EitherWriter::Database(provider.tx_ref().cursor_write::<tables::StoragesHistory>()?))
    }

    /// Creates a new [`EitherWriter`] for transaction hash numbers based on storage settings.
    pub fn new_transaction_hash_numbers<P>(
        provider: &P,
        rocksdb_tx: RocksTxArg<'a>,
    ) -> ProviderResult<EitherWriterTy<'a, P, tables::TransactionHashNumbers>>
    where
        P: DBProvider + NodePrimitivesProvider + StorageSettingsCache,
        P::Tx: DbTxMut,
    {
        #[cfg(all(unix, feature = "rocksdb"))]
        if provider.cached_storage_settings().transaction_hash_numbers_in_rocksdb {
            return Ok(EitherWriter::RocksDB(rocksdb_tx));
        }
        #[cfg(not(all(unix, feature = "rocksdb")))]
        let _ = rocksdb_tx;

        Ok(EitherWriter::Database(
            provider.tx_ref().cursor_write::<tables::TransactionHashNumbers>()?,
        ))
    }
}

impl<'a, CURSOR, N: NodePrimitives> EitherWriter<'a, CURSOR, N> {
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

    /// Commits the `RocksDB` transaction if this is a `RocksDB` writer.
    ///
    /// For [`Self::Database`] and [`Self::StaticFile`], this is a no-op as they use
    /// different commit patterns (MDBX transaction commit, static file sync).
    #[cfg(all(unix, feature = "rocksdb"))]
    pub fn commit(self) -> ProviderResult<()> {
        match self {
            Self::Database(_) | Self::StaticFile(_) => Ok(()),
            Self::RocksDB(tx) => tx.commit(),
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

/// Represents a source for reading data, either from database, static files, or `RocksDB`.
#[derive(Debug, Display)]
pub enum EitherReader<'a, CURSOR, N> {
    /// Read from database table via cursor
    Database(CURSOR, PhantomData<&'a ()>),
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
                PhantomData,
            ))
        }
    }

    /// Creates a new [`EitherReader`] for storages history based on storage settings.
    pub fn new_storages_history<P>(
        provider: &P,
        rocksdb_tx: RocksTxRefArg<'a>,
    ) -> ProviderResult<EitherReaderTy<'a, P, tables::StoragesHistory>>
    where
        P: DBProvider + NodePrimitivesProvider + StorageSettingsCache,
        P::Tx: DbTx,
    {
        #[cfg(all(unix, feature = "rocksdb"))]
        if provider.cached_storage_settings().storages_history_in_rocksdb {
            return Ok(EitherReader::RocksDB(rocksdb_tx));
        }
        #[cfg(not(all(unix, feature = "rocksdb")))]
        let _ = rocksdb_tx;

        Ok(EitherReader::Database(
            provider.tx_ref().cursor_read::<tables::StoragesHistory>()?,
            PhantomData,
        ))
    }

    /// Creates a new [`EitherReader`] for transaction hash numbers based on storage settings.
    pub fn new_transaction_hash_numbers<P>(
        provider: &P,
        rocksdb_tx: RocksTxRefArg<'a>,
    ) -> ProviderResult<EitherReaderTy<'a, P, tables::TransactionHashNumbers>>
    where
        P: DBProvider + NodePrimitivesProvider + StorageSettingsCache,
        P::Tx: DbTx,
    {
        #[cfg(all(unix, feature = "rocksdb"))]
        if provider.cached_storage_settings().transaction_hash_numbers_in_rocksdb {
            return Ok(EitherReader::RocksDB(rocksdb_tx));
        }
        #[cfg(not(all(unix, feature = "rocksdb")))]
        let _ = rocksdb_tx;

        Ok(EitherReader::Database(
            provider.tx_ref().cursor_read::<tables::TransactionHashNumbers>()?,
            PhantomData,
        ))
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
            Self::Database(cursor, _) => cursor
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
                assert!(matches!(reader, EitherReader::Database(_, _)));
            }

            assert_eq!(
                reader.senders_by_tx_range(0..6).unwrap(),
                senders.iter().copied().collect::<HashMap<_, _>>(),
                "{reader}"
            );
        }
    }
}
