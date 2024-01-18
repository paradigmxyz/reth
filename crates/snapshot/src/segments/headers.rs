use crate::segments::{prepare_jar, Segment, SegmentHeader};
use reth_db::{
    cursor::DbCursorRO, database::Database, snapshot::create_snapshot_T1_T2_T3, tables,
    transaction::DbTx, RawKey, RawTable,
};
use reth_interfaces::provider::ProviderResult;
use reth_primitives::{
    snapshot::{Compression, Filters, SegmentConfig},
    BlockNumber, SnapshotSegment,
};
use reth_provider::DatabaseProviderRO;
use std::{ops::RangeInclusive, path::Path};

/// Snapshot segment responsible for [SnapshotSegment::Headers] part of data.
#[derive(Debug)]
pub struct Headers {
    config: SegmentConfig,
}

impl Headers {
    /// Creates new instance of [Headers] snapshot segment.
    pub fn new(compression: Compression, filters: Filters) -> Self {
        Self { config: SegmentConfig { compression, filters } }
    }
}

impl Default for Headers {
    fn default() -> Self {
        Self { config: SnapshotSegment::Headers.config() }
    }
}

impl Segment for Headers {
    fn segment(&self) -> SnapshotSegment {
        SnapshotSegment::Headers
    }

    fn snapshot<DB: Database>(
        &self,
        provider: &DatabaseProviderRO<DB>,
        directory: impl AsRef<Path>,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<()> {
        let range_len = range.clone().count();
        let mut jar = prepare_jar::<DB, 3>(
            provider,
            directory,
            self.segment(),
            self.config,
            range.clone(),
            range_len,
            || {
                Ok([
                    self.dataset_for_compression::<DB, tables::Headers>(
                        provider, &range, range_len,
                    )?,
                    self.dataset_for_compression::<DB, tables::HeaderTD>(
                        provider, &range, range_len,
                    )?,
                    self.dataset_for_compression::<DB, tables::CanonicalHeaders>(
                        provider, &range, range_len,
                    )?,
                ])
            },
        )?;

        // Generate list of hashes for filters & PHF
        let mut cursor = provider.tx_ref().cursor_read::<RawTable<tables::CanonicalHeaders>>()?;
        let mut hashes = None;
        if self.config.filters.has_filters() {
            hashes = Some(
                cursor
                    .walk(Some(RawKey::from(*range.start())))?
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
            range,
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
