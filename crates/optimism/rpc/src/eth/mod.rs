//! OP-Reth `eth_` endpoint implementation.

pub mod ext;
pub mod receipt;
pub mod transaction;

mod block;
mod call;
mod pending_block;

use alloy_primitives::U256;
use eyre::WrapErr;
use op_alloy_network::Optimism;
pub use receipt::{OpReceiptBuilder, OpReceiptFieldsBuilder};
use reth_chain_state::CanonStateSubscriptions;
use reth_chainspec::{ChainSpecProvider, EthChainSpec, EthereumHardforks};
use reth_consensus::NoopConsensus;
use reth_db_api::FullDatabase;
use reth_evm::ConfigureEvm;
use reth_network_api::{noop::NoopNetwork, NetworkInfo};
use reth_node_api::{
    FullNodeComponents, FullNodeTypesAdapter, NodePrimitives, NodeTypesWithDBAdapter,
};
use reth_node_builder::{
    components::Components,
    rpc::{EthApiBuilder, EthApiCtx},
    NodeAdapter,
};
use reth_optimism_evm::OpEvmConfig;
use reth_optimism_network::OpNetworkPrimitives;
use reth_optimism_node_types::OpTypes;
use reth_optimism_primitives::{OpBlock, OpPrimitives, OpReceipt, OpTransactionSigned};
use reth_optimism_txpool::OpPooledTransaction;
use reth_provider::FullProvider;
use reth_rpc::eth::{core::EthApiInner, DevSigner};
use reth_rpc_eth_api::{
    helpers::{
        AddDevSigners, EthApiSpec, EthFees, EthSigner, EthState, LoadBlock, LoadFee, LoadState,
        SpawnBlocking, Trace,
    },
    EthApiTypes, FromEvmError, FullEthApiServer, RpcNodeCore, RpcNodeCoreExt,
};
use reth_rpc_eth_types::{EthStateCache, FeeHistoryCache, GasPriceOracle};
use reth_storage_api::{
    BlockNumReader, BlockReader, BlockReaderIdExt, ProviderBlock, ProviderHeader, ProviderReceipt,
    ProviderTx, StageCheckpointReader, StateProviderFactory,
};
use reth_tasks::{
    pool::{BlockingTaskGuard, BlockingTaskPool},
    TaskSpawner,
};
use reth_transaction_pool::{noop::NoopTransactionPool, TransactionPool};
use std::{fmt, sync::Arc};

use crate::{OpEthApiError, SequencerClient};

/// Adapter for [`EthApiInner`], which holds all the data required to serve core `eth_` API.
pub type EthApiNodeBackend<N> = EthApiInner<
    <N as RpcNodeCore>::Provider,
    <N as RpcNodeCore>::Pool,
    <N as RpcNodeCore>::Network,
    <N as RpcNodeCore>::Evm,
>;

/// A helper trait with requirements for [`RpcNodeCore`] to be used in [`OpEthApi`].
pub trait OpNodeCore: RpcNodeCore<Provider: BlockReader> {}
impl<T> OpNodeCore for T where T: RpcNodeCore<Provider: BlockReader> {}

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
#[derive(Clone)]
pub struct OpEthApi<N: OpNodeCore> {
    /// Gateway to node's core components.
    inner: Arc<OpEthApiInner<N>>,
}

impl<N> OpEthApi<N>
where
    N: OpNodeCore<
        Provider: BlockReaderIdExt
                      + ChainSpecProvider
                      + CanonStateSubscriptions<Primitives = OpPrimitives>
                      + Clone
                      + 'static,
    >,
{
    /// Returns a reference to the [`EthApiNodeBackend`].
    pub fn eth_api(&self) -> &EthApiNodeBackend<N> {
        self.inner.eth_api()
    }

    /// Returns the configured sequencer client, if any.
    pub fn sequencer_client(&self) -> Option<&SequencerClient> {
        self.inner.sequencer_client()
    }

    /// Build a [`OpEthApi`] using [`OpEthApiBuilder`].
    pub const fn builder() -> OpEthApiBuilder {
        OpEthApiBuilder::new()
    }
}

impl<N> EthApiTypes for OpEthApi<N>
where
    Self: Send + Sync,
    N: OpNodeCore,
{
    type Error = OpEthApiError;
    type NetworkTypes = Optimism;
    type TransactionCompat = Self;

    fn tx_resp_builder(&self) -> &Self::TransactionCompat {
        self
    }
}

impl<N> RpcNodeCore for OpEthApi<N>
where
    N: OpNodeCore,
{
    type Primitives = OpPrimitives;
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

impl<N> RpcNodeCoreExt for OpEthApi<N>
where
    N: OpNodeCore,
{
    #[inline]
    fn cache(&self) -> &EthStateCache<ProviderBlock<N::Provider>, ProviderReceipt<N::Provider>> {
        self.inner.eth_api.cache()
    }
}

impl<N> EthApiSpec for OpEthApi<N>
where
    N: OpNodeCore<
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

impl<N> SpawnBlocking for OpEthApi<N>
where
    Self: Send + Sync + Clone + 'static,
    N: OpNodeCore,
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

impl<N> LoadFee for OpEthApi<N>
where
    Self: LoadBlock<Provider = N::Provider>,
    N: OpNodeCore<
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

impl<N> LoadState for OpEthApi<N> where
    N: OpNodeCore<
        Provider: StateProviderFactory + ChainSpecProvider<ChainSpec: EthereumHardforks>,
        Pool: TransactionPool,
    >
{
}

impl<N> EthState for OpEthApi<N>
where
    Self: LoadState + SpawnBlocking,
    N: OpNodeCore,
{
    #[inline]
    fn max_proof_window(&self) -> u64 {
        self.inner.eth_api.eth_proof_window()
    }
}

impl<N> EthFees for OpEthApi<N>
where
    Self: LoadFee,
    N: OpNodeCore,
{
}

impl<N> Trace for OpEthApi<N>
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
    N: OpNodeCore,
{
}

impl<N> AddDevSigners for OpEthApi<N>
where
    N: OpNodeCore,
{
    fn with_dev_accounts(&self) {
        *self.inner.eth_api.signers().write() = DevSigner::random_signers(20)
    }
}

impl<N: OpNodeCore> fmt::Debug for OpEthApi<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpEthApi").finish_non_exhaustive()
    }
}

/// Container type `OpEthApi`
struct OpEthApiInner<N: OpNodeCore> {
    /// Gateway to node's core components.
    eth_api: EthApiNodeBackend<N>,
    /// Sequencer client, configured to forward submitted transactions to sequencer of given OP
    /// network.
    sequencer_client: Option<SequencerClient>,
}

impl<N: OpNodeCore> OpEthApiInner<N> {
    /// Returns a reference to the [`EthApiNodeBackend`].
    const fn eth_api(&self) -> &EthApiNodeBackend<N> {
        &self.eth_api
    }

    /// Returns the configured sequencer client, if any.
    const fn sequencer_client(&self) -> Option<&SequencerClient> {
        self.sequencer_client.as_ref()
    }
}

/// Builds [`OpEthApi`] for Optimism.
#[derive(Debug, Default)]
pub struct OpEthApiBuilder {
    /// Sequencer client, configured to forward submitted transactions to sequencer of given OP
    /// network.
    sequencer_url: Option<String>,
}

impl OpEthApiBuilder {
    /// Creates a [`OpEthApiBuilder`] instance from core components.
    pub const fn new() -> Self {
        Self { sequencer_url: None }
    }

    /// With a [`SequencerClient`].
    pub fn with_sequencer(mut self, sequencer_url: Option<String>) -> Self {
        self.sequencer_url = sequencer_url;
        self
    }

    /// Creates a new light-weight [`OpEthApi`] that has only the database provider component and
    /// [`OpEvmConfig`] component.
    ///
    /// This is useful for building [`TraceApi`](reth_rpc::TraceApi) as standalone program without
    /// mempool.
    #[expect(clippy::complexity)]
    pub fn build_evm_eth_api<DB, P>(
        self,
        provider: P,
        evm_config: OpEvmConfig,
    ) -> OpEthApi<
        NodeAdapter<
            FullNodeTypesAdapter<OpTypes, DB, P>,
            Components<
                FullNodeTypesAdapter<OpTypes, DB, P>,
                NoopNetwork<OpNetworkPrimitives>,
                NoopTransactionPool<OpPooledTransaction>,
                OpEvmConfig,
                NoopConsensus,
            >,
        >,
    >
    where
        DB: FullDatabase + 'static,
        P: FullProvider<
            NodeTypesWithDBAdapter<OpTypes, DB>,
            Block = OpBlock,
            Receipt = OpReceipt,
            Transaction = OpTransactionSigned,
            DB = DB,
        >,
    {
        let eth_api = reth_rpc::EthApiBuilder::new(
            provider,
            NoopTransactionPool::<OpPooledTransaction>::new(),
            NoopNetwork::<OpNetworkPrimitives>::new(),
            evm_config,
        )
        .build_inner();

        OpEthApi { inner: Arc::new(OpEthApiInner::<_> { eth_api, sequencer_client: None }) }
    }
}

impl<N> EthApiBuilder<N> for OpEthApiBuilder
where
    N: FullNodeComponents,
    OpEthApi<N>: FullEthApiServer<Provider = N::Provider, Pool = N::Pool>,
{
    type EthApi = OpEthApi<N>;

    async fn build_eth_api(self, ctx: EthApiCtx<'_, N>) -> eyre::Result<Self::EthApi> {
        let Self { sequencer_url } = self;
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
        .gas_oracle_config(ctx.config.gas_oracle)
        .build_inner();

        let sequencer_client = if let Some(url) = sequencer_url {
            Some(
                SequencerClient::new(&url)
                    .await
                    .wrap_err_with(|| "Failed to init sequencer client with: {url}")?,
            )
        } else {
            None
        };

        Ok(OpEthApi { inner: Arc::new(OpEthApiInner { eth_api, sequencer_client }) })
    }
}

/// Minimal OP node, with only EVM and database, useful for building
/// [`TraceApi`](reth_rpc::TraceApi) as alone-standing program.
#[derive(Debug, Clone)]
pub struct OpNodeOnlyEvm<P> {
    provider: P,
    evm_config: OpEvmConfig,
    noop_pool: NoopTransactionPool<OpPooledTransaction>,
    noop_network: NoopNetwork<OpNetworkPrimitives>,
}

impl<P> OpNodeOnlyEvm<P> {
    /// Returns new instance from given provider and [`OpEvmConfig`].
    pub fn new(provider: P, evm_config: OpEvmConfig) -> Self {
        Self {
            provider,
            evm_config,
            noop_pool: NoopTransactionPool::<OpPooledTransaction>::new(),
            noop_network: NoopNetwork::<OpNetworkPrimitives>::new(),
        }
    }
}

impl<P> RpcNodeCore for OpNodeOnlyEvm<P>
where
    P: Send + Sync + Clone + Unpin,
{
    type Primitives = OpPrimitives;
    type Provider = P;
    type Pool = NoopTransactionPool<OpPooledTransaction>;
    type Evm = OpEvmConfig;
    type Network = NoopNetwork<OpNetworkPrimitives>;
    type PayloadBuilder = ();

    fn pool(&self) -> &Self::Pool {
        &self.noop_pool
    }

    fn evm_config(&self) -> &Self::Evm {
        &self.evm_config
    }

    fn network(&self) -> &Self::Network {
        &self.noop_network
    }

    fn payload_builder(&self) -> &Self::PayloadBuilder {
        &()
    }

    fn provider(&self) -> &Self::Provider {
        &self.provider
    }
}

#[cfg(test)]
mod test {
    use alloy_rpc_types_eth::BlockId;
    use reth_db_common::init::init_genesis;
    use reth_optimism_chainspec::OP_DEV;
    use reth_optimism_evm::OpEvmConfig;
    use reth_provider::{
        providers::BlockchainProvider, test_utils::create_test_provider_factory_with_node_types,
    };
    use reth_rpc::TraceApi;
    use reth_rpc_eth_types::EthConfig;

    use super::*;

    #[tokio::test]
    async fn build_trace_api() {
        let factory = create_test_provider_factory_with_node_types::<OpTypes>(OP_DEV.clone());
        let _ = init_genesis(&factory).expect("should init genesis");
        let provider = BlockchainProvider::new(factory).expect("should build provider");

        let eth_api =
            OpEthApiBuilder::default().build_evm_eth_api::<_, _>(provider, OpEvmConfig::op_dev());

        let trace = TraceApi::new(eth_api, BlockingTaskGuard::new(10), EthConfig::default());

        let _traces = trace.trace_block(BlockId::latest()).await.expect("should get traces");
    }
}
