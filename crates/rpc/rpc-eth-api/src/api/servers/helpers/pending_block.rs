//! Support for building a pending block with transactions from local view of mempool.

/// Implements [`LoadPendingBlock`](crate::servers::LoadPendingBlock) for a type, that has similar
/// data layout to [`EthApi`](crate::EthApi).
#[macro_export]
macro_rules! load_pending_block_impl {
    ($network_api:ty) => {
        impl<Provider, Pool, Network, EvmConfig> $crate::servers::LoadPendingBlock for $network_api
        where
            Self: $crate::servers::SpawnBlocking,
            Provider: reth_provider::BlockReaderIdExt
                + reth_provider::EvmEnvProvider
                + reth_provider::ChainSpecProvider
                + reth_provider::StateProviderFactory,
            Pool: reth_transaction_pool::TransactionPool,
            EvmConfig: reth_evm::ConfigureEvm,
        {
            #[inline]
            fn provider(
                &self,
            ) -> impl reth_provider::BlockReaderIdExt
                   + reth_provider::EvmEnvProvider
                   + reth_provider::ChainSpecProvider
                   + reth_provider::StateProviderFactory {
                self.inner.provider()
            }

            #[inline]
            fn pool(&self) -> impl reth_transaction_pool::TransactionPool {
                self.inner.pool()
            }

            #[inline]
            fn pending_block(&self) -> &tokio::sync::Mutex<Option<$crate::PendingBlock>> {
                self.inner.pending_block()
            }

            #[inline]
            fn evm_config(&self) -> &impl reth_evm::ConfigureEvm {
                self.inner.evm_config()
            }
        }
    };
}

#[cfg(not(feature = "optimism"))]
load_pending_block_impl!(crate::EthApi<Provider, Pool, Network, EvmConfig>);
