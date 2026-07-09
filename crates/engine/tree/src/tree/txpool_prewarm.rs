//! Txpool candidate interface for engine cache prewarming.

use alloy_consensus::transaction::Recovered;
use alloy_primitives::Address;
use reth_engine_primitives::TxPoolPrewarmingConfig;
use reth_primitives_traits::{NodePrimitives, TxTy};
use std::{fmt::Debug, sync::atomic::AtomicBool};

/// A transaction selected from the txpool for cache-only prewarming.
#[derive(Debug, Clone)]
pub struct TxPoolPrewarmTransaction<N: NodePrimitives> {
    /// Recovered sender of the transaction.
    pub sender: Address,
    /// Recovered consensus transaction to execute for cache warming.
    pub transaction: Recovered<TxTy<N>>,
}

/// Transactions selected for one txpool prewarming wave.
#[derive(Debug, Clone)]
pub struct TxPoolPrewarmSelection<N: NodePrimitives> {
    /// Selected transactions in candidate order.
    pub transactions: Vec<TxPoolPrewarmTransaction<N>>,
    /// Number of txpool candidates scanned while building this selection.
    pub scanned: usize,
    /// Total gas limit of the selected transactions.
    pub selected_gas: u64,
    /// Whether selection stopped because cancellation was requested.
    pub canceled: bool,
}

impl<N: NodePrimitives> Default for TxPoolPrewarmSelection<N> {
    fn default() -> Self {
        Self { transactions: Vec::new(), scanned: 0, selected_gas: 0, canceled: false }
    }
}

impl<N: NodePrimitives> TxPoolPrewarmSelection<N> {
    /// Returns true if no transactions were selected.
    pub fn is_empty(&self) -> bool {
        self.transactions.is_empty()
    }

    /// Returns the number of selected transactions.
    pub fn len(&self) -> usize {
        self.transactions.len()
    }
}

/// Source of txpool transactions for best-effort engine cache prewarming.
pub trait TxPoolPrewarmSource<N: NodePrimitives>: Send + Sync + Debug {
    /// Returns transactions selected in block-building order for one prewarming wave.
    fn best_transactions(
        &self,
        block_gas_limit: u64,
        config: TxPoolPrewarmingConfig,
        stop: &AtomicBool,
    ) -> TxPoolPrewarmSelection<N>;
}
