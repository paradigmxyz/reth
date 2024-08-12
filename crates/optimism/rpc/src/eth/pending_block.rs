//! Loads OP pending block for a RPC response.   

use crate::OpEthApi;
use reth_evm::ConfigureEvm;
use reth_node_api::FullNodeComponents;
use reth_primitives::{
    revm_primitives::BlockEnv, BlockHashOrNumber, BlockNumber, SealedBlockWithSenders, B256,
};
use reth_provider::{
    BlockReader, BlockReaderIdExt, ChainSpecProvider, EvmEnvProvider, ExecutionOutcome,
    StateProviderFactory,
};
use reth_rpc_eth_api::{
    helpers::{LoadPendingBlock, SpawnBlocking},
    FromEthApiError,
};
use reth_rpc_eth_types::{EthApiError, PendingBlock};
use reth_transaction_pool::TransactionPool;

impl<N> LoadPendingBlock for OpEthApi<N>
where
    Self: SpawnBlocking,
    N: FullNodeComponents,
{
    #[inline]
    fn provider(
        &self,
    ) -> impl BlockReaderIdExt + EvmEnvProvider + ChainSpecProvider + StateProviderFactory {
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
    async fn local_pending_block(&self) -> Result<Option<SealedBlockWithSenders>, Self::Error> {
        // See: <https://github.com/ethereum-optimism/op-geth/blob/f2e69450c6eec9c35d56af91389a1c47737206ca/miner/worker.go#L367-L375>
        let latest = self
            .provider()
            .latest_header()
            .map_err(Self::Error::from_eth_err)?
            .ok_or_else(|| EthApiError::UnknownBlockNumber)?;
        let (_, block_hash) = latest.split();
        self.provider()
            .sealed_block_with_senders(BlockHashOrNumber::from(block_hash), Default::default())
            .map_err(Self::Error::from_eth_err)
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
