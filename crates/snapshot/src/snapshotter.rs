//! Support for snapshotting.

use crate::{segments, segments::Segment, SnapshotterError};
use reth_db::{database::Database, snapshot::iter_snapshots};
use reth_interfaces::{RethError, RethResult};
use reth_primitives::{snapshot::HighestSnapshots, BlockNumber, TxNumber};
use reth_provider::{BlockReader, DatabaseProviderRO, ProviderFactory, TransactionsProviderExt};
use std::{
    collections::HashMap,
    ops::RangeInclusive,
    path::{Path, PathBuf},
};
use tokio::sync::watch;
use tracing::warn;

/// Result of [Snapshotter::run] execution.
pub type SnapshotterResult = Result<SnapshotTargets, SnapshotterError>;

/// The snapshotter type itself with the result of [Snapshotter::run]
pub type SnapshotterWithResult<DB> = (Snapshotter<DB>, SnapshotterResult);

/// Snapshots are initially created in `{...}/datadir/snapshots/temp` and moved once finished. This
/// directory is cleaned up on every booting up of the node.
const TEMPORARY_SUBDIRECTORY: &str = "temp";

/// Snapshotting routine. Main snapshotting logic happens in [Snapshotter::run].
#[derive(Debug)]
pub struct Snapshotter<DB> {
    /// Provider factory
    provider_factory: ProviderFactory<DB>,
    /// Directory where snapshots are located
    snapshots_path: PathBuf,
    /// Highest snapshotted block numbers for each segment
    highest_snapshots: HighestSnapshots,
    /// Channel sender to notify other components of the new highest snapshots
    highest_snapshots_notifier: watch::Sender<Option<HighestSnapshots>>,
    /// Channel receiver to be cloned and shared that already comes with the newest value
    highest_snapshots_tracker: HighestSnapshotsTracker,
    /// Block interval after which the snapshot is taken.
    block_interval: u64,
}

/// Tracker for the latest [`HighestSnapshots`] value.
pub type HighestSnapshotsTracker = watch::Receiver<Option<HighestSnapshots>>;

/// Snapshot targets, per data part, measured in [`BlockNumber`] and [`TxNumber`], if applicable.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SnapshotTargets {
    headers: Option<RangeInclusive<BlockNumber>>,
    receipts: Option<(RangeInclusive<BlockNumber>, RangeInclusive<TxNumber>)>,
    transactions: Option<(RangeInclusive<BlockNumber>, RangeInclusive<TxNumber>)>,
}

impl SnapshotTargets {
    /// Returns `true` if any of the targets are [Some].
    pub fn any(&self) -> bool {
        self.headers.is_some() || self.receipts.is_some() || self.transactions.is_some()
    }

    /// Returns `true` if all targets are either [None] or multiple of `block_interval`.
    fn is_multiple_of_block_interval(&self, block_interval: u64) -> bool {
        [
            self.headers.as_ref(),
            self.receipts.as_ref().map(|(blocks, _)| blocks),
            self.transactions.as_ref().map(|(blocks, _)| blocks),
        ]
        .iter()
        .all(|blocks| blocks.map_or(true, |blocks| (blocks.end() + 1) % block_interval == 0))
    }

    // Returns `true` if all targets are either [`None`] or has beginning of the range equal to the
    // highest snapshot.
    fn is_contiguous_to_highest_snapshots(&self, snapshots: HighestSnapshots) -> bool {
        [
            (self.headers.as_ref(), snapshots.headers),
            (self.receipts.as_ref().map(|(blocks, _)| blocks), snapshots.receipts),
            (self.transactions.as_ref().map(|(blocks, _)| blocks), snapshots.transactions),
        ]
        .iter()
        .all(|(target, highest)| {
            target.map_or(true, |block_number| {
                highest.map_or(*block_number.start() == 0, |previous_block_number| {
                    *block_number.start() == previous_block_number + 1
                })
            })
        })
    }
}

impl<DB: Database> Snapshotter<DB> {
    /// Creates a new [Snapshotter].
    pub fn new(
        provider_factory: ProviderFactory<DB>,
        snapshots_path: impl AsRef<Path>,
        block_interval: u64,
    ) -> RethResult<Self> {
        let (highest_snapshots_notifier, highest_snapshots_tracker) = watch::channel(None);

        let mut snapshotter = Self {
            provider_factory,
            snapshots_path: snapshots_path.as_ref().into(),
            highest_snapshots: HighestSnapshots::default(),
            highest_snapshots_notifier,
            highest_snapshots_tracker,
            block_interval,
        };

        snapshotter.create_directory()?;
        snapshotter.update_highest_snapshots_tracker()?;

        Ok(snapshotter)
    }

    /// Ensures the snapshots directory and its temporary subdirectory are properly set up.
    ///
    /// This function performs the following actions:
    /// 1. If `datadir/snapshots` does not exist, it creates it.
    /// 2. Ensures `datadir/snapshots/temp` exists and is empty.
    ///
    /// The `temp` subdirectory is where snapshots are initially created before being
    /// moved to their final location within `datadir/snapshots`.
    fn create_directory(&self) -> RethResult<()> {
        let temporary_path = self.snapshots_path.join(TEMPORARY_SUBDIRECTORY);

        if !self.snapshots_path.exists() {
            reth_primitives::fs::create_dir_all(&self.snapshots_path)?;
        } else if temporary_path.exists() {
            reth_primitives::fs::remove_dir_all(&temporary_path)?;
        }

        reth_primitives::fs::create_dir_all(temporary_path)?;

        Ok(())
    }

    #[cfg(test)]
    fn set_highest_snapshots_from_targets(&mut self, targets: &SnapshotTargets) {
        if let Some(block_number) = &targets.headers {
            self.highest_snapshots.headers = Some(*block_number.end());
        }
        if let Some((block_number, _)) = &targets.receipts {
            self.highest_snapshots.receipts = Some(*block_number.end());
        }
        if let Some((block_number, _)) = &targets.transactions {
            self.highest_snapshots.transactions = Some(*block_number.end());
        }
    }

    /// Looks into the snapshot directory to find the highest snapshotted block of each segment, and
    /// notifies every tracker.
    fn update_highest_snapshots_tracker(&mut self) -> RethResult<()> {
        // It walks over the directory and parses the snapshot filenames extracting
        // `SnapshotSegment` and their inclusive range. It then takes the maximum block
        // number for each specific segment.
        for (segment, ranges) in
            iter_snapshots(&self.snapshots_path).map_err(|err| RethError::Provider(err.into()))?
        {
            for (block_range, _) in ranges {
                let max_segment_block = self.highest_snapshots.as_mut(segment);
                if max_segment_block.map_or(true, |block| block < *block_range.end()) {
                    *max_segment_block = Some(*block_range.end());
                }
            }
        }

        let _ = self.highest_snapshots_notifier.send(Some(self.highest_snapshots)).map_err(|_| {
            warn!(target: "snapshot", "Highest snapshots channel closed");
        });

        Ok(())
    }

    /// Returns a new [`HighestSnapshotsTracker`].
    pub fn highest_snapshot_receiver(&self) -> HighestSnapshotsTracker {
        self.highest_snapshots_tracker.clone()
    }

    /// Run the snapshotter
    pub fn run(&mut self, targets: SnapshotTargets) -> SnapshotterResult {
        debug_assert!(targets.is_multiple_of_block_interval(self.block_interval));
        debug_assert!(targets.is_contiguous_to_highest_snapshots(self.highest_snapshots));

        self.run_segment::<segments::Receipts>(targets.receipts.clone().map(|(range, _)| range))?;

        self.run_segment::<segments::Transactions>(
            targets.transactions.clone().map(|(range, _)| range),
        )?;

        self.run_segment::<segments::Headers>(targets.headers.clone())?;

        self.update_highest_snapshots_tracker()?;

        Ok(targets)
    }

    /// Run the snapshotter for one segment.
    ///
    /// It first builds the snapshot in a **temporary directory** inside the snapshots directory. If
    /// for some reason the node is terminated during the snapshot process, it will be cleaned
    /// up on boot (on [`Snapshotter::new`]) and the snapshot process restarted from scratch for
    /// this block range and segment.
    ///
    /// If it succeeds, then we move the snapshot file from the temporary directory to its main one.
    fn run_segment<S: Segment>(
        &self,
        block_range: Option<RangeInclusive<BlockNumber>>,
    ) -> RethResult<()> {
        if let Some(block_range) = block_range {
            let temp = self.snapshots_path.join(TEMPORARY_SUBDIRECTORY);
            let provider = self.provider_factory.provider()?;
            let tx_range = provider.transaction_range_by_block_range(block_range.clone())?;
            let segment = S::default();
            let filename = segment.segment().filename(&block_range, &tx_range);

            segment.snapshot::<DB>(&provider, temp.clone(), block_range)?;

            reth_primitives::fs::rename(temp.join(&filename), self.snapshots_path.join(filename))?;
        }
        Ok(())
    }

    /// Returns a snapshot targets at the provided finalized block number, respecting the block
    /// interval. The target is determined by the check against last snapshots.
    pub fn get_snapshot_targets(
        &self,
        finalized_block_number: BlockNumber,
    ) -> RethResult<SnapshotTargets> {
        let provider = self.provider_factory.provider()?;

        // Round down `finalized_block_number` to a multiple of `block_interval`
        let to_block_number = finalized_block_number.saturating_sub(
            // Adjust for 0-indexed block numbers
            (finalized_block_number + 1) % self.block_interval,
        );

        // Calculate block ranges to snapshot
        let headers_block_range =
            self.get_snapshot_target_block_range(to_block_number, self.highest_snapshots.headers);
        let receipts_block_range =
            self.get_snapshot_target_block_range(to_block_number, self.highest_snapshots.receipts);
        let transactions_block_range = self
            .get_snapshot_target_block_range(to_block_number, self.highest_snapshots.transactions);

        // Calculate transaction ranges to snapshot
        let mut block_to_tx_number_cache = HashMap::default();
        let receipts_tx_range = self.get_snapshot_target_tx_range(
            &provider,
            &mut block_to_tx_number_cache,
            self.highest_snapshots.receipts,
            &receipts_block_range,
        )?;
        let transactions_tx_range = self.get_snapshot_target_tx_range(
            &provider,
            &mut block_to_tx_number_cache,
            self.highest_snapshots.transactions,
            &transactions_block_range,
        )?;

        Ok(SnapshotTargets {
            headers: headers_block_range
                .size_hint()
                .1
                .expect("finalized block should be >= last headers snapshot")
                .ge(&(self.block_interval as usize))
                .then_some(headers_block_range),
            receipts: receipts_block_range
                .size_hint()
                .1
                .expect("finalized block should be >= last receipts snapshot")
                .ge(&(self.block_interval as usize))
                .then_some((receipts_block_range, receipts_tx_range)),
            transactions: transactions_block_range
                .size_hint()
                .1
                .expect("finalized block should be >= last transactions snapshot")
                .ge(&(self.block_interval as usize))
                .then_some((transactions_block_range, transactions_tx_range)),
        })
    }

    fn get_snapshot_target_block_range(
        &self,
        to_block_number: BlockNumber,
        highest_snapshot: Option<BlockNumber>,
    ) -> RangeInclusive<BlockNumber> {
        let highest_snapshot = highest_snapshot.map_or(0, |block_number| block_number + 1);
        highest_snapshot..=(highest_snapshot + self.block_interval - 1).min(to_block_number)
    }

    fn get_snapshot_target_tx_range(
        &self,
        provider: &DatabaseProviderRO<DB>,
        block_to_tx_number_cache: &mut HashMap<BlockNumber, TxNumber>,
        highest_snapshot: Option<BlockNumber>,
        block_range: &RangeInclusive<BlockNumber>,
    ) -> RethResult<RangeInclusive<TxNumber>> {
        let from_tx_number = if let Some(block_number) = highest_snapshot {
            *block_to_tx_number_cache.entry(block_number).or_insert(
                provider
                    .block_body_indices(block_number)?
                    .ok_or(RethError::Custom(
                        "Block body indices for highest snapshot not found".to_string(),
                    ))?
                    .next_tx_num(),
            )
        } else {
            0
        };

        let to_tx_number = *block_to_tx_number_cache.entry(*block_range.end()).or_insert(
            provider
                .block_body_indices(*block_range.end())?
                .ok_or(RethError::Custom(
                    "Block body indices for block range end not found".to_string(),
                ))?
                .last_tx_num(),
        );
        Ok(from_tx_number..=to_tx_number)
    }
}

#[cfg(test)]
mod tests {
    use crate::{snapshotter::SnapshotTargets, Snapshotter};
    use assert_matches::assert_matches;
    use reth_interfaces::{
        test_utils::{generators, generators::random_block_range},
        RethError,
    };
    use reth_primitives::{snapshot::HighestSnapshots, B256};
    use reth_stages::test_utils::TestStageDB;

    #[test]
    fn new() {
        let db = TestStageDB::default();
        let snapshots_dir = tempfile::TempDir::new().unwrap();
        let snapshotter = Snapshotter::new(db.factory, snapshots_dir.into_path(), 2).unwrap();

        assert_eq!(
            *snapshotter.highest_snapshot_receiver().borrow(),
            Some(HighestSnapshots::default())
        );
    }

    #[test]
    fn get_snapshot_targets() {
        let db = TestStageDB::default();
        let snapshots_dir = tempfile::TempDir::new().unwrap();
        let mut rng = generators::rng();

        let blocks = random_block_range(&mut rng, 0..=3, B256::ZERO, 2..3);
        db.insert_blocks(blocks.iter(), None).expect("insert blocks");

        let mut snapshotter = Snapshotter::new(db.factory, snapshots_dir.into_path(), 2).unwrap();

        // Snapshot targets has data per part up to the passed finalized block number,
        // respecting the block interval
        let targets = snapshotter.get_snapshot_targets(1).expect("get snapshot targets");
        assert_eq!(
            targets,
            SnapshotTargets {
                headers: Some(0..=1),
                receipts: Some((0..=1, 0..=3)),
                transactions: Some((0..=1, 0..=3))
            }
        );
        assert!(targets.is_multiple_of_block_interval(snapshotter.block_interval));
        assert!(targets.is_contiguous_to_highest_snapshots(snapshotter.highest_snapshots));
        // Imitate snapshotter run according to the targets which updates the last snapshots state
        snapshotter.set_highest_snapshots_from_targets(&targets);

        // Nothing to snapshot, last snapshots state of snapshotter doesn't pass the thresholds
        assert_eq!(
            snapshotter.get_snapshot_targets(2),
            Ok(SnapshotTargets { headers: None, receipts: None, transactions: None })
        );

        // Snapshot targets has data per part up to the passed finalized block number,
        // respecting the block interval
        let targets = snapshotter.get_snapshot_targets(5).expect("get snapshot targets");
        assert_eq!(
            targets,
            SnapshotTargets {
                headers: Some(2..=3),
                receipts: Some((2..=3, 4..=7)),
                transactions: Some((2..=3, 4..=7))
            }
        );
        assert!(targets.is_multiple_of_block_interval(snapshotter.block_interval));
        assert!(targets.is_contiguous_to_highest_snapshots(snapshotter.highest_snapshots));
        // Imitate snapshotter run according to the targets which updates the last snapshots state
        snapshotter.set_highest_snapshots_from_targets(&targets);

        // Block body indices not found
        assert_matches!(snapshotter.get_snapshot_targets(5), Err(RethError::Custom(_)));
    }
}
