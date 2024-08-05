//! Contains RPC handler implementations specific to state.

use reth_provider::{ChainSpecProvider, StateProviderFactory};
use reth_transaction_pool::TransactionPool;

use reth_rpc_eth_api::helpers::{EthState, LoadState, SpawnBlocking};
use reth_rpc_eth_types::EthStateCache;

use crate::EthApi;

impl<Provider, Pool, Network, EvmConfig> EthState for EthApi<Provider, Pool, Network, EvmConfig>
where
    Self: LoadState + SpawnBlocking,
{
    fn max_proof_window(&self) -> u64 {
        self.inner.eth_proof_window()
    }
}

impl<Provider, Pool, Network, EvmConfig> LoadState for EthApi<Provider, Pool, Network, EvmConfig>
where
    Self: Send + Sync,
    Provider: StateProviderFactory + ChainSpecProvider,
    Pool: TransactionPool,
{
    #[inline]
    fn provider(&self) -> impl StateProviderFactory + ChainSpecProvider {
        self.inner.provider()
    }

    #[inline]
    fn cache(&self) -> &EthStateCache {
        self.inner.cache()
    }

    #[inline]
    fn pool(&self) -> impl TransactionPool {
        self.inner.pool()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_evm_ethereum::EthEvmConfig;
    use reth_primitives::{
        constants::ETHEREUM_BLOCK_GAS_LIMIT, Address, StorageKey, StorageValue, U256,
    };
    use reth_provider::test_utils::{ExtendedAccount, MockEthProvider, NoopProvider};
    use reth_rpc_eth_api::helpers::EthState;
    use reth_rpc_eth_types::{
        EthStateCache, FeeHistoryCache, FeeHistoryCacheConfig, GasPriceOracle,
    };
    use reth_rpc_server_types::constants::{DEFAULT_ETH_PROOF_WINDOW, DEFAULT_PROOF_PERMITS};
    use reth_tasks::pool::BlockingTaskPool;
    use reth_transaction_pool::test_utils::testing_pool;
    use std::collections::HashMap;

    #[tokio::test]
    async fn test_storage() {
        // === Noop ===
        let pool = testing_pool();
        let evm_config = EthEvmConfig::default();

        let cache = EthStateCache::spawn(NoopProvider::default(), Default::default(), evm_config);
        let eth_api = EthApi::new(
            NoopProvider::default(),
            pool.clone(),
            (),
            cache.clone(),
            GasPriceOracle::new(NoopProvider::default(), Default::default(), cache.clone()),
            ETHEREUM_BLOCK_GAS_LIMIT,
            DEFAULT_ETH_PROOF_WINDOW,
            BlockingTaskPool::build().expect("failed to build tracing pool"),
            FeeHistoryCache::new(cache, FeeHistoryCacheConfig::default()),
            evm_config,
            None,
            DEFAULT_PROOF_PERMITS,
        );
        let address = Address::random();
        let storage = eth_api.storage_at(address, U256::ZERO.into(), None).await.unwrap();
        assert_eq!(storage, U256::ZERO.to_be_bytes());

        // === Mock ===
        let mock_provider = MockEthProvider::default();
        let storage_value = StorageValue::from(1337);
        let storage_key = StorageKey::random();
        let storage = HashMap::from([(storage_key, storage_value)]);
        let account = ExtendedAccount::new(0, U256::ZERO).extend_storage(storage);
        mock_provider.add_account(address, account);

        let cache = EthStateCache::spawn(mock_provider.clone(), Default::default(), evm_config);
        let eth_api = EthApi::new(
            mock_provider.clone(),
            pool,
            (),
            cache.clone(),
            GasPriceOracle::new(mock_provider, Default::default(), cache.clone()),
            ETHEREUM_BLOCK_GAS_LIMIT,
            DEFAULT_ETH_PROOF_WINDOW,
            BlockingTaskPool::build().expect("failed to build tracing pool"),
            FeeHistoryCache::new(cache, FeeHistoryCacheConfig::default()),
            evm_config,
            None,
            DEFAULT_PROOF_PERMITS,
        );

        let storage_key: U256 = storage_key.into();
        let storage = eth_api.storage_at(address, storage_key.into(), None).await.unwrap();
        assert_eq!(storage, storage_value.to_be_bytes());
    }
}
