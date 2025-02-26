//! `EthApiBuilder` implementation

use crate::{
    eth::{core::EthApiInner, EthTxBuilder},
    EthApi,
};
use reth_provider::{BlockReaderIdExt, StateProviderFactory};
use reth_rpc_eth_types::{EthStateCache, FeeHistoryCache, GasCap, GasPriceOracle};
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
pub struct EthApiBuilder<Provider, Pool, Network, EvmConfig>
where
    Provider: BlockReaderIdExt,
{
    provider: Provider,
    pool: Pool,
    network: Network,
    evm_config: EvmConfig,
    gas_cap: GasCap,
    max_simulate_blocks: u64,
    eth_proof_window: u64,
    fee_history_cache: FeeHistoryCache,
    proof_permits: usize,
    eth_cache: Option<EthStateCache<Provider::Block, Provider::Receipt>>,
    gas_oracle: Option<GasPriceOracle<Provider>>,
    blocking_task_pool: Option<BlockingTaskPool>,
    task_spawner: Box<dyn TaskSpawner + 'static>,
}

impl<Provider, Pool, Network, EvmConfig> EthApiBuilder<Provider, Pool, Network, EvmConfig>
where
    Provider: BlockReaderIdExt,
{
    /// Creates a new `EthApiBuilder` instance.
    pub fn new(provider: Provider, pool: Pool, network: Network, evm_config: EvmConfig) -> Self
    where
        Provider: BlockReaderIdExt,
    {
        Self {
            provider,
            pool,
            network,
            evm_config,
            eth_cache: None,
            gas_oracle: None,
            gas_cap: GasCap::default(),
            max_simulate_blocks: DEFAULT_MAX_SIMULATE_BLOCKS,
            eth_proof_window: DEFAULT_ETH_PROOF_WINDOW,
            blocking_task_pool: None,
            fee_history_cache: FeeHistoryCache::new(Default::default()),
            proof_permits: DEFAULT_PROOF_PERMITS,
            task_spawner: TokioTaskExecutor::default().boxed(),
        }
    }

    /// Configures the task spawner used to spawn additional tasks.
    pub fn task_spawner(mut self, spawner: impl TaskSpawner + 'static) -> Self {
        self.task_spawner = Box::new(spawner);
        self
    }

    /// Sets `eth_cache` instance
    pub fn eth_cache(
        mut self,
        eth_cache: EthStateCache<Provider::Block, Provider::Receipt>,
    ) -> Self {
        self.eth_cache = Some(eth_cache);
        self
    }

    /// Sets `gas_oracle` instance
    pub fn gas_oracle(mut self, gas_oracle: GasPriceOracle<Provider>) -> Self {
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
    pub fn fee_history_cache(mut self, fee_history_cache: FeeHistoryCache) -> Self {
        self.fee_history_cache = fee_history_cache;
        self
    }

    /// Sets the proof permits.
    pub const fn proof_permits(mut self, proof_permits: usize) -> Self {
        self.proof_permits = proof_permits;
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
    pub fn build_inner(self) -> EthApiInner<Provider, Pool, Network, EvmConfig>
    where
        Provider: BlockReaderIdExt + StateProviderFactory + Clone + Unpin + 'static,
    {
        let Self {
            provider,
            pool,
            network,
            evm_config,
            eth_cache,
            gas_oracle,
            gas_cap,
            max_simulate_blocks,
            eth_proof_window,
            blocking_task_pool,
            fee_history_cache,
            proof_permits,
            task_spawner,
        } = self;

        let eth_cache =
            eth_cache.unwrap_or_else(|| EthStateCache::spawn(provider.clone(), Default::default()));
        let gas_oracle = gas_oracle.unwrap_or_else(|| {
            GasPriceOracle::new(provider.clone(), Default::default(), eth_cache.clone())
        });

        EthApiInner::new(
            provider,
            pool,
            network,
            eth_cache,
            gas_oracle,
            gas_cap,
            max_simulate_blocks,
            eth_proof_window,
            blocking_task_pool.unwrap_or_else(|| {
                BlockingTaskPool::build().expect("failed to build blocking task pool")
            }),
            fee_history_cache,
            evm_config,
            task_spawner,
            proof_permits,
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
    pub fn build(self) -> EthApi<Provider, Pool, Network, EvmConfig>
    where
        Provider: BlockReaderIdExt + StateProviderFactory + Clone + Unpin + 'static,
    {
        EthApi { inner: Arc::new(self.build_inner()), tx_resp_builder: EthTxBuilder }
    }
}
