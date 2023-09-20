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
        struct Snapshots {
            $($name: $ty,)+
        }

        pub struct SnapshotThresholds {
            $($name: u64,)+
        }

        pub struct SnapshotRequest {
            $($name: Option<$ty>,)+
        }

        impl SnapshotRequest {
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

    /// Run the snapshotter
    pub fn run(&mut self, request: SnapshotRequest) -> SnapshotterResult {
        // TODO(alexey): snapshot logic

        if let Some(headers) = request.headers {
            self.last_snapshots.headers = headers;
        }
        if let Some(receipts) = request.receipts {
            self.last_snapshots.receipts = receipts;
        }
        if let Some(transactions) = request.transactions {
            self.last_snapshots.transactions = transactions;
        }

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
            .ok_or(reth_interfaces::Error::Custom("Block indices not found".to_string()))?;
        let finalized_tx_number = finalized_block_indices.last_tx_num();

        Ok(SnapshotRequest {
            headers: finalized_block_number
                .checked_sub(self.last_snapshots.headers)
                .expect("finalized block should be greater than last headers snapshot")
                .gt(&self.thresholds.headers)
                .then_some(finalized_block_number),
            receipts: finalized_tx_number
                .checked_sub(self.last_snapshots.receipts)
                .expect("finalized tx number should be greater than last receipts snapshot")
                .gt(&self.thresholds.receipts)
                .then_some(finalized_tx_number),
            transactions: finalized_tx_number
                .checked_sub(self.last_snapshots.transactions)
                .expect("finalized tx number should be greater than last transactions snapshot")
                .gt(&self.thresholds.transactions)
                .then_some(finalized_tx_number),
        })
    }
}
