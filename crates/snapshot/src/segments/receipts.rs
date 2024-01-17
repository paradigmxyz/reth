use crate::segments::{dataset_for_compression, prepare_jar, Segment};
use reth_db::{database::Database, snapshot::create_snapshot_T1, tables};
use reth_interfaces::provider::ProviderResult;
use reth_primitives::{
    snapshot::{SegmentConfig, SegmentHeader},
    BlockNumber, SnapshotSegment, TxNumber,
};
use reth_provider::{DatabaseProviderRO, TransactionsProviderExt};
use std::{ops::RangeInclusive, path::PathBuf};

/// Snapshot segment responsible for [SnapshotSegment::Receipts] part of data.
#[derive(Debug, Default)]
pub struct Receipts;

impl<DB: Database> Segment<DB> for Receipts {
    fn segment(&self) -> SnapshotSegment {
        SnapshotSegment::Receipts
    }

    fn create_snapshot_file(
        &self,
        provider: &DatabaseProviderRO<DB>,
        directory: &PathBuf,
        config: SegmentConfig,
        block_range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<()> {
        let tx_range = provider.transaction_range_by_block_range(block_range.clone())?;
        let tx_range_len = tx_range.clone().count();

        let mut jar = prepare_jar::<DB, 1>(
            provider,
            directory,
            SnapshotSegment::Receipts,
            config,
            block_range,
            tx_range_len,
            || {
                Ok([dataset_for_compression::<DB, tables::Receipts>(
                    provider,
                    &tx_range,
                    tx_range_len,
                )?])
            },
        )?;

        // Generate list of hashes for filters & PHF
        let mut hashes = None;
        if config.filters.has_filters() {
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
