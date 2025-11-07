//! Generic writer abstraction for writing to either database tables or static files.

use crate::{
    providers::{StaticFileProvider, StaticFileProviderRWRefMut},
    DBProvider, StaticFileWriter,
};
use alloy_primitives::{BlockNumber, TxNumber};
use reth_db::table::Value;
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW},
    tables,
    transaction::DbTxMut,
};
use reth_node_types::NodePrimitives;
use reth_static_file_types::StaticFileSegment;
use reth_storage_api::StorageSettingsCache;
use reth_storage_errors::provider::ProviderResult;
use std::ops::RangeBounds;

/// Represents a destination for writing/pruning data, either to database or static files.
#[derive(Debug)]
pub enum EitherWriter<'a, CURSOR, N> {
    /// Write to database table via cursor
    Database(CURSOR),
    /// Write to static file
    StaticFile(StaticFileProviderRWRefMut<'a, N>),
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
}

impl<'a, N: NodePrimitives> EitherWriter<'a, (), N> {
    /// Creates a new `EitherWriter` for `TransactionBlocks` based on storage settings.
    pub fn new_transaction_blocks<P>(
        provider: &'a P,
        static_file_provider: &'a StaticFileProvider<N>,
        block_number: BlockNumber,
    ) -> ProviderResult<EitherWriter<'a, <P::Tx as DbTxMut>::CursorMut<tables::TransactionBlocks>, N>>
    where
        P: DBProvider + StorageSettingsCache,
        P::Tx: DbTxMut,
    {
        let storage_settings = provider.cached_storage_settings();
        if storage_settings.transaction_blocks_in_static_files {
            Ok(EitherWriter::StaticFile(
                static_file_provider
                    .get_writer(block_number, StaticFileSegment::TransactionBlocks)?,
            ))
        } else {
            Ok(EitherWriter::Database(
                provider.tx_ref().cursor_write::<tables::TransactionBlocks>()?,
            ))
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
        }
    }
}

impl<'a, CURSOR, N: NodePrimitives> EitherWriter<'a, CURSOR, N>
where
    CURSOR: DbCursorRW<tables::TransactionBlocks> + DbCursorRO<tables::TransactionBlocks>,
{
    /// Append a transaction block mapping to the destination
    pub fn append_transaction_block(
        &mut self,
        tx_num: TxNumber,
        block_number: &BlockNumber,
    ) -> ProviderResult<()> {
        match self {
            Self::Database(cursor) => Ok(cursor.append(tx_num, block_number)?),
            Self::StaticFile(writer) => writer.append_transaction_block(tx_num, block_number),
        }
    }

    /// Prune transaction blocks from the destination
    pub fn prune_transaction_blocks(
        self,
        range: impl RangeBounds<TxNumber>,
        last_block: BlockNumber,
    ) -> ProviderResult<()> {
        match self {
            Self::Database(mut cursor) => {
                // Remove entries from the database table
                let mut walker = cursor.walk_range(range)?;
                while walker.next().transpose()?.is_some() {
                    walker.delete_current()?;
                }
                Ok(())
            }
            Self::StaticFile(mut writer) => {
                // Calculate how many entries to delete from static files
                let highest_static_file_tx =
                    writer.user_header().tx_range().map(|range| range.end()).unwrap_or_default();

                let start = match range.start_bound() {
                    std::ops::Bound::Included(&n) => n,
                    std::ops::Bound::Excluded(&n) => n + 1,
                    std::ops::Bound::Unbounded => 0,
                };

                let to_delete = (highest_static_file_tx + 1).saturating_sub(start);
                writer.prune_transaction_blocks(to_delete, last_block)
            }
        }
    }
}
