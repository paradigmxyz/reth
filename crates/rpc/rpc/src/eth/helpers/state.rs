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
        map::{AddressMap, B256Map},
        Address, StorageKey, StorageValue, B256, U256,
    };
    use alloy_rpc_types_eth::TransactionRequest;
    use reth_chain_state::{ExecutedBlock, NewCanonicalChain};
    use reth_chainspec::{ChainSpec, ChainSpecBuilder};
    use reth_db_common::init::init_genesis;
    use reth_ethereum_primitives::Block;
    use reth_evm::NextBlockEnvAttributes;
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
        helpers::{
            pending_block::{BuildPendingEnv, PendingEnvBuilder},
            Call, EthCall, EthState, SpawnBlocking,
        },
        node::{RpcNodeCoreAdapter, RpcNodeCoreExt},
        EthApiTypes,
    };
    use reth_rpc_eth_types::{
        builder::config::PendingBlockKind, EthApiSettings, EthStateCache, PendingBlock,
    };
    use reth_storage_api::{BlockReaderIdExt, StateProviderBox, StateProviderFactory};
    use reth_storage_overlay::OverlayManager;
    use reth_tasks::{
        pool::{BlockingTaskGuard, BlockingTaskPool},
        Runtime,
    };
    use reth_transaction_pool::test_utils::{testing_pool, TestPool};
    use reth_trie_common::{ComputedTrieData, LazyTrieData};
    use revm::{
        context::result::ExecutionResult,
        database::{
            states::{AccountStatus, StorageSlot},
            BundleAccount,
        },
    };
    use std::{
        future::Future,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
        time::{Duration, Instant},
    };
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
    async fn pending_simulations_use_existing_override() {
        let address = Address::with_last_byte(0x42);
        let code: alloy_primitives::Bytes = "0x60005460005260206000f3".parse().unwrap();
        let account = |value| {
            ExtendedAccount::new(0, U256::ZERO)
                .with_bytecode(code.clone())
                .extend_storage([(B256::ZERO, U256::from(value))])
        };
        let eth_api = mock_eth_api(AddressMap::from_iter([(address, account(7))]));
        let mut block = Block::default();
        block.header.number = 1;
        block.header.gas_limit = 30_000_000;
        let hash = block.header.hash_slow();
        eth_api.provider().add_block(hash, block);

        let pending = MockEthProvider::default();
        pending.extend_accounts([(address, account(42))]);
        let eth_api = CustomPendingState { inner: eth_api, pending };

        let stored =
            eth_api.storage_at(address, U256::ZERO.into(), Some(BlockId::pending())).await.unwrap();
        let simulated = eth_api
            .transact_call_at(
                TransactionRequest::default().to(address),
                BlockId::pending(),
                Default::default(),
            )
            .await
            .unwrap();
        let output = match simulated.result {
            ExecutionResult::Success { output, .. } => output.into_data(),
            other => panic!("pending call failed: {other:?}"),
        };
        assert_eq!(stored, U256::from(42).to_be_bytes());
        assert_eq!(U256::from_be_slice(&output), U256::from(42));
    }

    #[tokio::test]
    async fn derived_pending_block_keeps_origin_state() {
        let eth_api = mock_eth_api(AddressMap::default());
        let mut block = Block::default();
        block.header.number = 1;
        block.header.gas_limit = 30_000_000;
        let hash = block.header.hash_slow();
        eth_api.provider().add_block(hash, block);

        let (parent, _, state) =
            eth_api.evm_env_and_recovered_block_at(BlockId::pending()).await.unwrap();
        assert_eq!(parent.hash(), hash);
        assert_eq!(state, BlockId::from(hash));
    }

    type SnapshotProvider = BlockchainProvider<MockNodeTypesWithDB>;
    type SnapshotEthApi = EthApi<
        RpcNodeCoreAdapter<SnapshotProvider, TestPool, NoopNetwork, EthEvmConfig>,
        EthRpcConverter<ChainSpec>,
    >;

    fn snapshot_provider() -> (SnapshotProvider, OverlayManager) {
        // Return SLOAD(0), NUMBER, TIMESTAMP, PREVRANDAO and BASEFEE as five words.
        let code = "6000546000524360205242604052446060524860805260a06000f3".parse().unwrap();
        let contract = Address::with_last_byte(0x42);
        let genesis = Genesis::default()
            .with_gas_limit(30_000_000)
            .with_timestamp(100)
            .with_base_fee(Some(1_000_000_000))
            .extend_accounts([
                (Address::ZERO, GenesisAccount::default().with_balance(U256::from(10u128.pow(18)))),
                (
                    contract,
                    GenesisAccount::default().with_code(Some(code)).with_storage(Some(
                        std::iter::once((B256::ZERO, B256::from(U256::from(7)))).collect(),
                    )),
                ),
            ]);
        let chain_spec =
            Arc::new(ChainSpecBuilder::mainnet().cancun_activated().genesis(genesis).build());
        let overlay = OverlayManager::default();
        let factory = create_test_provider_factory_with_chain_spec(chain_spec)
            .with_overlay_manager(overlay.clone());
        init_genesis(&factory).unwrap();
        (BlockchainProvider::new(factory).unwrap(), overlay)
    }

    fn storage_snapshot(api: &SnapshotEthApi) -> ExecutedBlock {
        let parent = api.provider().latest_header().unwrap().unwrap();
        let mut executed = api.build_block(&parent).unwrap();
        let address = Address::with_last_byte(0x42);
        let state = api.provider().latest().unwrap();
        let info = revm::state::AccountInfo::from(state.basic_account(&address).unwrap().unwrap());
        Arc::make_mut(&mut executed.execution_output).state.state.insert(
            address,
            BundleAccount::new(
                Some(info.clone()),
                Some(info),
                std::iter::once((
                    U256::ZERO,
                    StorageSlot::new_changed(U256::from(7), U256::from(42)),
                ))
                .collect(),
                AccountStatus::Changed,
            ),
        );
        let hashed = state.hashed_post_state(&executed.execution_output.state).unwrap();
        executed.trie_data = LazyTrieData::ready(ComputedTrieData::new(
            Arc::new(hashed.into_sorted()),
            Arc::default(),
        ));
        executed
    }

    async fn snapshot_call(api: &SnapshotEthApi) -> Vec<U256> {
        let simulated = api
            .transact_call_at(
                TransactionRequest::default()
                    .to(Address::with_last_byte(0x42))
                    .gas_price(1_000_000_000),
                BlockId::pending(),
                Default::default(),
            )
            .await
            .unwrap();
        let output = match simulated.result {
            ExecutionResult::Success { output, .. } => output.into_data(),
            other => panic!("pending call failed: {other:?}"),
        };
        output.chunks_exact(32).map(U256::from_be_slice).collect()
    }

    #[tokio::test]
    async fn pending_simulation_uses_cached_block_header_and_state() {
        let (provider, _) = snapshot_provider();
        let api = EthApi::builder(
            provider.clone(),
            testing_pool(),
            NoopNetwork::default(),
            EthEvmConfig::new(provider.chain_spec()),
        )
        .build();
        let executed = storage_snapshot(&api);
        let header = executed.recovered_block.header().clone();
        *api.pending_block().lock().await = Some(PendingBlock::with_executed_block(
            Instant::now() + Duration::from_secs(60),
            executed,
        ));

        assert_eq!(
            snapshot_call(&api).await,
            vec![
                U256::from(42),
                U256::from(header.number),
                U256::from(header.timestamp),
                U256::from_be_bytes(header.mix_hash.0),
                U256::from(header.base_fee_per_gas.unwrap()),
            ]
        );
    }

    struct AdvanceHeadEnvBuilder {
        advanced: AtomicBool,
        advance: Box<dyn Fn() + Send + Sync>,
    }

    impl PendingEnvBuilder<EthEvmConfig> for AdvanceHeadEnvBuilder {
        fn pending_env_attributes(
            &self,
            parent: &reth_primitives_traits::SealedHeader,
            overrides: Option<&alloy_rpc_types_eth::BlockOverrides>,
        ) -> Result<NextBlockEnvAttributes, EthApiError> {
            // Change the head after the environment has selected its parent, before state lookup.
            if !self.advanced.swap(true, Ordering::SeqCst) {
                (self.advance)();
            }
            Ok(NextBlockEnvAttributes::build_pending_env(parent, overrides))
        }
    }

    async fn snapshot_during_head_change(kind: PendingBlockKind) {
        let (provider, overlay) = snapshot_provider();
        let api = EthApi::builder(
            provider.clone(),
            testing_pool(),
            NoopNetwork::default(),
            EthEvmConfig::new(provider.chain_spec()),
        )
        .build();
        let next = storage_snapshot(&api);
        let advance_provider = provider.clone();
        let builder = AdvanceHeadEnvBuilder {
            advanced: AtomicBool::new(false),
            advance: Box::new(move || {
                overlay.insert_block(next.clone());
                let canonical = advance_provider.canonical_in_memory_state();
                canonical.update_chain(NewCanonicalChain::Commit { new: vec![next.clone()] });
                canonical.set_canonical_head(
                    next.recovered_block.sealed_block().sealed_header().clone(),
                );
            }),
        };
        let api = EthApi::builder(
            provider.clone(),
            testing_pool(),
            NoopNetwork::default(),
            EthEvmConfig::new(provider.chain_spec()),
        )
        .pending_block_kind(kind)
        .with_pending_env_builder(builder)
        .build();
        let words = snapshot_call(&api).await;
        if kind.is_none() {
            // The fallback remains pinned to genesis, despite the new canonical head.
            assert_eq!(&words[..3], &[U256::from(7), U256::from(1), U256::from(112)]);
        } else {
            let pending = api.pending_block().lock().await.clone().unwrap();
            let header = pending.block().header();
            assert_eq!(
                words,
                vec![
                    U256::from(42),
                    U256::from(header.number),
                    U256::from(header.timestamp),
                    U256::from_be_bytes(header.mix_hash.0),
                    U256::from(header.base_fee_per_gas.unwrap())
                ]
            );
            assert_eq!(header.number, 2);
        }
    }

    #[tokio::test]
    async fn pending_simulation_keeps_snapshot_when_head_changes() {
        snapshot_during_head_change(PendingBlockKind::Full).await;
    }

    #[tokio::test]
    async fn pending_simulation_fallback_keeps_origin_when_head_changes() {
        snapshot_during_head_change(PendingBlockKind::None).await;
    }

    #[tokio::test]
    async fn pending_simulation_pins_provider_pending_block_hash() {
        let (provider, _) = snapshot_provider();
        let api = EthApi::builder(
            provider.clone(),
            testing_pool(),
            NoopNetwork::default(),
            EthEvmConfig::new(provider.chain_spec()),
        )
        .build();
        let executed = storage_snapshot(&api);
        let header = executed.recovered_block.header().clone();
        let hash = executed.recovered_block.hash();
        provider.canonical_in_memory_state().set_pending_block(executed);
        let (_, at) = api.evm_env_at(BlockId::pending()).await.unwrap();
        assert_eq!(at, BlockId::from(hash));
        assert_eq!(
            snapshot_call(&api).await,
            vec![
                U256::from(42),
                U256::from(header.number),
                U256::from(header.timestamp),
                U256::from_be_bytes(header.mix_hash.0),
                U256::from(header.base_fee_per_gas.unwrap())
            ]
        );
    }
}
