use crate::segments::{dataset_for_compression, prepare_jar, Segment, SegmentHeader};
use reth_db::{
    cursor::DbCursorRO, database::Database, snapshot::create_snapshot_T1_T2_T3, tables,
    transaction::DbTx, RawKey, RawTable,
};
use reth_interfaces::provider::ProviderResult;
use reth_primitives::{snapshot::SegmentConfig, BlockNumber, SnapshotSegment};
use reth_provider::DatabaseProviderRO;
use std::{ops::RangeInclusive, path::PathBuf};

/// Snapshot segment responsible for [SnapshotSegment::Headers] part of data.
#[derive(Debug, Default)]
pub struct Headers;

impl<DB: Database> Segment<DB> for Headers {
    fn segment(&self) -> SnapshotSegment {
        SnapshotSegment::Headers
    }

    fn create_snapshot_file(
        &self,
        provider: &DatabaseProviderRO<DB>,
        directory: &PathBuf,
        config: SegmentConfig,
        block_range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<()> {
        let range_len = block_range.clone().count();
        let mut jar = prepare_jar::<DB, 3>(
            provider,
            directory,
            SnapshotSegment::Headers,
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
                    dataset_for_compression::<DB, tables::HeaderTD>(
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
        let mut hashes = None;
        if config.filters.has_filters() {
            hashes = Some(
                cursor
                    .walk(Some(RawKey::from(*block_range.start())))?
                    .take(range_len)
                    .map(|row| row.map(|(_key, value)| value.into_value()).map_err(|e| e.into())),
            );
        }

        create_snapshot_T1_T2_T3::<
            tables::Headers,
            tables::HeaderTD,
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
            &mut jar,
        )?;

        Ok(())
    }
}
