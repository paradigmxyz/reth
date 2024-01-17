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

/// Snapshot segment responsible for [SnapshotSegment::Transactions] part of data.
#[derive(Debug, Default)]
pub struct Transactions;

impl<DB: Database> Segment<DB> for Transactions {
    fn segment(&self) -> SnapshotSegment {
        SnapshotSegment::Transactions
    }

    /// Write transactions from database table [tables::Transactions] to static files with segment
    /// [SnapshotSegment::Transactions] for the provided block range.
    fn snapshot(
        &self,
        provider: DatabaseProviderRO<DB>,
        snapshot_provider: Arc<SnapshotProvider>,
        block_range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<()> {
        let mut snapshot_writer =
            snapshot_provider.writer(*block_range.start(), SnapshotSegment::Transactions)?;

        for block in block_range {
            let block_body_indices = provider
                .block_body_indices(block)?
                .ok_or(ProviderError::BlockBodyIndicesNotFound(block))?;

            let mut transactions_cursor =
                provider.tx_ref().cursor_read::<tables::Transactions>()?;
            let transactions_walker =
                transactions_cursor.walk_range(block_body_indices.tx_num_range())?;

            for entry in transactions_walker {
                let (tx_number, transaction) = entry?;

                snapshot_writer.append_transaction(tx_number, transaction)?;
            }
            snapshot_writer.increment_block(SnapshotSegment::Transactions)?;
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
            SnapshotSegment::Transactions,
            config,
            block_range,
            tx_range_len,
            || {
                Ok([dataset_for_compression::<DB, tables::Transactions>(
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
