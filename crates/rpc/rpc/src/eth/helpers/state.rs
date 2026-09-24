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
    use alloy_serde::JsonStorageKey;
    use reth_chainspec::ChainSpec;
    use reth_ethereum_primitives::Block;
    use reth_evm_ethereum::EthEvmConfig;
    use reth_network_api::noop::NoopNetwork;
    use reth_provider::{
        test_utils::{ExtendedAccount, MockEthProvider, NoopProvider},
        ChainSpecProvider,
    };
    use reth_rpc_eth_api::{
        helpers::{EthCall, EthState, LoadState},
        node::RpcNodeCoreAdapter,
    };
    use reth_transaction_pool::test_utils::{testing_pool, TestPool};
    use std::{collections::HashMap, future::Future, time::Duration};

    type MockEthApi = EthApi<
        RpcNodeCoreAdapter<MockEthProvider, TestPool, NoopNetwork, EthEvmConfig>,
        EthRpcConverter<ChainSpec>,
    >;

    fn noop_eth_api() -> EthApi<
        RpcNodeCoreAdapter<NoopProvider, TestPool, NoopNetwork, EthEvmConfig>,
        EthRpcConverter<ChainSpec>,
    > {
        let provider = NoopProvider::default();
        let pool = testing_pool();
        let evm_config = EthEvmConfig::mainnet();

        EthApi::builder(provider, pool, NoopNetwork::default(), evm_config).build()
    }

    fn mock_eth_api(accounts: AddressMap<ExtendedAccount>) -> MockEthApi {
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

    /// Runs `f` against a mock API with one block, on a runtime with a single blocking thread, and
    /// returns whether it finished before the timeout.
    ///
    /// A request that holds its blocking thread while it waits for more blocking work never
    /// finishes here, because that work can never get a thread.
    fn completes_on_one_blocking_thread<F, Fut>(f: F) -> bool
    where
        F: FnOnce(MockEthApi) -> Fut,
        Fut: Future,
    {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .max_blocking_threads(1)
            .enable_all()
            .build()
            .unwrap();

        let completed = runtime.block_on(async {
            let eth_api = mock_eth_api(AddressMap::default());
            eth_api.provider().add_block(B256::ZERO, Block::default());
            tokio::time::timeout(Duration::from_secs(10), f(eth_api)).await.is_ok()
        });
        // A deadlocked blocking thread would also block a regular runtime drop.
        runtime.shutdown_background();
        completed
    }

    #[test]
    fn pending_balance_does_not_hold_the_blocking_pool() {
        assert!(completes_on_one_blocking_thread(|eth_api| async move {
            let _ = eth_api.balance(Address::ZERO, Some(BlockId::pending())).await;
        }));
    }

    #[test]
    fn pending_storage_at_does_not_hold_the_blocking_pool() {
        assert!(completes_on_one_blocking_thread(|eth_api| async move {
            let _ = eth_api
                .storage_at(Address::ZERO, JsonStorageKey::default(), Some(BlockId::pending()))
                .await;
        }));
    }

    #[test]
    fn pending_storage_values_does_not_hold_the_blocking_pool() {
        assert!(completes_on_one_blocking_thread(|eth_api| async move {
            let requests = HashMap::from([(Address::ZERO, vec![JsonStorageKey::default()])]);
            let _ = eth_api.storage_values(requests, Some(BlockId::pending())).await;
        }));
    }

    #[test]
    fn pending_code_does_not_hold_the_blocking_pool() {
        assert!(completes_on_one_blocking_thread(|eth_api| async move {
            let _ = EthState::get_code(&eth_api, Address::ZERO, Some(BlockId::pending())).await;
        }));
    }

    #[test]
    fn pending_transaction_count_does_not_hold_the_blocking_pool() {
        assert!(completes_on_one_blocking_thread(|eth_api| async move {
            let _ = EthState::transaction_count(&eth_api, Address::ZERO, Some(BlockId::pending()))
                .await;
        }));
    }

    #[test]
    fn pending_account_info_does_not_hold_the_blocking_pool() {
        assert!(completes_on_one_blocking_thread(|eth_api| async move {
            let _ = eth_api.get_account_info(Address::ZERO, BlockId::pending()).await;
        }));
    }

    /// `eth_createAccessList` must not wait on one blocking task from another, for any block tag.
    #[test]
    fn create_access_list_does_not_hold_the_blocking_pool() {
        assert!(completes_on_one_blocking_thread(|eth_api| async move {
            let _ = eth_api
                .create_access_list_at(TransactionRequest::default(), Some(BlockId::latest()), None)
                .await;
        }));
    }

    /// Moving the state lookup onto a blocking task must not change which state a request reads.
    #[tokio::test]
    async fn state_on_blocking_task_matches_the_async_lookup() {
        let address = Address::random();
        let accounts =
            AddressMap::from_iter([(address, ExtendedAccount::new(7, U256::from(1337)))]);
        let eth_api = mock_eth_api(accounts);
        eth_api.provider().add_block(B256::ZERO, Block::default());

        for block_id in [None, Some(BlockId::latest()), Some(BlockId::pending()), Some(0u64.into())]
        {
            let expected = eth_api
                .state_at_block_id_or_latest(block_id)
                .await
                .map(|state| state.account_balance(&address).unwrap());
            let actual = eth_api
                .spawn_blocking_io_with_state(block_id, move |_, state| {
                    Ok(state.account_balance(&address).unwrap())
                })
                .await;

            assert_eq!(actual.ok(), expected.ok(), "state differs for {block_id:?}");
        }

        let latest = eth_api
            .spawn_blocking_io_with_state(None, move |_, state| {
                Ok(state.account_balance(&address).unwrap())
            })
            .await
            .unwrap();
        assert_eq!(latest, Some(U256::from(1337)));
    }
}
