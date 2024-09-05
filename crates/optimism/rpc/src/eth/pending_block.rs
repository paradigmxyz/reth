//! Loads OP pending block for a RPC response.

use reth_chainspec::ChainSpec;
use reth_evm::ConfigureEvm;
use reth_node_api::{FullNodeComponents, NodeTypes};
use reth_primitives::{
    revm_primitives::BlockEnv, BlockNumber, Receipt, SealedBlockWithSenders, B256,
};
use reth_provider::{
    BlockReader, BlockReaderIdExt, ChainSpecProvider, EvmEnvProvider, ExecutionOutcome,
    ReceiptProvider, StateProviderFactory,
};
use reth_rpc_eth_api::{
    helpers::{LoadPendingBlock, SpawnBlocking},
    FromEthApiError,
};
use reth_rpc_eth_types::{EthApiError, PendingBlock};
use reth_transaction_pool::TransactionPool;

use crate::OpEthApi;

impl<N> LoadPendingBlock for OpEthApi<N>
where
    Self: SpawnBlocking,
    N: FullNodeComponents<Types: NodeTypes<ChainSpec = ChainSpec>>,
{
    #[inline]
    fn provider(
        &self,
    ) -> impl BlockReaderIdExt
           + EvmEnvProvider
           + ChainSpecProvider<ChainSpec = ChainSpec>
           + StateProviderFactory {
        self.inner.provider()
    }

    #[inline]
    fn pool(&self) -> impl TransactionPool {
        self.inner.pool()
    }

    #[inline]
    fn pending_block(&self) -> &tokio::sync::Mutex<Option<PendingBlock>> {
        self.inner.pending_block()
    }

    #[inline]
    fn evm_config(&self) -> &impl ConfigureEvm {
        self.inner.evm_config()
    }

    /// Returns the locally built pending block
    async fn local_pending_block(
        &self,
    ) -> Result<Option<(SealedBlockWithSenders, Vec<Receipt>)>, Self::Error> {
        // See: <https://github.com/ethereum-optimism/op-geth/blob/f2e69450c6eec9c35d56af91389a1c47737206ca/miner/worker.go#L367-L375>
        let latest = self
            .provider()
            .latest_header()
            .map_err(Self::Error::from_eth_err)?
            .ok_or_else(|| EthApiError::UnknownBlockNumber)?;
        let block = self
            .provider()
            .block_with_senders(latest.hash().into(), Default::default())
            .map_err(Self::Error::from_eth_err)?
            .ok_or_else(|| EthApiError::UnknownBlockNumber)?
            .seal(latest.hash());

        let receipts = self
            .provider()
            .receipts_by_block(block.hash().into())
            .map_err(Self::Error::from_eth_err)?
            .ok_or_else(|| EthApiError::UnknownBlockNumber)?;
        Ok(Some((block, receipts)))
    }

    fn receipts_root(
        &self,
        _block_env: &BlockEnv,
        execution_outcome: &ExecutionOutcome,
        block_number: BlockNumber,
    ) -> B256 {
        execution_outcome
            .optimism_receipts_root_slow(
                block_number,
                self.provider().chain_spec().as_ref(),
                _block_env.timestamp.to::<u64>(),
            )
            .expect("Block is present")
    }
}
