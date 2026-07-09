//! Transaction-pool backed candidate source for engine cache prewarming.

use alloy_primitives::Address;
use reth_engine_primitives::TxPoolPrewarmingConfig;
use reth_engine_tree::tree::{
    TxPoolPrewarmSelection, TxPoolPrewarmSource, TxPoolPrewarmTransaction,
};
use reth_primitives_traits::{NodePrimitives, TxTy};
use reth_transaction_pool::{
    BestTransactionsAttributes, PoolTransaction, TransactionPool, ValidPoolTransaction,
};
use std::{
    collections::HashMap,
    fmt::Debug,
    marker::PhantomData,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

/// [`TransactionPool`]-backed [`TxPoolPrewarmSource`].
#[derive(Debug, Clone)]
pub(crate) struct PoolTxPoolPrewarmSource<N: NodePrimitives, P> {
    pool: P,
    _marker: PhantomData<N>,
}

impl<N: NodePrimitives, P> PoolTxPoolPrewarmSource<N, P> {
    /// Creates a new txpool prewarm source.
    pub(crate) const fn new(pool: P) -> Self {
        Self { pool, _marker: PhantomData }
    }
}

impl<N, P> TxPoolPrewarmSource<N> for PoolTxPoolPrewarmSource<N, P>
where
    N: NodePrimitives,
    P: TransactionPool<Transaction: PoolTransaction<Consensus = TxTy<N>>>
        + Clone
        + Send
        + Sync
        + Debug
        + 'static,
{
    fn best_transactions(
        &self,
        block_gas_limit: u64,
        config: TxPoolPrewarmingConfig,
        stop: &AtomicBool,
    ) -> TxPoolPrewarmSelection<N> {
        if !config.should_prewarm() {
            return TxPoolPrewarmSelection::default()
        }

        let block_info = self.pool.block_info();
        let mut best =
            self.pool.best_transactions_with_attributes(BestTransactionsAttributes::new(
                block_info.pending_basefee,
                block_info.pending_blob_fee.and_then(|blob_fee| u64::try_from(blob_fee).ok()),
            ));
        best.no_updates();

        select_best_transactions(&mut best, block_gas_limit, config, stop)
    }
}

fn select_best_transactions<N, Tx, I>(
    best: &mut I,
    block_gas_limit: u64,
    config: TxPoolPrewarmingConfig,
    stop: &AtomicBool,
) -> TxPoolPrewarmSelection<N>
where
    N: NodePrimitives,
    Tx: PoolTransaction<Consensus = TxTy<N>>,
    I: Iterator<Item = Arc<ValidPoolTransaction<Tx>>> + ?Sized,
{
    let gas_limit = block_gas_limit.saturating_mul(config.gas_limit_multiplier);
    let mut selection = TxPoolPrewarmSelection::default();
    let mut per_sender = HashMap::<Address, usize>::new();

    while selection.scanned < config.max_candidate_scan {
        if stop.load(Ordering::Relaxed) {
            selection.canceled = true;
            break
        }

        let Some(transaction) = best.next() else { break };
        selection.scanned += 1;

        let tx_gas_limit = transaction.transaction.gas_limit();
        if selection.selected_gas.saturating_add(tx_gas_limit) > gas_limit {
            break
        }

        let sender = transaction.sender();
        let selected_for_sender = per_sender.entry(sender).or_default();
        if *selected_for_sender >= config.max_transactions_per_sender {
            continue
        }

        *selected_for_sender += 1;
        selection.selected_gas = selection.selected_gas.saturating_add(tx_gas_limit);
        selection.transactions.push(to_prewarm_transaction(transaction, sender));
    }

    selection
}

fn to_prewarm_transaction<N, Tx>(
    transaction: Arc<ValidPoolTransaction<Tx>>,
    sender: Address,
) -> TxPoolPrewarmTransaction<N>
where
    N: NodePrimitives,
    Tx: PoolTransaction<Consensus = TxTy<N>>,
{
    TxPoolPrewarmTransaction { sender, transaction: transaction.transaction.clone_into_consensus() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_ethereum_primitives::EthPrimitives;
    use reth_transaction_pool::test_utils::{MockTransaction, MockTransactionFactory};

    fn valid_tx(
        factory: &mut MockTransactionFactory,
        sender: Address,
        nonce: u64,
        gas_limit: u64,
    ) -> Arc<ValidPoolTransaction<MockTransaction>> {
        Arc::new(
            factory.validated(
                MockTransaction::eip1559()
                    .rng_hash()
                    .with_sender(sender)
                    .with_nonce(nonce)
                    .with_gas_limit(gas_limit),
            ),
        )
    }

    #[test]
    fn selection_respects_sender_cap_and_scan_limit() {
        let sender = Address::random();
        let mut factory = MockTransactionFactory::default();
        let mut best = (0..10)
            .map(|nonce| valid_tx(&mut factory, sender, nonce, 21_000))
            .collect::<Vec<_>>()
            .into_iter();
        let stop = AtomicBool::new(false);
        let config = TxPoolPrewarmingConfig::DEFAULT
            .with_enabled(true)
            .with_max_transactions_per_sender(2)
            .with_max_candidate_scan(3);

        let selection =
            select_best_transactions::<EthPrimitives, _, _>(&mut best, 30_000_000, config, &stop);

        assert_eq!(selection.scanned, 3);
        assert_eq!(selection.len(), 2);
        assert_eq!(selection.selected_gas, 42_000);
        assert!(!selection.canceled);
    }

    #[test]
    fn selection_respects_gas_limit() {
        let sender = Address::random();
        let mut factory = MockTransactionFactory::default();
        let mut best =
            vec![valid_tx(&mut factory, sender, 0, 60), valid_tx(&mut factory, sender, 1, 60)]
                .into_iter();
        let stop = AtomicBool::new(false);
        let config = TxPoolPrewarmingConfig::DEFAULT
            .with_enabled(true)
            .with_max_transactions_per_sender(16)
            .with_max_candidate_scan(16)
            .with_gas_limit_multiplier(1);

        let selection =
            select_best_transactions::<EthPrimitives, _, _>(&mut best, 100, config, &stop);

        assert_eq!(selection.scanned, 2);
        assert_eq!(selection.len(), 1);
        assert_eq!(selection.selected_gas, 60);
    }

    #[test]
    fn selection_observes_cancellation() {
        let sender = Address::random();
        let mut factory = MockTransactionFactory::default();
        let mut best = vec![valid_tx(&mut factory, sender, 0, 21_000)].into_iter();
        let stop = AtomicBool::new(true);
        let config = TxPoolPrewarmingConfig::DEFAULT.with_enabled(true);

        let selection =
            select_best_transactions::<EthPrimitives, _, _>(&mut best, 30_000_000, config, &stop);

        assert_eq!(selection.scanned, 0);
        assert!(selection.is_empty());
        assert!(selection.canceled);
    }
}
