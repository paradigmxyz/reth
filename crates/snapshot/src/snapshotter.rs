//! Support for snapshotting.

use crate::SnapshotterError;
use reth_db::database::Database;
use reth_primitives::{BlockNumber, ChainSpec, TxNumber};
use reth_provider::{BlockReader, ProviderFactory};
use std::sync::Arc;

/// Result of [Snapshotter::run] execution.
pub type SnapshotterResult = Result<SnapshotTargets, SnapshotterError>;

/// The snapshotter type itself with the result of [Snapshotter::run]
pub type SnapshotterWithResult<DB> = (Snapshotter<DB>, SnapshotterResult);

/// Snapshotting routine. Main snapshotting logic happens in [Snapshotter::run].
pub struct Snapshotter<DB> {
    provider_factory: ProviderFactory<DB>,
    highest_snapshots: HighestSnapshots,
    block_threshold: u64,
}

/// Highest snapshotted block number, per data part.
#[derive(Debug, Clone, Copy)]
struct HighestSnapshots {
    headers: BlockNumber,
    receipts: BlockNumber,
    transactions: BlockNumber,
}

/// Snapshot targets, per data part, measured in [`BlockNumber`] and [`TxNumber`], if applicable.
#[derive(Debug, Clone, Copy)]
pub struct SnapshotTargets {
    headers: Option<BlockNumber>,
    receipts: Option<(BlockNumber, TxNumber)>,
    transactions: Option<(BlockNumber, TxNumber)>,
}

impl SnapshotTargets {
    /// Returns `true` if any of the data parts has targets, i.e. is [`Option::Some`].
    pub fn any(&self) -> bool {
        self.headers.is_some() || self.receipts.is_some() || self.transactions.is_some()
    }
}

impl<DB: Database> Snapshotter<DB> {
    /// Creates a new [Snapshotter].
    pub fn new(db: DB, chain_spec: Arc<ChainSpec>, block_threshold: u64) -> Self {
        Self {
            provider_factory: ProviderFactory::new(db, chain_spec),
            // TODO(alexey): fill from on-disk snapshot data
            highest_snapshots: HighestSnapshots { headers: 0, receipts: 0, transactions: 0 },
            block_threshold,
        }
    }

    fn set_last_snapshots_from_targets(&mut self, targets: SnapshotTargets) {
        if let Some(block_number) = targets.headers {
            self.highest_snapshots.headers = block_number;
        }
        if let Some((block_number, _)) = targets.receipts {
            self.highest_snapshots.receipts = block_number;
        }
        if let Some((block_number, _)) = targets.transactions {
            self.highest_snapshots.transactions = block_number;
        }
    }

    /// Run the snapshotter
    pub fn run(&mut self, targets: SnapshotTargets) -> SnapshotterResult {
        // TODO(alexey): snapshot logic

        self.set_last_snapshots_from_targets(targets);

        Ok(targets)
    }

    /// Returns a snapshot targets at the provided finalized block number.
    /// This determined by the check against last snapshots.
    pub fn get_snapshot_targets(
        &self,
        finalized_block_number: BlockNumber,
    ) -> reth_interfaces::Result<SnapshotTargets> {
        let provider = self.provider_factory.provider()?;

        let finalized_block_indices = provider
            .block_body_indices(finalized_block_number)?
            .ok_or(reth_interfaces::Error::Custom("Block body indices not found".to_string()))?;
        let finalized_tx_number = finalized_block_indices.last_tx_num();

        Ok(SnapshotTargets {
            headers: finalized_block_number
                .checked_sub(self.highest_snapshots.headers)
                .expect("finalized block should be greater than last headers snapshot")
                .ge(&self.block_threshold)
                .then_some(finalized_block_number),
            receipts: finalized_block_number
                .checked_sub(self.highest_snapshots.receipts)
                .expect("finalized block should be greater than last receipts snapshot")
                .ge(&self.block_threshold)
                .then_some((finalized_block_number, finalized_tx_number)),
            transactions: finalized_block_number
                .checked_sub(self.highest_snapshots.transactions)
                .expect("finalized block should be greater than last transactions snapshot")
                .ge(&self.block_threshold)
                .then_some((finalized_block_number, finalized_tx_number)),
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::{snapshotter::SnapshotTargets, Snapshotter};
    use assert_matches::assert_matches;
    use reth_interfaces::test_utils::{generators, generators::random_block_range};
    use reth_primitives::{H256, MAINNET};
    use reth_stages::test_utils::TestTransaction;

    #[test]
    fn get_snapshot_targets() {
        let tx = TestTransaction::default();
        let mut rng = generators::rng();

        let blocks = random_block_range(&mut rng, 0..=2, H256::zero(), 2..3);
        tx.insert_blocks(blocks.iter(), None).expect("insert blocks");

        let mut snapshotter = Snapshotter::new(tx.inner_raw(), MAINNET.clone(), 1);

        // Block body indices not found
        assert_matches!(
            snapshotter.get_snapshot_targets(3),
            Err(reth_interfaces::Error::Custom(_))
        );

        // Snapshot targets has data per part up to the passed finalized block number
        let targets = snapshotter.get_snapshot_targets(1).expect("get snapshot request");
        assert_matches!(
            targets,
            SnapshotTargets {
                headers: Some(1),
                receipts: Some((1, 3)),
                transactions: Some((1, 3))
            }
        );
        // Imitate snapshotter run according to the targets which updates the last snapshots state
        snapshotter.set_last_snapshots_from_targets(targets);

        // Nothing to snapshot, last snapshots state of snapshotter doesn't pass the thresholds
        assert_matches!(
            snapshotter.get_snapshot_targets(1),
            Ok(SnapshotTargets { headers: None, receipts: None, transactions: None })
        );

        // Snapshot targets has data per part up to the passed finalized block number
        assert_matches!(
            snapshotter.get_snapshot_targets(2),
            Ok(SnapshotTargets {
                headers: Some(2),
                receipts: Some((2, 5)),
                transactions: Some((2, 5))
            })
        );
    }
}
