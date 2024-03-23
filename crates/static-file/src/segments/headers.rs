use crate::segments::{dataset_for_compression, prepare_jar, Segment, SegmentHeader};
use reth_db::{
    cursor::DbCursorRO, database::Database, static_file::create_static_file_T1_T2_T3, tables,
    transaction::DbTx, RawKey, RawTable,
};
use reth_interfaces::provider::ProviderResult;
use reth_primitives::{static_file::SegmentConfig, BlockNumber, StaticFileSegment};
use reth_provider::{
    providers::{StaticFileProvider, StaticFileWriter},
    DatabaseProviderRO,
};
use std::{ops::RangeInclusive, path::Path};

/// Static File segment responsible for [StaticFileSegment::Headers] part of data.
#[derive(Debug, Default)]
pub struct Headers;

impl<DB: Database> Segment<DB> for Headers {
    fn segment(&self) -> StaticFileSegment {
        StaticFileSegment::Headers
    }

    fn copy_to_static_files(
        &self,
        provider: DatabaseProviderRO<DB>,
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
                static_file_writer.append_header(header, header_td.0, canonical_header)?;
            debug_assert_eq!(_static_file_block, header_block);
        }

        Ok(())
    }

    fn create_static_file_file(
        &self,
        provider: &DatabaseProviderRO<DB>,
        directory: &Path,
        config: SegmentConfig,
        block_range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<()> {
        let range_len = block_range.clone().count();
        let jar = prepare_jar::<DB, 3>(
            provider,
            directory,
            StaticFileSegment::Headers,
            config,
            block_range.clone(),
            range_len,
            || {
                Ok([
                    dataset_for_compression::<DB, tables::Headers>(
                        provider,
                        &block_range,
                        range_len,
                    )?,
                    dataset_for_compression::<DB, tables::HeaderTerminalDifficulties>(
                        provider,
                        &block_range,
                        range_len,
                    )?,
                    dataset_for_compression::<DB, tables::CanonicalHeaders>(
                        provider,
                        &block_range,
                        range_len,
                    )?,
                ])
            },
        )?;

        // Generate list of hashes for filters & PHF
        let mut cursor = provider.tx_ref().cursor_read::<RawTable<tables::CanonicalHeaders>>()?;
        let hashes = if config.filters.has_filters() {
            Some(
                cursor
                    .walk(Some(RawKey::from(*block_range.start())))?
                    .take(range_len)
                    .map(|row| row.map(|(_key, value)| value.into_value()).map_err(|e| e.into())),
            )
        } else {
            None
        };

        create_static_file_T1_T2_T3::<
            tables::Headers,
            tables::HeaderTerminalDifficulties,
            tables::CanonicalHeaders,
            BlockNumber,
            SegmentHeader,
        >(
            provider.tx_ref(),
            block_range,
            None,
            // We already prepared the dictionary beforehand
            None::<Vec<std::vec::IntoIter<Vec<u8>>>>,
            hashes,
            range_len,
            jar,
        )?;

        Ok(())
    }
}
