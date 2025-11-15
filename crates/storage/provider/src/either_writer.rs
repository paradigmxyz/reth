//! Generic writer abstraction for writing to either database tables or static files.

use crate::{providers::StaticFileProviderRWRefMut, StaticFileProviderFactory};
use alloy_primitives::{BlockNumber, TxNumber};
use reth_db::{
    cursor::DbCursorRO,
    table::{Table, Value},
    transaction::{CursorMutTy, DbTxMut},
};
use reth_db_api::{cursor::DbCursorRW, tables};
use reth_node_types::NodePrimitives;
use reth_primitives_traits::ReceiptTy;
use reth_static_file_types::StaticFileSegment;
use reth_storage_api::{DBProvider, NodePrimitivesProvider, StorageSettingsCache};
use reth_storage_errors::provider::ProviderResult;
use strum::EnumIs;

/// Type alias for [`EitherWriter`] constructors.
type EitherWriterTy<'a, P, T> = EitherWriter<
    'a,
    CursorMutTy<<P as DBProvider>::Tx, T>,
    <P as NodePrimitivesProvider>::Primitives,
>;

/// Represents a destination for writing data, either to database or static files.
#[derive(Debug)]
pub enum EitherWriter<'a, CURSOR, N> {
    /// Write to database table via cursor
    Database(CURSOR),
    /// Write to static file
    StaticFile(StaticFileProviderRWRefMut<'a, N>),
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
        match Self::receipts_destination(provider) {
            EitherWriterDestination::Database => Ok(EitherWriter::Database(
                provider.tx_ref().cursor_write::<tables::Receipts<ReceiptTy<P::Primitives>>>()?,
            )),
            EitherWriterDestination::StaticFile => Ok(EitherWriter::StaticFile(
                provider.get_static_file_writer(block_number, StaticFileSegment::Receipts)?,
            )),
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
}

impl<'a, CURSOR, N: NodePrimitives> EitherWriter<'a, CURSOR, N> {
    /// Increment the block number.
    ///
    /// Relevant only for [`Self::StaticFile`]. It is a no-op for [`Self::Database`].
    pub fn increment_block(&mut self, expected_block_number: BlockNumber) -> ProviderResult<()> {
        match self {
            Self::Database(_) => Ok(()),
            Self::StaticFile(writer) => writer.increment_block(expected_block_number),
        }
    }

    /// Prune transaction number based table or static file above the specified transaction number.
    fn prune_tx_based_above<T>(&mut self, from_tx: TxNumber) -> ProviderResult<()>
    where
        T: Table<Key = u64, Value: Value>,
        CURSOR: DbCursorRO<T> + DbCursorRW<T>,
    {
        match self {
            Self::Database(cursor) => {
                let mut walker = cursor.walk_range(from_tx..)?;
                while walker.next().transpose()?.is_some() {
                    walker.delete_current()?;
                }
            }
            Self::StaticFile(writer) => {
                let highest_block = writer.get_highest_static_file_block().unwrap_or_default();
                let highest_tx = writer.get_highest_static_file_tx();

                let to_delete = highest_tx
                    .map(|tx_number| (tx_number + 1).saturating_sub(from_tx))
                    .unwrap_or_default();

                writer.queue_prune(to_delete, Some(highest_block))?;
            }
        }

        Ok(())
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
        }
    }

    /// Removes receipts for transaction numbers above the specified.
    pub fn prune_receipts_above(&mut self, from_tx: TxNumber) -> ProviderResult<()>
    where
        CURSOR: DbCursorRO<tables::Receipts<N::Receipt>>,
    {
        self.prune_tx_based_above(from_tx)
    }
}

#[derive(Debug, EnumIs)]
#[allow(missing_docs)]
pub enum EitherWriterDestination {
    Database,
    StaticFile,
}
