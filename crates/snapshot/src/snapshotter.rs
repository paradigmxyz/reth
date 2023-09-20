//! Support for snapshotting.

use crate::SnapshotterError;
use reth_db::database::Database;
use reth_primitives::{BlockNumber, ChainSpec, TxNumber};
use reth_provider::{BlockReader, ProviderFactory};
use std::sync::Arc;

/// Result of [Snapshotter::run] execution.
pub type SnapshotterResult = Result<(), SnapshotterError>;

/// The snapshotter type itself with the result of [Snapshotter::run]
pub type SnapshotterWithResult<DB> = (Snapshotter<DB>, SnapshotterResult);

/// Snapshotting routine. Main snapshotting logic happens in [Snapshotter::run].
pub struct Snapshotter<DB> {
    provider_factory: ProviderFactory<DB>,
    last_snapshots: Snapshots,
    thresholds: SnapshotThresholds,
}

macro_rules! snapshots {
    ($($name:ident: $ty:ty),* $(,)?) => {
        #[derive(Debug, Clone, Copy)]
        struct Snapshots {
            $($name: $ty,)+
        }

        /// Thresholds for snapshotting per data part, measured in part-specific unit (usually
        /// [BlockNumber] or [TxNumber]).
        ///
        /// When the specified number of entities accumulated in database since last snapshot for
        /// data part, [Snapshotter::get_snapshot_request] will return [Option::Some].
        #[derive(Debug, Clone, Copy)]
        pub struct SnapshotThresholds {
            $($name: u64,)+
        }

        /// Snapshot request with targets per data part, measured in part-specific unit (usually
        /// [BlockNumber] or [TxNumber]).
        #[derive(Debug, Clone, Copy)]
        pub struct SnapshotRequest {
            $($name: Option<$ty>,)+
        }

        impl SnapshotRequest {
            /// Returns `true` if any of the data parts has targets, i.e. is [`Option::Some`].
            pub fn any(&self) -> bool {
                $(self.$name.is_some())||+
            }
        }
    };
}

snapshots!(
    headers: BlockNumber,
    receipts: TxNumber,
    transactions: TxNumber,
);

impl<DB: Database> Snapshotter<DB> {
    /// Creates a new [Snapshotter].
    pub fn new(db: DB, chain_spec: Arc<ChainSpec>, thresholds: SnapshotThresholds) -> Self {
        Self {
            provider_factory: ProviderFactory::new(db, chain_spec),
            // TODO(alexey): fill from on-disk snapshot data
            last_snapshots: Snapshots { headers: 0, receipts: 0, transactions: 0 },
            thresholds,
        }
    }

    fn set_last_snapshots_from_request(&mut self, request: SnapshotRequest) {
        if let Some(headers) = request.headers {
            self.last_snapshots.headers = headers;
        }
        if let Some(receipts) = request.receipts {
            self.last_snapshots.receipts = receipts;
        }
        if let Some(transactions) = request.transactions {
            self.last_snapshots.transactions = transactions;
        }
    }

    /// Run the snapshotter
    pub fn run(&mut self, request: SnapshotRequest) -> SnapshotterResult {
        // TODO(alexey): snapshot logic

        self.set_last_snapshots_from_request(request);

        Ok(())
    }

    /// Returns a snapshot request at the provided finalized block number.
    /// This determined by the check against last snapshots.
    pub fn get_snapshot_request(
        &self,
        finalized_block_number: BlockNumber,
    ) -> reth_interfaces::Result<SnapshotRequest> {
        let provider = self.provider_factory.provider()?;

        let finalized_block_indices = provider
            .block_body_indices(finalized_block_number)?
            .ok_or(reth_interfaces::Error::Custom("Block body indices not found".to_string()))?;
        let finalized_tx_number = finalized_block_indices.last_tx_num();

        Ok(SnapshotRequest {
            headers: finalized_block_number
                .checked_sub(self.last_snapshots.headers)
                .expect("finalized block should be greater than last headers snapshot")
                .ge(&self.thresholds.headers)
                .then_some(finalized_block_number),
            receipts: finalized_tx_number
                .checked_sub(self.last_snapshots.receipts)
                .expect("finalized tx number should be greater than last receipts snapshot")
                .ge(&self.thresholds.receipts)
                .then_some(finalized_tx_number),
            transactions: finalized_tx_number
                .checked_sub(self.last_snapshots.transactions)
                .expect("finalized tx number should be greater than last transactions snapshot")
                .ge(&self.thresholds.transactions)
                .then_some(finalized_tx_number),
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::{snapshotter::SnapshotThresholds, SnapshotRequest, Snapshotter};
    use assert_matches::assert_matches;
    use reth_interfaces::test_utils::{generators, generators::random_block_range};
    use reth_primitives::{H256, MAINNET};
    use reth_stages::test_utils::TestTransaction;

    #[test]
    fn get_snapshot_request() {
        let tx = TestTransaction::default();
        let mut rng = generators::rng();

        let blocks = random_block_range(&mut rng, 0..=2, H256::zero(), 2..3);
        tx.insert_blocks(blocks.iter(), None).expect("insert blocks");

        let thresholds = SnapshotThresholds { headers: 1, receipts: 2, transactions: 2 };
        let mut snapshotter = Snapshotter::new(tx.inner_raw(), MAINNET.clone(), thresholds);

        // Block body indices not found
        assert_matches!(
            snapshotter.get_snapshot_request(3),
            Err(reth_interfaces::Error::Custom(_))
        );

        // Snapshot request returns data per part up to the passed finalized block number
        let request = snapshotter.get_snapshot_request(1).expect("get snapshot request");
        assert_matches!(
            request,
            SnapshotRequest { headers: Some(1), receipts: Some(3), transactions: Some(3) }
        );
        // Imitate snapshotter run according to the request which updates the last snapshots state
        snapshotter.set_last_snapshots_from_request(request);

        // Nothing to snapshot, last snapshots state of snapshotter doesn't pass the thresholds
        assert_matches!(
            snapshotter.get_snapshot_request(1),
            Ok(SnapshotRequest { headers: None, receipts: None, transactions: None })
        );

        // Snapshot request returns data per part up to the passed finalized block number
        assert_matches!(
            snapshotter.get_snapshot_request(2),
            Ok(SnapshotRequest { headers: Some(2), receipts: Some(5), transactions: Some(5) })
        );
    }
}
