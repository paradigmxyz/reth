//! Loads and formats Scroll transaction RPC response.

use alloy_consensus::{Signed, Transaction as _};
use alloy_primitives::{Bytes, Sealable, Sealed, Signature, B256};
use alloy_rpc_types_eth::TransactionInfo;
use reth_node_api::FullNodeComponents;
use reth_primitives::Recovered;
use reth_primitives_traits::SignedTransaction;
use reth_provider::{
    BlockReader, BlockReaderIdExt, ProviderTx, ReceiptProvider, TransactionsProvider,
};
use reth_rpc_eth_api::{
    helpers::{EthSigner, EthTransactions, LoadTransaction, SpawnBlocking},
    FromEthApiError, FullEthApiTypes, RpcNodeCore, RpcNodeCoreExt, TransactionCompat,
};
use reth_rpc_eth_types::{utils::recover_raw_transaction, EthApiError};
use reth_scroll_primitives::{ScrollReceipt, ScrollTransactionSigned};
use reth_transaction_pool::{PoolTransaction, TransactionOrigin, TransactionPool};

use scroll_alloy_consensus::{ScrollTxEnvelope, ScrollTypedTransaction};
use scroll_alloy_rpc_types::{ScrollTransactionRequest, Transaction};

use crate::{eth::ScrollNodeCore, ScrollEthApi, ScrollEthApiError};

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

impl<N> TransactionCompat<ScrollTransactionSigned> for ScrollEthApi<N>
where
    N: FullNodeComponents<Provider: ReceiptProvider<Receipt = ScrollReceipt>>,
{
    type Transaction = Transaction;
    type Error = ScrollEthApiError;

    fn fill(
        &self,
        tx: Recovered<ScrollTransactionSigned>,
        tx_info: TransactionInfo,
    ) -> Result<Self::Transaction, Self::Error> {
        let from = tx.signer();
        let hash = *tx.tx_hash();
        let ScrollTransactionSigned { transaction, signature, .. } = tx.into_inner();

        let inner = match transaction {
            ScrollTypedTransaction::Legacy(tx) => Signed::new_unchecked(tx, signature, hash).into(),
            ScrollTypedTransaction::Eip2930(tx) => {
                Signed::new_unchecked(tx, signature, hash).into()
            }
            ScrollTypedTransaction::Eip1559(tx) => {
                Signed::new_unchecked(tx, signature, hash).into()
            }
            ScrollTypedTransaction::Eip7702(tx) => {
                Signed::new_unchecked(tx, signature, hash).into()
            }
            ScrollTypedTransaction::L1Message(tx) => {
                ScrollTxEnvelope::L1Message(tx.seal_unchecked(hash))
            }
        };

        let TransactionInfo {
            block_hash, block_number, index: transaction_index, base_fee, ..
        } = tx_info;

        let effective_gas_price = if inner.is_l1_message() {
            // For l1 message, we must always set the `gasPrice` field to 0 in rpc
            // l1 message tx don't have a gas price field, but serde of `Transaction` will take care
            // of it
            0
        } else {
            base_fee
                .map(|base_fee| {
                    inner.effective_tip_per_gas(base_fee).unwrap_or_default() + base_fee as u128
                })
                .unwrap_or_else(|| inner.max_fee_per_gas())
        };
        let inner = Recovered::new_unchecked(inner, from);

        Ok(Transaction {
            inner: alloy_rpc_types_eth::Transaction {
                inner,
                block_hash,
                block_number,
                transaction_index,
                effective_gas_price: Some(effective_gas_price),
            },
        })
    }

    fn build_simulate_v1_transaction(
        &self,
        request: alloy_rpc_types_eth::TransactionRequest,
    ) -> Result<ScrollTransactionSigned, Self::Error> {
        let request: ScrollTransactionRequest = request.into();
        let Ok(tx) = request.build_typed_tx() else {
            return Err(ScrollEthApiError::Eth(EthApiError::TransactionConversionError))
        };

        // Create an empty signature for the transaction.
        let signature = Signature::new(Default::default(), Default::default(), false);
        Ok(ScrollTransactionSigned::new_unhashed(tx, signature))
    }

    fn otterscan_api_truncate_input(tx: &mut Self::Transaction) {
        let mut tx = tx.inner.inner.inner_mut();
        let input = match &mut tx {
            ScrollTxEnvelope::Eip1559(tx) => &mut tx.tx_mut().input,
            ScrollTxEnvelope::Eip2930(tx) => &mut tx.tx_mut().input,
            ScrollTxEnvelope::Legacy(tx) => &mut tx.tx_mut().input,
            ScrollTxEnvelope::L1Message(tx) => {
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
