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
    use alloy_genesis::{Genesis, GenesisAccount};
    use alloy_primitives::{
        keccak256,
        map::{AddressMap, B256Map},
        Address, StorageKey, StorageValue, B256, U256,
    };
    use alloy_rpc_types_eth::TransactionRequest;
    use reth_chainspec::{ChainSpec, ChainSpecBuilder};
    use reth_db_common::init::init_genesis;
    use reth_ethereum_primitives::Block;
    use reth_evm_ethereum::EthEvmConfig;
    use reth_network_api::noop::NoopNetwork;
    use reth_provider::{
        providers::BlockchainProvider,
        test_utils::{
            create_test_provider_factory_with_chain_spec, ExtendedAccount, MockEthProvider,
            NoopProvider,
        },
        ChainSpecProvider,
    };
    use reth_rpc_eth_api::{
        helpers::{pending_block::PendingEnvBuilder, EthCall, EthState, SpawnBlocking},
        node::{RpcNodeCoreAdapter, RpcNodeCoreExt},
        EthApiTypes,
    };
    use reth_rpc_eth_types::{EthApiSettings, EthStateCache, PendingBlock};
    use reth_storage_api::{StateProviderBox, StateProviderFactory};
    use reth_storage_overlay::OverlayManager;
    use reth_tasks::{
        pool::{BlockingTaskGuard, BlockingTaskPool},
        Runtime,
    };
    use reth_transaction_pool::test_utils::{testing_pool, TestPool};
    use reth_trie_common::{AccountProof, Nibbles};
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

    #[cfg(feature = "account-ext")]
    #[tokio::test]
    async fn test_get_account_extension() {
        let address = Address::random();
        let extension = reth_primitives_traits::AccountExtension::copy_from_slice(&[0x01]);
        let eth_api = mock_eth_api(AddressMap::from_iter([(
            address,
            ExtendedAccount::new(0, U256::ZERO).with_extension(extension.clone()),
        )]));
        eth_api.provider().add_block(B256::ZERO, Block::default());

        // An account whose only non-default field is its extension still exists.
        let info = eth_api.get_account_info(address, Default::default()).await.unwrap();
        assert!(!info.is_empty());
        assert_eq!(info.extension, extension);
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["extension"], "0x01");
        assert_eq!(serde_json::from_value::<alloy_rpc_types_eth::AccountInfo>(json).unwrap(), info);

        let account = eth_api.get_account(address, Default::default()).await.unwrap().unwrap();
        assert_eq!(account.extension, extension);

        let missing =
            eth_api.get_account_info(Address::random(), Default::default()).await.unwrap();
        assert!(missing.is_empty());
        assert!(serde_json::to_value(missing).unwrap().get("extension").is_none());
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

        fn local_pending_state(
            &self,
        ) -> impl Future<Output = Result<Option<StateProviderBox>, Self::Error>> + Send {
            let state = self.pending.latest().map_err(EthApiError::from);
            async move { state.map(Some) }
        }
    }

    impl LoadState for CustomPendingState {}

    impl EthState for CustomPendingState {}

    #[tokio::test]
    async fn pending_state_reads_use_existing_override() {
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

    #[tokio::test]
    async fn get_proof_verifies_embedded_storage_leaves() {
        let address = Address::repeat_byte(0x11);
        // These slots share eight hashed nibbles, making their small-value leaves inline.
        let slots = [50_541, 125_299].map(|slot| B256::from(U256::from(slot)));
        assert_eq!(
            Nibbles::unpack(keccak256(slots[0]))
                .common_prefix_length(&Nibbles::unpack(keccak256(slots[1]))),
            8
        );
        let values = [U256::from(0x4b), U256::from(7)];
        let genesis = Genesis::default().with_gas_limit(30_000_000).extend_accounts([(
            address,
            GenesisAccount {
                balance: U256::ONE,
                storage: Some(slots.into_iter().zip(values.map(B256::from)).collect()),
                ..Default::default()
            },
        )]);
        let chain_spec =
            Arc::new(ChainSpecBuilder::mainnet().cancun_activated().genesis(genesis).build());
        let state_root = chain_spec.genesis_header().state_root;
        let factory = create_test_provider_factory_with_chain_spec(chain_spec)
            .with_overlay_manager(OverlayManager::default());
        init_genesis(&factory).unwrap();
        let provider = BlockchainProvider::new(factory).unwrap();
        let evm_config = EthEvmConfig::new(provider.chain_spec());
        let eth_api =
            EthApi::builder(provider, testing_pool(), NoopNetwork::default(), evm_config).build();

        // Preserve request order, duplicates, and both quantity and full-width storage keys.
        let keys =
            vec![U256::from(125_299).into(), slots[0].into(), B256::ZERO.into(), slots[0].into()];
        for block_id in [None, Some(BlockId::number(0))] {
            let response =
                eth_api.get_proof(address, keys.clone(), block_id).unwrap().await.unwrap();
            assert_eq!(response.address, address);
            assert_eq!(response.balance, U256::ONE);
            assert_eq!(
                response.storage_proof.iter().map(|proof| proof.key).collect::<Vec<_>>(),
                keys
            );
            assert_eq!(
                response.storage_proof.iter().map(|proof| proof.value).collect::<Vec<_>>(),
                [values[1], values[0], U256::ZERO, values[0]]
            );
            AccountProof::from_eip1186_proof(response.clone()).verify(state_root).unwrap();
            for proof in &response.storage_proof {
                assert!(proof.proof.iter().skip(1).all(|node| node.len() >= B256::len_bytes()));
            }
        }

        let error =
            eth_api.get_proof(address, keys, Some(BlockId::pending())).unwrap().await.unwrap_err();
        assert!(matches!(error, EthApiError::HeaderNotFound(id) if id == BlockId::pending()));

        // Account-only and missing-account proofs must still verify.
        for account in [address, Address::repeat_byte(0x22)] {
            let response = eth_api.get_proof(account, Vec::new(), None).unwrap().await.unwrap();
            assert!(response.storage_proof.is_empty());
            AccountProof::from_eip1186_proof(response).verify(state_root).unwrap();
        }
    }
}
