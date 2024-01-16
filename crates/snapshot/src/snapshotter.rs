//! Support for snapshotting.

use crate::SnapshotterError;
use reth_db::{cursor::DbCursorRO, database::Database, tables, transaction::DbTx};
use reth_interfaces::RethResult;
use reth_primitives::{snapshot::HighestSnapshots, BlockNumber, SnapshotSegment};
use reth_provider::{
    providers::{SnapshotProvider, SnapshotWriter},
    BlockReader, ProviderFactory,
};
use std::{ops::RangeInclusive, sync::Arc, time::Instant};
use tracing::{debug, trace};

/// Result of [Snapshotter::run] execution.
pub type SnapshotterResult = Result<SnapshotTargets, SnapshotterError>;

/// The snapshotter type itself with the result of [Snapshotter::run]
pub type SnapshotterWithResult<DB> = (Snapshotter<DB>, SnapshotterResult);

/// Snapshotting routine. See [Snapshotter::run] for more detailed description.
#[derive(Debug)]
pub struct Snapshotter<DB> {
    /// Provider factory
    provider_factory: ProviderFactory<DB>,
    /// Snapshot provider
    snapshot_provider: Arc<SnapshotProvider>,
}

/// Snapshot targets, per data part, measured in [`BlockNumber`].
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SnapshotTargets {
    headers: Option<RangeInclusive<BlockNumber>>,
    receipts: Option<RangeInclusive<BlockNumber>>,
    transactions: Option<RangeInclusive<BlockNumber>>,
}

impl SnapshotTargets {
    /// Returns `true` if any of the targets are [Some].
    pub fn any(&self) -> bool {
        // TODO(alexey): headers and receipts snapshotting isn't implemented yet, see
        //  `Snapshotter::run`
        // self.headers.is_some() || self.receipts.is_some() ||
        self.transactions.is_some()
    }

    // Returns `true` if all targets are either [`None`] or has beginning of the range equal to the
    // highest snapshot.
    fn is_contiguous_to_highest_snapshots(&self, snapshots: HighestSnapshots) -> bool {
        [
            (self.headers.as_ref(), snapshots.headers),
            (self.receipts.as_ref(), snapshots.receipts),
            (self.transactions.as_ref(), snapshots.transactions),
        ]
        .iter()
        .all(|(target_block_range, highest_snapshotted_block)| {
            target_block_range.map_or(true, |target_block_range| {
                highest_snapshotted_block.map_or(
                    *target_block_range.start() == 1,
                    |highest_snapshotted_block| {
                        *target_block_range.start() == highest_snapshotted_block + 1
                    },
                )
            })
        })
    }
}

impl<DB: Database> Snapshotter<DB> {
    /// Creates a new [Snapshotter].
    pub fn new(
        provider_factory: ProviderFactory<DB>,
        snapshot_provider: Arc<SnapshotProvider>,
    ) -> Self {
        Self { provider_factory, snapshot_provider }
    }

    /// Run the snapshotter.
    ///
    /// For each [Some] target in [SnapshotTargets], runs the snapshotting logic by opening a
    /// _read-only_ database transaction, walking through corresponding tables and writing to static
    /// files via [SnapshotProvider].
    ///
    /// NOTE: it doesn't delete the data from database, and the actual deleting (aka pruning) logic
    /// is in `prune` crate.
    pub fn run(&self, targets: SnapshotTargets) -> SnapshotterResult {
        debug_assert!(targets
            .is_contiguous_to_highest_snapshots(self.snapshot_provider.get_highest_snapshots()));

        debug!(target: "snapshot", ?targets, "Snapshotter started");
        let start = Instant::now();

        if let Some(block_range) = targets.transactions.clone() {
            self.snapshot_transactions(block_range)?;
        }

        // TODO(alexey): snapshot headers and receipts

        self.snapshot_provider.commit()?;
        if let Some(block_range) = targets.transactions.clone() {
            self.snapshot_provider
                .update_index(SnapshotSegment::Transactions, Some(*block_range.end()))?;
        }

        let elapsed = start.elapsed(); // TODO(alexey): track in metrics
        debug!(target: "snapshot", ?targets, ?elapsed, "Snapshotter finished");

        Ok(targets)
    }

    /// Write transactions from database table [tables::Transactions] to static files with segment
    /// [SnapshotSegment::Transactions] for the provided block range.
    fn snapshot_transactions(&self, block_range: RangeInclusive<BlockNumber>) -> RethResult<()> {
        debug!(target: "snapshot", ?block_range, "Snapshotting transactions");
        let start = Instant::now();

        let mut snapshot_writer =
            self.snapshot_provider.writer(*block_range.start(), SnapshotSegment::Transactions)?;

        for block in block_range.clone() {
            // Create a new database transaction on every block to prevent long-lived read-only
            // transactions
            let provider = self.provider_factory.provider()?;

            let Some(block_body_indices) = provider.block_body_indices(block)? else {
                // TODO(alexey): this shouldn't be possible, return a fatal error?
                continue
            };

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

        let elapsed = start.elapsed(); // TODO(alexey): track in metrics
        debug!(target: "snapshot", ?block_range, ?elapsed, "Finished snapshotting transactions");

        Ok(())
    }

    /// Returns a snapshot targets at the provided finalized block number.
    /// The target is determined by the check against highest snapshots using
    /// [SnapshotProvider::get_highest_snapshots].
    pub fn get_snapshot_targets(
        &self,
        finalized_block_number: BlockNumber,
    ) -> RethResult<SnapshotTargets> {
        let highest_snapshots = self.snapshot_provider.get_highest_snapshots();
        let targets = SnapshotTargets {
            headers: self.get_snapshot_target(highest_snapshots.headers, finalized_block_number),
            receipts: self.get_snapshot_target(highest_snapshots.receipts, finalized_block_number),
            transactions: self
                .get_snapshot_target(highest_snapshots.transactions, finalized_block_number),
        };

        trace!(
            target: "snapshot",
            %finalized_block_number,
            ?highest_snapshots,
            ?targets,
            any = %targets.any(),
            "Snapshot targets"
        );

        Ok(targets)
    }

    fn get_snapshot_target(
        &self,
        highest_snapshot: Option<BlockNumber>,
        finalized_block_number: BlockNumber,
    ) -> Option<RangeInclusive<BlockNumber>> {
        let range = highest_snapshot.unwrap_or_default() + 1..=finalized_block_number;
        (!range.is_empty()).then_some(range)
    }
}

#[cfg(test)]
mod tests {
    use crate::{snapshotter::SnapshotTargets, Snapshotter};
    use reth_interfaces::test_utils::{generators, generators::random_block_range};
    use reth_primitives::{snapshot::HighestSnapshots, B256};
    use reth_stages::test_utils::TestStageDB;

    #[test]
    fn run() {
        let mut rng = generators::rng();

        let db = TestStageDB::default();

        let blocks = random_block_range(&mut rng, 1..=3, B256::ZERO, 2..3);
        db.insert_blocks(blocks.iter(), Some(1)).expect("insert blocks");

        let snapshots_dir = tempfile::TempDir::new().unwrap();
        let provider_factory = db
            .factory
            .with_snapshots(snapshots_dir.path().to_path_buf())
            .expect("factory with snapshots");
        let snapshot_provider = provider_factory.snapshot_provider().unwrap();

        let snapshotter = Snapshotter::new(provider_factory, snapshot_provider.clone());

        let targets = snapshotter.get_snapshot_targets(1).expect("get snapshot targets");
        assert_eq!(
            targets,
            SnapshotTargets {
                headers: Some(1..=1),
                receipts: Some(1..=1),
                transactions: Some(1..=1)
            }
        );

        snapshotter.run(targets).expect("run snapshotter");
        assert_eq!(
            snapshot_provider.get_highest_snapshots(),
            HighestSnapshots { headers: None, receipts: None, transactions: Some(1) }
        );

        let targets = snapshotter.get_snapshot_targets(5).expect("get snapshot targets");
        assert_eq!(
            targets,
            SnapshotTargets {
                headers: Some(1..=5),
                receipts: Some(1..=5),
                transactions: Some(2..=5)
            }
        );

        snapshotter.run(targets).expect("run snapshotter");
        assert_eq!(
            snapshot_provider.get_highest_snapshots(),
            HighestSnapshots { headers: None, receipts: None, transactions: Some(3) }
        );
    }
}
