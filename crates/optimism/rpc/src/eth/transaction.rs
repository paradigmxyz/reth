//! Loads and formats OP transaction RPC response.

use crate::{OpEthApi, OpEthApiError, SequencerClient};
use alloy_consensus::transaction::TransactionMeta;
use alloy_consensus::BlockHeader;
use alloy_primitives::{Bytes, B256};
use alloy_rpc_types_eth::TransactionInfo;
use op_alloy_consensus::{transaction::OpTransactionInfo, OpTransaction};
use reth_optimism_primitives::DepositReceipt;
use reth_primitives_traits::SignedTransaction;
use reth_rpc_eth_api::FromEvmError;
use reth_rpc_eth_api::RpcNodeCore;
use reth_rpc_eth_api::{
    helpers::{spec::SignersForRpc, EthTransactions, LoadReceipt, LoadTransaction},
    try_into_op_tx_info, FromEthApiError, RpcConvert, RpcReceipt, TxInfoMapper,
};
use reth_rpc_eth_types::utils::recover_raw_transaction;
use reth_rpc_eth_types::EthApiError;
use reth_storage_api::TransactionsProvider;
use reth_storage_api::{errors::ProviderError, ReceiptProvider};
use reth_transaction_pool::{
    AddedTransactionOutcome, PoolTransaction, TransactionOrigin, TransactionPool,
};
use std::fmt::{Debug, Formatter};
use std::future::Future;

impl<N, Rpc> EthTransactions for OpEthApi<N, Rpc>
where
    N: RpcNodeCore,
    OpEthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = OpEthApiError>,
{
    fn signers(&self) -> &SignersForRpc<Self::Provider, Self::NetworkTypes> {
        self.inner.eth_api.signers()
    }

    /// Decodes and recovers the transaction and submits it to the pool.
    ///
    /// Returns the hash of the transaction.
    async fn send_raw_transaction(&self, tx: Bytes) -> Result<B256, Self::Error> {
        let recovered = recover_raw_transaction(&tx)?;

        // broadcast raw transaction to subscribers if there is any.
        self.eth_api().broadcast_raw_transaction(tx.clone());

        let pool_transaction = <Self::Pool as TransactionPool>::Transaction::from_pooled(recovered);

        // On optimism, transactions are forwarded directly to the sequencer to be included in
        // blocks that it builds.
        if let Some(client) = self.raw_tx_forwarder().as_ref() {
            tracing::debug!(target: "rpc::eth", hash = %pool_transaction.hash(), "forwarding raw transaction to sequencer");
            let hash = client.forward_raw_transaction(&tx).await.inspect_err(|err| {
                    tracing::debug!(target: "rpc::eth", %err, hash=% *pool_transaction.hash(), "failed to forward raw transaction");
                })?;

            // Retain tx in local tx pool after forwarding, for local RPC usage.
            let _ = self.inner.eth_api.add_pool_transaction(pool_transaction).await.inspect_err(|err| {
                tracing::warn!(target: "rpc::eth", %err, %hash, "successfully sent tx to sequencer, but failed to persist in local tx pool");
            });

            return Ok(hash);
        }

        // submit the transaction to the pool with a `Local` origin
        let AddedTransactionOutcome { hash, .. } = self
            .pool()
            .add_transaction(TransactionOrigin::Local, pool_transaction)
            .await
            .map_err(Self::Error::from_eth_err)?;

        Ok(hash)
    }

    /// Returns the transaction receipt for the given hash.
    ///
    /// With flashblocks, we should also lookup the pending block for the transaction
    /// because this is considered confirmed/mined.
    fn transaction_receipt(
        &self,
        hash: B256,
    ) -> impl Future<Output = Result<Option<RpcReceipt<Self::NetworkTypes>>, Self::Error>> + Send
    where
        Self: LoadReceipt + 'static,
    {
        let this = self.clone();
        async move {
            match this.load_transaction_and_receipt(hash).await? {
                Some((tx, meta, receipt)) => {
                    return this.build_transaction_receipt(tx, meta, receipt).await.map(Some)
                }
                None => {
                    if let Ok(Some(pending_block)) = this.pending_flashblock() {
                        let block = pending_block.0;
                        let receipts = pending_block.1;

                        if let Some((index, (signer, tx))) = block
                            .transactions_with_sender()
                            .enumerate()
                            .find(|(_, (_, tx))| *tx.tx_hash() == hash)
                        {
                            // Get the corresponding receipt
                            if let Some(receipt) = receipts.get(index) {
                                // Create TransactionMeta for the pending transaction
                                let meta = TransactionMeta {
                                    tx_hash: hash,
                                    index: index as u64,
                                    block_hash: block.hash(),
                                    block_number: block.number(),
                                    base_fee: block.base_fee_per_gas(),
                                    excess_blob_gas: block.excess_blob_gas(),
                                    timestamp: block.timestamp(),
                                };

                                // Convert the transaction to the expected type
                                let provider_tx = this
                                    .provider()
                                    .transaction_by_hash(hash)
                                    .map_err(Self::Error::from_eth_err)?
                                    .ok_or(EthApiError::TransactionNotFound)?;

                                if let Some((_, _, provider_receipt)) =
                                    this.load_transaction_and_receipt(hash).await?
                                {
                                    let rpc_receipt = this
                                        .build_transaction_receipt(
                                            provider_tx,
                                            meta,
                                            provider_receipt,
                                        )
                                        .await?;
                                    return Ok(Some(rpc_receipt));
                                }
                            }
                        };
                    }

                    Ok(None)
                }
            }
        }
    }
}

impl<N, Rpc> LoadTransaction for OpEthApi<N, Rpc>
where
    N: RpcNodeCore,
    OpEthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = OpEthApiError>,
{
}

impl<N, Rpc> OpEthApi<N, Rpc>
where
    N: RpcNodeCore,
    Rpc: RpcConvert<Primitives = N::Primitives>,
{
    /// Returns the [`SequencerClient`] if one is set.
    pub fn raw_tx_forwarder(&self) -> Option<SequencerClient> {
        self.inner.sequencer_client.clone()
    }
}

/// Optimism implementation of [`TxInfoMapper`].
///
/// For deposits, receipt is fetched to extract `deposit_nonce` and `deposit_receipt_version`.
/// Otherwise, it works like regular Ethereum implementation, i.e. uses [`TransactionInfo`].
pub struct OpTxInfoMapper<Provider> {
    provider: Provider,
}

impl<Provider: Clone> Clone for OpTxInfoMapper<Provider> {
    fn clone(&self) -> Self {
        Self { provider: self.provider.clone() }
    }
}

impl<Provider> Debug for OpTxInfoMapper<Provider> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpTxInfoMapper").finish()
    }
}

impl<Provider> OpTxInfoMapper<Provider> {
    /// Creates [`OpTxInfoMapper`] that uses [`ReceiptProvider`] borrowed from given `eth_api`.
    pub const fn new(provider: Provider) -> Self {
        Self { provider }
    }
}

impl<T, Provider> TxInfoMapper<T> for OpTxInfoMapper<Provider>
where
    T: OpTransaction + SignedTransaction,
    Provider: ReceiptProvider<Receipt: DepositReceipt>,
{
    type Out = OpTransactionInfo;
    type Err = ProviderError;

    fn try_map(&self, tx: &T, tx_info: TransactionInfo) -> Result<Self::Out, ProviderError> {
        try_into_op_tx_info(&self.provider, tx, tx_info)
    }
}
