//! Support for building a pending block with transactions from local view of mempool.

use crate::EthApi;
use reth_evm::ConfigureEvmCommit;
use reth_provider::{BlockReaderIdExt, ChainSpecProvider, EvmEnvProvider, StateProviderFactory};
use reth_rpc_eth_api::helpers::{LoadPendingBlock, SpawnBlocking};
use reth_rpc_eth_types::PendingBlock;
use reth_transaction_pool::TransactionPool;

impl<Provider, Pool, Network, EvmConfig> LoadPendingBlock
    for EthApi<Provider, Pool, Network, EvmConfig>
where
    Self: SpawnBlocking,
    Provider: BlockReaderIdExt + EvmEnvProvider + ChainSpecProvider + StateProviderFactory,
    Pool: TransactionPool,
    EvmConfig: ConfigureEvmCommit,
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
    fn evm_config(&self) -> &impl ConfigureEvmCommit {
        self.inner.evm_config()
    }
}
