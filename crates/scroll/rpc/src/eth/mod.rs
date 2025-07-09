//! Scroll-Reth `eth_` endpoint implementation.

use alloy_primitives::U256;
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_evm::ConfigureEvm;
use reth_network_api::NetworkInfo;
use reth_node_api::FullNodeComponents;
use reth_provider::{
    BlockNumReader, BlockReader, BlockReaderIdExt, CanonStateSubscriptions, ChainSpecProvider,
    ProviderBlock, ProviderHeader, ProviderReceipt, ProviderTx, StageCheckpointReader,
    StateProviderFactory,
};
use reth_rpc::eth::{core::EthApiInner, DevSigner};
use reth_rpc_eth_api::{
    helpers::{
        AddDevSigners, EthApiSpec, EthFees, EthSigner, EthState, LoadBlock, LoadFee, LoadState,
        SpawnBlocking, Trace,
    },
    EthApiTypes, FullEthApiServer, RpcConverter, RpcNodeCore, RpcNodeCoreExt,
};
use reth_rpc_eth_types::{EthStateCache, FeeHistoryCache, GasPriceOracle};
use reth_tasks::{
    pool::{BlockingTaskGuard, BlockingTaskPool},
    TaskSpawner,
};
use reth_transaction_pool::TransactionPool;
use std::{fmt, marker::PhantomData, sync::Arc};

use crate::{eth::transaction::ScrollTxInfoMapper, ScrollEthApiError};
pub use receipt::ScrollReceiptBuilder;
use reth_node_builder::rpc::{EthApiBuilder, EthApiCtx};
use reth_primitives_traits::NodePrimitives;
use reth_rpc_eth_types::error::FromEvmError;
use reth_scroll_primitives::ScrollPrimitives;
use scroll_alloy_network::{Network, Scroll};

mod block;
mod call;
mod pending_block;
pub mod receipt;
pub mod transaction;

/// Adapter for [`EthApiInner`], which holds all the data required to serve core `eth_` API.
pub type EthApiNodeBackend<N> = EthApiInner<
    <N as RpcNodeCore>::Provider,
    <N as RpcNodeCore>::Pool,
    <N as RpcNodeCore>::Network,
    <N as RpcNodeCore>::Evm,
>;

/// A helper trait with requirements for [`RpcNodeCore`] to be used in [`ScrollEthApi`].
pub trait ScrollNodeCore: RpcNodeCore<Provider: BlockReader> {}
impl<T> ScrollNodeCore for T where T: RpcNodeCore<Provider: BlockReader> {}

/// Scroll-Reth `Eth` API implementation.
///
/// This type provides the functionality for handling `eth_` related requests.
///
/// This wraps a default `Eth` implementation, and provides additional functionality where the
/// scroll spec deviates from the default (ethereum) spec, e.g. transaction forwarding to the
/// receipts, additional RPC fields for transaction receipts.
///
/// This type implements the [`FullEthApi`](reth_rpc_eth_api::helpers::FullEthApi) by implemented
/// all the `Eth` helper traits and prerequisite traits.
#[derive(Clone)]
pub struct ScrollEthApi<N: ScrollNodeCore, NetworkT = Scroll> {
    /// Gateway to node's core components.
    inner: Arc<ScrollEthApiInner<N>>,
    /// Marker for the network types.
    _nt: PhantomData<NetworkT>,
    tx_resp_builder: RpcConverter<NetworkT, N::Evm, ScrollEthApiError, ScrollTxInfoMapper<N>>,
}

impl<N: ScrollNodeCore, NetworkT> ScrollEthApi<N, NetworkT> {
    /// Creates a new [`ScrollEthApi`].
    pub fn new(eth_api: EthApiNodeBackend<N>) -> Self {
        let inner = Arc::new(ScrollEthApiInner { eth_api });
        Self {
            inner: inner.clone(),
            _nt: PhantomData,
            tx_resp_builder: RpcConverter::with_mapper(ScrollTxInfoMapper::new(inner)),
        }
    }
}

impl<N> ScrollEthApi<N>
where
    N: ScrollNodeCore<
        Provider: BlockReaderIdExt
                      + ChainSpecProvider
                      + CanonStateSubscriptions<Primitives = ScrollPrimitives>
                      + Clone
                      + 'static,
    >,
{
    /// Returns a reference to the [`EthApiNodeBackend`].
    pub fn eth_api(&self) -> &EthApiNodeBackend<N> {
        self.inner.eth_api()
    }

    /// Return a builder for the [`ScrollEthApi`].
    pub const fn builder() -> ScrollEthApiBuilder {
        ScrollEthApiBuilder::new()
    }
}

impl<N, NetworkT> EthApiTypes for ScrollEthApi<N, NetworkT>
where
    Self: Send + Sync + fmt::Debug,
    N: ScrollNodeCore,
    NetworkT: Network + Clone + fmt::Debug,
    <N as RpcNodeCore>::Evm: fmt::Debug,
    <N as RpcNodeCore>::Primitives: fmt::Debug,
{
    type Error = ScrollEthApiError;
    type NetworkTypes = Scroll;
    type RpcConvert = RpcConverter<NetworkT, N::Evm, ScrollEthApiError, ScrollTxInfoMapper<N>>;

    fn tx_resp_builder(&self) -> &Self::RpcConvert {
        &self.tx_resp_builder
    }
}

impl<N, NetworkT> RpcNodeCore for ScrollEthApi<N, NetworkT>
where
    N: ScrollNodeCore,
    NetworkT: Network,
{
    type Primitives = N::Primitives;
    type Provider = N::Provider;
    type Pool = N::Pool;
    type Evm = <N as RpcNodeCore>::Evm;
    type Network = <N as RpcNodeCore>::Network;
    type PayloadBuilder = ();

    #[inline]
    fn pool(&self) -> &Self::Pool {
        self.inner.eth_api.pool()
    }

    #[inline]
    fn evm_config(&self) -> &Self::Evm {
        self.inner.eth_api.evm_config()
    }

    #[inline]
    fn network(&self) -> &Self::Network {
        self.inner.eth_api.network()
    }

    #[inline]
    fn payload_builder(&self) -> &Self::PayloadBuilder {
        &()
    }

    #[inline]
    fn provider(&self) -> &Self::Provider {
        self.inner.eth_api.provider()
    }
}

impl<N, NetworkT> RpcNodeCoreExt for ScrollEthApi<N, NetworkT>
where
    N: ScrollNodeCore,
    NetworkT: Network,
{
    #[inline]
    fn cache(&self) -> &EthStateCache<ProviderBlock<N::Provider>, ProviderReceipt<N::Provider>> {
        self.inner.eth_api.cache()
    }
}

impl<N, NetworkT> EthApiSpec for ScrollEthApi<N, NetworkT>
where
    N: ScrollNodeCore<
        Provider: ChainSpecProvider<ChainSpec: EthereumHardforks>
                      + BlockNumReader
                      + StageCheckpointReader,
        Network: NetworkInfo,
    >,
    NetworkT: Network,
{
    type Transaction = ProviderTx<Self::Provider>;

    #[inline]
    fn starting_block(&self) -> U256 {
        self.inner.eth_api.starting_block()
    }

    #[inline]
    fn signers(&self) -> &parking_lot::RwLock<Vec<Box<dyn EthSigner<ProviderTx<Self::Provider>>>>> {
        self.inner.eth_api.signers()
    }
}

impl<N, NetworkT> SpawnBlocking for ScrollEthApi<N, NetworkT>
where
    Self: Send + Sync + Clone + 'static,
    N: ScrollNodeCore,
    NetworkT: Network,
    <N as RpcNodeCore>::Evm: fmt::Debug,
    <N as RpcNodeCore>::Primitives: fmt::Debug,
{
    #[inline]
    fn io_task_spawner(&self) -> impl TaskSpawner {
        self.inner.eth_api.task_spawner()
    }

    #[inline]
    fn tracing_task_pool(&self) -> &BlockingTaskPool {
        self.inner.eth_api.blocking_task_pool()
    }

    #[inline]
    fn tracing_task_guard(&self) -> &BlockingTaskGuard {
        self.inner.eth_api.blocking_task_guard()
    }
}

impl<N, NetworkT> LoadFee for ScrollEthApi<N, NetworkT>
where
    Self: LoadBlock<Provider = N::Provider>,
    N: ScrollNodeCore<
        Provider: BlockReaderIdExt
                      + ChainSpecProvider<ChainSpec: EthChainSpec + EthereumHardforks>
                      + StateProviderFactory,
    >,
{
    #[inline]
    fn gas_oracle(&self) -> &GasPriceOracle<Self::Provider> {
        self.inner.eth_api.gas_oracle()
    }

    #[inline]
    fn fee_history_cache(&self) -> &FeeHistoryCache<ProviderHeader<N::Provider>> {
        self.inner.eth_api.fee_history_cache()
    }
}

impl<N, NetworkT> LoadState for ScrollEthApi<N, NetworkT>
where
    N: ScrollNodeCore<
        Provider: StateProviderFactory + ChainSpecProvider<ChainSpec: EthereumHardforks>,
        Pool: TransactionPool,
    >,
    NetworkT: Network,
    <N as RpcNodeCore>::Evm: fmt::Debug,
    <N as RpcNodeCore>::Primitives: fmt::Debug,
{
}

impl<N, NetworkT> EthState for ScrollEthApi<N, NetworkT>
where
    Self: LoadState + SpawnBlocking,
    N: ScrollNodeCore,
{
    #[inline]
    fn max_proof_window(&self) -> u64 {
        self.inner.eth_api.eth_proof_window()
    }
}

impl<N, NetworkT> EthFees for ScrollEthApi<N, NetworkT>
where
    Self: LoadFee<
        Provider: ChainSpecProvider<
            ChainSpec: EthChainSpec<Header = ProviderHeader<Self::Provider>>,
        >,
    >,
    N: ScrollNodeCore,
{
}

impl<N, NetworkT> Trace for ScrollEthApi<N, NetworkT>
where
    Self: RpcNodeCore<Provider: BlockReader>
        + LoadState<
            Evm: ConfigureEvm<
                Primitives: NodePrimitives<
                    BlockHeader = ProviderHeader<Self::Provider>,
                    SignedTx = ProviderTx<Self::Provider>,
                >,
            >,
            Error: FromEvmError<Self::Evm>,
        >,
    N: ScrollNodeCore,
{
}

impl<N, NetworkT> AddDevSigners for ScrollEthApi<N, NetworkT>
where
    N: ScrollNodeCore,
{
    fn with_dev_accounts(&self) {
        *self.inner.eth_api.signers().write() = DevSigner::random_signers(20)
    }
}

impl<N: ScrollNodeCore, NetworkT> fmt::Debug for ScrollEthApi<N, NetworkT> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScrollEthApi").finish_non_exhaustive()
    }
}

/// Container type `ScrollEthApi`
#[allow(missing_debug_implementations)]
pub struct ScrollEthApiInner<N: ScrollNodeCore> {
    /// Gateway to node's core components.
    pub eth_api: EthApiNodeBackend<N>,
}

impl<N: ScrollNodeCore> ScrollEthApiInner<N> {
    /// Returns a reference to the [`EthApiNodeBackend`].
    const fn eth_api(&self) -> &EthApiNodeBackend<N> {
        &self.eth_api
    }
}

/// A type that knows how to build a [`ScrollEthApi`].
#[derive(Debug, Default)]
pub struct ScrollEthApiBuilder {}

impl ScrollEthApiBuilder {
    /// Creates a [`ScrollEthApiBuilder`] instance.
    pub const fn new() -> Self {
        Self {}
    }
}

impl<N> EthApiBuilder<N> for ScrollEthApiBuilder
where
    N: FullNodeComponents,
    ScrollEthApi<N>: FullEthApiServer<Provider = N::Provider, Pool = N::Pool>,
{
    type EthApi = ScrollEthApi<N>;

    async fn build_eth_api(self, ctx: EthApiCtx<'_, N>) -> eyre::Result<Self::EthApi> {
        let eth_api = reth_rpc::EthApiBuilder::new(
            ctx.components.provider().clone(),
            ctx.components.pool().clone(),
            ctx.components.network().clone(),
            ctx.components.evm_config().clone(),
        )
        .eth_cache(ctx.cache)
        .task_spawner(ctx.components.task_executor().clone())
        .gas_cap(ctx.config.rpc_gas_cap.into())
        .max_simulate_blocks(ctx.config.rpc_max_simulate_blocks)
        .eth_proof_window(ctx.config.eth_proof_window)
        .fee_history_cache_config(ctx.config.fee_history_cache)
        .proof_permits(ctx.config.proof_permits)
        .build_inner();

        Ok(ScrollEthApi::new(eth_api))
    }
}
