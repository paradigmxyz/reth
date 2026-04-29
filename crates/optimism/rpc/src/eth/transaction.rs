//! Loads and formats OP transaction RPC response.

use crate::{OpEthApi, OpEthApiError, SequencerClient};
use alloy_consensus::Transaction;
use alloy_primitives::{Bytes, B256};
use alloy_rpc_types_eth::TransactionInfo;
use futures::StreamExt;
use op_alloy_consensus::{transaction::OpTransactionInfo, OpTransaction};
use reth_chain_state::CanonStateSubscriptions;
use reth_mantle_forks::is_mantle_meta_tx;
use reth_optimism_primitives::DepositReceipt;
use reth_primitives_traits::{BlockBody, SignedTransaction};
use reth_rpc_eth_api::{
    helpers::{spec::SignersForRpc, EthTransactions, LoadReceipt, LoadTransaction},
    try_into_op_tx_info, EthApiTypes as _, FromEthApiError, FromEvmError, RpcConvert, RpcNodeCore,
    RpcReceipt, TxInfoMapper,
};
use reth_rpc_eth_types::{utils::recover_raw_transaction, EthApiError};
use reth_storage_api::{errors::ProviderError, ReceiptProvider};
use reth_transaction_pool::{
    AddedTransactionOutcome, PoolTransaction, TransactionOrigin, TransactionPool,
};
use std::{
    fmt::{Debug, Formatter},
    future::Future,
    time::Duration,
};
use tokio_stream::wrappers::WatchStream;

pub(crate) fn ensure_transaction_input_supported(input: &[u8]) -> Result<(), EthApiError> {
    if is_mantle_meta_tx(input) {
        return Err(EthApiError::other(jsonrpsee_types::error::ErrorObject::owned(
            -32000,
            "meta tx is disabled",
            None::<()>,
        )));
    }

    Ok(())
}

impl<N, Rpc> EthTransactions for OpEthApi<N, Rpc>
where
    N: RpcNodeCore,
    OpEthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = OpEthApiError>,
{
    fn signers(&self) -> &SignersForRpc<Self::Provider, Self::NetworkTypes> {
        self.inner.eth_api.signers()
    }

    fn send_raw_transaction_sync_timeout(&self) -> Duration {
        self.inner.eth_api.send_raw_transaction_sync_timeout()
    }

    /// Decodes and recovers the transaction and submits it to the pool.
    ///
    /// Returns the hash of the transaction.
    async fn send_raw_transaction(&self, tx: Bytes) -> Result<B256, Self::Error> {
        let recovered = recover_raw_transaction(&tx)?;
        let pool_transaction = <Self::Pool as TransactionPool>::Transaction::from_pooled(recovered);
        ensure_transaction_input_supported(pool_transaction.input())
            .map_err(Self::Error::from_eth_err)?;

        // broadcast raw transaction to subscribers if there is any.
        self.eth_api().broadcast_raw_transaction(tx.clone());

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

    /// Decodes and recovers the transaction and submits it to the pool.
    ///
    /// And awaits the receipt, checking both canonical blocks and flashblocks for faster
    /// confirmation.
    fn send_raw_transaction_sync(
        &self,
        tx: Bytes,
    ) -> impl Future<Output = Result<RpcReceipt<Self::NetworkTypes>, Self::Error>> + Send {
        let this = self.clone();
        let timeout_duration = self.send_raw_transaction_sync_timeout();
        async move {
            let mut canonical_stream = this.provider().canonical_state_stream();
            let hash = EthTransactions::send_raw_transaction(&this, tx).await?;
            let mut flashblock_stream = this.pending_block_rx().map(WatchStream::new);

            tokio::time::timeout(timeout_duration, async {
                loop {
                    tokio::select! {
                        biased;
                        // check if the tx was preconfirmed in a new flashblock
                        flashblock = async {
                            if let Some(stream) = &mut flashblock_stream {
                                stream.next().await
                            } else {
                                futures::future::pending().await
                            }
                        } => {
                            if let Some(flashblock) = flashblock.flatten() {
                                // if flashblocks are supported, attempt to find id from the pending block
                                if let Some(receipt) = flashblock
                                .find_and_convert_transaction_receipt(hash, this.tx_resp_builder())
                                {
                                    return receipt;
                                }
                            }
                        }
                        // Listen for regular canonical block updates for inclusion
                        canonical_notification = canonical_stream.next() => {
                            if let Some(notification) = canonical_notification {
                                let chain = notification.committed();
                                for block in chain.blocks_iter() {
                                    if block.body().contains_transaction(&hash)
                                        && let Some(receipt) = this.transaction_receipt(hash).await? {
                                            return Ok(receipt);
                                        }
                                }
                            } else {
                                // Canonical stream ended
                                break;
                            }
                        }
                    }
                }
                Err(Self::Error::from_eth_err(EthApiError::TransactionConfirmationTimeout {
                    hash,
                    duration: timeout_duration,
                }))
            })
            .await
            .unwrap_or_else(|_elapsed| {
                Err(Self::Error::from_eth_err(EthApiError::TransactionConfirmationTimeout {
                    hash,
                    duration: timeout_duration,
                }))
            })
        }
    }

    /// Returns the transaction receipt for the given hash.
    ///
    /// With flashblocks, we should also lookup the pending block for the transaction
    /// because this is considered confirmed/mined.
    fn transaction_receipt(
        &self,
        hash: B256,
    ) -> impl Future<Output = Result<Option<RpcReceipt<Self::NetworkTypes>>, Self::Error>> + Send
    {
        let this = self.clone();
        async move {
            // first attempt to fetch the mined transaction receipt data
            let tx_receipt = this.load_transaction_and_receipt(hash).await?;

            if tx_receipt.is_none() {
                // if flashblocks are supported, attempt to find id from the pending block
                if let Ok(Some(pending_block)) = this.pending_flashblock().await &&
                    let Some(Ok(receipt)) = pending_block
                        .find_and_convert_transaction_receipt(hash, this.tx_resp_builder())
                {
                    return Ok(Some(receipt));
                }
            }
            let Some((tx, meta, receipt)) = tx_receipt else { return Ok(None) };
            self.build_transaction_receipt(tx, meta, receipt).await.map(Some)
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

#[cfg(test)]
mod tests {
    use super::ensure_transaction_input_supported;
    use alloy_primitives::Bytes;
    use jsonrpsee_types::error::ErrorObject;
    use reth_mantle_forks::MANTLE_META_TX_PREFIX;

    #[test]
    fn rpc_transaction_input_rejects_mantle_meta_tx_before_forwarding() {
        let mut input = MANTLE_META_TX_PREFIX.to_vec();
        input.push(0xF8);

        let err = ensure_transaction_input_supported(&Bytes::from(input)).unwrap_err();
        let rpc_error: ErrorObject<'static> = err.into();

        assert_eq!(rpc_error.code(), -32000);
        assert_eq!(rpc_error.message(), "meta tx is disabled");
    }

    #[test]
    fn rpc_transaction_input_allows_non_meta_tx_boundaries() {
        assert!(ensure_transaction_input_supported(&Bytes::default()).is_ok());
        assert!(ensure_transaction_input_supported(&Bytes::from(MANTLE_META_TX_PREFIX.to_vec()))
            .is_ok());

        let mut wrong_prefix = MANTLE_META_TX_PREFIX;
        wrong_prefix[31] ^= 0x01;
        let mut wrong_input = wrong_prefix.to_vec();
        wrong_input.push(0xF8);
        assert!(ensure_transaction_input_supported(&Bytes::from(wrong_input)).is_ok());
    }
}
