//! Loads and formats OP transaction RPC response.

use alloy_consensus::{Signed, Transaction as _};
use alloy_primitives::{Bytes, PrimitiveSignature as Signature, Sealable, Sealed, B256};
use alloy_rpc_types_eth::TransactionInfo;
use op_alloy_consensus::{OpTxEnvelope, OpTypedTransaction};
use op_alloy_rpc_types::{OpTransactionRequest, Transaction};
use reth_node_api::FullNodeComponents;
use reth_optimism_primitives::{OpReceipt, OpTransactionSigned};
use reth_primitives::Recovered;
use reth_primitives_traits::transaction::signed::SignedTransaction;
use reth_provider::{
    BlockReader, BlockReaderIdExt, ProviderTx, ReceiptProvider, TransactionsProvider,
};
use reth_rpc_eth_api::{
    helpers::{EthSigner, EthTransactions, LoadTransaction, SpawnBlocking},
    FromEthApiError, FullEthApiTypes, RpcNodeCore, RpcNodeCoreExt, TransactionCompat,
};
use reth_rpc_eth_types::{utils::recover_raw_transaction, EthApiError};
use reth_transaction_pool::{PoolTransaction, TransactionOrigin, TransactionPool};

use crate::{eth::OpNodeCore, OpEthApi, OpEthApiError, SequencerClient};

impl<N> EthTransactions for OpEthApi<N>
where
    Self: LoadTransaction<Provider: BlockReaderIdExt>,
    N: OpNodeCore<Provider: BlockReader<Transaction = ProviderTx<Self::Provider>>>,
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

impl<N> TransactionCompat<OpTransactionSigned> for OpEthApi<N>
where
    N: FullNodeComponents<Provider: ReceiptProvider<Receipt = OpReceipt>>,
{
    type Transaction = Transaction;
    type Error = OpEthApiError;

    fn fill(
        &self,
        tx: Recovered<OpTransactionSigned>,
        tx_info: TransactionInfo,
    ) -> Result<Self::Transaction, Self::Error> {
        let from = tx.signer();
        let hash = *tx.tx_hash();
        let OpTransactionSigned { transaction, signature, .. } = tx.into_tx();
        let mut deposit_receipt_version = None;
        let mut deposit_nonce = None;

        let inner = match transaction {
            OpTypedTransaction::Legacy(tx) => Signed::new_unchecked(tx, signature, hash).into(),
            OpTypedTransaction::Eip2930(tx) => Signed::new_unchecked(tx, signature, hash).into(),
            OpTypedTransaction::Eip1559(tx) => Signed::new_unchecked(tx, signature, hash).into(),
            OpTypedTransaction::Eip7702(tx) => Signed::new_unchecked(tx, signature, hash).into(),
            OpTypedTransaction::Deposit(tx) => {
                self.inner
                    .eth_api
                    .provider()
                    .receipt_by_hash(hash)
                    .map_err(Self::Error::from_eth_err)?
                    .inspect(|receipt| {
                        if let OpReceipt::Deposit(receipt) = receipt {
                            deposit_receipt_version = receipt.deposit_receipt_version;
                            deposit_nonce = receipt.deposit_nonce;
                        }
                    });

                OpTxEnvelope::Deposit(tx.seal_unchecked(hash))
            }
        };

        let TransactionInfo {
            block_hash, block_number, index: transaction_index, base_fee, ..
        } = tx_info;

        let effective_gas_price = if matches!(inner, OpTxEnvelope::Deposit(_)) {
            // For deposits, we must always set the `gasPrice` field to 0 in rpc
            // deposit tx don't have a gas price field, but serde of `Transaction` will take care of
            // it
            0
        } else {
            base_fee
                .map(|base_fee| {
                    inner.effective_tip_per_gas(base_fee as u64).unwrap_or_default() + base_fee
                })
                .unwrap_or_else(|| inner.max_fee_per_gas())
        };

        Ok(Transaction {
            inner: alloy_rpc_types_eth::Transaction {
                inner,
                block_hash,
                block_number,
                transaction_index,
                from,
                effective_gas_price: Some(effective_gas_price),
            },
            deposit_nonce,
            deposit_receipt_version,
        })
    }

    fn build_simulate_v1_transaction(
        &self,
        request: alloy_rpc_types_eth::TransactionRequest,
    ) -> Result<OpTransactionSigned, Self::Error> {
        let request: OpTransactionRequest = request.into();
        let Ok(tx) = request.build_typed_tx() else {
            return Err(OpEthApiError::Eth(EthApiError::TransactionConversionError))
        };

        // Create an empty signature for the transaction.
        let signature = Signature::new(Default::default(), Default::default(), false);
        Ok(OpTransactionSigned::new_unhashed(tx, signature))
    }

    fn otterscan_api_truncate_input(tx: &mut Self::Transaction) {
        let input = match &mut tx.inner.inner {
            OpTxEnvelope::Eip1559(tx) => &mut tx.tx_mut().input,
            OpTxEnvelope::Eip2930(tx) => &mut tx.tx_mut().input,
            OpTxEnvelope::Legacy(tx) => &mut tx.tx_mut().input,
            OpTxEnvelope::Eip7702(tx) => &mut tx.tx_mut().input,
            OpTxEnvelope::Deposit(tx) => {
                let (mut deposit, hash) = std::mem::replace(
                    tx,
                    Sealed::new_unchecked(Default::default(), Default::default()),
                )
                .split();
                deposit.input = deposit.input.slice(..4);
                let mut deposit = deposit.seal_unchecked(hash);
                std::mem::swap(tx, &mut deposit);
                return
            }
            _ => return,
        };
        *input = input.slice(..4);
    }
}
