use async_trait::async_trait;
use jsonrpsee::core::RpcResult as Result;
use reth_primitives::{Address, U256, U64};
use reth_rpc_api::TxPoolApiServer;
use reth_rpc_types::{
    txpool::{TxpoolContent, TxpoolContentFrom, TxpoolInspect, TxpoolInspectSummary, TxpoolStatus},
    Transaction,
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
    pub fn new(pool: Pool) -> Self {
        TxPoolApi { pool }
    }
}

impl<Pool> TxPoolApi<Pool>
where
    Pool: TransactionPool + 'static,
{
    fn content(&self) -> TxpoolContent {
        #[inline]
        fn insert<T: PoolTransaction>(
            tx: &T,
            content: &mut BTreeMap<Address, BTreeMap<String, Transaction>>,
        ) {
            let entry = content.entry(tx.sender()).or_default();
            let key = tx.nonce().to_string();
            let tx = tx.to_recovered_transaction();
            let tx = reth_rpc_types_compat::transaction::from_recovered(tx);
            entry.insert(key, tx);
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
        Ok(TxpoolStatus {
            pending: U64::from(all.pending.len()),
            queued: U64::from(all.queued.len()),
        })
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
        fn insert<T: PoolTransaction>(
            tx: &T,
            inspect: &mut BTreeMap<Address, BTreeMap<String, TxpoolInspectSummary>>,
        ) {
            let entry = inspect.entry(tx.sender()).or_default();
            let key = tx.nonce().to_string();
            let tx = tx.to_recovered_transaction();
            let to = tx.to();
            let gas_price = tx.transaction.max_fee_per_gas();
            let value = tx.value();
            let gas = tx.gas_limit();
            let summary = TxpoolInspectSummary {
                to,
                value: value.into(),
                gas: U256::from(gas),
                gas_price: U256::from(gas_price),
            };
            entry.insert(key, summary);
        }

        let mut inspect = TxpoolInspect::default();
        let AllPoolTransactions { pending, queued } = self.pool.all_transactions();

        for pending in pending {
            insert(&pending.transaction, &mut inspect.pending);
        }
        for queued in queued {
            insert(&queued.transaction, &mut inspect.queued);
        }

        Ok(inspect)
    }

    /// Retrieves the transactions contained within the txpool, returning pending as well as queued
    /// transactions of this address, grouped by nonce.
    ///
    /// See [here](https://geth.ethereum.org/docs/rpc/ns-txpool#txpool_contentFrom) for more details
    /// Handler for `txpool_contentFrom`
    async fn txpool_content_from(&self, from: Address) -> Result<TxpoolContentFrom> {
        trace!(target: "rpc::eth", ?from, "Serving txpool_contentFrom");
        Ok(self.content().remove_from(&from))
    }

    /// Returns the details of all transactions currently pending for inclusion in the next
    /// block(s), as well as the ones that are being scheduled for future execution only.
    ///
    /// See [here](https://geth.ethereum.org/docs/rpc/ns-txpool#txpool_content) for more details
    /// Handler for `txpool_inspect`
    async fn txpool_content(&self) -> Result<TxpoolContent> {
        trace!(target: "rpc::eth", "Serving txpool_inspect");
        Ok(self.content())
    }
}

impl<Pool> std::fmt::Debug for TxPoolApi<Pool> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TxpoolApi").finish_non_exhaustive()
    }
}
