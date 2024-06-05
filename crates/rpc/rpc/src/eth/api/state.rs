//! Contains RPC handler implementations specific to state.

use crate::{eth::api::LoadState, EthApi};
use reth_provider::StateProviderFactory;

impl<Provider, Pool, Network, EvmConfig> LoadState for EthApi<Provider, Pool, Network, EvmConfig>
where
    Provider: StateProviderFactory,
{
    fn provider(&self) -> &impl StateProviderFactory {
        &self.inner.provider
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eth::{
        cache::EthStateCache, gas_oracle::GasPriceOracle, FeeHistoryCache, FeeHistoryCacheConfig,
    };
    use reth_evm_ethereum::EthEvmConfig;
    use reth_primitives::{constants::ETHEREUM_BLOCK_GAS_LIMIT, StorageKey, StorageValue};
    use reth_provider::test_utils::{ExtendedAccount, MockEthProvider, NoopProvider};
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
            BlockingTaskPool::build().expect("failed to build tracing pool"),
            FeeHistoryCache::new(cache, FeeHistoryCacheConfig::default()),
            evm_config,
            None,
        );
        let address = Address::random();
        let storage = eth_api.storage_at(address, U256::ZERO.into(), None).unwrap();
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
            BlockingTaskPool::build().expect("failed to build tracing pool"),
            FeeHistoryCache::new(cache, FeeHistoryCacheConfig::default()),
            evm_config,
            None,
        );

        let storage_key: U256 = storage_key.into();
        let storage = eth_api.storage_at(address, storage_key.into(), None).unwrap();
        assert_eq!(storage, storage_value.to_be_bytes());
    }
}
