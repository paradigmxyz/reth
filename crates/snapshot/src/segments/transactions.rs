use crate::segments::{prepare_jar, Segment};
use reth_db::{database::Database, snapshot::create_snapshot_T1, tables};
use reth_interfaces::provider::ProviderResult;
use reth_primitives::{
    snapshot::{Compression, Filters, SegmentConfig, SegmentHeader},
    BlockNumber, SnapshotSegment, TxNumber,
};
use reth_provider::{DatabaseProviderRO, TransactionsProviderExt};
use std::{ops::RangeInclusive, path::Path};

/// Snapshot segment responsible for [SnapshotSegment::Transactions] part of data.
#[derive(Debug)]
pub struct Transactions {
    config: SegmentConfig,
}

impl Transactions {
    /// Creates new instance of [Transactions] snapshot segment.
    pub fn new(compression: Compression, filters: Filters) -> Self {
        Self { config: SegmentConfig { compression, filters } }
    }
}

impl Default for Transactions {
    fn default() -> Self {
        Self { config: SnapshotSegment::Transactions.config() }
    }
}

impl Segment for Transactions {
    fn segment(&self) -> SnapshotSegment {
        SnapshotSegment::Transactions
    }

    fn snapshot<DB: Database>(
        &self,
        provider: &DatabaseProviderRO<DB>,
        directory: impl AsRef<Path>,
        block_range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<()> {
        let tx_range = provider.transaction_range_by_block_range(block_range.clone())?;
        let tx_range_len = tx_range.clone().count();

        let mut jar = prepare_jar::<DB, 1>(
            provider,
            directory,
            self.segment(),
            self.config,
            block_range,
            tx_range_len,
            || {
                Ok([self.dataset_for_compression::<DB, tables::Transactions>(
                    provider,
                    &tx_range,
                    tx_range_len,
                )?])
            },
        )?;

        // Generate list of hashes for filters & PHF
        let mut hashes = None;
        if self.config.filters.has_filters() {
            hashes = Some(
                provider
                    .transaction_hashes_by_range(*tx_range.start()..(*tx_range.end() + 1))?
                    .into_iter()
                    .map(|(tx, _)| Ok(tx)),
            );
        }

        create_snapshot_T1::<tables::Transactions, TxNumber, SegmentHeader>(
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
