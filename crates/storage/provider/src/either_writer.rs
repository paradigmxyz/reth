//! Generic writer abstraction for writing to either database tables or static files.

use crate::{providers::StaticFileProviderRWRefMut, StaticFileProviderFactory};
use alloy_primitives::{BlockNumber, TxNumber};
use rayon::slice::ParallelSliceMut;
use reth_db::{
    cursor::DbDupCursorRW,
    models::AccountBeforeTx,
    table::Value,
    transaction::{CursorMutTy, DbTxMut, DupCursorMutTy},
};
use reth_db_api::{cursor::DbCursorRW, tables};
use reth_node_types::NodePrimitives;
use reth_primitives_traits::ReceiptTy;
use reth_static_file_types::StaticFileSegment;
use reth_storage_api::{DBProvider, NodePrimitivesProvider, StorageSettingsCache};
use reth_storage_errors::provider::ProviderResult;
use strum::EnumIs;

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
        // Write receipts to static files only if they're explicitly enabled or we don't have
        // receipts pruning
        if provider.cached_storage_settings().receipts_in_static_files ||
            !provider.prune_modes_ref().has_receipts_pruning()
        {
            Ok(EitherWriter::StaticFile(
                provider.get_static_file_writer(block_number, StaticFileSegment::Receipts)?,
            ))
        } else {
            Ok(EitherWriter::Database(
                provider.tx_ref().cursor_write::<tables::Receipts<ReceiptTy<P::Primitives>>>()?,
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
        }

        Ok(())
    }
}

impl EitherWriter<'_, (), ()> {
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
}

#[derive(Debug, EnumIs)]
#[allow(missing_docs)]
pub enum EitherWriterDestination {
    Database,
    StaticFile,
}
