use alloy_network::Ethereum;
use alloy_primitives::U256;
use reth::{
    builder::{
        rpc::{EthApiBuilder, EthApiCtx},
        FullNodeComponents,
    },
    chainspec::EthChainSpec,
    primitives::{EthPrimitives, EthereumHardforks},
    providers::ChainSpecProvider,
    rpc::{
        eth::{DevSigner, FullEthApiServer},
        server_types::eth::{EthApiError, EthStateCache, FeeHistoryCache, GasPriceOracle},
    },
    tasks::{
        pool::{BlockingTaskGuard, BlockingTaskPool},
        TaskSpawner,
    },
    transaction_pool::TransactionPool,
};
use reth_evm::ConfigureEvm;
use reth_network::NetworkInfo;
use reth_optimism_rpc::eth::EthApiNodeBackend;
use reth_primitives::NodePrimitives;
use reth_provider::{
    BlockNumReader, BlockReader, BlockReaderIdExt, ProviderBlock, ProviderHeader, ProviderReceipt,
    ProviderTx, StageCheckpointReader, StateProviderFactory,
};
use reth_rpc_eth_api::{
    helpers::{
        AddDevSigners, EthApiSpec, EthFees, EthSigner, EthState, LoadBlock, LoadFee, LoadState,
        SpawnBlocking, Trace,
    },
    EthApiTypes, FromEvmError, RpcNodeCore, RpcNodeCoreExt,
};
use std::{fmt, sync::Arc};

mod block;
mod call;
pub mod engine_api;
mod transaction;

/// A helper trait with requirements for [`RpcNodeCore`] to be used in [`BscEthApi`].
pub trait BscNodeCore: RpcNodeCore<Provider: BlockReader> {}
impl<T> BscNodeCore for T where T: RpcNodeCore<Provider: BlockReader> {}

/// Container type `BscEthApi`
#[allow(missing_debug_implementations)]
pub(crate) struct BscEthApiInner<N: BscNodeCore> {
    /// Gateway to node's core components.
    pub(crate) eth_api: EthApiNodeBackend<N>,
}

#[derive(Clone)]
pub struct BscEthApi<N: BscNodeCore> {
    /// Gateway to node's core components.
    pub(crate) inner: Arc<BscEthApiInner<N>>,
}

impl<N: BscNodeCore> fmt::Debug for BscEthApi<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BscEthApi").finish_non_exhaustive()
    }
}

impl<N> EthApiTypes for BscEthApi<N>
where
    Self: Send + Sync,
    N: BscNodeCore,
{
    type Error = EthApiError;
    type NetworkTypes = Ethereum;
    type TransactionCompat = Self;

    fn tx_resp_builder(&self) -> &Self::TransactionCompat {
        self
    }
}

impl<N> RpcNodeCore for BscEthApi<N>
where
    N: BscNodeCore,
{
    type Primitives = EthPrimitives;
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

impl<N> RpcNodeCoreExt for BscEthApi<N>
where
    N: BscNodeCore,
{
    #[inline]
    fn cache(&self) -> &EthStateCache<ProviderBlock<N::Provider>, ProviderReceipt<N::Provider>> {
        self.inner.eth_api.cache()
    }
}

impl<N> EthApiSpec for BscEthApi<N>
where
    N: BscNodeCore<
        Provider: ChainSpecProvider<ChainSpec: EthereumHardforks>
                      + BlockNumReader
                      + StageCheckpointReader,
        Network: NetworkInfo,
    >,
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

impl<N> SpawnBlocking for BscEthApi<N>
where
    Self: Send + Sync + Clone + 'static,
    N: BscNodeCore,
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

impl<N> LoadFee for BscEthApi<N>
where
    Self: LoadBlock<Provider = N::Provider>,
    N: BscNodeCore<
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
    fn fee_history_cache(&self) -> &FeeHistoryCache {
        self.inner.eth_api.fee_history_cache()
    }
}

impl<N> LoadState for BscEthApi<N> where
    N: BscNodeCore<
        Provider: StateProviderFactory + ChainSpecProvider<ChainSpec: EthereumHardforks>,
        Pool: TransactionPool,
    >
{
}

impl<N> EthState for BscEthApi<N>
where
    Self: LoadState + SpawnBlocking,
    N: BscNodeCore,
{
    #[inline]
    fn max_proof_window(&self) -> u64 {
        self.inner.eth_api.eth_proof_window()
    }
}

impl<N> EthFees for BscEthApi<N>
where
    Self: LoadFee,
    N: BscNodeCore,
{
}

impl<N> Trace for BscEthApi<N>
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
    N: BscNodeCore,
{
}

impl<N> AddDevSigners for BscEthApi<N>
where
    N: BscNodeCore,
{
    fn with_dev_accounts(&self) {
        *self.inner.eth_api.signers().write() = DevSigner::random_signers(20)
    }
}

/// Builds [`BscEthApi`] for BSC.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct BscEthApiBuilder;

impl<N> EthApiBuilder<N> for BscEthApiBuilder
where
    N: FullNodeComponents,
    BscEthApi<N>: FullEthApiServer<Provider = N::Provider, Pool = N::Pool>,
{
    type EthApi = BscEthApi<N>;

    async fn build_eth_api(self, ctx: EthApiCtx<'_, N>) -> eyre::Result<Self::EthApi> {
        let eth_api = reth::rpc::eth::EthApiBuilder::new(
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

        Ok(BscEthApi { inner: Arc::new(BscEthApiInner { eth_api }) })
    }
}
