use crate::segments::Segment;
use alloy_primitives::BlockNumber;
use reth_db::tables;
use reth_db_api::{cursor::DbCursorRO, transaction::DbTx};
use reth_provider::{
    providers::{StaticFileProvider, StaticFileWriter},
    DBProvider,
};
use reth_static_file_types::StaticFileSegment;
use reth_storage_errors::provider::ProviderResult;
use std::ops::RangeInclusive;

/// Static File segment responsible for [`StaticFileSegment::Headers`] part of data.
#[derive(Debug, Default)]
pub struct Headers;

impl<Provider: DBProvider> Segment<Provider> for Headers {
    fn segment(&self) -> StaticFileSegment {
        StaticFileSegment::Headers
    }

    fn copy_to_static_files(
        &self,
        provider: Provider,
        static_file_provider: StaticFileProvider,
        block_range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<()> {
        let mut static_file_writer =
            static_file_provider.get_writer(*block_range.start(), StaticFileSegment::Headers)?;

        let mut headers_cursor = provider.tx_ref().cursor_read::<tables::Headers>()?;
        let headers_walker = headers_cursor.walk_range(block_range.clone())?;

        let mut header_td_cursor =
            provider.tx_ref().cursor_read::<tables::HeaderTerminalDifficulties>()?;
        let header_td_walker = header_td_cursor.walk_range(block_range.clone())?;

        let mut canonical_headers_cursor =
            provider.tx_ref().cursor_read::<tables::CanonicalHeaders>()?;
        let canonical_headers_walker = canonical_headers_cursor.walk_range(block_range)?;

        for ((header_entry, header_td_entry), canonical_header_entry) in
            headers_walker.zip(header_td_walker).zip(canonical_headers_walker)
        {
            let (header_block, header) = header_entry?;
            let (header_td_block, header_td) = header_td_entry?;
            let (canonical_header_block, canonical_header) = canonical_header_entry?;

            debug_assert_eq!(header_block, header_td_block);
            debug_assert_eq!(header_td_block, canonical_header_block);

            let _static_file_block =
                static_file_writer.append_header(&header, header_td.0, &canonical_header)?;
            debug_assert_eq!(_static_file_block, header_block);
        }

        Ok(())
    }
}
