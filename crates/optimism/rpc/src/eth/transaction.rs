//! Loads and formats OP transaction RPC response.

use alloy_consensus::{transaction::Recovered, SignableTransaction};
use alloy_primitives::{Bytes, Signature, B256};
use alloy_rpc_types_eth::TransactionInfo;
use futures::StreamExt;
use op_alloy_consensus::{
    transaction::{OpDepositInfo, OpTransactionInfo},
    OpTxEnvelope,
};
use op_alloy_rpc_types::{OpTransactionRequest, Transaction};
use reth_chain_state::CanonStateSubscriptions;
use reth_node_api::{FullNodeComponents, FullNodeTypes, NodeTypes};
use reth_optimism_primitives::DepositReceipt;
use reth_primitives_traits::{BlockBody, NodePrimitives, SignedTransaction, TxTy};
use reth_rpc_eth_api::{
    helpers::{EthSigner, EthTransactions, LoadTransaction, SpawnBlocking},
    EthApiTypes, FromEthApiError, FullEthApiTypes, RpcNodeCore, RpcNodeCoreExt, RpcReceipt,
    TransactionCompat,
};
use reth_rpc_eth_types::{utils::recover_raw_transaction, EthApiError};
use reth_storage_api::{
    BlockReader, BlockReaderIdExt, ProviderTx, ReceiptProvider, TransactionsProvider,
};
use reth_transaction_pool::{PoolTransaction, TransactionOrigin, TransactionPool};

use crate::{eth::OpNodeCore, OpEthApi, OpEthApiError, SequencerClient};

impl<N> EthTransactions for OpEthApi<N>
where
    Self: LoadTransaction<Provider: BlockReaderIdExt>
        + reth_rpc_eth_api::helpers::LoadReceipt
        + EthApiTypes<Error = OpEthApiError>,
    N: OpNodeCore<Provider: BlockReader<Transaction = ProviderTx<Self::Provider>>>,
    Self::Provider: CanonStateSubscriptions + reth_chainspec::ChainSpecProvider,
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

        // On optimism, transactions are forwarded directly to the sequencer to be included in
        // blocks that it builds.
        if let Some(client) = self.raw_tx_forwarder().as_ref() {
            tracing::debug!(target: "rpc::eth", hash = %pool_transaction.hash(), "forwarding raw transaction to sequencer");
            let hash = client.forward_raw_transaction(&tx).await.inspect_err(|err| {
                    tracing::debug!(target: "rpc::eth", %err, hash=% *pool_transaction.hash(), "failed to forward raw transaction");
                })?;

            // Retain tx in local tx pool after forwarding, for local RPC usage.
            let _ = self
                .pool()
                .add_transaction(TransactionOrigin::Local, pool_transaction)
                .await.inspect_err(|err| {
                    tracing::warn!(target: "rpc::eth", %err, %hash, "successfully sent tx to sequencer, but failed to persist in local tx pool");
            });

            return Ok(hash)
        }

        // submit the transaction to the pool with a `Local` origin
        let hash = self
            .pool()
            .add_transaction(TransactionOrigin::Local, pool_transaction)
            .await
            .map_err(Self::Error::from_eth_err)?;

        Ok(hash)
    }

    async fn send_raw_transaction_sync(
        &self,
        tx: Bytes,
    ) -> Result<Option<RpcReceipt<Self::NetworkTypes>>, Self::Error> {
        let hash = self.send_raw_transaction(tx).await?;
        let mut stream = self.provider().canonical_state_stream();
        match tokio::time::timeout(tokio::time::Duration::from_secs(30), async {
            while let Some(notification) = stream.next().await {
                let chain = notification.committed();
                for block in chain.blocks_iter() {
                    if block.body().contains_transaction(&hash) {
                        if let Some(receipt) = self.transaction_receipt(hash).await? {
                            return Ok(Some(receipt));
                        }
                    }
                }
            }
            Ok(None) // Stream ended, no transaction found
        })
        .await
        {
            Ok(result) => result,
            Err(_elapsed) => Ok(None), // Timeout reached, no transaction found
        }
    }
}

impl<N> LoadTransaction for OpEthApi<N>
where
    Self: SpawnBlocking + FullEthApiTypes + RpcNodeCoreExt,
    N: OpNodeCore<Provider: TransactionsProvider, Pool: TransactionPool>,
    Self::Pool: TransactionPool,
{
}

impl<N> OpEthApi<N>
where
    N: OpNodeCore,
{
    /// Returns the [`SequencerClient`] if one is set.
    pub fn raw_tx_forwarder(&self) -> Option<SequencerClient> {
        self.inner.sequencer_client.clone()
    }
}

impl<N> TransactionCompat for OpEthApi<N>
where
    N: FullNodeComponents,
    N::Provider: ReceiptProvider<Receipt: DepositReceipt>,
    <<N as FullNodeTypes>::Types as NodeTypes>::Primitives: NodePrimitives<SignedTx = OpTxEnvelope>,
{
    type Primitives = <<N as FullNodeTypes>::Types as NodeTypes>::Primitives;
    type Transaction = Transaction;
    type Error = OpEthApiError;

    fn fill(
        &self,
        tx: Recovered<TxTy<Self::Primitives>>,
        tx_info: TransactionInfo,
    ) -> Result<Self::Transaction, Self::Error> {
        let tx = tx.convert::<TxTy<Self::Primitives>>();
        let mut deposit_receipt_version = None;
        let mut deposit_nonce = None;

        if tx.is_deposit() {
            // for depost tx we need to fetch the receipt
            self.inner
                .eth_api
                .provider()
                .receipt_by_hash(tx.tx_hash())
                .map_err(Self::Error::from_eth_err)?
                .inspect(|receipt| {
                    if let Some(receipt) = receipt.as_deposit_receipt() {
                        deposit_receipt_version = receipt.deposit_receipt_version;
                        deposit_nonce = receipt.deposit_nonce;
                    }
                });
        }

        let tx_info = OpTransactionInfo::new(
            tx_info,
            OpDepositInfo { deposit_nonce, deposit_receipt_version },
        );

        Ok(Transaction::from_transaction(tx, tx_info))
    }

    fn build_simulate_v1_transaction(
        &self,
        request: alloy_rpc_types_eth::TransactionRequest,
    ) -> Result<TxTy<Self::Primitives>, Self::Error> {
        let request: OpTransactionRequest = request.into();
        let Ok(tx) = request.build_typed_tx() else {
            return Err(OpEthApiError::Eth(EthApiError::TransactionConversionError))
        };

        // Create an empty signature for the transaction.
        let signature = Signature::new(Default::default(), Default::default(), false);
        Ok(tx.into_signed(signature).into())
    }
}
