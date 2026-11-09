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
    use alloy_consensus::Transaction as _;
    use alloy_eips::{eip1559::INITIAL_BASE_FEE, BlockId};
    use alloy_genesis::{Genesis, GenesisAccount};
    use alloy_primitives::{
        map::{AddressMap, B256Map},
        Address, Bytes, StorageKey, StorageValue, B256, U256,
    };
    use alloy_rpc_types_eth::TransactionRequest;
    use alloy_serde::JsonStorageKey;
    use reth_chainspec::{ChainSpec, ChainSpecBuilder};
    use reth_db_common::init::init_genesis;
    use reth_ethereum_primitives::Block;
    use reth_evm_ethereum::EthEvmConfig;
    use reth_network_api::noop::NoopNetwork;
    use reth_provider::{
        providers::BlockchainProvider,
        test_utils::{
            create_test_provider_factory_with_chain_spec, ExtendedAccount, MockEthProvider,
            MockNodeTypesWithDB, NoopProvider,
        },
        ChainSpecProvider,
    };
    use reth_rpc_eth_api::{
        helpers::{pending_block::PendingEnvBuilder, Call, EthCall, EthState, SpawnBlocking},
        node::{RpcNodeCoreAdapter, RpcNodeCoreExt},
        EthApiTypes,
    };
    use reth_rpc_eth_types::{
        builder::config::PendingBlockKind, EthApiSettings, EthStateCache, PendingBlock,
        PendingBlockEnv,
    };
    use reth_rpc_server_types::constants::DEFAULT_MAX_PROOF_SLOTS;
    use reth_storage_api::{StateProviderBox, StateProviderFactory};
    use reth_tasks::{
        pool::{BlockingTaskGuard, BlockingTaskPool},
        Runtime,
    };
    use reth_transaction_pool::{
        test_utils::{testing_pool, MockTransaction, TestPool},
        TransactionPool,
    };
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
    fn test_get_proof_key_cap() {
        let eth_api = noop_eth_api();
        let address = Address::random();
        let keys = |len| vec![JsonStorageKey::from(B256::ZERO); len];

        assert!(eth_api.get_proof(address, keys(DEFAULT_MAX_PROOF_SLOTS), None).is_ok());

        let Err(err) = eth_api.get_proof(address, keys(DEFAULT_MAX_PROOF_SLOTS + 1), None) else {
            panic!("expected over-limit request to be rejected");
        };
        assert_eq!(err.to_string(), "storage key count 1025 exceeds limit 1024");
    }

    #[test]
    fn test_get_multi_proof_cap() {
        let eth_api = noop_eth_api();
        let address = Address::random();

        // Exactly at the limit, on both axes.
        let targets = vec![(address, vec![B256::ZERO; DEFAULT_MAX_PROOF_SLOTS])];
        assert!(eth_api.get_multi_proof(targets, None).is_ok());
        let targets = vec![(address, Vec::new()); DEFAULT_MAX_PROOF_SLOTS];
        assert!(eth_api.get_multi_proof(targets, None).is_ok());

        // Too many slots in a single target.
        let targets = vec![(address, vec![B256::ZERO; DEFAULT_MAX_PROOF_SLOTS + 1])];
        let Err(err) = eth_api.get_multi_proof(targets, None) else {
            panic!("expected over-limit request to be rejected");
        };
        assert_eq!(err.to_string(), "total slot count 1025 exceeds limit 1024");

        // Too many slots in total, spread across targets.
        let targets = vec![
            (address, vec![B256::ZERO; DEFAULT_MAX_PROOF_SLOTS]),
            (Address::random(), vec![B256::ZERO]),
        ];
        let Err(err) = eth_api.get_multi_proof(targets, None) else {
            panic!("expected over-limit request to be rejected");
        };
        assert_eq!(err.to_string(), "total slot count 1025 exceeds limit 1024");

        // Too many targets without any slots.
        let targets = vec![(address, Vec::new()); DEFAULT_MAX_PROOF_SLOTS + 1];
        let Err(err) = eth_api.get_multi_proof(targets, None) else {
            panic!("expected over-limit request to be rejected");
        };
        assert_eq!(err.to_string(), "target count 1025 exceeds limit 1024");
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

    impl Call for CustomPendingState {}

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
    async fn pending_call_uses_local_pending_block() {
        // Stores 42 in slot 0 when called with calldata, otherwise returns slot 0.
        let code = "0x3615600b57602a600055005b60005460005260206000f3".parse().unwrap();
        let tx = MockTransaction::eip1559()
            .with_gas_limit(100_000)
            .with_max_fee(INITIAL_BASE_FEE.into())
            .with_input(Bytes::from_static(&[1]));
        let contract = tx.to().unwrap();
        let genesis = Genesis::default()
            .with_gas_limit(30_000_000)
            .with_base_fee(Some(INITIAL_BASE_FEE.into()))
            .extend_accounts([
                (
                    *tx.get_sender(),
                    GenesisAccount::default().with_balance(U256::from(10u128.pow(18))),
                ),
                (
                    contract,
                    GenesisAccount::default()
                        .with_code(Some(code))
                        .with_storage(Some([(B256::ZERO, B256::with_last_byte(7))].into())),
                ),
            ]);
        let chain_spec =
            Arc::new(ChainSpecBuilder::mainnet().cancun_activated().genesis(genesis).build());
        let factory = create_test_provider_factory_with_chain_spec(chain_spec);
        init_genesis(&factory).unwrap();
        let provider = BlockchainProvider::new(factory).unwrap();
        let pool = testing_pool();
        pool.add_external_transaction(tx).await.unwrap();
        let eth_api = EthApi::builder(
            provider.clone(),
            pool,
            NoopNetwork::default(),
            EthEvmConfig::new(provider.chain_spec()),
        )
        .build();

        let pending = Some(BlockId::pending());
        let stored = eth_api.storage_at(contract, U256::ZERO.into(), pending).await.unwrap();
        let output = eth_api
            .call(TransactionRequest::default().to(contract), pending, Default::default())
            .await
            .unwrap();
        assert_eq!(stored, B256::with_last_byte(42));
        assert_eq!(output[..], stored[..]);
    }

    #[tokio::test]
    async fn pending_call_uses_existing_override() {
        let address = Address::random();
        // Returns slot 0.
        let code: Bytes = "0x60005460005260206000f3".parse().unwrap();
        let account = |value: u64| {
            ExtendedAccount::new(0, U256::ZERO)
                .with_bytecode(code.clone())
                .extend_storage([(B256::ZERO, U256::from(value))])
        };
        let eth_api = mock_eth_api(AddressMap::from_iter([(address, account(7))]));
        let header = alloy_consensus::Header { gas_limit: 30_000_000, ..Default::default() };
        eth_api.provider().add_block(B256::ZERO, Block { header, ..Default::default() });

        let pending = MockEthProvider::default();
        pending.extend_accounts([(address, account(42))]);
        let eth_api = CustomPendingState { inner: eth_api, pending };

        let res = eth_api
            .transact_call_at(
                TransactionRequest::default().to(address),
                BlockId::pending(),
                Default::default(),
            )
            .await
            .unwrap();
        assert_eq!(res.result.output[..], B256::with_last_byte(42)[..]);
    }

    type ProviderEthApi = EthApi<
        RpcNodeCoreAdapter<
            BlockchainProvider<MockNodeTypesWithDB>,
            TestPool,
            NoopNetwork,
            EthEvmConfig,
        >,
        EthRpcConverter<ChainSpec>,
    >;

    /// Withdraws the provider's pending block right after its environment is read.
    #[derive(Clone)]
    struct WithdrawnPending(ProviderEthApi);

    impl EthApiTypes for WithdrawnPending {
        type Error = EthApiError;
        type NetworkTypes = <ProviderEthApi as EthApiTypes>::NetworkTypes;
        type RpcConvert = <ProviderEthApi as EthApiTypes>::RpcConvert;

        fn eth_api_settings(&self) -> &EthApiSettings {
            self.0.eth_api_settings()
        }

        fn converter(&self) -> &Self::RpcConvert {
            self.0.converter()
        }
    }

    impl RpcNodeCore for WithdrawnPending {
        type Primitives = <ProviderEthApi as RpcNodeCore>::Primitives;
        type Provider = <ProviderEthApi as RpcNodeCore>::Provider;
        type Pool = <ProviderEthApi as RpcNodeCore>::Pool;
        type Evm = <ProviderEthApi as RpcNodeCore>::Evm;
        type Network = <ProviderEthApi as RpcNodeCore>::Network;

        fn pool(&self) -> &Self::Pool {
            self.0.pool()
        }

        fn evm_config(&self) -> &Self::Evm {
            self.0.evm_config()
        }

        fn network(&self) -> &Self::Network {
            self.0.network()
        }

        fn provider(&self) -> &Self::Provider {
            self.0.provider()
        }
    }

    impl RpcNodeCoreExt for WithdrawnPending {
        fn cache(&self) -> &EthStateCache<Self::Primitives> {
            self.0.cache()
        }
    }

    impl SpawnBlocking for WithdrawnPending {
        fn io_task_spawner(&self) -> &Runtime {
            self.0.io_task_spawner()
        }

        fn tracing_task_pool(&self) -> &BlockingTaskPool {
            self.0.tracing_task_pool()
        }

        fn tracing_task_guard(&self) -> &BlockingTaskGuard {
            self.0.tracing_task_guard()
        }

        fn blocking_io_task_guard(&self) -> &Arc<Semaphore> {
            self.0.blocking_io_task_guard()
        }
    }

    impl LoadPendingBlock for WithdrawnPending {
        fn pending_block(&self) -> &Mutex<Option<PendingBlock<Self::Primitives>>> {
            self.0.pending_block()
        }

        fn pending_env_builder(&self) -> &dyn PendingEnvBuilder<Self::Evm> {
            self.0.pending_env_builder()
        }

        fn pending_block_env_and_cfg(&self) -> Result<PendingBlockEnv<Self::Evm>, Self::Error> {
            let pending = self.0.pending_block_env_and_cfg();
            self.provider().canonical_in_memory_state().clear_state();
            pending
        }
    }

    impl LoadState for WithdrawnPending {}

    impl Call for WithdrawnPending {}

    #[tokio::test]
    async fn pending_call_does_not_mix_withdrawn_pending_block_with_other_state() {
        // Stores 42 in slot 0 when called with calldata, otherwise returns slot 0 and the block
        // number.
        let code = "0x3615600b57602a600055005b6000546000524360205260406000f3".parse().unwrap();
        let tx = MockTransaction::eip1559()
            .with_gas_limit(100_000)
            .with_max_fee(INITIAL_BASE_FEE.into())
            .with_input(Bytes::from_static(&[1]));
        let contract = tx.to().unwrap();
        let genesis = Genesis::default()
            .with_gas_limit(30_000_000)
            .with_base_fee(Some(INITIAL_BASE_FEE.into()))
            .extend_accounts([
                (
                    *tx.get_sender(),
                    GenesisAccount::default().with_balance(U256::from(10u128.pow(18))),
                ),
                (
                    contract,
                    GenesisAccount::default()
                        .with_code(Some(code))
                        .with_storage(Some([(B256::ZERO, B256::with_last_byte(7))].into())),
                ),
            ]);
        let chain_spec =
            Arc::new(ChainSpecBuilder::mainnet().cancun_activated().genesis(genesis).build());
        let overlays = reth_storage_overlay::OverlayManager::default();
        let factory = create_test_provider_factory_with_chain_spec(chain_spec)
            .with_overlay_manager(overlays.clone());
        init_genesis(&factory).unwrap();
        let provider = BlockchainProvider::new(factory).unwrap();
        let pool = testing_pool();
        pool.add_external_transaction(tx).await.unwrap();
        let eth_api = EthApi::builder(
            provider.clone(),
            pool,
            NoopNetwork::default(),
            EthEvmConfig::new(provider.chain_spec()),
        )
        .build();

        // Make the locally built block the provider's pending block.
        let pending = eth_api.pool_pending_block().await.unwrap().unwrap().executed_block;
        overlays.insert_block(pending.clone());
        provider.canonical_in_memory_state().set_pending_block(pending);

        let request = TransactionRequest::default().to(contract);
        let output =
            eth_api.call(request.clone(), Some(BlockId::pending()), Default::default()).await;
        let mut expected = [0; 64];
        expected[31] = 42;
        expected[63] = 1;
        assert_eq!(output.unwrap()[..], expected);

        let eth_api = WithdrawnPending(
            EthApi::builder(
                provider.clone(),
                testing_pool(),
                NoopNetwork::default(),
                EthEvmConfig::new(provider.chain_spec()),
            )
            .pending_block_kind(PendingBlockKind::None)
            .build(),
        );
        let res = eth_api.transact_call_at(request, BlockId::pending(), Default::default()).await;
        assert!(res.is_err(), "{:?}", res.map(|res| res.result.output));
    }
}
