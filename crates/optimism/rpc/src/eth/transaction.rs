//! Loads and formats OP transaction RPC response.

use std::marker::PhantomData;

use alloy_primitives::{Bytes, B256};
use op_alloy_network::{Network, Optimism};
use op_alloy_rpc_types::OptimismTransactionFields;
use reth_evm_optimism::RethL1BlockInfo;
use reth_node_api::FullNodeComponents;
use reth_primitives::TransactionSignedEcRecovered;
use reth_provider::{BlockReaderIdExt, TransactionsProvider};
use reth_rpc_eth_api::{
    helpers::{EthApiSpec, EthSigner, EthTransactions, LoadTransaction, SpawnBlocking},
    FromEthApiError, TransactionCompat,
};
use reth_rpc_eth_types::{utils::recover_raw_transaction, EthStateCache};
use reth_rpc_types::TransactionInfo;
use reth_transaction_pool::{PoolTransaction, TransactionOrigin, TransactionPool};
use revm::L1BlockInfo;

use crate::{OpEthApi, SequencerClient};

impl<N, Eth> EthTransactions for OpEthApi<N, Eth>
where
    Self: LoadTransaction,
    N: FullNodeComponents,
{
    fn provider(&self) -> impl BlockReaderIdExt {
        self.inner.provider()
    }

    fn signers(&self) -> &parking_lot::RwLock<Vec<Box<dyn EthSigner>>> {
        self.inner.signers()
    }

    /// Decodes and recovers the transaction and submits it to the pool.
    ///
    /// Returns the hash of the transaction.
    async fn send_raw_transaction(&self, tx: Bytes) -> Result<B256, Self::Error> {
        let recovered = recover_raw_transaction(tx.clone())?;
        let pool_transaction = <Self::Pool as TransactionPool>::Transaction::from_pooled(recovered);

        // On optimism, transactions are forwarded directly to the sequencer to be included in
        // blocks that it builds.
        if let Some(client) = self.raw_tx_forwarder().as_ref() {
            tracing::debug!( target: "rpc::eth",  "forwarding raw transaction to");
            let _ = client.forward_raw_transaction(&tx).await.inspect_err(|err| {
                    tracing::debug!(target: "rpc::eth", %err, hash=% *pool_transaction.hash(), "failed to forward raw transaction");
                });
        }

        // submit the transaction to the pool with a `Local` origin
        let hash = self
            .pool()
            .add_transaction(TransactionOrigin::Local, pool_transaction)
            .await
            .map_err(Self::Error::from_eth_err)?;

        Ok(hash)
    }
}

impl<N, Eth> LoadTransaction for OpEthApi<N, Eth>
where
    Self: SpawnBlocking,
    N: FullNodeComponents,
{
    type Pool = N::Pool;

    fn provider(&self) -> impl TransactionsProvider {
        self.inner.provider()
    }

    fn cache(&self) -> &EthStateCache {
        self.inner.cache()
    }

    fn pool(&self) -> &Self::Pool {
        self.inner.pool()
    }
}

impl<N, Eth> OpEthApi<N, Eth>
where
    N: FullNodeComponents,
{
    /// Sets a [`SequencerClient`] for `eth_sendRawTransaction` to forward transactions to.
    pub fn set_sequencer_client(
        &self,
        sequencer_client: SequencerClient,
    ) -> Result<(), tokio::sync::SetError<SequencerClient>> {
        self.sequencer_client.set(sequencer_client)
    }

    /// Returns the [`SequencerClient`] if one is set.
    pub fn raw_tx_forwarder(&self) -> Option<SequencerClient> {
        self.sequencer_client.get().cloned()
    }
}

/// Builds OP transaction response type.
#[derive(Debug, Clone, Copy)]
pub struct OpTxBuilder<Eth> {
    _l1_builders: PhantomData<Eth>,
}

impl<Eth: TransactionCompat<Transaction = <Optimism as Network>::TransactionResponse>>
    TransactionCompat for OpTxBuilder<Eth>
{
    type Transaction = <Optimism as Network>::TransactionResponse;

    fn fill(tx: TransactionSignedEcRecovered, tx_info: TransactionInfo) -> Self::Transaction {
        let signed_tx = tx.clone().into_signed();

        let mut resp = Eth::fill(tx, tx_info);

        resp.other = OptimismTransactionFields {
            source_hash: signed_tx.source_hash(),
            mint: signed_tx.mint(),
            is_system_tx: signed_tx.is_deposit().then_some(signed_tx.is_system_transaction()),
        }
        .into();

        resp
    }
}
