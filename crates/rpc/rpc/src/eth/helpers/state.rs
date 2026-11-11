//! Contains RPC handler implementations specific to state.

use crate::EthApi;
use alloy_eips::BlockId;
use alloy_primitives::{Address, U256};
use reth_rpc_convert::RpcConvert;
use reth_rpc_eth_api::{
    helpers::{EthState, LoadPendingBlock, LoadState, SpawnBlocking},
    FromEthApiError, RpcNodeCore,
};
use reth_rpc_eth_types::EthApiError;
use reth_storage_api::StateProviderFactory;

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
    /// Reads the nonce from the provider's state instead of a locally built pending block.
    ///
    /// A local pending block only applies pool transactions on top of the latest state, and the
    /// pool lookup for the pending tag already accounts for the sender's transactions, so
    /// building the block and its state overlay only to read one nonce is redundant.
    fn transaction_count(
        &self,
        address: Address,
        block_id: Option<BlockId>,
    ) -> impl Future<Output = Result<U256, Self::Error>> + Send
    where
        Self: SpawnBlocking,
    {
        let at = block_id.unwrap_or_default();
        self.spawn_blocking_io(move |this| {
            let state = if at.is_latest() {
                this.latest_state()?
            } else {
                this.provider().state_by_block_id(at).map_err(Self::Error::from_eth_err)?
            };
            this.transaction_count_with_state(address, at, &state)
        })
    }
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
        helpers::{pending_block::PendingEnvBuilder, EthCall, EthState, SpawnBlocking},
        node::{RpcNodeCoreAdapter, RpcNodeCoreExt},
        EthApiTypes,
    };
    use reth_rpc_eth_types::{EthApiSettings, EthStateCache, PendingBlock};
    use reth_rpc_server_types::constants::DEFAULT_MAX_PROOF_SLOTS;
    use reth_storage_api::{StateProviderBox, StateProviderFactory};
    use reth_tasks::{
        pool::{BlockingTaskGuard, BlockingTaskPool},
        Runtime,
    };
    use reth_transaction_pool::{
        test_utils::{testing_pool, MockTransaction, TestPool},
        TransactionOrigin, TransactionPool,
    };
    use std::{future::Future, sync::Arc, thread, time::Duration};
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
    async fn pending_transaction_count_skips_local_pending_block() {
        let address = Address::random();
        let accounts = AddressMap::from_iter([(address, ExtendedAccount::new(7, U256::ZERO))]);
        let eth_api = mock_eth_api(accounts);
        let mut block = Block::default();
        block.header.number = 13_000_000;
        block.header.timestamp = 1_629_000_000;
        block.header.gas_limit = 30_000_000;
        block.header.base_fee_per_gas = Some(1_000_000_000);
        eth_api.provider().add_block(B256::ZERO, block);

        let pending = Some(BlockId::pending());
        let count = EthState::transaction_count(&eth_api, address, pending).await.unwrap();
        assert_eq!(count, U256::from(7));

        // Nonces 7 and 8 follow the account nonce, nonce 10 is gapped.
        for nonce in [7, 8, 10] {
            let tx = MockTransaction::eip1559().with_sender(address).with_nonce(nonce);
            eth_api.pool().add_transaction(TransactionOrigin::External, tx).await.unwrap();
        }
        let count = EthState::transaction_count(&eth_api, address, pending).await.unwrap();
        assert_eq!(count, U256::from(9));
        assert_eq!(
            EthState::transaction_count(&eth_api, address, None).await.unwrap(),
            U256::from(7)
        );

        assert!(eth_api.pending_block().lock().await.is_none());
    }

    /// Returns the id of the thread that `f` of a point read at `at` runs on.
    async fn point_read_thread(eth_api: &MockEthApi, at: BlockId) -> thread::ThreadId {
        eth_api.spawn_point_read_with_state(at, |_, _| Ok(thread::current().id())).await.unwrap()
    }

    #[tokio::test]
    async fn point_read_at_latest_runs_inline() {
        let eth_api = mock_eth_api(AddressMap::default());

        assert_eq!(point_read_thread(&eth_api, BlockId::latest()).await, thread::current().id());
        // The permit is released after the read.
        assert!(eth_api.inline_io_request_semaphore().available_permits() > 0);
    }

    #[tokio::test]
    async fn point_read_without_inline_permit_uses_blocking_pool() {
        let eth_api = mock_eth_api(AddressMap::default());
        let permits =
            u32::try_from(eth_api.inline_io_request_semaphore().available_permits()).unwrap();
        let _permits = eth_api.inline_io_request_semaphore().try_acquire_many(permits).unwrap();

        assert_ne!(point_read_thread(&eth_api, BlockId::latest()).await, thread::current().id());
    }

    #[tokio::test]
    async fn point_read_at_historical_block_uses_blocking_pool() {
        let eth_api = mock_eth_api(AddressMap::default());

        assert_ne!(point_read_thread(&eth_api, BlockId::earliest()).await, thread::current().id());
    }
}
