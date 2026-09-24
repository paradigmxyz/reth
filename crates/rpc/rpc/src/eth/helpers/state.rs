//! Contains RPC handler implementations specific to state.

use crate::EthApi;
use reth_rpc_convert::RpcConvert;
use reth_rpc_eth_api::{
    helpers::{EthState, LoadPendingBlock, LoadState},
    RpcNodeCore,
};
use reth_rpc_eth_types::EthApiError;

impl<N, Rpc> EthState for EthApi<N, Rpc>
where
    N: RpcNodeCore,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = EthApiError>,
    Self: LoadPendingBlock,
{
}

impl<N, Rpc> LoadState for EthApi<N, Rpc>
where
    N: RpcNodeCore,
    Rpc: RpcConvert<Primitives = N::Primitives>,
    Self: LoadPendingBlock,
{
}

#[cfg(test)]
mod tests {
    use crate::eth::helpers::types::EthRpcConverter;

    use super::*;
    use alloy_eips::BlockId;
    use alloy_primitives::{
        map::{AddressMap, B256Map},
        Address, StorageKey, StorageValue, B256, U256,
    };
    use alloy_rpc_types_eth::TransactionRequest;
    use reth_chainspec::ChainSpec;
    use reth_ethereum_primitives::Block;
    use reth_evm_ethereum::EthEvmConfig;
    use reth_network_api::noop::NoopNetwork;
    use reth_provider::{
        test_utils::{ExtendedAccount, MockEthProvider, NoopProvider},
        ChainSpecProvider,
    };
    use reth_rpc_eth_api::{
        helpers::{
            pending_block::{PendingEnvBuilder, PendingStateSource},
            EthCall, EthState, SpawnBlocking,
        },
        node::{RpcNodeCoreAdapter, RpcNodeCoreExt},
        EthApiTypes,
    };
    use reth_rpc_eth_types::{EthApiSettings, EthStateCache, PendingBlock};
    use reth_storage_api::StateProviderFactory;
    use reth_tasks::{
        pool::{BlockingTaskGuard, BlockingTaskPool},
        Runtime,
    };
    use reth_transaction_pool::test_utils::{testing_pool, TestPool};
    use std::{future::Future, sync::Arc, time::Duration};
    use tokio::sync::{Mutex, Semaphore};

    fn noop_eth_api() -> EthApi<
        RpcNodeCoreAdapter<NoopProvider, TestPool, NoopNetwork, EthEvmConfig>,
        EthRpcConverter<ChainSpec>,
    > {
        let provider = NoopProvider::default();
        let pool = testing_pool();
        let evm_config = EthEvmConfig::mainnet();

        EthApi::builder(provider, pool, NoopNetwork::default(), evm_config).build()
    }

    fn mock_eth_api(
        accounts: AddressMap<ExtendedAccount>,
    ) -> EthApi<
        RpcNodeCoreAdapter<MockEthProvider, TestPool, NoopNetwork, EthEvmConfig>,
        EthRpcConverter<ChainSpec>,
    > {
        let pool = testing_pool();
        let mock_provider = MockEthProvider::default();

        let evm_config = EthEvmConfig::new(mock_provider.chain_spec());
        mock_provider.extend_accounts(accounts);

        EthApi::builder(mock_provider, pool, NoopNetwork::default(), evm_config).build()
    }

    #[tokio::test]
    async fn test_storage() {
        // === Noop ===
        let eth_api = noop_eth_api();
        let address = Address::random();
        let storage = eth_api.storage_at(address, U256::ZERO.into(), None).await.unwrap();
        assert_eq!(storage, U256::ZERO.to_be_bytes());

        // === Mock ===
        let storage_value = StorageValue::from(1337);
        let storage_key = StorageKey::random();
        let storage: B256Map<_> = core::iter::once((storage_key, storage_value)).collect();

        let accounts = AddressMap::from_iter([(
            address,
            ExtendedAccount::new(0, U256::ZERO).extend_storage(storage),
        )]);
        let eth_api = mock_eth_api(accounts);

        let storage_key: U256 = storage_key.into();
        let storage = eth_api.storage_at(address, storage_key.into(), None).await.unwrap();
        assert_eq!(storage, storage_value.to_be_bytes());
    }

    #[tokio::test]
    async fn test_get_account_missing() {
        let eth_api = noop_eth_api();
        let address = Address::random();
        let account = eth_api.get_account(address, Default::default()).await.unwrap();
        assert!(account.is_none());
    }

    #[test]
    fn pending_state_and_access_list_do_not_deadlock() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .max_blocking_threads(1)
            .enable_all()
            .build()
            .unwrap();

        let result = runtime.block_on(async {
            let address = Address::random();
            let accounts =
                AddressMap::from_iter([(address, ExtendedAccount::new(0, U256::from(1337)))]);
            let eth_api = mock_eth_api(accounts);
            // Use a block after access-list transactions became valid.
            let mut block = Block::default();
            block.header.number = 13_000_000;
            block.header.timestamp = 1_629_000_000;
            block.header.gas_limit = 30_000_000;
            block.header.base_fee_per_gas = Some(1_000_000_000);
            eth_api.provider().add_block(B256::ZERO, block);
            tokio::time::timeout(Duration::from_secs(10), async {
                let balance = eth_api.balance(address, Some(BlockId::pending())).await?;
                eth_api
                    .create_access_list_at(
                        TransactionRequest { gas_price: Some(0), ..Default::default() },
                        Some(BlockId::latest()),
                        None,
                    )
                    .await?;
                Ok::<_, EthApiError>(balance)
            })
            .await
        });
        // A deadlocked blocking thread would also block a regular runtime drop.
        runtime.shutdown_background();
        assert_eq!(
            result.expect("RPC timed out on one blocking thread").expect("RPC returned an error"),
            U256::from(1337)
        );
    }

    type MockEthApi = EthApi<
        RpcNodeCoreAdapter<MockEthProvider, TestPool, NoopNetwork, EthEvmConfig>,
        EthRpcConverter<ChainSpec>,
    >;

    #[derive(Clone)]
    struct CustomPendingState {
        inner: MockEthApi,
        pending: MockEthProvider,
    }

    impl EthApiTypes for CustomPendingState {
        type Error = EthApiError;
        type NetworkTypes = <MockEthApi as EthApiTypes>::NetworkTypes;
        type RpcConvert = <MockEthApi as EthApiTypes>::RpcConvert;

        fn eth_api_settings(&self) -> &EthApiSettings {
            self.inner.eth_api_settings()
        }

        fn converter(&self) -> &Self::RpcConvert {
            self.inner.converter()
        }
    }

    impl RpcNodeCore for CustomPendingState {
        type Primitives = <MockEthApi as RpcNodeCore>::Primitives;
        type Provider = <MockEthApi as RpcNodeCore>::Provider;
        type Pool = <MockEthApi as RpcNodeCore>::Pool;
        type Evm = <MockEthApi as RpcNodeCore>::Evm;
        type Network = <MockEthApi as RpcNodeCore>::Network;

        fn pool(&self) -> &Self::Pool {
            self.inner.pool()
        }

        fn evm_config(&self) -> &Self::Evm {
            self.inner.evm_config()
        }

        fn network(&self) -> &Self::Network {
            self.inner.network()
        }

        fn provider(&self) -> &Self::Provider {
            self.inner.provider()
        }
    }

    impl RpcNodeCoreExt for CustomPendingState {
        fn cache(&self) -> &EthStateCache<Self::Primitives> {
            self.inner.cache()
        }
    }

    impl SpawnBlocking for CustomPendingState {
        fn io_task_spawner(&self) -> &Runtime {
            self.inner.io_task_spawner()
        }

        fn tracing_task_pool(&self) -> &BlockingTaskPool {
            self.inner.tracing_task_pool()
        }

        fn tracing_task_guard(&self) -> &BlockingTaskGuard {
            self.inner.tracing_task_guard()
        }

        fn blocking_io_task_guard(&self) -> &Arc<Semaphore> {
            self.inner.blocking_io_task_guard()
        }
    }

    impl LoadPendingBlock for CustomPendingState {
        fn pending_block(&self) -> &Mutex<Option<PendingBlock<Self::Primitives>>> {
            self.inner.pending_block()
        }

        fn pending_env_builder(&self) -> &dyn PendingEnvBuilder<Self::Evm> {
            self.inner.pending_env_builder()
        }

        fn local_pending_block_or_state(
            &self,
        ) -> impl Future<Output = Result<Option<PendingStateSource<Self::Primitives>>, Self::Error>> + Send
        where
            Self: SpawnBlocking,
        {
            let state = self.pending.latest().map_err(EthApiError::from);
            async move { state.map(|state| Some(PendingStateSource::State(state))) }
        }
    }

    impl LoadState for CustomPendingState {}

    impl EthState for CustomPendingState {}

    #[tokio::test]
    async fn pending_state_reads_use_custom_state_source() {
        let address = Address::random();
        let chain = AddressMap::from_iter([(address, ExtendedAccount::new(0, U256::from(1337)))]);
        let eth_api = mock_eth_api(chain);
        eth_api.provider().add_block(B256::ZERO, Block::default());

        let pending = MockEthProvider::default();
        pending.extend_accounts([(address, ExtendedAccount::new(0, U256::from(42)))]);
        let eth_api = CustomPendingState { inner: eth_api, pending };

        assert_eq!(
            eth_api.balance(address, Some(BlockId::pending())).await.unwrap(),
            U256::from(42)
        );
        assert_eq!(eth_api.balance(address, None).await.unwrap(), U256::from(1337));
    }
}
