//! Generic writer abstraction for writing to either database tables or static files.

use std::fmt::{Display, Formatter};

use crate::{providers::StaticFileProviderRWRefMut, StaticFileProviderFactory};
use alloy_primitives::{Address, BlockNumber, TxNumber};
use reth_db::{
    table::Value,
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

impl<'a, CURSOR, N> Display for EitherWriter<'a, CURSOR, N> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            EitherWriter::Database(_) => write!(f, "Database"),
            EitherWriter::StaticFile(_) => write!(f, "StaticFile"),
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

    /// Returns the destination for writing receipts based on storage settings.
    pub fn receipts_destination<P>(provider: &P) -> EitherWriterDestination
    where
        P: DBProvider + StorageSettingsCache,
    {
        // Write receipts to static files only if they're explicitly enabled or we don't have
        // receipts pruning
        if provider.cached_storage_settings().receipts_in_static_files ||
            !provider.prune_modes_ref().has_receipts_pruning()
        {
            EitherWriterDestination::StaticFile
        } else {
            EitherWriterDestination::Database
        }
    }

    /// Creates a new [`EitherWriter`] for senders based on storage settings and prune modes.
    pub fn new_senders<P>(
        provider: &'a P,
        block_number: BlockNumber,
    ) -> ProviderResult<EitherWriterTy<'a, P, tables::TransactionSenders>>
    where
        P: DBProvider + NodePrimitivesProvider + StorageSettingsCache + StaticFileProviderFactory,
        P::Tx: DbTxMut,
    {
        if Self::senders_destination(provider).is_static_file() {
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

    /// Returns the destination for writing senders based on storage settings.
    pub fn senders_destination<P>(provider: &P) -> EitherWriterDestination
    where
        P: StorageSettingsCache,
    {
        // Write senders to static files only if they're explicitly enabled
        if provider.cached_storage_settings().senders_in_static_files {
            EitherWriterDestination::StaticFile
        } else {
            EitherWriterDestination::Database
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

    pub fn next_block_number(&self) -> Option<BlockNumber> {
        match self {
            Self::Database(_) => None,
            Self::StaticFile(writer) => Some(writer.next_block_number()),
        }
    }

    pub fn ensure_at_before_block(&mut self, block_number: BlockNumber) -> ProviderResult<()> {
        match self {
            Self::Database(_) => Ok(()),
            Self::StaticFile(writer) => writer.ensure_at_before_block(block_number),
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
    CURSOR: DbCursorRW<tables::TransactionSenders>,
{
    /// Append a transaction sender to the destination
    pub fn append_sender(&mut self, tx_num: TxNumber, sender: &Address) -> ProviderResult<()> {
        match self {
            Self::Database(cursor) => Ok(cursor.append(tx_num, sender)?),
            Self::StaticFile(writer) => writer.append_transaction_sender(tx_num, sender),
        }
    }
}
