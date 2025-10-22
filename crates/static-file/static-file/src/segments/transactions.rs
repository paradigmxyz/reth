use crate::segments::Segment;
use alloy_primitives::BlockNumber;
use reth_codecs::Compact;
use reth_db_api::{cursor::DbCursorRO, table::Value, tables, transaction::DbTx};
use reth_primitives_traits::NodePrimitives;
use reth_provider::{
    providers::StaticFileWriter, BlockReader, DBProvider, StaticFileProviderFactory,
};
use reth_static_file_types::StaticFileSegment;
use reth_storage_errors::provider::{ProviderError, ProviderResult};
use std::ops::RangeInclusive;

/// Static File segment responsible for [`StaticFileSegment::Transactions`] part of data.
#[derive(Debug, Default)]
pub struct Transactions;

impl<Provider> Segment<Provider> for Transactions
where
    Provider: StaticFileProviderFactory<Primitives: NodePrimitives<SignedTx: Value + Compact>>
        + DBProvider
        + BlockReader,
{
    fn segment(&self) -> StaticFileSegment {
        StaticFileSegment::Transactions
    }

    /// Write transactions from database table [`tables::Transactions`] to static files with segment
    /// [`StaticFileSegment::Transactions`] for the provided block range.
    fn copy_to_static_files(
        &self,
        provider: Provider,
        block_range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<()> {
        let static_file_provider = provider.static_file_provider();
        let mut static_file_writer = static_file_provider
            .get_writer(*block_range.start(), StaticFileSegment::Transactions)?;

        let indices = provider.block_body_indices_range(block_range.clone())?;
        let first_tx = indices.iter().map(|i| i.first_tx_num).min().unwrap();
        let last_tx = indices.iter().map(|i| i.last_tx_num()).max().unwrap();

        let mut transactions_cursor = provider.tx_ref().cursor_read::<tables::Transactions<
            <Provider::Primitives as NodePrimitives>::SignedTx,
        >>()?;

        // Compute transactions range once
        let mut transactions_walker =
            transactions_cursor.walk_range(first_tx..=last_tx)?.peekable();

        for (current_block_index, block) in block_range.enumerate() {
            static_file_writer.increment_block(block)?;

            let block_body_indices = indices
                .get(current_block_index)
                .ok_or(ProviderError::BlockBodyIndicesNotFound(block))?;

            // Use peek to check the next transaction without consuming it if it's out of the
            // current block's range
            while let Some(entry_ref) = transactions_walker.peek() {
                let (tx_number, _transaction) =
                    entry_ref.as_ref().map_err(|e| ProviderError::Database(e.clone()))?;

                if *tx_number > block_body_indices.last_tx_num() {
                    break;
                }

                // Consume transaction
                let (tx_number, transaction) = transactions_walker.next().unwrap()?;

                static_file_writer.append_transaction(tx_number, &transaction)?;
            }
        }

        Ok(())
    }
}
