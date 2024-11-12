use core::fmt;
use std::collections::BTreeMap;

use alloy_consensus::Transaction;
use alloy_primitives::Address;
use alloy_rpc_types_txpool::{
    TxpoolContent, TxpoolContentFrom, TxpoolInspect, TxpoolInspectSummary, TxpoolStatus,
};
use async_trait::async_trait;
use jsonrpsee::core::RpcResult;
use reth_primitives::TransactionSignedEcRecovered;
use reth_rpc_api::TxPoolApiServer;
use reth_rpc_types_compat::{transaction::from_recovered, TransactionCompat};
use reth_transaction_pool::{AllPoolTransactions, PoolTransaction, TransactionPool};
use tracing::trace;

/// `txpool` API implementation.
///
/// This type provides the functionality for handling `txpool` related requests.
#[derive(Clone)]
pub struct TxPoolApi<Pool, Eth> {
    /// An interface to interact with the pool
    pool: Pool,
    tx_resp_builder: Eth,
}

impl<Pool, Eth> TxPoolApi<Pool, Eth> {
    /// Creates a new instance of `TxpoolApi`.
    pub const fn new(pool: Pool, tx_resp_builder: Eth) -> Self {
        Self { pool, tx_resp_builder }
    }
}

impl<Pool, Eth> TxPoolApi<Pool, Eth>
where
    Pool: TransactionPool + 'static,
    Eth: TransactionCompat,
{
    fn content(&self) -> Result<TxpoolContent<Eth::Transaction>, Eth::Error> {
        #[inline]
        fn insert<Tx, RpcTxB>(
            tx: &Tx,
            content: &mut BTreeMap<Address, BTreeMap<String, RpcTxB::Transaction>>,
            resp_builder: &RpcTxB,
        ) -> Result<(), RpcTxB::Error>
        where
            Tx: PoolTransaction<Consensus: Into<TransactionSignedEcRecovered>>,
            RpcTxB: TransactionCompat,
        {
            content.entry(tx.sender()).or_default().insert(
                tx.nonce().to_string(),
                from_recovered(tx.clone().into_consensus().into(), resp_builder)?,
            );

            Ok(())
        }

        let AllPoolTransactions { pending, queued } = self.pool.all_transactions();

        let mut content = TxpoolContent { pending: BTreeMap::new(), queued: BTreeMap::new() };
        for pending in pending {
            insert::<_, Eth>(&pending.transaction, &mut content.pending, &self.tx_resp_builder)?;
        }
        for queued in queued {
            insert::<_, Eth>(&queued.transaction, &mut content.queued, &self.tx_resp_builder)?;
        }

        Ok(content)
    }
}

#[async_trait]
impl<Pool, Eth> TxPoolApiServer<Eth::Transaction> for TxPoolApi<Pool, Eth>
where
    Pool: TransactionPool + 'static,
    Eth: TransactionCompat + 'static,
{
    /// Returns the number of transactions currently pending for inclusion in the next block(s), as
    /// well as the ones that are being scheduled for future execution only.
    /// Ref: [Here](https://geth.ethereum.org/docs/rpc/ns-txpool#txpool_status)
    ///
    /// Handler for `txpool_status`
    async fn txpool_status(&self) -> RpcResult<TxpoolStatus> {
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
    async fn txpool_inspect(&self) -> RpcResult<TxpoolInspect> {
        trace!(target: "rpc::eth", "Serving txpool_inspect");

        #[inline]
        fn insert<T: PoolTransaction<Consensus: Into<TransactionSignedEcRecovered>>>(
            tx: &T,
            inspect: &mut BTreeMap<Address, BTreeMap<String, TxpoolInspectSummary>>,
        ) {
            let entry = inspect.entry(tx.sender()).or_default();
            let tx: TransactionSignedEcRecovered = tx.clone().into_consensus().into();
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
    ) -> RpcResult<TxpoolContentFrom<Eth::Transaction>> {
        trace!(target: "rpc::eth", ?from, "Serving txpool_contentFrom");
        Ok(self.content().map_err(Into::into)?.remove_from(&from))
    }

    /// Returns the details of all transactions currently pending for inclusion in the next
    /// block(s), as well as the ones that are being scheduled for future execution only.
    ///
    /// See [here](https://geth.ethereum.org/docs/rpc/ns-txpool#txpool_content) for more details
    /// Handler for `txpool_content`
    async fn txpool_content(&self) -> RpcResult<TxpoolContent<Eth::Transaction>> {
        trace!(target: "rpc::eth", "Serving txpool_content");
        Ok(self.content().map_err(Into::into)?)
    }
}

impl<Pool, Eth> fmt::Debug for TxPoolApi<Pool, Eth> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TxpoolApi").finish_non_exhaustive()
    }
}
