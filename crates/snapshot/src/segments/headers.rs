use crate::segments::{prepare_jar, Segment};
use reth_db::{
    cursor::DbCursorRO, snapshot::create_snapshot_T1_T2_T3, table::Table, tables,
    transaction::DbTx, RawKey, RawTable,
};
use reth_interfaces::RethResult;
use reth_primitives::{
    snapshot::{Compression, Filters},
    BlockNumber, SnapshotSegment,
};
use std::ops::RangeInclusive;

/// Snapshot segment responsible for [SnapshotSegment::Headers] part of data.
#[derive(Debug)]
pub struct Headers {
    compression: Compression,
    filters: Filters,
}

impl Headers {
    /// Creates new instance of [Headers] snapshot segment.
    pub fn new(compression: Compression, filters: Filters) -> Self {
        Self { compression, filters }
    }

    // Generates the dataset to train a zstd dictionary with the most recent rows (at most 1000).
    fn dataset_for_compression<'tx, T: Table<Key = BlockNumber>>(
        &self,
        tx: &impl DbTx<'tx>,
        range: &RangeInclusive<BlockNumber>,
        range_len: usize,
    ) -> RethResult<Vec<Vec<u8>>> {
        let mut cursor = tx.cursor_read::<RawTable<T>>()?;
        Ok(cursor
            .walk_back(Some(RawKey::from(*range.end())))?
            .take(range_len.min(1000))
            .map(|row| row.map(|(_key, value)| value.into_value()).expect("should exist"))
            .collect::<Vec<_>>())
    }
}

impl Segment for Headers {
    fn snapshot<'tx>(
        &self,
        tx: &impl DbTx<'tx>,
        range: RangeInclusive<BlockNumber>,
    ) -> RethResult<()> {
        let range_len = range.clone().count();
        let mut jar = prepare_jar::<3, tables::Headers>(
            tx,
            SnapshotSegment::Headers,
            self.filters,
            self.compression,
            range.clone(),
            range_len,
            || {
                Ok([
                    self.dataset_for_compression::<tables::Headers>(tx, &range, range_len)?,
                    self.dataset_for_compression::<tables::HeaderTD>(tx, &range, range_len)?,
                    self.dataset_for_compression::<tables::CanonicalHeaders>(
                        tx, &range, range_len,
                    )?,
                ])
            },
        )?;

        // Generate list of hashes for filters & PHF
        let mut cursor = tx.cursor_read::<RawTable<tables::CanonicalHeaders>>()?;
        let mut hashes = None;
        if self.filters.has_filters() {
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
        >(
            tx,
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
