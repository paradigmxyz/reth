use crate::segments::Segment;
use alloy_primitives::BlockNumber;
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_codecs::Compact;
use reth_db_api::{cursor::DbCursorRO, table::Value, tables, transaction::DbTx};
use reth_primitives_traits::NodePrimitives;
use reth_provider::{
    providers::StaticFileWriter, ChainSpecProvider, DBProvider, StaticFileProviderFactory,
};
use reth_static_file_types::StaticFileSegment;
use reth_storage_errors::provider::ProviderResult;
use std::ops::RangeInclusive;

/// Static File segment responsible for [`StaticFileSegment::Headers`] part of data.
#[derive(Debug, Default)]
pub struct Headers;

impl<Provider> Segment<Provider> for Headers
where
    Provider: StaticFileProviderFactory<Primitives: NodePrimitives<BlockHeader: Compact + Value>>
        + DBProvider
        + ChainSpecProvider<ChainSpec: EthereumHardforks>,
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
            static_file_provider.get_writer(*block_range.start(), StaticFileSegment::Headers)?;

        let mut headers_cursor = provider
            .tx_ref()
            .cursor_read::<tables::Headers<<Provider::Primitives as NodePrimitives>::BlockHeader>>(
            )?;
        let headers_walker = headers_cursor.walk_range(block_range.clone())?;

        let mut header_td_cursor =
            provider.tx_ref().cursor_read::<tables::HeaderTerminalDifficulties>()?;

        let mut canonical_headers_cursor =
            provider.tx_ref().cursor_read::<tables::CanonicalHeaders>()?;
        let canonical_headers_walker = canonical_headers_cursor.walk_range(block_range.clone())?;

        // Get the final Paris difficulty for post-merge blocks
        let final_paris_difficulty = provider.chain_spec().final_paris_total_difficulty();

        let header_td_walker = header_td_cursor.walk_range(block_range)?;
        let mut header_td_iter = header_td_walker.peekable();

        for (header_entry, canonical_header_entry) in headers_walker.zip(canonical_headers_walker) {
            let (header_block, header) = header_entry?;
            let (canonical_header_block, canonical_header) = canonical_header_entry?;

            debug_assert_eq!(header_block, canonical_header_block);

            // For post-merge blocks, use final Paris difficulty
            // For pre-merge blocks, get the stored difficulty from the iterator
            let total_difficulty = if provider.chain_spec().is_paris_active_at_block(header_block) {
                final_paris_difficulty.unwrap_or_default()
            } else {
                // For pre-merge blocks, we expect an entry in the terminal difficulties table
                // Check if we have a matching entry in our iterator
                match header_td_iter.peek() {
                    Some(Ok((td_block, _))) if *td_block == header_block => {
                        // We have a matching entry, consume it
                        let (_, header_td) = header_td_iter.next().unwrap()?;
                        header_td.0
                    }
                    _ => {
                        // No matching entry for this pre-merge block - this shouldn't happen
                        return Err(reth_storage_errors::provider::ProviderError::HeaderNotFound(
                            header_block.into(),
                        ));
                    }
                }
            };

            static_file_writer.append_header(&header, total_difficulty, &canonical_header)?;
        }

        Ok(())
    }
}
