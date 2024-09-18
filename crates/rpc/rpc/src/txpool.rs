use alloy_primitives::Address;
use async_trait::async_trait;
use jsonrpsee::core::RpcResult as Result;
use reth_primitives::TransactionSignedEcRecovered;
use reth_rpc_api::TxPoolApiServer;
use reth_rpc_types::{
    txpool::{TxpoolContent, TxpoolContentFrom, TxpoolInspect, TxpoolInspectSummary, TxpoolStatus},
    Transaction, WithOtherFields,
};
use reth_transaction_pool::{AllPoolTransactions, PoolTransaction, TransactionPool};
use std::collections::BTreeMap;
use tracing::trace;

/// `txpool` API implementation.
///
/// This type provides the functionality for handling `txpool` related requests.
#[derive(Clone)]
pub struct TxPoolApi<Pool> {
    /// An interface to interact with the pool
    pool: Pool,
}

impl<Pool> TxPoolApi<Pool> {
    /// Creates a new instance of `TxpoolApi`.
    pub const fn new(pool: Pool) -> Self {
        Self { pool }
    }
}

impl<Pool> TxPoolApi<Pool>
where
    Pool: TransactionPool + 'static,
{
    fn content(&self) -> TxpoolContent<WithOtherFields<Transaction>> {
        #[inline]
        fn insert<T: PoolTransaction<Consensus = TransactionSignedEcRecovered>>(
            tx: &T,
            content: &mut BTreeMap<Address, BTreeMap<String, WithOtherFields<Transaction>>>,
        ) {
            content.entry(tx.sender()).or_default().insert(
                tx.nonce().to_string(),
                reth_rpc_types_compat::transaction::from_recovered(tx.clone().into_consensus()),
            );
        }

        let AllPoolTransactions { pending, queued } = self.pool.all_transactions();

        let mut content = TxpoolContent::default();
        for pending in pending {
            insert(&pending.transaction, &mut content.pending);
        }
        for queued in queued {
            insert(&queued.transaction, &mut content.queued);
        }

        content
    }
}

#[async_trait]
impl<Pool> TxPoolApiServer for TxPoolApi<Pool>
where
    Pool: TransactionPool + 'static,
{
    /// Returns the number of transactions currently pending for inclusion in the next block(s), as
    /// well as the ones that are being scheduled for future execution only.
    /// Ref: [Here](https://geth.ethereum.org/docs/rpc/ns-txpool#txpool_status)
    ///
    /// Handler for `txpool_status`
    async fn txpool_status(&self) -> Result<TxpoolStatus> {
        trace!(target: "rpc::eth", "Serving txpool_status");
        let all = self.pool.all_transactions();
        Ok(TxpoolStatus { pending: all.pending.len() as u64, queued: all.queued.len() as u64 })
    }

    /// Returns a summary of all the transactions currently pending for inclusion in the next
    /// block(s), as well as the ones that are being scheduled for future execution only.
    ///
    /// See [here](https://geth.ethereum.org/docs/rpc/ns-txpool#txpool_inspect) for more details
    ///
    /// Handler for `txpool_inspect`
    async fn txpool_inspect(&self) -> Result<TxpoolInspect> {
        trace!(target: "rpc::eth", "Serving txpool_inspect");

        #[inline]
        fn insert<T: PoolTransaction<Consensus = TransactionSignedEcRecovered>>(
            tx: &T,
            inspect: &mut BTreeMap<Address, BTreeMap<String, TxpoolInspectSummary>>,
        ) {
            let entry = inspect.entry(tx.sender()).or_default();
            let tx = tx.clone().into_consensus();
            entry.insert(
                tx.nonce().to_string(),
                TxpoolInspectSummary {
                    to: tx.to(),
                    value: tx.value(),
                    gas: tx.gas_limit() as u128,
                    gas_price: tx.transaction.max_fee_per_gas(),
                },
            );
        }

        let AllPoolTransactions { pending, queued } = self.pool.all_transactions();

        Ok(TxpoolInspect {
            pending: pending.iter().fold(Default::default(), |mut acc, tx| {
                insert(&tx.transaction, &mut acc);
                acc
            }),
            queued: queued.iter().fold(Default::default(), |mut acc, tx| {
                insert(&tx.transaction, &mut acc);
                acc
            }),
        })
    }

    /// Retrieves the transactions contained within the txpool, returning pending as well as queued
    /// transactions of this address, grouped by nonce.
    ///
    /// See [here](https://geth.ethereum.org/docs/rpc/ns-txpool#txpool_contentFrom) for more details
    /// Handler for `txpool_contentFrom`
    async fn txpool_content_from(
        &self,
        from: Address,
    ) -> Result<TxpoolContentFrom<WithOtherFields<Transaction>>> {
        trace!(target: "rpc::eth", ?from, "Serving txpool_contentFrom");
        Ok(self.content().remove_from(&from))
    }

    /// Returns the details of all transactions currently pending for inclusion in the next
    /// block(s), as well as the ones that are being scheduled for future execution only.
    ///
    /// See [here](https://geth.ethereum.org/docs/rpc/ns-txpool#txpool_content) for more details
    /// Handler for `txpool_content`
    async fn txpool_content(&self) -> Result<TxpoolContent<WithOtherFields<Transaction>>> {
        trace!(target: "rpc::eth", "Serving txpool_content");
        Ok(self.content())
    }
}

impl<Pool> std::fmt::Debug for TxPoolApi<Pool> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TxpoolApi").finish_non_exhaustive()
    }
}
