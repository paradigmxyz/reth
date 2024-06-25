//! Implementation of the [`jsonrpsee`] generated [`EthApiServer`](crate::EthApi) trait
//! Handles RPC requests for the `eth_` namespace.

use std::{ops::Deref, sync::Arc};

use derive_more::Deref;
use reth_primitives::{BlockNumberOrTag, U256};
use reth_provider::{BlockReaderIdExt, ChainSpecProvider};

pub mod bundle;
pub mod filter;
pub mod helpers;
pub mod pubsub;

mod server;

pub use helpers::{
    signer::DevSigner,
    traits::{
        block::{EthBlocks, LoadBlock},
        blocking_task::SpawnBlocking,
        call::{Call, EthCall},
        fee::{EthFees, LoadFee},
        pending_block::LoadPendingBlock,
        receipt::LoadReceipt,
        signer::EthSigner,
        spec::EthApiSpec,
        state::{EthState, LoadState},
        trace::Trace,
        transaction::{
            EthTransactions, LoadTransaction, RawTransactionForwarder, UpdateRawTxForwarder,
        },
        EthApiServerComponents, TraceExt,
    },
};
use reth_tasks::{pool::BlockingTaskPool, TaskSpawner, TokioTaskExecutor};
use tokio::sync::Mutex;

use crate::{EthStateCache, FeeHistoryCache, GasCap, GasPriceOracle, PendingBlock};

/// `Eth` API implementation.
///
/// This type provides the functionality for handling `eth_` related requests.
/// These are implemented two-fold: Core functionality is implemented as [`EthApiSpec`]
/// trait. Additionally, the required server implementations (e.g.
/// [`EthApiServer`](crate::EthApiServer)) are implemented separately in submodules. The rpc handler
/// implementation can then delegate to the main impls. This way [`EthApi`] is not limited to
/// [`jsonrpsee`] and can be used standalone or in other network handlers (for example ipc).
#[derive(Deref)]
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
            raw_transaction_forwarder: parking_lot::RwLock::new(raw_transaction_forwarder),
        };

        Self { inner: Arc::new(inner) }
    }

    /// Returns the state cache frontend
    pub fn cache(&self) -> &EthStateCache {
        &self.inner.eth_cache
    }

    /// Returns the gas oracle frontend
    pub fn gas_oracle(&self) -> &GasPriceOracle<Provider> {
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

/// Implements [`SpawnBlocking`] for a type, that has similar data layout to [`EthApi`].
#[macro_export]
macro_rules! spawn_blocking_impl {
    ($network_api:ty) => {
        impl<Provider, Pool, Network, EvmConfig> $crate::servers::SpawnBlocking for $network_api
        where
            Self: Clone + Send + Sync + 'static,
        {
            #[inline]
            fn io_task_spawner(&self) -> impl reth_tasks::TaskSpawner {
                self.inner.task_spawner()
            }

            #[inline]
            fn tracing_task_pool(&self) -> &reth_tasks::pool::BlockingTaskPool {
                self.inner.blocking_task_pool()
            }
        }
    };
}

spawn_blocking_impl!(EthApi<Provider, Pool, Network, EvmConfig>);

impl<Provider, Pool, Network, EvmConfig> EthApi<Provider, Pool, Network, EvmConfig> {
    /// Generates 20 random developer accounts.
    /// Used in DEV mode.
    pub fn with_dev_accounts(&self) {
        let mut signers = self.inner.signers.write();
        *signers = DevSigner::random_signers(20);
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
    raw_transaction_forwarder: parking_lot::RwLock<Option<Arc<dyn RawTransactionForwarder>>>,
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
    pub fn raw_tx_forwarder(&self) -> Option<Arc<dyn RawTransactionForwarder>> {
        self.raw_transaction_forwarder.read().clone()
    }

    /// Returns the gas cap.
    #[inline]
    pub const fn gas_cap(&self) -> u64 {
        self.gas_cap
    }

    /// Returns a handle to the gas oracle.
    #[inline]
    pub const fn gas_oracle(&self) -> &GasPriceOracle<Provider> {
        &self.gas_oracle
    }

    /// Returns a handle to the fee history cache.
    #[inline]
    pub const fn fee_history_cache(&self) -> &FeeHistoryCache {
        &self.fee_history_cache
    }

    /// Returns a handle to the signers.
    #[inline]
    pub const fn signers(&self) -> &parking_lot::RwLock<Vec<Box<dyn EthSigner>>> {
        &self.signers
    }
}

impl<Provider, Pool, Network, EvmConfig> UpdateRawTxForwarder
    for EthApiInner<Provider, Pool, Network, EvmConfig>
{
    fn set_eth_raw_transaction_forwarder(&self, forwarder: Arc<dyn RawTransactionForwarder>) {
        self.raw_transaction_forwarder.write().replace(forwarder);
    }
}

impl<T, Provider, Pool, Network, EvmConfig> UpdateRawTxForwarder for T
where
    T: Deref<Target = Arc<EthApiInner<Provider, Pool, Network, EvmConfig>>>,
{
    fn set_eth_raw_transaction_forwarder(&self, forwarder: Arc<dyn RawTransactionForwarder>) {
        self.deref().deref().set_eth_raw_transaction_forwarder(forwarder);
    }
}
