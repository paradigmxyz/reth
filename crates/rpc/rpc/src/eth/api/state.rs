//! Contains RPC handler implementations specific to state.

use crate::EthApi;

/// Implements [`EthState`](crate::eth::api::EthState) for a type, that has similar
/// data layout to [`EthApi`].
#[macro_export]
macro_rules! eth_state_impl {
    ($network_api:ty, $(<$($generic:ident,)+>)*) => {
        impl$(<$($generic,)+>)* $crate::eth::api::EthState
            for $network_api
        where
            Self: $crate::eth::api::LoadState + $crate::eth::api::SpawnBlocking
        {
        }
    };
}

/// Implements [`LoadState`](crate::eth::api::LoadState) for a type, that has similar
/// data layout to [`EthApi`].
#[macro_export]
macro_rules! load_state_impl {
    ($network_api:ty, $(<$($generic:ident,)+>)*) => {
        impl$(<$($generic,)+>)* $crate::eth::api::LoadState
            for $network_api
        where
            Provider: reth_provider::StateProviderFactory,
            Pool: reth_transaction_pool::TransactionPool,
        {
            #[inline]
            fn provider(&self) -> impl reth_provider::StateProviderFactory {
                self.inner.provider()
            }

            #[inline]
            fn cache(&self) -> &$crate::eth::cache::EthStateCache {
                self.inner.cache()
            }

            #[inline]
            fn pool(&self) -> impl reth_transaction_pool::TransactionPool {
                self.inner.pool()
            }
        }
    };
}

eth_state_impl!(EthApi<Provider, Pool, Network, EvmConfig>, <Provider, Pool, Network, EvmConfig,>);
load_state_impl!(EthApi<Provider, Pool, Network, EvmConfig>, <Provider, Pool, Network, EvmConfig,>);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eth::{
        api::EthState, cache::EthStateCache, gas_oracle::GasPriceOracle, FeeHistoryCache,
        FeeHistoryCacheConfig,
    };
    use reth_evm_ethereum::EthEvmConfig;
    use reth_primitives::{
        constants::ETHEREUM_BLOCK_GAS_LIMIT, Address, StorageKey, StorageValue, U256,
    };
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
            BlockingTaskPool::build().expect("failed to build tracing pool"),
            FeeHistoryCache::new(cache, FeeHistoryCacheConfig::default()),
            evm_config,
            None,
        );

        let storage_key: U256 = storage_key.into();
        let storage = eth_api.storage_at(address, storage_key.into(), None).await.unwrap();
        assert_eq!(storage, storage_value.to_be_bytes());
    }
}
