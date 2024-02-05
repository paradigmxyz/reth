//! Support for snapshotting.

use crate::{segments, segments::Segment};
use rayon::prelude::*;
use reth_db::database::Database;
use reth_interfaces::RethResult;
use reth_primitives::{snapshot::HighestSnapshots, BlockNumber, PruneModes};
use reth_provider::{
    providers::{SnapshotProvider, SnapshotWriter},
    ProviderFactory,
};
use std::{ops::RangeInclusive, sync::Arc, time::Instant};
use tracing::{debug, trace};

/// Result of [Snapshotter::run] execution.
pub type SnapshotterResult = RethResult<SnapshotTargets>;

/// The snapshotter type itself with the result of [Snapshotter::run]
pub type SnapshotterWithResult<DB> = (Snapshotter<DB>, SnapshotterResult);

/// Snapshotting routine. See [Snapshotter::run] for more detailed description.
#[derive(Debug, Clone)]
pub struct Snapshotter<DB> {
    /// Provider factory
    provider_factory: ProviderFactory<DB>,
    /// Snapshot provider
    snapshot_provider: Arc<SnapshotProvider>,
    /// Pruning configuration for every part of the data that can be pruned. Set by user, and
    /// needed in [Snapshotter] to prevent snapshotting the prunable data.
    /// See [Snapshotter::get_snapshot_targets].
    prune_modes: PruneModes,
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
                *target_block_range.start() ==
                    highest_snapshotted_block
                        .map_or(0, |highest_snapshotted_block| highest_snapshotted_block + 1)
            })
        })
    }
}

impl<DB: Database> Snapshotter<DB> {
    /// Creates a new [Snapshotter].
    pub fn new(
        provider_factory: ProviderFactory<DB>,
        snapshot_provider: Arc<SnapshotProvider>,
        prune_modes: PruneModes,
    ) -> Self {
        Self { provider_factory, snapshot_provider, prune_modes }
    }

    /// Run the snapshotter.
    ///
    /// For each [Some] target in [SnapshotTargets], initializes a corresponding [Segment] and runs
    /// it with the provided block range using [SnapshotProvider] and a read-only database
    /// transaction from [ProviderFactory]. All segments are run in parallel.
    ///
    /// NOTE: it doesn't delete the data from database, and the actual deleting (aka pruning) logic
    /// lives in the `prune` crate.
    pub fn run(&self, targets: SnapshotTargets) -> SnapshotterResult {
        debug_assert!(targets
            .is_contiguous_to_highest_snapshots(self.snapshot_provider.get_highest_snapshots()));

        debug!(target: "snapshot", ?targets, "Snapshotter started");
        let start = Instant::now();

        let mut segments = Vec::<(Box<dyn Segment<DB>>, RangeInclusive<BlockNumber>)>::new();

        if let Some(block_range) = targets.transactions.clone() {
            segments.push((Box::new(segments::Transactions), block_range));
        }
        if let Some(block_range) = targets.headers.clone() {
            segments.push((Box::new(segments::Headers), block_range));
        }
        if let Some(block_range) = targets.receipts.clone() {
            segments.push((Box::new(segments::Receipts), block_range));
        }

        segments.par_iter().try_for_each(|(segment, block_range)| -> RethResult<()> {
            debug!(target: "snapshot", segment = %segment.segment(), ?block_range, "Snapshotting segment");
            let start = Instant::now();

            // Create a new database transaction on every segment to prevent long-lived read-only
            // transactions
            let provider = self.provider_factory.provider()?.disable_long_read_transaction_safety();
            segment.snapshot(provider, self.snapshot_provider.clone(), block_range.clone())?;

            let elapsed = start.elapsed(); // TODO(alexey): track in metrics
            debug!(target: "snapshot", segment = %segment.segment(), ?block_range, ?elapsed, "Finished snapshotting segment");

            Ok(())
        })?;

        self.snapshot_provider.commit()?;
        for (segment, block_range) in segments {
            self.snapshot_provider.update_index(segment.segment(), Some(*block_range.end()))?;
        }

        let elapsed = start.elapsed(); // TODO(alexey): track in metrics
        debug!(target: "snapshot", ?targets, ?elapsed, "Snapshotter finished");

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

        let targets = SnapshotTargets {
            headers: self.get_snapshot_target(highest_snapshots.headers, finalized_block_number),
            // Snapshot receipts only if they're not pruned according to the user configuration
            receipts: if self.prune_modes.receipts.is_none() &&
                self.prune_modes.receipts_log_filter.is_empty()
            {
                self.get_snapshot_target(highest_snapshots.receipts, finalized_block_number)
            } else {
                None
            },
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
        let range = highest_snapshot.map_or(0, |block| block + 1)..=finalized_block_number;
        (!range.is_empty()).then_some(range)
    }
}

#[cfg(test)]
mod tests {
    use crate::{snapshotter::SnapshotTargets, Snapshotter};
    use assert_matches::assert_matches;
    use reth_db::{database::Database, transaction::DbTx};
    use reth_interfaces::{
        provider::ProviderError,
        test_utils::{
            generators,
            generators::{random_block_range, random_receipt},
        },
        RethError,
    };
    use reth_primitives::{snapshot::HighestSnapshots, PruneModes, SnapshotSegment, B256, U256};
    use reth_provider::providers::SnapshotWriter;
    use reth_stages::test_utils::TestStageDB;

    #[test]
    fn run() {
        let mut rng = generators::rng();

        let db = TestStageDB::default();

        let blocks = random_block_range(&mut rng, 0..=3, B256::ZERO, 2..3);
        db.insert_blocks(blocks.iter(), None).expect("insert blocks");
        // Unwind headers from snapshots and manually insert them into the database, so we're able
        // to check that snapshotter works
        db.factory
            .snapshot_provider()
            .latest_writer(SnapshotSegment::Headers)
            .expect("get snapshot writer for headers")
            .prune_headers(blocks.len() as u64)
            .expect("prune headers");
        let tx = db.factory.db_ref().tx_mut().expect("init tx");
        blocks.iter().for_each(|block| {
            TestStageDB::insert_header(None, &tx, &block.header, U256::ZERO)
                .expect("insert block header");
        });
        tx.commit().expect("commit tx");

        let mut receipts = Vec::new();
        for block in &blocks {
            for transaction in &block.body {
                receipts
                    .push((receipts.len() as u64, random_receipt(&mut rng, transaction, Some(0))));
            }
        }
        db.insert_receipts(receipts).expect("insert receipts");

        let provider_factory = db.factory;
        let snapshot_provider = provider_factory.snapshot_provider();

        let snapshotter =
            Snapshotter::new(provider_factory, snapshot_provider.clone(), PruneModes::default());

        let targets = snapshotter.get_snapshot_targets(1).expect("get snapshot targets");
        assert_eq!(
            targets,
            SnapshotTargets {
                headers: Some(0..=1),
                receipts: Some(0..=1),
                transactions: Some(0..=1)
            }
        );
        assert_matches!(snapshotter.run(targets), Ok(_));
        assert_eq!(
            snapshot_provider.get_highest_snapshots(),
            HighestSnapshots { headers: Some(1), receipts: Some(1), transactions: Some(1) }
        );

        let targets = snapshotter.get_snapshot_targets(3).expect("get snapshot targets");
        assert_eq!(
            targets,
            SnapshotTargets {
                headers: Some(2..=3),
                receipts: Some(2..=3),
                transactions: Some(2..=3)
            }
        );
        assert_matches!(snapshotter.run(targets), Ok(_));
        assert_eq!(
            snapshot_provider.get_highest_snapshots(),
            HighestSnapshots { headers: Some(3), receipts: Some(3), transactions: Some(3) }
        );

        let targets = snapshotter.get_snapshot_targets(4).expect("get snapshot targets");
        assert_eq!(
            targets,
            SnapshotTargets {
                headers: Some(4..=4),
                receipts: Some(4..=4),
                transactions: Some(4..=4)
            }
        );
        assert_matches!(
            snapshotter.run(targets),
            Err(RethError::Provider(ProviderError::BlockBodyIndicesNotFound(4)))
        );
        assert_eq!(
            snapshot_provider.get_highest_snapshots(),
            HighestSnapshots { headers: Some(3), receipts: Some(3), transactions: Some(3) }
        );
    }
}
