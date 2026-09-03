use core::fmt;
use std::collections::BTreeMap;

use alloy_consensus::Transaction;
use alloy_primitives::Address;
use alloy_rpc_types_txpool::{
    TxpoolContent, TxpoolContentFrom, TxpoolInspect, TxpoolInspectSummary, TxpoolStatus,
};
use async_trait::async_trait;
use jsonrpsee::core::RpcResult;
use reth_primitives_traits::NodePrimitives;
use reth_rpc_api::TxPoolApiServer;
use reth_rpc_convert::RpcConvert;
use reth_rpc_eth_api::RpcTransaction;
use reth_transaction_pool::{
    AllPoolTransactions, PoolConsensusTx, PoolTransaction, TransactionPool,
};
use tracing::trace;

/// `txpool` API implementation.
///
/// This type provides the functionality for handling `txpool` related requests.
#[derive(Clone)]
pub struct TxPoolApi<Pool, Eth> {
    /// An interface to interact with the pool
    pool: Pool,
    converter: Eth,
}

impl<Pool, Eth> TxPoolApi<Pool, Eth> {
    /// Creates a new instance of `TxpoolApi`.
    pub const fn new(pool: Pool, converter: Eth) -> Self {
        Self { pool, converter }
    }
}

impl<Pool, Eth> TxPoolApi<Pool, Eth>
where
    Pool: TransactionPool<Transaction: PoolTransaction<Consensus: Transaction>> + 'static,
    Eth: RpcConvert<Primitives: NodePrimitives<SignedTx = PoolConsensusTx<Pool>>>,
{
    fn content(&self) -> Result<TxpoolContent<RpcTransaction<Eth::Network>>, Eth::Error> {
        let AllPoolTransactions { pending, queued } = self.pool.all_transactions();

        let mut content = TxpoolContent::default();
        for tx in pending {
            let sender = tx.transaction.sender();
            self.insert_by_nonce(&tx.transaction, content.pending.entry(sender).or_default())?;
        }
        for tx in queued {
            let sender = tx.transaction.sender();
            self.insert_by_nonce(&tx.transaction, content.queued.entry(sender).or_default())?;
        }

        Ok(content)
    }

    fn content_from(
        &self,
        from: Address,
    ) -> Result<TxpoolContentFrom<RpcTransaction<Eth::Network>>, Eth::Error> {
        let mut content = TxpoolContentFrom::default();
        // one snapshot for both sides, so a transaction moving between sub-pools meanwhile is
        // reported on exactly one of them
        let AllPoolTransactions { pending, queued } = self.pool.all_transactions_by_sender(from);
        for tx in pending {
            self.insert_by_nonce(&tx.transaction, &mut content.pending)?;
        }
        for tx in queued {
            self.insert_by_nonce(&tx.transaction, &mut content.queued)?;
        }

        Ok(content)
    }

    /// Converts the pool transaction and inserts it into the given map, keyed by its nonce.
    #[inline]
    fn insert_by_nonce(
        &self,
        tx: &Pool::Transaction,
        txs: &mut BTreeMap<String, RpcTransaction<Eth::Network>>,
    ) -> Result<(), Eth::Error> {
        txs.insert(tx.nonce().to_string(), self.converter.fill_pending(tx.clone_into_consensus())?);

        Ok(())
    }
}

#[async_trait]
impl<Pool, Eth> TxPoolApiServer<RpcTransaction<Eth::Network>> for TxPoolApi<Pool, Eth>
where
    Pool: TransactionPool<Transaction: PoolTransaction<Consensus: Transaction>> + 'static,
    Eth: RpcConvert<Primitives: NodePrimitives<SignedTx = PoolConsensusTx<Pool>>> + 'static,
{
    /// Returns the number of transactions currently pending for inclusion in the next block(s), as
    /// well as the ones that are being scheduled for future execution only.
    /// Ref: [Here](https://geth.ethereum.org/docs/rpc/ns-txpool#txpool_status)
    ///
    /// Handler for `txpool_status`
    async fn txpool_status(&self) -> RpcResult<TxpoolStatus> {
        trace!(target: "rpc::eth", "Serving txpool_status");
        let (pending, queued) = self.pool.pending_and_queued_txn_count();
        Ok(TxpoolStatus { pending: pending as u64, queued: queued as u64 })
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
        fn insert<T: PoolTransaction<Consensus: Transaction>>(
            tx: &T,
            inspect: &mut BTreeMap<Address, BTreeMap<String, TxpoolInspectSummary>>,
        ) {
            let entry = inspect.entry(tx.sender()).or_default();
            let tx = tx.clone_into_consensus();
            entry.insert(tx.nonce().to_string(), tx.into_inner().into());
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
    ) -> RpcResult<TxpoolContentFrom<RpcTransaction<Eth::Network>>> {
        trace!(target: "rpc::eth", ?from, "Serving txpool_contentFrom");
        Ok(self.content_from(from).map_err(Into::into)?)
    }

    /// Returns the details of all transactions currently pending for inclusion in the next
    /// block(s), as well as the ones that are being scheduled for future execution only.
    ///
    /// See [here](https://geth.ethereum.org/docs/rpc/ns-txpool#txpool_content) for more details
    /// Handler for `txpool_content`
    async fn txpool_content(&self) -> RpcResult<TxpoolContent<RpcTransaction<Eth::Network>>> {
        trace!(target: "rpc::eth", "Serving txpool_content");
        Ok(self.content().map_err(Into::into)?)
    }
}

impl<Pool, Eth> fmt::Debug for TxPoolApi<Pool, Eth> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TxpoolApi").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eth::helpers::types::EthRpcConverter;
    use reth_chainspec::MAINNET;
    use reth_rpc_eth_types::receipt::EthReceiptConverter;
    use reth_transaction_pool::{
        test_utils::{testing_pool, MockTransaction},
        TransactionOrigin,
    };

    #[tokio::test]
    async fn content_from_matches_content() {
        let senders = [Address::with_last_byte(1), Address::with_last_byte(2)];

        let pool = testing_pool();
        for sender in senders {
            // nonces 0 and 1 end up in the pending sub-pool, the gapped nonce 9 in a parked one
            for nonce in [0, 1, 9] {
                let tx = MockTransaction::legacy()
                    .with_sender(sender)
                    .with_nonce(nonce)
                    .with_gas_price(100);
                pool.add_transaction(TransactionOrigin::External, tx).await.unwrap();
            }
        }

        let api =
            TxPoolApi::new(pool, EthRpcConverter::new(EthReceiptConverter::new(MAINNET.clone())));
        let mut content = api.content().unwrap();
        // guards the fixture: both sub-pools must be populated for the comparison to mean anything
        assert_eq!((content.pending.len(), content.queued.len()), (senders.len(), senders.len()));

        // the unknown sender must yield the same empty result as the whole-pool path
        for sender in senders.into_iter().chain([Address::with_last_byte(3)]) {
            assert_eq!(api.content_from(sender).unwrap(), content.remove_from(&sender));
        }
    }
}
