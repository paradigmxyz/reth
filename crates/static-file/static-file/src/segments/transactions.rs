use crate::segments::Segment;
use alloy_primitives::BlockNumber;
use reth_db::tables;
use reth_db_api::{cursor::DbCursorRO, transaction::DbTx};
use reth_provider::{
    providers::{StaticFileProvider, StaticFileWriter},
    BlockReader, DBProvider,
};
use reth_static_file_types::StaticFileSegment;
use reth_storage_errors::provider::{ProviderError, ProviderResult};
use std::ops::RangeInclusive;

/// Static File segment responsible for [`StaticFileSegment::Transactions`] part of data.
#[derive(Debug, Default)]
pub struct Transactions;

impl<Provider: DBProvider + BlockReader> Segment<Provider> for Transactions {
    fn segment(&self) -> StaticFileSegment {
        StaticFileSegment::Transactions
    }

    /// Write transactions from database table [`tables::Transactions`] to static files with segment
    /// [`StaticFileSegment::Transactions`] for the provided block range.
    fn copy_to_static_files(
        &self,
        provider: Provider,
        static_file_provider: StaticFileProvider,
        block_range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<()> {
        let mut static_file_writer = static_file_provider
            .get_writer(*block_range.start(), StaticFileSegment::Transactions)?;

        for block in block_range {
            let _static_file_block = static_file_writer.increment_block(block)?;
            debug_assert_eq!(_static_file_block, block);

            let block_body_indices = provider
                .block_body_indices(block)?
                .ok_or(ProviderError::BlockBodyIndicesNotFound(block))?;

            let mut transactions_cursor =
                provider.tx_ref().cursor_read::<tables::Transactions>()?;
            let transactions_walker =
                transactions_cursor.walk_range(block_body_indices.tx_num_range())?;

            for entry in transactions_walker {
                let (tx_number, transaction) = entry?;

                static_file_writer.append_transaction(tx_number, &transaction)?;
            }
        }

        Ok(())
    }
}
