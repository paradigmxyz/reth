//! Support for snapshotting.

use crate::SnapshotterError;
use reth_db::{cursor::DbCursorRO, database::Database, tables, transaction::DbTx};
use reth_interfaces::RethResult;
use reth_primitives::{snapshot::HighestSnapshots, BlockNumber, SnapshotSegment};
use reth_provider::{
    providers::{SnapshotProvider, SnapshotWriter},
    BlockReader, ProviderFactory,
};
use std::{ops::RangeInclusive, sync::Arc};

/// Result of [Snapshotter::run] execution.
pub type SnapshotterResult = Result<SnapshotTargets, SnapshotterError>;

/// The snapshotter type itself with the result of [Snapshotter::run]
pub type SnapshotterWithResult<DB> = (Snapshotter<DB>, SnapshotterResult);

/// Snapshotting routine. Main snapshotting logic happens in [Snapshotter::run].
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
        self.headers.is_some() || self.receipts.is_some() || self.transactions.is_some()
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

    /// Run the snapshotter
    pub fn run(&self, targets: SnapshotTargets) -> SnapshotterResult {
        let snapshot_provider = &self.snapshot_provider;

        debug_assert!(
            targets.is_contiguous_to_highest_snapshots(snapshot_provider.get_highest_snapshots())
        );

        if let Some(block_range) = targets.transactions.clone() {
            let mut snapshot_writer =
                snapshot_provider.writer(*block_range.start(), SnapshotSegment::Transactions)?;

            for block in block_range {
                // Create a new database transaction on every block to prevent long-lived read-only
                // transactions
                let provider = self.provider_factory.provider()?;

                let Some(block_body_indices) = provider.block_body_indices(block)? else {
                    // TODO(alexey): return an error?
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
            }
        }

        // TODO(alexey): snapshot headers and receipts

        snapshot_provider.commit()?;
        snapshot_provider.update_index()?;

        Ok(targets)
    }

    /// Returns a snapshot targets at the provided finalized block number.
    /// The target is determined by the check against highest snapshots using
    /// [SnapshotProvider::get_highest_snapshots].
    pub fn get_snapshot_targets(
        &self,
        finalized_block_number: BlockNumber,
    ) -> RethResult<SnapshotTargets> {
        let highest_snapshots = self.snapshot_provider.get_highest_snapshots();

        Ok(SnapshotTargets {
            headers: self.get_snapshot_target(highest_snapshots.headers, finalized_block_number),
            receipts: self.get_snapshot_target(highest_snapshots.receipts, finalized_block_number),
            transactions: self
                .get_snapshot_target(highest_snapshots.transactions, finalized_block_number),
        })
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
