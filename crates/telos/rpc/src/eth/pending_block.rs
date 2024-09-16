//! Loads OP pending block for a RPC response.   

use reth_chainspec::ChainSpec;
use reth_evm::ConfigureEvm;
use reth_node_api::FullNodeComponents;
use reth_provider::{
    BlockReaderIdExt, ChainSpecProvider, EvmEnvProvider, StateProviderFactory,
};
use reth_rpc_eth_api::{
    helpers::{LoadPendingBlock, SpawnBlocking},
};
use reth_rpc_eth_types::{PendingBlock};
use reth_transaction_pool::TransactionPool;
use crate::eth::TelosEthApi;

impl<N> LoadPendingBlock for TelosEthApi<N>
where
    Self: SpawnBlocking,
    N: FullNodeComponents,
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

}
