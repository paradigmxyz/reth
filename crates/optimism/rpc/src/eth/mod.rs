//! OP-Reth `eth_` endpoint implementation.

pub mod receipt;
pub mod rpc;
pub mod transaction;

mod block;
mod call;
mod pending_block;

pub use receipt::{OpReceiptBuilder, OpReceiptFieldsBuilder};

use std::{fmt, marker::PhantomData, sync::Arc};

use alloy_primitives::U256;
use derive_more::Deref;
use op_alloy_network::{Network, Optimism};
use reth_chainspec::ChainSpec;
use reth_evm::ConfigureEvm;
use reth_network_api::NetworkInfo;
use reth_node_api::{BuilderProvider, FullNodeComponents, FullNodeTypes, NodeTypes};
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
use reth_rpc_types_compat::TransactionCompat;
use reth_tasks::{
    pool::{BlockingTaskGuard, BlockingTaskPool},
    TaskSpawner,
};
use reth_transaction_pool::TransactionPool;

use crate::{eth::rpc::SequencerClient, OpEthApiError, OpTxBuilder};

/// Adapter for [`EthApiInner`], which holds all the data required to serve core `eth_` API.
pub type EthApiNodeBackend<N> = EthApiInner<
    <N as FullNodeTypes>::Provider,
    <N as FullNodeComponents>::Pool,
    <N as FullNodeComponents>::Network,
    <N as FullNodeComponents>::Evm,
>;

/// OP-Reth `Eth` API implementation.
///
/// This type provides the functionality for handling `eth_` related requests.
///
/// This wraps a default `Eth` implementation, and provides additional functionality where the
/// optimism spec deviates from the default (ethereum) spec, e.g. transaction forwarding to the
/// sequencer, receipts, additional RPC fields for transaction receipts.
///
/// This type implements the [`FullEthApi`](reth_rpc_eth_api::helpers::FullEthApi) by implemented
/// all the `Eth` helper traits and prerequisite traits.
#[derive(Clone, Deref)]
pub struct OpEthApi<N: FullNodeComponents, Eth> {
    #[deref]
    inner: Arc<EthApiNodeBackend<N>>,
    sequencer_client: Arc<parking_lot::RwLock<Option<SequencerClient>>>,
    /// L1 RPC type builders.
    _eth_ty_builders: PhantomData<Eth>,
}

impl<N: FullNodeComponents, Eth> OpEthApi<N, Eth> {
    /// Creates a new instance for given context.
    #[allow(clippy::type_complexity)]
    pub fn with_spawner(ctx: &EthApiBuilderCtx<N, Self>) -> Self {
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

        Self {
            inner: Arc::new(inner),
            sequencer_client: Arc::new(parking_lot::RwLock::new(None)),
            _eth_ty_builders: PhantomData,
        }
    }
}

impl<N, Eth> EthApiTypes for OpEthApi<N, Eth>
where
    Self: Send + Sync,
    N: FullNodeComponents,
    Eth: TransactionCompat<Transaction = <Optimism as Network>::TransactionResponse>,
{
    type Error = OpEthApiError;
    type NetworkTypes = Optimism;
    type TransactionCompat = OpTxBuilder<Eth>;
}

impl<N, Eth> EthApiSpec for OpEthApi<N, Eth>
where
    Self: Send + Sync,
    N: FullNodeComponents<Types: NodeTypes<ChainSpec = ChainSpec>>,
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

impl<N, Eth> SpawnBlocking for OpEthApi<N, Eth>
where
    Self: Send + Sync + Clone + 'static,
    N: FullNodeComponents,
    Eth: TransactionCompat<Transaction = <Optimism as Network>::TransactionResponse>,
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

impl<N, Eth> LoadFee for OpEthApi<N, Eth>
where
    Self: LoadBlock,
    N: FullNodeComponents<Types: NodeTypes<ChainSpec = ChainSpec>>,
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

impl<N, Eth> LoadState for OpEthApi<N, Eth>
where
    Self: Send + Sync,
    N: FullNodeComponents<Types: NodeTypes<ChainSpec = ChainSpec>>,
    Eth: TransactionCompat<Transaction = <Optimism as Network>::TransactionResponse>,
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

impl<N, Eth> EthState for OpEthApi<N, Eth>
where
    Self: LoadState + SpawnBlocking,
    N: FullNodeComponents,
{
    #[inline]
    fn max_proof_window(&self) -> u64 {
        self.inner.eth_proof_window()
    }
}

impl<N, Eth> EthFees for OpEthApi<N, Eth>
where
    Self: LoadFee,
    N: FullNodeComponents,
{
}

impl<N, Eth> Trace for OpEthApi<N, Eth>
where
    Self: LoadState,
    N: FullNodeComponents,
{
    #[inline]
    fn evm_config(&self) -> &impl ConfigureEvm {
        self.inner.evm_config()
    }
}

impl<N, Eth> AddDevSigners for OpEthApi<N, Eth>
where
    N: FullNodeComponents<Types: NodeTypes<ChainSpec = ChainSpec>>,
{
    fn with_dev_accounts(&self) {
        *self.signers().write() = DevSigner::random_signers(20)
    }
}

impl<N, Eth> BuilderProvider<N> for OpEthApi<N, Eth>
where
    Self: Send,
    N: FullNodeComponents,
    Eth: TransactionCompat<Transaction = <Optimism as Network>::TransactionResponse> + 'static,
{
    type Ctx<'a> = &'a EthApiBuilderCtx<N, Self>;

    fn builder() -> Box<dyn for<'a> Fn(Self::Ctx<'a>) -> Self + Send> {
        Box::new(|ctx| Self::with_spawner(ctx))
    }
}

impl<N: FullNodeComponents, Eth> fmt::Debug for OpEthApi<N, Eth> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpEthApi").finish_non_exhaustive()
    }
}
