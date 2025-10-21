use crate::segments::Segment;
use alloy_primitives::BlockNumber;
use reth_codecs::Compact;
use reth_db_api::{cursor::DbCursorRO, table::Value, tables, transaction::DbTx};
use reth_primitives_traits::NodePrimitives;
use reth_provider::{providers::StaticFileWriter, DBProvider, StaticFileProviderFactory};
use reth_static_file_types::StaticFileSegment;
use reth_storage_errors::provider::ProviderResult;
use std::ops::RangeInclusive;

/// Static File segment responsible for [`StaticFileSegment::Headers`] part of data.
#[derive(Debug, Default)]
pub struct Headers;

impl<Provider> Segment<Provider> for Headers
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
            static_file_provider.get_writer(*block_range.start(), StaticFileSegment::Headers)?;

        let mut headers_cursor = provider
            .tx_ref()
            .cursor_read::<tables::Headers<<Provider::Primitives as NodePrimitives>::BlockHeader>>(
            )?;
        let headers_walker = headers_cursor.walk_range(block_range.clone())?;

        let mut canonical_headers_cursor =
            provider.tx_ref().cursor_read::<tables::CanonicalHeaders>()?;
        let canonical_headers_walker = canonical_headers_cursor.walk_range(block_range)?;

        for (header_entry, canonical_header_entry) in headers_walker.zip(canonical_headers_walker) {
            let (header_block, header) = header_entry?;
            let (canonical_header_block, canonical_header) = canonical_header_entry?;

            debug_assert_eq!(header_block, canonical_header_block);

            static_file_writer.append_header(&header, &canonical_header)?;
        }

        Ok(())
    }
}
