use std::sync::Arc;

use crate::{providers::StaticFileProviderRWRefMut, DatabaseProviderRW, ProviderFactory};
use derive_more::Display;
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW},
    tables,
    transaction::{DbTx, DbTxMut},
    Database, DatabaseError,
};
use reth_errors::{ProviderError, ProviderResult};
use reth_primitives::{Block, BlockNumber, Receipt, StaticFileSegment};
use reth_storage_api::ReceiptWriter;
use sf::SfWriter;
use thiserror;

mod database;
mod sf;
use database::DatabaseWriter;

enum StorageType<C = (), S = ()> {
    Database(C),
    StaticFile(S),
}

#[derive(thiserror::Error, Debug, Display)]
enum StorageError {
    MissingDatabaseWriter,
    MissingStaticFileWriter,
    /// Database-related errors.
    #[error(transparent)]
    Database(#[from] DatabaseError),
    /// Provider-related errors.
    #[error(transparent)]
    ProviderError(#[from] ProviderError),
}

// struct StorageWriter<'a, 'b, DB: Database>(Arc<StorageWriterInner<'a, 'b, DB>>);

#[derive(Debug)]
pub struct StorageWriter<'a, 'b, DB, TX> {
    database_rw: Option<&'a DatabaseProviderRW<DB>>,
    tx: Option<&'a TX>,
    sf: Option<StaticFileProviderRWRefMut<'b>>,
    has_receipt_pruning: bool
}

impl<'a, 'b, DB: Database, TX: DbTxMut> StorageWriter<'a, 'b, DB, TX> {
    pub fn new(
    ) -> Self {
        Self { database_rw: None, sf: None, tx: None, has_receipt_pruning: false }
    }

    pub fn with_tx(mut self, tx: &'a TX) -> Self {
        self.tx = Some(tx);
        self
    }

    pub fn with_database_rw(mut self, database_rw: &'a DatabaseProviderRW<DB>) -> Self {
        self.database_rw = Some(database_rw);
        self
    }

    pub fn with_sf(mut self, sf: Option<StaticFileProviderRWRefMut<'b>>) -> Self {
        self.sf = sf;
        self
    }


    fn sf<'s>(&'s mut self) -> &'b mut StaticFileProviderRWRefMut<'_>
    where
        's: 'b,
    {
        self.sf.as_mut().expect("should exist")
    }
}

impl<'a, 'b, DB: Database, TX: DbTxMut> StorageWriter<'a, 'b, DB, TX> {
    fn db_tx<'s>(&'s self) -> &'a TX
    where
        's: 'a,
    {
        self.tx.clone().unwrap_or_else(|| self.database_rw.as_ref().expect("should exist").tx_ref())
    }

    fn ensure_db_tx(&self) -> Result<(), StorageError> {
        if self.database_rw.is_none() && self.tx.is_none() {
            return Err(StorageError::MissingDatabaseWriter)
        }
        Ok(())
    }

    fn ensure_db(&self) -> Result<(), StorageError> {
        if self.database_rw.is_none() {
            return Err(StorageError::MissingDatabaseWriter)
        }
        Ok(())
    }

    fn ensure_sf(&self) -> Result<(), StorageError> {
        if self.sf.is_none() {
            return Err(StorageError::MissingStaticFileWriter)
        }
        Ok(())
    }

    pub fn write_blocks_receipts<'s>(
        &'s mut self,
        first_block_number: BlockNumber,
        blocks: impl Iterator<Item = Vec<Option<reth_primitives::Receipt>>>,
    ) -> ProviderResult<()>
    where
        's: 'b,
        's: 'a
    {
        self.ensure_db_tx().unwrap();
        let mut bodies_cursor = self.db_tx().cursor_read::<tables::BlockBodyIndices>()?;

        // If there is any kind of receipt pruning, then we store the receipts to database instead
        // of static file.
        let mut storage_type = if self.has_receipt_pruning {
            StorageType::Database(self.db_tx().cursor_write::<tables::Receipts>()?)
        } else {
            self.ensure_sf().unwrap();
            StorageType::StaticFile(SfWriter(self.sf()))
        };

        for (idx, receipts) in blocks.enumerate() {
            let block_number = first_block_number + idx as u64;

            // TODO: move to block provider fn
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
                    sf.append_block_receipts(first_tx_index, block_number, receipts)?;
                }
            };
        }

        Ok(())
    }
}
