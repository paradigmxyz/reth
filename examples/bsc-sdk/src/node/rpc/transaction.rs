use super::BscNodeCore;
use crate::node::rpc::BscEthApi;
use alloy_consensus::{Transaction, TxEnvelope};
use alloy_network::{Ethereum, Network};
use alloy_primitives::{Bytes, Signature, B256};
use reth::{
    builder::FullNodeComponents,
    primitives::{Receipt, Recovered, TransactionSigned},
    providers::ReceiptProvider,
    rpc::{
        server_types::eth::{utils::recover_raw_transaction, EthApiError},
        types::{TransactionInfo, TransactionRequest},
    },
    transaction_pool::{PoolTransaction, TransactionOrigin, TransactionPool},
};
use reth_provider::{BlockReader, BlockReaderIdExt, ProviderTx, TransactionsProvider};
use reth_rpc_eth_api::{
    helpers::{EthSigner, EthTransactions, LoadTransaction, SpawnBlocking},
    FromEthApiError, FullEthApiTypes, RpcNodeCore, RpcNodeCoreExt, TransactionCompat,
};
impl<N> LoadTransaction for BscEthApi<N>
where
    Self: SpawnBlocking + FullEthApiTypes + RpcNodeCoreExt,
    N: BscNodeCore<Provider: TransactionsProvider, Pool: TransactionPool>,
    Self::Pool: TransactionPool,
{
}

impl<N> TransactionCompat<TransactionSigned> for BscEthApi<N>
where
    N: FullNodeComponents<Provider: ReceiptProvider<Receipt = Receipt>>,
{
    type Transaction = <Ethereum as Network>::TransactionResponse;

    type Error = EthApiError;

    fn fill(
        &self,
        tx: Recovered<TransactionSigned>,
        tx_info: TransactionInfo,
    ) -> Result<Self::Transaction, Self::Error> {
        let tx = tx.convert::<TxEnvelope>();

        let TransactionInfo {
            block_hash, block_number, index: transaction_index, base_fee, ..
        } = tx_info;

        let effective_gas_price = base_fee
            .map(|base_fee| {
                tx.effective_tip_per_gas(base_fee).unwrap_or_default() + base_fee as u128
            })
            .unwrap_or_else(|| tx.max_fee_per_gas());

        Ok(alloy_rpc_types::Transaction {
            inner: tx,
            block_hash,
            block_number,
            transaction_index,
            effective_gas_price: Some(effective_gas_price),
        })
    }

    fn build_simulate_v1_transaction(
        &self,
        request: TransactionRequest,
    ) -> Result<TransactionSigned, Self::Error> {
        let Ok(tx) = request.build_typed_tx() else {
            return Err(EthApiError::TransactionConversionError)
        };

        // Create an empty signature for the transaction.
        let signature = Signature::new(Default::default(), Default::default(), false);
        Ok(TransactionSigned::new_unhashed(tx.into(), signature))
    }

    fn otterscan_api_truncate_input(tx: &mut Self::Transaction) {
        let input = tx.inner.inner_mut().input_mut();
        *input = input.slice(..4);
    }
}

impl<N> EthTransactions for BscEthApi<N>
where
    Self: LoadTransaction<Provider: BlockReaderIdExt>,
    N: BscNodeCore<Provider: BlockReader<Transaction = ProviderTx<Self::Provider>>>,
{
    fn signers(&self) -> &parking_lot::RwLock<Vec<Box<dyn EthSigner<ProviderTx<Self::Provider>>>>> {
        self.inner.eth_api.signers()
    }

    /// Decodes and recovers the transaction and submits it to the pool.
    ///
    /// Returns the hash of the transaction.
    async fn send_raw_transaction(&self, tx: Bytes) -> Result<B256, Self::Error> {
        let recovered = recover_raw_transaction(&tx)?;

        // broadcast raw transaction to subscribers if there is any.
        self.inner.eth_api.broadcast_raw_transaction(tx);

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
