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

/// 
#[derive(Debug)]
pub struct StorageWriter<'a, 'b, DB: Database> {
    db: Option<&'a DatabaseProviderRW<DB>>,
    sf: Option<StaticFileProviderRWRefMut<'b>>,
}

impl<'a, 'b, DB: Database> StorageWriter<'a, 'b, DB> {
    /// Creates a new instance of [`StorageWriter`].
    pub fn new(
        db: Option<&'a DatabaseProviderRW<DB>>,
        sf: Option<StaticFileProviderRWRefMut<'b>>,
    ) -> Self {
        Self { db, sf }
    }

    fn db(&self) -> &DatabaseProviderRW<DB> {
        self.db.as_ref().expect("should exist")
    }

    fn sf(&mut self) -> &mut StaticFileProviderRWRefMut<'b> {
        self.sf.as_mut().expect("should exist")
    }
}

impl<'a, 'b, DB: Database> StorageWriter<'a, 'b, DB> {
    fn ensure_db(&self) -> Result<(), StorageError> {
        if self.db.is_none() {
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

    /// Writes receipts block by block.
    pub fn write_blocks_receipts(
        mut self,
        initial_block_number: BlockNumber,
        blocks: impl Iterator<Item = Vec<Option<reth_primitives::Receipt>>>,
    ) -> ProviderResult<()> {
        // TODO: error handle
        self.ensure_db().unwrap();
        let mut bodies_cursor = self.db().tx_ref().cursor_read::<tables::BlockBodyIndices>()?;

        // If there is any kind of receipt pruning, then we store the receipts to database instead
        // of static file.
        let mut storage_type = if self.db().prune_modes_ref().has_receipts_pruning() {
            StorageType::Database(self.db().tx_ref().cursor_write::<tables::Receipts>()?)
        } else {
            self.ensure_sf().unwrap();
            StorageType::StaticFile(self.sf())
        };

        for (idx, receipts) in blocks.enumerate() {
            let block_number = initial_block_number + idx as u64;

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
                    SfWriter(*sf).append_block_receipts(first_tx_index, block_number, receipts)?;
                }
            };
        }

        Ok(())
    }
}
