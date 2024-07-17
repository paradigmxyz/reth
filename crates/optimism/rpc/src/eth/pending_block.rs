//! Loads OP pending block for a RPC response.   

use reth_evm::ConfigureEvm;
use reth_primitives::{revm_primitives::BlockEnv, BlockNumber, B256};
use reth_provider::{
    BlockReaderIdExt, ChainSpecProvider, EvmEnvProvider, ExecutionOutcome, StateProviderFactory,
};
use reth_rpc_eth_api::helpers::LoadPendingBlock;
use reth_rpc_eth_types::PendingBlock;
use reth_transaction_pool::TransactionPool;

use crate::OpEthApi;

impl<Eth> LoadPendingBlock for OpEthApi<Eth>
where
    Eth: LoadPendingBlock,
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
