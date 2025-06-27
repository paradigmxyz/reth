//! Loads and formats Scroll transaction RPC response.

use crate::{
    eth::{ScrollEthApiInner, ScrollNodeCore},
    ScrollEthApi, SequencerClient,
};
use alloy_consensus::transaction::TransactionInfo;
use alloy_primitives::{Bytes, B256};
use reth_evm::execute::ProviderError;
use reth_node_api::FullNodeComponents;
use reth_provider::{
    BlockReader, BlockReaderIdExt, ProviderTx, ReceiptProvider, TransactionsProvider,
};
use reth_rpc_eth_api::{
    helpers::{EthSigner, EthTransactions, LoadTransaction, SpawnBlocking},
    try_into_scroll_tx_info, FromEthApiError, FullEthApiTypes, RpcNodeCore, RpcNodeCoreExt,
    TxInfoMapper,
};
use reth_rpc_eth_types::utils::recover_raw_transaction;
use reth_scroll_primitives::ScrollReceipt;
use reth_transaction_pool::{PoolTransaction, TransactionOrigin, TransactionPool};
use scroll_alloy_consensus::{ScrollTransactionInfo, ScrollTxEnvelope};
use std::{
    fmt::{Debug, Formatter},
    sync::Arc,
};

impl<N> EthTransactions for ScrollEthApi<N>
where
    Self: LoadTransaction<Provider: BlockReaderIdExt>,
    N: ScrollNodeCore<Provider: BlockReader<Transaction = ProviderTx<Self::Provider>>>,
{
    fn signers(&self) -> &parking_lot::RwLock<Vec<Box<dyn EthSigner<ProviderTx<Self::Provider>>>>> {
        self.inner.eth_api.signers()
    }

    /// Decodes and recovers the transaction and submits it to the pool.
    ///
    /// Returns the hash of the transaction.
    async fn send_raw_transaction(&self, tx: Bytes) -> Result<B256, Self::Error> {
        let recovered = recover_raw_transaction(&tx)?;
        let pool_transaction = <Self::Pool as TransactionPool>::Transaction::from_pooled(recovered);

        // On scroll, transactions are forwarded directly to the sequencer to be included in
        // blocks that it builds.
        if let Some(client) = self.raw_tx_forwarder().as_ref() {
            tracing::debug!(target: "rpc::eth", hash = %pool_transaction.hash(), "forwarding raw transaction to sequencer");
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

impl<N> LoadTransaction for ScrollEthApi<N>
where
    Self: SpawnBlocking + FullEthApiTypes + RpcNodeCoreExt,
    N: ScrollNodeCore<Provider: TransactionsProvider, Pool: TransactionPool>,
    Self::Pool: TransactionPool,
{
}

impl<N> ScrollEthApi<N>
where
    N: ScrollNodeCore,
{
    /// Returns the [`SequencerClient`] if one is set.
    pub fn raw_tx_forwarder(&self) -> Option<SequencerClient> {
        self.inner.sequencer_client.clone()
    }
}

/// Scroll implementation of [`TxInfoMapper`].
///
/// Receipt is fetched to extract the `l1_fee` for all transactions but L1 messages.
#[derive(Clone)]
pub struct ScrollTxInfoMapper<N: ScrollNodeCore>(Arc<ScrollEthApiInner<N>>);

impl<N: ScrollNodeCore> Debug for ScrollTxInfoMapper<N> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScrollTxInfoMapper").finish()
    }
}

impl<N: ScrollNodeCore> ScrollTxInfoMapper<N> {
    /// Creates [`ScrollTxInfoMapper`] that uses [`ReceiptProvider`] borrowed from given `eth_api`.
    pub const fn new(eth_api: Arc<ScrollEthApiInner<N>>) -> Self {
        Self(eth_api)
    }
}

impl<N> TxInfoMapper<&ScrollTxEnvelope> for ScrollTxInfoMapper<N>
where
    N: FullNodeComponents,
    N::Provider: ReceiptProvider<Receipt = ScrollReceipt>,
{
    type Out = ScrollTransactionInfo;
    type Err = ProviderError;

    fn try_map(
        &self,
        tx: &ScrollTxEnvelope,
        tx_info: TransactionInfo,
    ) -> Result<Self::Out, ProviderError> {
        try_into_scroll_tx_info(self.0.eth_api.provider(), tx, tx_info)
    }
}
