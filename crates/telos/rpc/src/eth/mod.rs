//! Telos `eth_` endpoint implementation.

pub mod receipt;
pub mod transaction;

mod block;
mod call;
mod pending_block;
pub mod telos_client;

use std::{fmt, sync::Arc};

use crate::TelosClient;
use alloy_primitives::U256;
use alloy_network::AnyNetwork;
use reth_chainspec::ChainSpec;
use reth_evm::ConfigureEvm;
use reth_network_api::NetworkInfo;
use reth_node_api::{BuilderProvider, FullNodeComponents, FullNodeTypes};
use reth_node_builder::EthApiBuilderCtx;
use reth_provider::{
    BlockIdReader, BlockNumReader, BlockReaderIdExt, ChainSpecProvider, HeaderProvider,
    StageCheckpointReader, StateProviderFactory,
};
use reth_rpc::eth::{core::EthApiInner, DevSigner};
use reth_rpc_eth_api::{
    helpers::{
        AddDevSigners, EthApiSpec, EthFees, EthSigner, EthState, LoadBlock, LoadFee, LoadState,
        SpawnBlocking, Trace,
    },
    EthApiTypes,
};
use reth_rpc_eth_types::{EthStateCache, FeeHistoryCache, GasPriceOracle};
use reth_tasks::{
    pool::{BlockingTaskGuard, BlockingTaskPool},
    TaskSpawner,
};
use reth_transaction_pool::TransactionPool;
use tokio::sync::OnceCell;
use crate::error::TelosEthApiError;

/// Adapter for [`EthApiInner`], which holds all the data required to serve core `eth_` API.
pub type EthApiNodeBackend<N> = EthApiInner<
    <N as FullNodeTypes>::Provider,
    <N as FullNodeComponents>::Pool,
    <N as FullNodeComponents>::Network,
    <N as FullNodeComponents>::Evm,
>;

/// Telos `Eth` API implementation.
///
/// This type provides the functionality for handling `eth_` related requests.
///
/// This wraps a default `Eth` implementation, and provides additional functionality where the
/// Telos spec deviates from the default (ethereum) spec, e.g. transaction forwarding to the
/// native network
///
/// This type implements the [`FullEthApi`](reth_rpc_eth_api::helpers::FullEthApi) by implemented
/// all the `Eth` helper traits and prerequisite traits.
#[derive(Clone)]
pub struct TelosEthApi<N: FullNodeComponents> {
    inner: Arc<EthApiNodeBackend<N>>,
    telos_client: Arc<OnceCell<TelosClient>>,
}

impl<N: FullNodeComponents> TelosEthApi<N> {
    /// Creates a new instance for given context.
    #[allow(clippy::type_complexity)]
    pub fn with_spawner(ctx: &EthApiBuilderCtx<N>) -> Self {
        let blocking_task_pool =
            BlockingTaskPool::build().expect("failed to build blocking task pool");

        let inner = EthApiInner::new(
            ctx.provider.clone(),
            ctx.pool.clone(),
            ctx.network.clone(),
            ctx.cache.clone(),
            ctx.new_gas_price_oracle(),
            ctx.config.rpc_gas_cap,
            ctx.config.eth_proof_window,
            blocking_task_pool,
            ctx.new_fee_history_cache(),
            ctx.evm_config.clone(),
            ctx.executor.clone(),
            ctx.config.proof_permits,
        );

        Self { inner: Arc::new(inner), telos_client: Arc::new(OnceCell::new()) }
    }
}

impl<N> EthApiTypes for TelosEthApi<N>
where
    Self: Send + Sync,
    N: FullNodeComponents,
{
    type Error = TelosEthApiError;
    type NetworkTypes = AnyNetwork;
}

impl<N> EthApiSpec for TelosEthApi<N>
where
    Self: Send + Sync,
    N: FullNodeComponents,
{
    #[inline]
    fn provider(
        &self,
    ) -> impl ChainSpecProvider<ChainSpec = ChainSpec> + BlockNumReader + StageCheckpointReader
    {
        self.inner.provider()
    }

    #[inline]
    fn network(&self) -> impl NetworkInfo {
        self.inner.network()
    }

    #[inline]
    fn starting_block(&self) -> U256 {
        self.inner.starting_block()
    }

    #[inline]
    fn signers(&self) -> &parking_lot::RwLock<Vec<Box<dyn EthSigner>>> {
        self.inner.signers()
    }
}

impl<N> SpawnBlocking for TelosEthApi<N>
where
    Self: Send + Sync + Clone + 'static,
    N: FullNodeComponents,
{
    #[inline]
    fn io_task_spawner(&self) -> impl TaskSpawner {
        self.inner.task_spawner()
    }

    #[inline]
    fn tracing_task_pool(&self) -> &BlockingTaskPool {
        self.inner.blocking_task_pool()
    }

    #[inline]
    fn tracing_task_guard(&self) -> &BlockingTaskGuard {
        self.inner.blocking_task_guard()
    }
}

impl<N> LoadFee for TelosEthApi<N>
where
    Self: LoadBlock,
    N: FullNodeComponents,
{
    #[inline]
    fn provider(
        &self,
    ) -> impl BlockIdReader + HeaderProvider + ChainSpecProvider<ChainSpec = ChainSpec> {
        self.inner.provider()
    }

    #[inline]
    fn cache(&self) -> &EthStateCache {
        self.inner.cache()
    }

    #[inline]
    fn gas_oracle(&self) -> &GasPriceOracle<impl BlockReaderIdExt> {
        self.inner.gas_oracle()
    }

    #[inline]
    fn fee_history_cache(&self) -> &FeeHistoryCache {
        self.inner.fee_history_cache()
    }
}

impl<N> LoadState for TelosEthApi<N>
where
    Self: Send + Sync,
    N: FullNodeComponents,
{
    #[inline]
    fn provider(&self) -> impl StateProviderFactory + ChainSpecProvider<ChainSpec = ChainSpec> {
        self.inner.provider()
    }

    #[inline]
    fn cache(&self) -> &EthStateCache {
        self.inner.cache()
    }

    #[inline]
    fn pool(&self) -> impl TransactionPool {
        self.inner.pool()
    }
}

impl<N> EthState for TelosEthApi<N>
where
    Self: LoadState + SpawnBlocking,
    N: FullNodeComponents,
{
    #[inline]
    fn max_proof_window(&self) -> u64 {
        self.inner.eth_proof_window()
    }
}

impl<N> EthFees for TelosEthApi<N>
where
    Self: LoadFee,
    N: FullNodeComponents,
{
}

impl<N> Trace for TelosEthApi<N>
where
    Self: LoadState,
    N: FullNodeComponents,
{
    #[inline]
    fn evm_config(&self) -> &impl ConfigureEvm {
        self.inner.evm_config()
    }
}

impl<N: FullNodeComponents> AddDevSigners for TelosEthApi<N> {
    fn with_dev_accounts(&self) {
        *self.signers().write() = DevSigner::random_signers(20)
    }
}

impl<N> BuilderProvider<N> for TelosEthApi<N>
where
    Self: Send,
    N: FullNodeComponents,
{
    type Ctx<'a> = &'a EthApiBuilderCtx<N>;

    fn builder() -> Box<dyn for<'a> Fn(Self::Ctx<'a>) -> Self + Send> {
        Box::new(|ctx| Self::with_spawner(ctx))
    }
}

impl<N: FullNodeComponents> fmt::Debug for TelosEthApi<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpEthApi").finish_non_exhaustive()
    }
}
