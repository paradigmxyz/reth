use crate::segments::Segment;
use alloy_primitives::BlockNumber;
use reth_codecs::Compact;
use reth_db::{table::Value, tables};
use reth_db_api::{cursor::DbCursorRO, transaction::DbTx};
use reth_primitives_traits::NodePrimitives;
use reth_provider::{
    providers::StaticFileWriter, BlockReader, DBProvider, StaticFileProviderFactory,
};
use reth_static_file_types::StaticFileSegment;
use reth_storage_errors::provider::{ProviderError, ProviderResult};
use std::ops::RangeInclusive;

/// Static File segment responsible for [`StaticFileSegment::Receipts`] part of data.
#[derive(Debug, Default)]
pub struct Receipts;

impl<Provider> Segment<Provider> for Receipts
where
    Provider: StaticFileProviderFactory<Primitives: NodePrimitives<Receipt: Value + Compact>>
        + DBProvider
        + BlockReader,
{
    fn segment(&self) -> StaticFileSegment {
        StaticFileSegment::Receipts
    }

    fn copy_to_static_files(
        &self,
        provider: Provider,
        block_range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<()> {
        let static_file_provider = provider.static_file_provider();
        let mut static_file_writer =
            static_file_provider.get_writer(*block_range.start(), StaticFileSegment::Receipts)?;

        for block in block_range {
            static_file_writer.increment_block(block)?;

            let block_body_indices = provider
                .block_body_indices(block)?
                .ok_or(ProviderError::BlockBodyIndicesNotFound(block))?;

            let mut receipts_cursor = provider
                .tx_ref()
                .cursor_read::<tables::Receipts<<Provider::Primitives as NodePrimitives>::Receipt>>(
                )?;
            let receipts_walker = receipts_cursor.walk_range(block_body_indices.tx_num_range())?;

            static_file_writer.append_receipts(
                receipts_walker.map(|result| result.map_err(ProviderError::from)),
            )?;
        }

        Ok(())
    }
}
