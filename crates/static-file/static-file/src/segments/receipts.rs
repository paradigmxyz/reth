use crate::segments::{dataset_for_compression, prepare_jar, Segment};
use alloy_primitives::{BlockNumber, TxNumber};
use reth_db::{static_file::create_static_file_T1, tables};
use reth_db_api::{cursor::DbCursorRO, database::Database, transaction::DbTx};
use reth_provider::{
    providers::{StaticFileProvider, StaticFileWriter},
    BlockReader, DatabaseProviderRO, TransactionsProviderExt,
};
use reth_static_file_types::{SegmentConfig, SegmentHeader, StaticFileSegment};
use reth_storage_errors::provider::{ProviderError, ProviderResult};
use std::{ops::RangeInclusive, path::Path};

/// Static File segment responsible for [`StaticFileSegment::Receipts`] part of data.
#[derive(Debug, Default)]
pub struct Receipts;

impl<DB: Database> Segment<DB> for Receipts {
    fn segment(&self) -> StaticFileSegment {
        StaticFileSegment::Receipts
    }

    fn copy_to_static_files(
        &self,
        provider: DatabaseProviderRO<DB>,
        static_file_provider: StaticFileProvider,
        block_range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<()> {
        let mut static_file_writer =
            static_file_provider.get_writer(*block_range.start(), StaticFileSegment::Receipts)?;

        for block in block_range {
            let _static_file_block =
                static_file_writer.increment_block(StaticFileSegment::Receipts, block)?;
            debug_assert_eq!(_static_file_block, block);

            let block_body_indices = provider
                .block_body_indices(block)?
                .ok_or(ProviderError::BlockBodyIndicesNotFound(block))?;

            let mut receipts_cursor = provider.tx_ref().cursor_read::<tables::Receipts>()?;
            let receipts_walker = receipts_cursor.walk_range(block_body_indices.tx_num_range())?;

            static_file_writer.append_receipts(
                receipts_walker.map(|result| result.map_err(ProviderError::from)),
            )?;
        }

        Ok(())
    }
}
