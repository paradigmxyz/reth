//! Support for snapshotting.

use crate::SnapshotterError;
use reth_db::database::Database;
use reth_interfaces::{RethError, RethResult};
use reth_primitives::{BlockNumber, ChainSpec, TxNumber};
use reth_provider::{BlockReader, ProviderFactory};
use std::{ops::RangeInclusive, sync::Arc};
use tokio::sync::watch;
use tracing::warn;

/// Result of [Snapshotter::run] execution.
pub type SnapshotterResult = Result<SnapshotTargets, SnapshotterError>;

/// The snapshotter type itself with the result of [Snapshotter::run]
pub type SnapshotterWithResult<DB> = (Snapshotter<DB>, SnapshotterResult);

/// Snapshotting routine. Main snapshotting logic happens in [Snapshotter::run].
pub struct Snapshotter<DB> {
    provider_factory: ProviderFactory<DB>,
    highest_snapshots: HighestSnapshots,
    highest_snapshots_tracker: watch::Sender<Option<HighestSnapshots>>,
    /// Block interval after which the snapshot is taken.
    block_interval: u64,
}

pub type HighestSnapshotsTracker = watch::Receiver<Option<HighestSnapshots>>;

/// Highest snapshotted block numbers, per data part.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct HighestSnapshots {
    /// Highest snapshotted block of headers, inclusive.
    /// If [`None`], no snapshot is available.
    pub headers: Option<BlockNumber>,
    /// Highest snapshotted block of receipts, inclusive.
    /// If [`None`], no snapshot is available.
    pub receipts: Option<BlockNumber>,
    /// Highest snapshotted block of transactions, inclusive.
    /// If [`None`], no snapshot is available.
    pub transactions: Option<BlockNumber>,
}

impl HighestSnapshots {
    /// Returns maximum snapshotted block number over all data parts.
    /// If [`None`], no snapshots are available.
    pub fn max_block_number(&self) -> Option<BlockNumber> {
        self.headers.max(self.receipts).max(self.transactions)
    }
}

/// Snapshot targets, per data part, measured in [`BlockNumber`] and [`TxNumber`], if applicable.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SnapshotTargets {
    headers: Option<RangeInclusive<BlockNumber>>,
    receipts: Option<(RangeInclusive<BlockNumber>, RangeInclusive<TxNumber>)>,
    transactions: Option<(RangeInclusive<BlockNumber>, RangeInclusive<TxNumber>)>,
}

impl SnapshotTargets {
    /// Returns `true` if any of the data parts has targets, i.e. is [`Some`].
    pub fn any(&self) -> bool {
        self.headers.is_some() || self.receipts.is_some() || self.transactions.is_some()
    }

    /// Returns `true` if all targets are either [`None`] or multiple of `block_interval`.
    #[cfg(debug_assertions)]
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
    #[cfg(debug_assertions)]
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
        db: DB,
        chain_spec: Arc<ChainSpec>,
        block_interval: u64,
        highest_snapshots_tracker: watch::Sender<Option<HighestSnapshots>>,
    ) -> Self {
        let snapshotter = Self {
            provider_factory: ProviderFactory::new(db, chain_spec),
            // TODO(alexey): fill from on-disk snapshot data
            highest_snapshots: HighestSnapshots::default(),
            highest_snapshots_tracker,
            block_interval,
        };

        snapshotter.update_highest_snapshots_tracker();

        snapshotter
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

    fn update_highest_snapshots_tracker(&self) {
        let _ = self.highest_snapshots_tracker.send(Some(self.highest_snapshots)).map_err(|_| {
            warn!(target: "snapshot", "Highest snapshots channel closed");
        });
    }

    /// Run the snapshotter
    pub fn run(&mut self, targets: SnapshotTargets) -> SnapshotterResult {
        debug_assert!(targets.is_multiple_of_block_interval(self.block_interval));
        debug_assert!(targets.is_contiguous_to_highest_snapshots(self.highest_snapshots));

        // TODO(alexey): snapshot logic

        self.update_highest_snapshots_tracker();

        Ok(targets)
    }

    /// Returns a snapshot targets at the provided finalized block number.
    /// This determined by the check against last snapshots.
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

        let to_tx_number = provider
            .block_body_indices(to_block_number)?
            .ok_or(RethError::Custom(
                "Block body indices for current block number not found".to_string(),
            ))?
            .last_tx_num();

        // Calculate block ranges to snapshot
        let headers_block_range =
            self.highest_snapshots.headers.map_or(0, |block_number| block_number + 1)..=
                to_block_number;
        let receipts_block_range =
            self.highest_snapshots.receipts.map_or(0, |block_number| block_number + 1)..=
                to_block_number;
        let transactions_block_range =
            self.highest_snapshots.transactions.map_or(0, |block_number| block_number + 1)..=
                to_block_number;

        // Calculate transaction ranges to snapshot
        let receipts_from_tx_number =
            if let Some(previous_block_number) = self.highest_snapshots.receipts {
                provider
                    .block_body_indices(previous_block_number)?
                    .ok_or(RethError::Custom(
                        "Block body indices for previous block number not found".to_string(),
                    ))?
                    .next_tx_num()
            } else {
                0
            };
        let receipts_tx_range = receipts_from_tx_number..=to_tx_number;

        let transactions_from_tx_number =
            if self.highest_snapshots.transactions == self.highest_snapshots.receipts {
                receipts_from_tx_number
            } else if let Some(previous_block_number) = self.highest_snapshots.transactions {
                provider
                    .block_body_indices(previous_block_number)?
                    .ok_or(RethError::Custom(
                        "Block body indices for previous block number not found".to_string(),
                    ))?
                    .next_tx_num()
            } else {
                0
            };
        let transactions_tx_range = transactions_from_tx_number..=to_tx_number;

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
}

#[cfg(test)]
mod tests {
    use crate::{snapshotter::SnapshotTargets, HighestSnapshots, Snapshotter};
    use assert_matches::assert_matches;
    use reth_interfaces::{
        test_utils::{generators, generators::random_block_range},
        RethError,
    };
    use reth_primitives::{H256, MAINNET};
    use reth_stages::test_utils::TestTransaction;
    use tokio::sync::watch;

    #[test]
    fn new() {
        let tx = TestTransaction::default();

        let (highest_snapshots_tx, highest_snapshots_rx) = watch::channel(None);
        assert_eq!(*highest_snapshots_rx.borrow(), None);

        Snapshotter::new(tx.inner_raw(), MAINNET.clone(), 2, highest_snapshots_tx);
        assert_eq!(*highest_snapshots_rx.borrow(), Some(HighestSnapshots::default()));
    }

    #[test]
    fn get_snapshot_targets() {
        let tx = TestTransaction::default();
        let mut rng = generators::rng();

        let blocks = random_block_range(&mut rng, 0..=3, H256::zero(), 2..3);
        tx.insert_blocks(blocks.iter(), None).expect("insert blocks");

        let mut snapshotter =
            Snapshotter::new(tx.inner_raw(), MAINNET.clone(), 2, watch::channel(None).0);

        // Block body indices not found
        assert_matches!(snapshotter.get_snapshot_targets(5), Err(RethError::Custom(_)));

        // Snapshot targets has data per part up to the passed finalized block number
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

        // Snapshot targets has data per part up to the passed finalized block number
        let targets = snapshotter.get_snapshot_targets(3).expect("get snapshot targets");
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
    }
}
