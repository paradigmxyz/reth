//! The entire implementation of the namespace is quite large, hence it is divided across several
//! files.

use std::{fmt::Debug, sync::Arc};

use async_trait::async_trait;
use reth_errors::{RethError, RethResult};
use reth_evm::ConfigureEvm;
use reth_network_api::NetworkInfo;
use reth_primitives::{Address, BlockNumberOrTag, ChainInfo, U256, U64};
use reth_provider::{BlockReaderIdExt, ChainSpecProvider, EvmEnvProvider, StateProviderFactory};
use reth_rpc_types::{SyncInfo, SyncStatus};
use reth_tasks::{pool::BlockingTaskPool, TaskSpawner, TokioTaskExecutor};
use reth_transaction_pool::TransactionPool;
use tokio::sync::Mutex;

use crate::eth::{
    api::fee_history::FeeHistoryCache, cache::EthStateCache, gas_oracle::GasPriceOracle,
    signer::EthSigner,
};

pub mod block;
mod call;
pub(crate) mod fee_history;
mod fees;
pub mod pending_block;
pub mod receipt;
mod server;
mod sign;
mod state;
pub mod traits;
pub mod transactions;

use crate::eth::traits::RawTransactionForwarder;
pub use pending_block::PendingBlock;
pub use receipt::ReceiptBuilder;
pub use traits::{
    BuildReceipt, EthBlocks, EthState, EthTransactions, LoadPendingBlock, LoadState, SpawnBlocking,
    StateCacheDB,
};
pub use transactions::TransactionSource;

/// `Eth` API trait.
///
/// Defines core functionality of the `eth` API implementation.
#[async_trait]
pub trait EthApiSpec: EthTransactions + Send + Sync {
    /// Returns the current ethereum protocol version.
    async fn protocol_version(&self) -> RethResult<U64>;

    /// Returns the chain id
    fn chain_id(&self) -> U64;

    /// Returns provider chain info
    fn chain_info(&self) -> RethResult<ChainInfo>;

    /// Returns a list of addresses owned by provider.
    fn accounts(&self) -> Vec<Address>;

    /// Returns `true` if the network is undergoing sync.
    fn is_syncing(&self) -> bool;

    /// Returns the [SyncStatus] of the network
    fn sync_status(&self) -> RethResult<SyncStatus>;
}

/// `Eth` API implementation.
///
/// This type provides the functionality for handling `eth_` related requests.
/// These are implemented two-fold: Core functionality is implemented as [`EthApiSpec`]
/// trait. Additionally, the required server implementations (e.g. [`reth_rpc_api::EthApiServer`])
/// are implemented separately in submodules. The rpc handler implementation can then delegate to
/// the main impls. This way [`EthApi`] is not limited to [`jsonrpsee`] and can be used standalone
/// or in other network handlers (for example ipc).
pub struct EthApi<Provider, Pool, Network, EvmConfig> {
    /// All nested fields bundled together.
    inner: Arc<EthApiInner<Provider, Pool, Network, EvmConfig>>,
}

impl<Provider, Pool, Network, EvmConfig> EthApi<Provider, Pool, Network, EvmConfig>
where
    Provider: BlockReaderIdExt + ChainSpecProvider,
{
    /// Creates a new, shareable instance using the default tokio task spawner.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        provider: Provider,
        pool: Pool,
        network: Network,
        eth_cache: EthStateCache,
        gas_oracle: GasPriceOracle<Provider>,
        gas_cap: impl Into<GasCap>,
        blocking_task_pool: BlockingTaskPool,
        fee_history_cache: FeeHistoryCache,
        evm_config: EvmConfig,
        raw_transaction_forwarder: Option<Arc<dyn RawTransactionForwarder>>,
    ) -> Self {
        Self::with_spawner(
            provider,
            pool,
            network,
            eth_cache,
            gas_oracle,
            gas_cap.into().into(),
            Box::<TokioTaskExecutor>::default(),
            blocking_task_pool,
            fee_history_cache,
            evm_config,
            raw_transaction_forwarder,
        )
    }

    /// Creates a new, shareable instance.
    #[allow(clippy::too_many_arguments)]
    pub fn with_spawner(
        provider: Provider,
        pool: Pool,
        network: Network,
        eth_cache: EthStateCache,
        gas_oracle: GasPriceOracle<Provider>,
        gas_cap: u64,
        task_spawner: Box<dyn TaskSpawner>,
        blocking_task_pool: BlockingTaskPool,
        fee_history_cache: FeeHistoryCache,
        evm_config: EvmConfig,
        raw_transaction_forwarder: Option<Arc<dyn RawTransactionForwarder>>,
    ) -> Self {
        // get the block number of the latest block
        let latest_block = provider
            .header_by_number_or_tag(BlockNumberOrTag::Latest)
            .ok()
            .flatten()
            .map(|header| header.number)
            .unwrap_or_default();

        let inner = EthApiInner {
            provider,
            pool,
            network,
            signers: parking_lot::RwLock::new(Default::default()),
            eth_cache,
            gas_oracle,
            gas_cap,
            starting_block: U256::from(latest_block),
            task_spawner,
            pending_block: Default::default(),
            blocking_task_pool,
            fee_history_cache,
            evm_config,
            raw_transaction_forwarder,
        };

        Self { inner: Arc::new(inner) }
    }

    /// Returns the state cache frontend
    pub fn cache(&self) -> &EthStateCache {
        &self.inner.eth_cache
    }

    /// Returns the gas oracle frontend
    pub(crate) fn gas_oracle(&self) -> &GasPriceOracle<Provider> {
        &self.inner.gas_oracle
    }

    /// Returns the configured gas limit cap for `eth_call` and tracing related calls
    pub fn gas_cap(&self) -> u64 {
        self.inner.gas_cap
    }

    /// Returns the inner `Provider`
    pub fn provider(&self) -> &Provider {
        &self.inner.provider
    }

    /// Returns the inner `Network`
    pub fn network(&self) -> &Network {
        &self.inner.network
    }

    /// Returns the inner `Pool`
    pub fn pool(&self) -> &Pool {
        &self.inner.pool
    }

    /// Returns fee history cache
    pub fn fee_history_cache(&self) -> &FeeHistoryCache {
        &self.inner.fee_history_cache
    }
}

impl<Provider, Pool, Network, EvmConfig> std::fmt::Debug
    for EthApi<Provider, Pool, Network, EvmConfig>
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EthApi").finish_non_exhaustive()
    }
}

impl<Provider, Pool, Network, EvmConfig> Clone for EthApi<Provider, Pool, Network, EvmConfig> {
    fn clone(&self) -> Self {
        Self { inner: Arc::clone(&self.inner) }
    }
}

#[async_trait]
impl<Provider, Pool, Network, EvmConfig> EthApiSpec for EthApi<Provider, Pool, Network, EvmConfig>
where
    Pool: TransactionPool + Clone + 'static,
    Provider:
        BlockReaderIdExt + ChainSpecProvider + StateProviderFactory + EvmEnvProvider + 'static,
    Network: NetworkInfo + 'static,
    EvmConfig: ConfigureEvm + 'static,
{
    /// Returns the current ethereum protocol version.
    ///
    /// Note: This returns an `U64`, since this should return as hex string.
    async fn protocol_version(&self) -> RethResult<U64> {
        let status = self.network().network_status().await.map_err(RethError::other)?;
        Ok(U64::from(status.protocol_version))
    }

    /// Returns the chain id
    fn chain_id(&self) -> U64 {
        U64::from(self.network().chain_id())
    }

    /// Returns the current info for the chain
    fn chain_info(&self) -> RethResult<ChainInfo> {
        Ok(self.provider().chain_info()?)
    }

    fn accounts(&self) -> Vec<Address> {
        self.inner.signers.read().iter().flat_map(|s| s.accounts()).collect()
    }

    fn is_syncing(&self) -> bool {
        self.network().is_syncing()
    }

    /// Returns the [SyncStatus] of the network
    fn sync_status(&self) -> RethResult<SyncStatus> {
        let status = if self.is_syncing() {
            let current_block = U256::from(
                self.provider().chain_info().map(|info| info.best_number).unwrap_or_default(),
            );
            SyncStatus::Info(SyncInfo {
                starting_block: self.inner.starting_block,
                current_block,
                highest_block: current_block,
                warp_chunks_amount: None,
                warp_chunks_processed: None,
            })
        } else {
            SyncStatus::None
        };
        Ok(status)
    }
}

impl<Provider, Pool, Network, EvmConfig> SpawnBlocking
    for EthApi<Provider, Pool, Network, EvmConfig>
where
    Self: Send + Sync + 'static,
{
    fn io_task_spawner(&self) -> &dyn TaskSpawner {
        self.inner.task_spawner()
    }

    fn tracing_task_pool(&self) -> &BlockingTaskPool {
        self.inner.blocking_task_pool()
    }
}

/// The default gas limit for `eth_call` and adjacent calls.
///
/// This is different from the default to regular 30M block gas limit
/// [`ETHEREUM_BLOCK_GAS_LIMIT`](reth_primitives::constants::ETHEREUM_BLOCK_GAS_LIMIT) to allow for
/// more complex calls.
pub const RPC_DEFAULT_GAS_CAP: GasCap = GasCap(50_000_000);

/// The wrapper type for gas limit
#[derive(Debug, Clone, Copy)]
pub struct GasCap(u64);

impl Default for GasCap {
    fn default() -> Self {
        RPC_DEFAULT_GAS_CAP
    }
}

impl From<u64> for GasCap {
    fn from(gas_cap: u64) -> Self {
        Self(gas_cap)
    }
}

impl From<GasCap> for u64 {
    fn from(gas_cap: GasCap) -> Self {
        gas_cap.0
    }
}

/// Container type `EthApi`
#[allow(missing_debug_implementations)]
pub struct EthApiInner<Provider, Pool, Network, EvmConfig> {
    /// The transaction pool.
    pool: Pool,
    /// The provider that can interact with the chain.
    provider: Provider,
    /// An interface to interact with the network
    network: Network,
    /// All configured Signers
    signers: parking_lot::RwLock<Vec<Box<dyn EthSigner>>>,
    /// The async cache frontend for eth related data
    eth_cache: EthStateCache,
    /// The async gas oracle frontend for gas price suggestions
    gas_oracle: GasPriceOracle<Provider>,
    /// Maximum gas limit for `eth_call` and call tracing RPC methods.
    gas_cap: u64,
    /// The block number at which the node started
    starting_block: U256,
    /// The type that can spawn tasks which would otherwise block.
    task_spawner: Box<dyn TaskSpawner>,
    /// Cached pending block if any
    pending_block: Mutex<Option<PendingBlock>>,
    /// A pool dedicated to CPU heavy blocking tasks.
    blocking_task_pool: BlockingTaskPool,
    /// Cache for block fees history
    fee_history_cache: FeeHistoryCache,
    /// The type that defines how to configure the EVM
    evm_config: EvmConfig,
    /// Allows forwarding received raw transactions
    raw_transaction_forwarder: Option<Arc<dyn RawTransactionForwarder>>,
}

impl<Provider, Pool, Network, EvmConfig> EthApiInner<Provider, Pool, Network, EvmConfig> {
    /// Returns a handle to data on disk.
    #[inline]
    pub const fn provider(&self) -> &Provider {
        &self.provider
    }

    /// Returns a handle to data in memory.
    #[inline]
    pub const fn cache(&self) -> &EthStateCache {
        &self.eth_cache
    }

    /// Returns a handle to the pending block.
    #[inline]
    pub const fn pending_block(&self) -> &Mutex<Option<PendingBlock>> {
        &self.pending_block
    }

    /// Returns a handle to the task spawner.
    #[inline]
    pub const fn task_spawner(&self) -> &dyn TaskSpawner {
        &*self.task_spawner
    }

    /// Returns a handle to the blocking thread pool.
    #[inline]
    pub const fn blocking_task_pool(&self) -> &BlockingTaskPool {
        &self.blocking_task_pool
    }

    /// Returns a handle to the EVM config.
    #[inline]
    pub const fn evm_config(&self) -> &EvmConfig {
        &self.evm_config
    }

    /// Returns a handle to the transaction pool.
    #[inline]
    pub const fn pool(&self) -> &Pool {
        &self.pool
    }

    /// Returns a handle to the transaction forwarder.
    #[inline]
    pub const fn raw_tx_forwarder(&self) -> &Option<Arc<dyn RawTransactionForwarder>> {
        &self.raw_transaction_forwarder
    }
}
