use crate::segments::{prepare_jar, Segment};
use reth_db::{database::Database, snapshot::create_snapshot_T1, tables};
use reth_interfaces::RethResult;
use reth_primitives::{
    snapshot::{Compression, Filters, SegmentHeader},
    BlockNumber, SnapshotSegment, TxNumber,
};
use reth_provider::{DatabaseProviderRO, TransactionsProviderExt};
use std::ops::RangeInclusive;

/// Snapshot segment responsible for [SnapshotSegment::Receipts] part of data.
#[derive(Debug)]
pub struct Receipts {
    compression: Compression,
    filters: Filters,
}

impl Receipts {
    /// Creates new instance of [Receipts] snapshot segment.
    pub fn new(compression: Compression, filters: Filters) -> Self {
        Self { compression, filters }
    }
}

impl Segment for Receipts {
    fn snapshot<DB: Database>(
        &self,
        provider: &DatabaseProviderRO<'_, DB>,
        block_range: RangeInclusive<BlockNumber>,
    ) -> RethResult<()> {
        let tx_range = provider.transaction_range_by_block_range(block_range.clone())?;
        let tx_range_len = tx_range.clone().count();

        let mut jar = prepare_jar::<DB, 1>(
            provider,
            SnapshotSegment::Receipts,
            self.filters,
            self.compression,
            block_range,
            tx_range_len,
            || {
                Ok([self.dataset_for_compression::<DB, tables::Receipts>(
                    provider,
                    &tx_range,
                    tx_range_len,
                )?])
            },
        )?;

        // Generate list of hashes for filters & PHF
        let mut hashes = None;
        if self.filters.has_filters() {
            hashes = Some(
                provider
                    .transaction_hashes_by_range(*tx_range.start()..(*tx_range.end() + 1))?
                    .into_iter()
                    .map(|(tx, _)| Ok(tx)),
            );
        }

        create_snapshot_T1::<tables::Receipts, TxNumber, SegmentHeader>(
            provider.tx_ref(),
            tx_range,
            None,
            // We already prepared the dictionary beforehand
            None::<Vec<std::vec::IntoIter<Vec<u8>>>>,
            hashes,
            tx_range_len,
            &mut jar,
        )?;

        Ok(())
    }
}
