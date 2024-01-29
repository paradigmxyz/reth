use crate::segments::{dataset_for_compression, prepare_jar, Segment};
use reth_db::{
    cursor::DbCursorRO, database::Database, snapshot::create_snapshot_T1, tables, transaction::DbTx,
};
use reth_interfaces::provider::{ProviderError, ProviderResult};
use reth_primitives::{
    snapshot::{SegmentConfig, SegmentHeader},
    BlockNumber, SnapshotSegment, TxNumber,
};
use reth_provider::{
    providers::{SnapshotProvider, SnapshotWriter},
    BlockReader, DatabaseProviderRO, TransactionsProviderExt,
};
use std::{ops::RangeInclusive, path::Path, sync::Arc};

/// Snapshot segment responsible for [SnapshotSegment::Receipts] part of data.
#[derive(Debug, Default)]
pub struct Receipts;

impl<DB: Database> Segment<DB> for Receipts {
    fn segment(&self) -> SnapshotSegment {
        SnapshotSegment::Receipts
    }

    fn snapshot(
        &self,
        provider: DatabaseProviderRO<DB>,
        snapshot_provider: Arc<SnapshotProvider>,
        block_range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<()> {
        let mut snapshot_writer =
            snapshot_provider.get_writer(*block_range.start(), SnapshotSegment::Receipts)?;

        let mut blocks = block_range.peekable();
        while let Some(block) = blocks.next() {
            let block_body_indices = provider
                .block_body_indices(block)?
                .ok_or(ProviderError::BlockBodyIndicesNotFound(block))?;

            let mut receipts_cursor = provider.tx_ref().cursor_read::<tables::Receipts>()?;
            let receipts_walker = receipts_cursor.walk_range(block_body_indices.tx_num_range())?;

            for entry in receipts_walker {
                let (tx_number, receipt) = entry?;

                snapshot_writer.append_receipt(tx_number, receipt)?;
            }

            if blocks.peek().is_some() {
                let _snapshot_block = snapshot_writer.increment_block(SnapshotSegment::Receipts)?;
                debug_assert_eq!(_snapshot_block, block);
            }
        }

        Ok(())
    }

    fn create_snapshot_file(
        &self,
        provider: &DatabaseProviderRO<DB>,
        directory: &Path,
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
