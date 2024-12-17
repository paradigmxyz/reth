use crate::segments::Segment;
use alloy_primitives::BlockNumber;
use reth_codecs::Compact;
use reth_db::{table::Value, tables};
use reth_db_api::{cursor::DbCursorRO, transaction::DbTx};
use reth_primitives_traits::NodePrimitives;
use reth_provider::{providers::StaticFileWriter, DBProvider, StaticFileProviderFactory};
use reth_static_file_types::StaticFileSegment;
use reth_storage_errors::provider::ProviderResult;
use std::ops::RangeInclusive;

/// Static File segment responsible for [`StaticFileSegment::Headers`] part of data.
#[derive(Debug, Default)]
pub struct BlockMeta;

impl<Provider> Segment<Provider> for BlockMeta
where
    Provider: StaticFileProviderFactory<Primitives: NodePrimitives<BlockHeader: Compact + Value>>
        + DBProvider,
{
    fn segment(&self) -> StaticFileSegment {
        StaticFileSegment::Headers
    }

    fn copy_to_static_files(
        &self,
        provider: Provider,
        block_range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<()> {
        let static_file_provider = provider.static_file_provider();
        let mut static_file_writer =
            static_file_provider.get_writer(*block_range.start(), StaticFileSegment::BlockMeta)?;

        let mut indices_cursor = provider.tx_ref().cursor_read::<tables::BlockBodyIndices>()?;
        let indices_walker = indices_cursor.walk_range(block_range.clone())?;

        let mut ommers_cursor = provider
            .tx_ref()
            .cursor_read::<tables::BlockOmmers<<Provider::Primitives as NodePrimitives>::BlockHeader>>(
            )?;

        let mut withdrawals_cursor = provider.tx_ref().cursor_read::<tables::BlockWithdrawals>()?;

        for entry in indices_walker {
            let (block_number, indices) = entry?;
            let ommers =
                ommers_cursor.seek_exact(block_number)?.map(|(_, l)| l).unwrap_or_default();
            let withdrawals =
                withdrawals_cursor.seek_exact(block_number)?.map(|(_, l)| l).unwrap_or_default();

            static_file_writer.append_eth_block_meta(
                &indices,
                &ommers,
                &withdrawals,
                block_number,
            )?;
        }

        Ok(())
    }
}
