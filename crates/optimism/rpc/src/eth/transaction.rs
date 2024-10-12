//! Loads and formats OP transaction RPC response.

use alloy_primitives::{Bytes, B256};

use reth_evm_optimism::RethL1BlockInfo;
use reth_node_api::FullNodeComponents;
use reth_primitives::TransactionSigned;
use reth_provider::{BlockReaderIdExt, TransactionsProvider};
use reth_rpc_eth_api::{
    helpers::{EthApiSpec, EthSigner, EthTransactions, LoadTransaction, SpawnBlocking},
    EthApiTypes, FromEthApiError,
};
use reth_rpc_eth_types::{utils::recover_raw_transaction, EthStateCache};
use reth_transaction_pool::{PoolTransaction, TransactionOrigin, TransactionPool};
use revm::L1BlockInfo;

use crate::{eth::rpc::SequencerClient, OpEthApi, OpEthApiError};

impl<N> EthTransactions for OpEthApi<N>
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

impl<N> LoadTransaction for OpEthApi<N>
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

/// L1 fee and data gas for a transaction, along with the L1 block info.
#[derive(Debug, Default, Clone)]
pub struct OptimismTxMeta {
    /// The L1 block info.
    pub l1_block_info: Option<L1BlockInfo>,
    /// The L1 fee for the block.
    pub l1_fee: Option<u128>,
    /// The L1 data gas for the block.
    pub l1_data_gas: Option<u128>,
}

impl OptimismTxMeta {
    /// Creates a new [`OptimismTxMeta`].
    pub const fn new(
        l1_block_info: Option<L1BlockInfo>,
        l1_fee: Option<u128>,
        l1_data_gas: Option<u128>,
    ) -> Self {
        Self { l1_block_info, l1_fee, l1_data_gas }
    }
}

impl<N> OpEthApi<N>
where
    Self: EthApiSpec + LoadTransaction,
    <Self as EthApiTypes>::Error: From<OpEthApiError>,
    N: FullNodeComponents,
{
    /// Builds [`OptimismTxMeta`] object using the provided [`TransactionSigned`], L1 block
    /// info and block timestamp. The [`L1BlockInfo`] is used to calculate the l1 fee and l1 data
    /// gas for the transaction. If the [`L1BlockInfo`] is not provided, the meta info will be
    /// empty.
    pub fn build_op_tx_meta(
        &self,
        tx: &TransactionSigned,
        l1_block_info: Option<L1BlockInfo>,
        block_timestamp: u64,
    ) -> Result<OptimismTxMeta, <Self as EthApiTypes>::Error> {
        let Some(l1_block_info) = l1_block_info else { return Ok(OptimismTxMeta::default()) };

        let (l1_fee, l1_data_gas) = if tx.is_deposit() {
            (None, None)
        } else {
            let envelope_buf = tx.envelope_encoded();

            let inner_l1_fee = l1_block_info
                .l1_tx_data_fee(&self.chain_spec(), block_timestamp, &envelope_buf, tx.is_deposit())
                .map_err(|_| OpEthApiError::L1BlockFeeError)?;
            let inner_l1_data_gas = l1_block_info
                .l1_data_gas(&self.chain_spec(), block_timestamp, &envelope_buf)
                .map_err(|_| OpEthApiError::L1BlockGasError)?;
            (
                Some(inner_l1_fee.saturating_to::<u128>()),
                Some(inner_l1_data_gas.saturating_to::<u128>()),
            )
        };

        Ok(OptimismTxMeta::new(Some(l1_block_info), l1_fee, l1_data_gas))
    }
}

impl<N> OpEthApi<N>
where
    N: FullNodeComponents,
{
    /// Sets a `SequencerClient` for `eth_sendRawTransaction` to forward transactions to.
    pub fn set_sequencer_client(&self, sequencer_client: SequencerClient) {
        *self.sequencer_client.write() = Some(sequencer_client);
    }

    /// Returns the `SequencerClient` if one is set.
    pub fn raw_tx_forwarder(&self) -> Option<SequencerClient> {
        self.sequencer_client.read().clone()
    }
}
