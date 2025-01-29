//! `EthApiBuilder` implementation

use std::sync::Arc;

use reth_provider::{BlockReaderIdExt, StateProviderFactory};
use reth_rpc_eth_types::{EthStateCache, FeeHistoryCache, GasCap, GasPriceOracle};
use reth_rpc_server_types::constants::{
    DEFAULT_ETH_PROOF_WINDOW, DEFAULT_MAX_SIMULATE_BLOCKS, DEFAULT_PROOF_PERMITS,
};
use reth_tasks::{pool::BlockingTaskPool, TokioTaskExecutor};

use crate::{
    eth::{core::EthApiInner, EthTxBuilder},
    EthApi,
};

/// A helper to build the `EthApi` instance.
#[derive(Debug)]
pub struct EthApiBuilder<Provider, Pool, Network, EvmConfig>
where
    Provider: BlockReaderIdExt,
{
    provider: Provider,
    pool: Pool,
    network: Network,
    evm_config: EvmConfig,
    eth_cache: EthStateCache<Provider::Block, Provider::Receipt>,
    gas_oracle: GasPriceOracle<Provider>,
    gas_cap: GasCap,
    max_simulate_blocks: u64,
    eth_proof_window: u64,
    blocking_task_pool: Option<BlockingTaskPool>,
    fee_history_cache: FeeHistoryCache,
    proof_permits: usize,
}

impl<Provider, Pool, Network, EvmConfig> EthApiBuilder<Provider, Pool, Network, EvmConfig>
where
    Provider: BlockReaderIdExt,
{
    /// Creates a new `EthApiBuilder` instance.
    pub fn new(provider: Provider, pool: Pool, network: Network, evm_config: EvmConfig) -> Self
    where
        Provider: BlockReaderIdExt + StateProviderFactory + Clone + Unpin + 'static,
    {
        let cache = EthStateCache::spawn(provider.clone(), Default::default());

        Self {
            provider: provider.clone(),
            pool,
            network,
            evm_config,
            eth_cache: cache.clone(),
            gas_oracle: GasPriceOracle::new(provider, Default::default(), cache),
            gas_cap: GasCap::default(),
            max_simulate_blocks: DEFAULT_MAX_SIMULATE_BLOCKS,
            eth_proof_window: DEFAULT_ETH_PROOF_WINDOW,
            blocking_task_pool: None,
            fee_history_cache: FeeHistoryCache::new(Default::default()),
            proof_permits: DEFAULT_PROOF_PERMITS,
        }
    }

    /// Sets `eth_cache` instance
    pub fn eth_cache(
        mut self,
        eth_cache: EthStateCache<Provider::Block, Provider::Receipt>,
    ) -> Self {
        self.eth_cache = eth_cache;
        self
    }

    /// Sets `gas_oracle` instance
    pub fn gas_oracle(mut self, gas_oracle: GasPriceOracle<Provider>) -> Self {
        self.gas_oracle = gas_oracle;
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

    /// Builds the `EthApiInner` instance
    ///
    /// # Panics
    ///
    /// This function panics if the blocking task pool cannot be built.
    fn build_inner(self) -> EthApiInner<Provider, Pool, Network, EvmConfig> {
        EthApiInner::new(
            self.provider,
            self.pool,
            self.network,
            self.eth_cache,
            self.gas_oracle,
            self.gas_cap,
            self.max_simulate_blocks,
            self.eth_proof_window,
            self.blocking_task_pool
                .unwrap_or(BlockingTaskPool::build().expect("failed to build blocking task pool")),
            self.fee_history_cache,
            self.evm_config,
            TokioTaskExecutor::default(),
            self.proof_permits,
        )
    }

    /// Builds the `EthApi` instance.
    ///
    /// # Panics
    ///
    /// This function panics if the blocking task pool cannot be built.
    pub fn build(self) -> EthApi<Provider, Pool, Network, EvmConfig> {
        EthApi { inner: Arc::new(self.build_inner()), tx_resp_builder: EthTxBuilder }
    }
}
