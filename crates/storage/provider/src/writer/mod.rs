use crate::{providers::StaticFileProviderRWRefMut, DatabaseProviderRW};
use reth_db::{
    cursor::DbCursorRO,
    tables,
    transaction::{DbTx, DbTxMut},
    Database,
};
use reth_errors::{ProviderError, ProviderResult};
use reth_primitives::BlockNumber;
use reth_storage_api::ReceiptWriter;
use reth_storage_errors::writer::StorageWriterError;
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
