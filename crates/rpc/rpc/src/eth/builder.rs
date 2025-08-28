//! `EthApiBuilder` implementation

use crate::{eth::core::EthApiInner, EthApi};
use alloy_network::Ethereum;
use reth_chain_state::CanonStateSubscriptions;
use reth_chainspec::ChainSpecProvider;
use reth_primitives_traits::HeaderTy;
use reth_rpc_convert::{RpcConvert, RpcConverter};
use reth_rpc_eth_api::{
    helpers::pending_block::PendingEnvBuilder, node::RpcNodeCoreAdapter, RpcNodeCore,
};
use reth_rpc_eth_types::{
    builder::config::PendingBlockKind, fee_history::fee_history_cache_new_blocks_task,
    receipt::EthReceiptConverter, EthStateCache, EthStateCacheConfig, FeeHistoryCache,
    FeeHistoryCacheConfig, ForwardConfig, GasCap, GasPriceOracle, GasPriceOracleConfig,
};
use reth_rpc_server_types::constants::{
    DEFAULT_ETH_PROOF_WINDOW, DEFAULT_MAX_SIMULATE_BLOCKS, DEFAULT_PROOF_PERMITS,
};
use reth_tasks::{pool::BlockingTaskPool, TaskSpawner, TokioTaskExecutor};
use std::sync::Arc;

/// A helper to build the `EthApi` handler instance.
///
/// This builder type contains all settings to create an [`EthApiInner`] or an [`EthApi`] instance
/// directly.
#[derive(Debug)]
pub struct EthApiBuilder<N: RpcNodeCore, Rpc, NextEnv = ()> {
    components: N,
    rpc_converter: Rpc,
    gas_cap: GasCap,
    max_simulate_blocks: u64,
    eth_proof_window: u64,
    fee_history_cache_config: FeeHistoryCacheConfig,
    proof_permits: usize,
    eth_state_cache_config: EthStateCacheConfig,
    eth_cache: Option<EthStateCache<N::Primitives>>,
    gas_oracle_config: GasPriceOracleConfig,
    gas_oracle: Option<GasPriceOracle<N::Provider>>,
    blocking_task_pool: Option<BlockingTaskPool>,
    task_spawner: Box<dyn TaskSpawner + 'static>,
    next_env: NextEnv,
    max_batch_size: usize,
    pending_block_kind: PendingBlockKind,
    raw_tx_forwarder: ForwardConfig,
}

impl<Provider, Pool, Network, EvmConfig, ChainSpec>
    EthApiBuilder<
        RpcNodeCoreAdapter<Provider, Pool, Network, EvmConfig>,
        RpcConverter<Ethereum, EvmConfig, EthReceiptConverter<ChainSpec>>,
    >
where
    RpcNodeCoreAdapter<Provider, Pool, Network, EvmConfig>:
        RpcNodeCore<Provider: ChainSpecProvider<ChainSpec = ChainSpec>, Evm = EvmConfig>,
{
    /// Creates a new `EthApiBuilder` instance.
    pub fn new(provider: Provider, pool: Pool, network: Network, evm_config: EvmConfig) -> Self {
        Self::new_with_components(RpcNodeCoreAdapter::new(provider, pool, network, evm_config))
    }
}

impl<N: RpcNodeCore, Rpc, NextEnv> EthApiBuilder<N, Rpc, NextEnv> {
    /// Converts the RPC converter type of this builder
    pub fn map_converter<F, R>(self, f: F) -> EthApiBuilder<N, R, NextEnv>
    where
        F: FnOnce(Rpc) -> R,
    {
        let Self {
            components,
            rpc_converter,
            gas_cap,
            max_simulate_blocks,
            eth_proof_window,
            fee_history_cache_config,
            proof_permits,
            eth_state_cache_config,
            eth_cache,
            gas_oracle_config,
            gas_oracle,
            blocking_task_pool,
            task_spawner,
            next_env,
            max_batch_size,
            pending_block_kind,
            raw_tx_forwarder,
        } = self;
        EthApiBuilder {
            components,
            rpc_converter: f(rpc_converter),
            gas_cap,
            max_simulate_blocks,
            eth_proof_window,
            fee_history_cache_config,
            proof_permits,
            eth_state_cache_config,
            eth_cache,
            gas_oracle_config,
            gas_oracle,
            blocking_task_pool,
            task_spawner,
            next_env,
            max_batch_size,
            pending_block_kind,
            raw_tx_forwarder,
        }
    }
}

impl<N, ChainSpec> EthApiBuilder<N, RpcConverter<Ethereum, N::Evm, EthReceiptConverter<ChainSpec>>>
where
    N: RpcNodeCore<Provider: ChainSpecProvider<ChainSpec = ChainSpec>>,
{
    /// Creates a new `EthApiBuilder` instance with the provided components.
    pub fn new_with_components(components: N) -> Self {
        let rpc_converter =
            RpcConverter::new(EthReceiptConverter::new(components.provider().chain_spec()));
        Self {
            components,
            rpc_converter,
            eth_cache: None,
            gas_oracle: None,
            gas_cap: GasCap::default(),
            max_simulate_blocks: DEFAULT_MAX_SIMULATE_BLOCKS,
            eth_proof_window: DEFAULT_ETH_PROOF_WINDOW,
            blocking_task_pool: None,
            fee_history_cache_config: FeeHistoryCacheConfig::default(),
            proof_permits: DEFAULT_PROOF_PERMITS,
            task_spawner: TokioTaskExecutor::default().boxed(),
            gas_oracle_config: Default::default(),
            eth_state_cache_config: Default::default(),
            next_env: Default::default(),
            max_batch_size: 1,
            pending_block_kind: PendingBlockKind::Full,
            raw_tx_forwarder: ForwardConfig::default(),
        }
    }
}

impl<N, Rpc, NextEnv> EthApiBuilder<N, Rpc, NextEnv>
where
    N: RpcNodeCore,
{
    /// Configures the task spawner used to spawn additional tasks.
    pub fn task_spawner(mut self, spawner: impl TaskSpawner + 'static) -> Self {
        self.task_spawner = Box::new(spawner);
        self
    }

    /// Changes the configured converter.
    pub fn with_rpc_converter<RpcNew>(
        self,
        rpc_converter: RpcNew,
    ) -> EthApiBuilder<N, RpcNew, NextEnv> {
        let Self {
            components,
            rpc_converter: _,
            gas_cap,
            max_simulate_blocks,
            eth_proof_window,
            fee_history_cache_config,
            proof_permits,
            eth_state_cache_config,
            eth_cache,
            gas_oracle,
            blocking_task_pool,
            task_spawner,
            gas_oracle_config,
            next_env,
            max_batch_size,
            pending_block_kind,
            raw_tx_forwarder,
        } = self;
        EthApiBuilder {
            components,
            rpc_converter,
            gas_cap,
            max_simulate_blocks,
            eth_proof_window,
            fee_history_cache_config,
            proof_permits,
            eth_state_cache_config,
            eth_cache,
            gas_oracle,
            blocking_task_pool,
            task_spawner,
            gas_oracle_config,
            next_env,
            max_batch_size,
            pending_block_kind,
            raw_tx_forwarder,
        }
    }

    /// Changes the configured pending environment builder.
    pub fn with_pending_env_builder<NextEnvNew>(
        self,
        next_env: NextEnvNew,
    ) -> EthApiBuilder<N, Rpc, NextEnvNew> {
        let Self {
            components,
            rpc_converter,
            gas_cap,
            max_simulate_blocks,
            eth_proof_window,
            fee_history_cache_config,
            proof_permits,
            eth_state_cache_config,
            eth_cache,
            gas_oracle,
            blocking_task_pool,
            task_spawner,
            gas_oracle_config,
            next_env: _,
            max_batch_size,
            pending_block_kind,
            raw_tx_forwarder,
        } = self;
        EthApiBuilder {
            components,
            rpc_converter,
            gas_cap,
            max_simulate_blocks,
            eth_proof_window,
            fee_history_cache_config,
            proof_permits,
            eth_state_cache_config,
            eth_cache,
            gas_oracle,
            blocking_task_pool,
            task_spawner,
            gas_oracle_config,
            next_env,
            max_batch_size,
            pending_block_kind,
            raw_tx_forwarder,
        }
    }

    /// Sets `eth_cache` config for the cache that will be used if no [`EthStateCache`] is
    /// configured.
    pub const fn eth_state_cache_config(
        mut self,
        eth_state_cache_config: EthStateCacheConfig,
    ) -> Self {
        self.eth_state_cache_config = eth_state_cache_config;
        self
    }

    /// Sets `eth_cache` instance
    pub fn eth_cache(mut self, eth_cache: EthStateCache<N::Primitives>) -> Self {
        self.eth_cache = Some(eth_cache);
        self
    }

    /// Sets `gas_oracle` config for the gas oracle that will be used if no [`GasPriceOracle`] is
    /// configured.
    pub const fn gas_oracle_config(mut self, gas_oracle_config: GasPriceOracleConfig) -> Self {
        self.gas_oracle_config = gas_oracle_config;
        self
    }

    /// Sets `gas_oracle` instance
    pub fn gas_oracle(mut self, gas_oracle: GasPriceOracle<N::Provider>) -> Self {
        self.gas_oracle = Some(gas_oracle);
        self
    }

    /// Sets the gas cap.
    pub const fn gas_cap(mut self, gas_cap: GasCap) -> Self {
        self.gas_cap = gas_cap;
        self
    }

    /// Sets the maximum number of blocks for `eth_simulateV1`.
    pub const fn max_simulate_blocks(mut self, max_simulate_blocks: u64) -> Self {
        self.max_simulate_blocks = max_simulate_blocks;
        self
    }

    /// Sets the maximum number of blocks into the past for generating state proofs.
    pub const fn eth_proof_window(mut self, eth_proof_window: u64) -> Self {
        self.eth_proof_window = eth_proof_window;
        self
    }

    /// Sets the blocking task pool.
    pub fn blocking_task_pool(mut self, blocking_task_pool: BlockingTaskPool) -> Self {
        self.blocking_task_pool = Some(blocking_task_pool);
        self
    }

    /// Sets the fee history cache.
    pub const fn fee_history_cache_config(
        mut self,
        fee_history_cache_config: FeeHistoryCacheConfig,
    ) -> Self {
        self.fee_history_cache_config = fee_history_cache_config;
        self
    }

    /// Sets the proof permits.
    pub const fn proof_permits(mut self, proof_permits: usize) -> Self {
        self.proof_permits = proof_permits;
        self
    }

    /// Sets the max batch size for batching transaction insertions.
    pub const fn max_batch_size(mut self, max_batch_size: usize) -> Self {
        self.max_batch_size = max_batch_size;
        self
    }

    /// Sets the pending block kind
    pub const fn pending_block_kind(mut self, pending_block_kind: PendingBlockKind) -> Self {
        self.pending_block_kind = pending_block_kind;
        self
    }

    /// Sets the raw transaction forwarder.
    pub fn raw_tx_forwarder(mut self, tx_forwarder: ForwardConfig) -> Self {
        self.raw_tx_forwarder = tx_forwarder;
        self
    }

    /// Builds the [`EthApiInner`] instance.
    ///
    /// If not configured, this will spawn the cache backend: [`EthStateCache::spawn`].
    ///
    /// # Panics
    ///
    /// This function panics if the blocking task pool cannot be built.
    /// This will panic if called outside the context of a Tokio runtime.
    pub fn build_inner(self) -> EthApiInner<N, Rpc>
    where
        Rpc: RpcConvert,
        NextEnv: PendingEnvBuilder<N::Evm>,
    {
        let Self {
            components,
            rpc_converter,
            eth_state_cache_config,
            gas_oracle_config,
            eth_cache,
            gas_oracle,
            gas_cap,
            max_simulate_blocks,
            eth_proof_window,
            blocking_task_pool,
            fee_history_cache_config,
            proof_permits,
            task_spawner,
            next_env,
            max_batch_size,
            pending_block_kind,
            raw_tx_forwarder,
        } = self;

        let provider = components.provider().clone();

        let eth_cache = eth_cache
            .unwrap_or_else(|| EthStateCache::spawn(provider.clone(), eth_state_cache_config));
        let gas_oracle = gas_oracle.unwrap_or_else(|| {
            GasPriceOracle::new(provider.clone(), gas_oracle_config, eth_cache.clone())
        });
        let fee_history_cache =
            FeeHistoryCache::<HeaderTy<N::Primitives>>::new(fee_history_cache_config);
        let new_canonical_blocks = provider.canonical_state_stream();
        let fhc = fee_history_cache.clone();
        let cache = eth_cache.clone();
        task_spawner.spawn_critical(
            "cache canonical blocks for fee history task",
            Box::pin(async move {
                fee_history_cache_new_blocks_task(fhc, new_canonical_blocks, provider, cache).await;
            }),
        );

        EthApiInner::new(
            components,
            eth_cache,
            gas_oracle,
            gas_cap,
            max_simulate_blocks,
            eth_proof_window,
            blocking_task_pool.unwrap_or_else(|| {
                BlockingTaskPool::build().expect("failed to build blocking task pool")
            }),
            fee_history_cache,
            task_spawner,
            proof_permits,
            rpc_converter,
            next_env,
            max_batch_size,
            pending_block_kind,
            raw_tx_forwarder.forwarder_client(),
        )
    }

    /// Builds the [`EthApi`] instance.
    ///
    /// If not configured, this will spawn the cache backend: [`EthStateCache::spawn`].
    ///
    /// # Panics
    ///
    /// This function panics if the blocking task pool cannot be built.
    /// This will panic if called outside the context of a Tokio runtime.
    pub fn build(self) -> EthApi<N, Rpc>
    where
        Rpc: RpcConvert,
        NextEnv: PendingEnvBuilder<N::Evm>,
    {
        EthApi { inner: Arc::new(self.build_inner()) }
    }
}
