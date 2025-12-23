use super::LaunchNode;
use crate::{rpc::RethRpcAddOns, EngineNodeLauncher, Node, NodeHandle};
use alloy_provider::{network::AnyNetwork, Network};
use async_trait::async_trait;
use jsonrpsee::core::{DeserializeOwned, Serialize};
use reth_chainspec::EthChainSpec;
use reth_consensus_debug_client::{
    BlockProvider, DebugConsensusClient, EtherscanBlockProvider, RpcBlockProvider,
};
use reth_engine_local::LocalMiner;
use reth_node_api::{
    BlockTy, FullNodeComponents, FullNodeTypes, HeaderTy, NodeTypes, PayloadAttrTy,
    PayloadAttributesBuilder, PayloadTypes,
};
use reth_node_core::node_config::NodeConfig;
use reth_payload_builder::PayloadBuilderHandle;
use reth_primitives_traits::Block;
use std::{
    any::Any,
    future::{Future, IntoFuture},
    pin::Pin,
    sync::Arc,
};
use tokio::sync::mpsc::Sender;
use tracing::info;

/// [`Node`] extension with support for debugging utilities.
///
/// This trait provides additional necessary conversion from RPC block type to the node's
/// primitive block type, e.g. `alloy_rpc_types_eth::Block` to the node's internal block
/// representation.
///
/// This is used in conjunction with the [`DebugNodeLauncher`] to enable debugging features such as:
///
/// - **Etherscan Integration**: Use Etherscan as a consensus client to follow the chain and submit
///   blocks to the local engine.
/// - **RPC Consensus Client**: Connect to an external RPC endpoint to fetch blocks and submit them
///   to the local engine to follow the chain.
///
/// See [`DebugNodeLauncher`] for the launcher that enables these features.
///
/// # Implementation
///
/// To implement this trait, you need to:
/// 1. Define the RPC block type (typically `alloy_rpc_types_eth::Block`)
/// 2. Implement the conversion from RPC format to your primitive block type
///
/// # Example
///
/// ```ignore
/// impl<N: FullNodeComponents> DebugNode<N> for MyNode {
///     type RpcBlock = alloy_rpc_types_eth::Block;
///
///     fn rpc_to_primitive_block(rpc_block: Self::RpcBlock) -> BlockTy<Self> {
///         // Convert from RPC format to primitive format by converting the transactions
///         rpc_block.into_consensus().convert_transactions()
///     }
/// }
/// ```
pub trait DebugNode<N: FullNodeComponents>: Node<N> {
    /// RPC block type. Used by [`DebugConsensusClient`] to fetch blocks and submit them to the
    /// engine. This is intended to match the block format returned by the external RPC endpoint.
    type RpcBlock: Serialize + DeserializeOwned + 'static;

    /// Converts an RPC block to a primitive block.
    ///
    /// This method handles the conversion between the RPC block format and the internal primitive
    /// block format used by the node's consensus engine.
    ///
    /// # Example
    ///
    /// For Ethereum nodes, this typically converts from `alloy_rpc_types_eth::Block`
    /// to the node's internal block representation.
    fn rpc_to_primitive_block(rpc_block: Self::RpcBlock) -> BlockTy<Self>;

    /// Creates a payload attributes builder for local mining in dev mode.
    ///
    ///  It will be used by the `LocalMiner` when dev mode is enabled.
    ///
    /// The builder is responsible for creating the payload attributes that define how blocks should
    /// be constructed during local mining.
    fn local_payload_attributes_builder(
        chain_spec: &Self::ChainSpec,
    ) -> impl PayloadAttributesBuilder<<Self::Payload as PayloadTypes>::PayloadAttributes, HeaderTy<Self>>;

    /// Converts network block response into the RPC block type.
    ///
    /// Override this to avoid serde round-trips when the response type already matches `RpcBlock`.
    fn rpc_block_from_response(response: <AnyNetwork as Network>::BlockResponse) -> Self::RpcBlock {
        // Fast path: when the response type already matches RpcBlock, avoid serialization.
        if std::any::TypeId::of::<Self::RpcBlock>() ==
            std::any::TypeId::of::<<AnyNetwork as Network>::BlockResponse>()
        {
            let boxed: Box<dyn Any> = Box::new(response);
            let block = boxed
                .downcast::<Self::RpcBlock>()
                .expect("TypeId matches RpcBlock; downcast cannot fail");
            return *block;
        }

        // Fallback: deserialize via JSON when types differ.
        let json =
            serde_json::to_value(response).expect("Block serialization cannot fail for debug");
        serde_json::from_value(json).expect("Block deserialization cannot fail for debug")
    }

    /// Default RPC-block-to-primitive converter used by the debug consensus client.
    fn default_rpc_convert() -> RpcConsensusConvert<N>
    where
        N: FullNodeComponents<Types = Self>,
    {
        Arc::new(|rpc_block: Self::RpcBlock| Self::rpc_to_primitive_block(rpc_block))
    }

    /// Constructs a block provider for the debug consensus client.
    ///
    /// Default implementation wires either RPC or Etherscan provider based on debug config,
    /// falling back to an error if none are configured.
    fn block_provider<'a>(ctx: DebugBlockProviderContext<'a, N>) -> DebugBlockProviderFuture<'a, N>
    where
        N: FullNodeComponents<Types = Self>,
    {
        Box::pin(async move {
            let config = ctx.config;
            if let Some(url) = config.debug.rpc_consensus_url.clone() {
                let block_provider = RpcBlockProvider::<AnyNetwork, _>::new(url.as_str(), {
                    let convert_rpc_block = ctx.convert_rpc.clone();
                    move |block_response| {
                        let rpc_block =
                            <N::Types as DebugNode<N>>::rpc_block_from_response(block_response);
                        convert_rpc_block(rpc_block)
                    }
                })
                .await?;
                return Ok(DynBlockProviderHandle::new(block_provider))
            }

            if let Some(maybe_custom_etherscan_url) = config.debug.etherscan.clone() {
                let chain = config.chain.chain();
                let etherscan_url = maybe_custom_etherscan_url.map(Ok).unwrap_or_else(|| {
                    chain.etherscan_urls().map(|urls| urls.0.to_string()).ok_or_else(|| {
                        eyre::eyre!("failed to get etherscan url for chain: {chain}")
                    })
                })?;

                let block_provider = EtherscanBlockProvider::new(
                    etherscan_url,
                    chain.etherscan_api_key().ok_or_else(|| {
                        eyre::eyre!(
                            "etherscan api key not found for rpc consensus client for chain: {chain}"
                        )
                    })?,
                    chain.id(),
                    Self::rpc_to_primitive_block,
                );
                return Ok(DynBlockProviderHandle::new(block_provider))
            }

            Err(eyre::eyre!("no debug consensus block provider configured"))
        })
    }
}

/// Converts an RPC block into the node's primitive block representation.
pub(crate) type RpcConsensusConvert<N> = Arc<
    dyn Fn(
            <<N as FullNodeTypes>::Types as DebugNode<N>>::RpcBlock,
        ) -> BlockTy<<N as FullNodeTypes>::Types>
        + Send
        + Sync
        + 'static,
>;

/// Context passed to construct a debug block provider.
#[allow(missing_debug_implementations)]
pub struct DebugBlockProviderContext<'a, N>
where
    N: FullNodeComponents<Types: DebugNode<N>>,
{
    /// Node configuration for accessing debug settings.
    pub config: &'a NodeConfig<<N::Types as NodeTypes>::ChainSpec>,
    /// Converter used to map RPC responses into primitive blocks.
    pub convert_rpc: RpcConsensusConvert<N>,
}

/// Future returning a dynamic block provider.
pub(crate) type DebugBlockProviderFuture<'a, N> = Pin<
    Box<
        dyn Future<
                Output = eyre::Result<DynBlockProviderHandle<BlockTy<<N as FullNodeTypes>::Types>>>,
            > + Send
            + 'a,
    >,
>;

/// Returns true if debug consensus block provider is configured via RPC or Etherscan flags.
const fn debug_block_provider_configured<ChainSpec>(config: &NodeConfig<ChainSpec>) -> bool {
    config.debug.rpc_consensus_url.is_some() || config.debug.etherscan.is_some()
}

/// Context for building dev-mode payload attributes after the node has launched.
#[derive(Clone, Debug)]
pub struct DevMiningContext<'a, N: FullNodeComponents> {
    /// Chain specification of the node.
    pub chain_spec: Arc<<<N as FullNodeTypes>::Types as NodeTypes>::ChainSpec>,
    /// Provider handle for accessing blockchain state.
    pub provider: &'a N::Provider,
    /// Transaction pool handle.
    pub pool: &'a N::Pool,
    /// Payload builder handle.
    pub payload_builder_handle:
        &'a PayloadBuilderHandle<<<N as FullNodeTypes>::Types as NodeTypes>::Payload>,
}

/// Factory for producing a payload attributes builder in dev mode with access to runtime context.
pub(crate) type DevPayloadAttributesBuilderFactory<N> = Box<
    dyn for<'a> FnOnce(
            DevMiningContext<'a, N>,
        ) -> Box<
            dyn PayloadAttributesBuilder<
                PayloadAttrTy<<N as FullNodeTypes>::Types>,
                HeaderTy<<N as FullNodeTypes>::Types>,
            >,
        > + Send
        + Sync
        + 'static,
>;

/// Convenience handle for using a dynamic block provider where [`BlockProvider`] is expected.
#[derive(Clone)]
#[allow(missing_debug_implementations)]
pub struct DynBlockProviderHandle<B: Block + 'static>(Arc<dyn BlockProvider<Block = B>>);

impl<B: Block + 'static> DynBlockProviderHandle<B> {
    /// Wraps any [`BlockProvider`] as a dynamic provider handle.
    pub fn new<P>(provider: P) -> Self
    where
        P: BlockProvider<Block = B> + Send + Sync + 'static,
    {
        Self(Arc::new(provider))
    }
}

#[async_trait]
impl<B: Block + 'static> BlockProvider for DynBlockProviderHandle<B> {
    type Block = B;

    async fn subscribe_blocks(&self, tx: Sender<Self::Block>) {
        self.0.subscribe_blocks(tx).await
    }

    async fn get_block(&self, block_number: u64) -> eyre::Result<Self::Block> {
        self.0.get_block(block_number).await
    }
}

/// Factory for producing a custom debug block provider.
pub(crate) type DebugBlockProviderFactory<N, AddOns> = Box<
    dyn Fn(
            &NodeConfig<<<N as FullNodeTypes>::Types as NodeTypes>::ChainSpec>,
            &NodeHandle<N, AddOns>,
        ) -> eyre::Result<DynBlockProviderHandle<BlockTy<<N as FullNodeTypes>::Types>>>
        + Send
        + Sync
        + 'static,
>;

type PayloadAttrMapper<N> = Box<
    dyn Fn(PayloadAttrTy<<N as FullNodeTypes>::Types>) -> PayloadAttrTy<<N as FullNodeTypes>::Types>
        + Send
        + Sync
        + 'static,
>;

fn spawn_consensus_client<N, AddOns>(
    handle: &NodeHandle<N, AddOns>,
    provider: DynBlockProviderHandle<BlockTy<<N as FullNodeTypes>::Types>>,
) where
    N: FullNodeComponents<Types: DebugNode<N>>,
    AddOns: RethRpcAddOns<N>,
{
    let client = DebugConsensusClient::new(
        handle.node.add_ons_handle.beacon_engine_handle.clone(),
        provider,
    );
    handle
        .node
        .task_executor
        .spawn_critical("debug consensus client", async move { client.run().await });
}

/// Dev payload builder configuration.
enum DevBuilderConfig<N: FullNodeComponents> {
    Factory(DevPayloadAttributesBuilderFactory<N>),
    Builder(Box<dyn PayloadAttributesBuilder<PayloadAttrTy<N::Types>, HeaderTy<N::Types>>>),
    DefaultWithMap(PayloadAttrMapper<N>),
}

/// Node launcher with support for launching various debugging utilities.
///
/// This launcher wraps an existing launcher and adds debugging capabilities when
/// certain debug flags are enabled. It provides two main debugging features:
///
/// ## RPC Consensus Client
///
/// When `--debug.rpc-consensus-ws <URL>` is provided, the launcher will:
/// - Connect to an external RPC endpoint (`WebSocket` or HTTP)
/// - Fetch blocks from that endpoint (using subscriptions for `WebSocket`, polling for HTTP)
/// - Submit them to the local engine for execution
/// - Useful for testing engine behavior with real network data
///
/// ## Etherscan Consensus Client
///
/// When `--debug.etherscan [URL]` is provided, the launcher will:
/// - Use Etherscan API as a consensus client
/// - Fetch recent blocks from Etherscan
/// - Submit them to the local engine
/// - Requires `ETHERSCAN_API_KEY` environment variable
/// - Falls back to default Etherscan URL for the chain if URL not provided
#[derive(Debug, Clone)]
pub struct DebugNodeLauncher<L = EngineNodeLauncher> {
    inner: L,
}

impl<L> DebugNodeLauncher<L> {
    /// Creates a new instance of the [`DebugNodeLauncher`].
    pub const fn new(inner: L) -> Self {
        Self { inner }
    }
}

/// Future for the [`DebugNodeLauncher`].
#[allow(missing_debug_implementations)]
pub struct DebugNodeLauncherFuture<L, Target, N, AddOns>
where
    N: FullNodeComponents<Types: DebugNode<N>>,
    AddOns: RethRpcAddOns<N>,
{
    inner: L,
    target: Target,
    dev_builder_config: Option<DevBuilderConfig<N>>,
    rpc_consensus_convert: Option<RpcConsensusConvert<N>>,
    debug_block_provider_factory: Option<DebugBlockProviderFactory<N, AddOns>>,
}

impl<L, Target, N, AddOns> DebugNodeLauncherFuture<L, Target, N, AddOns>
where
    N: FullNodeComponents<Types: DebugNode<N>>,
    AddOns: RethRpcAddOns<N>,
    L: LaunchNode<Target, Node = NodeHandle<N, AddOns>>,
{
    pub fn with_payload_attributes_builder(
        self,
        builder: impl PayloadAttributesBuilder<PayloadAttrTy<N::Types>, HeaderTy<N::Types>>,
    ) -> Self {
        Self {
            inner: self.inner,
            target: self.target,
            dev_builder_config: Some(DevBuilderConfig::Builder(Box::new(builder))),
            rpc_consensus_convert: self.rpc_consensus_convert,
            debug_block_provider_factory: self.debug_block_provider_factory,
        }
    }

    pub fn map_debug_payload_attributes(
        self,
        f: impl Fn(PayloadAttrTy<N::Types>) -> PayloadAttrTy<N::Types> + Send + Sync + 'static,
    ) -> Self {
        Self {
            inner: self.inner,
            target: self.target,
            dev_builder_config: Some(DevBuilderConfig::DefaultWithMap(Box::new(f))),
            rpc_consensus_convert: self.rpc_consensus_convert,
            debug_block_provider_factory: self.debug_block_provider_factory,
        }
    }

    /// Overrides the RPC block-to-primitive conversion used by the debug RPC consensus client.
    pub fn with_rpc_consensus_convert(
        self,
        convert: impl Fn(
                <<N as FullNodeTypes>::Types as DebugNode<N>>::RpcBlock,
            ) -> BlockTy<<N as FullNodeTypes>::Types>
            + Send
            + Sync
            + 'static,
    ) -> Self {
        Self {
            inner: self.inner,
            target: self.target,
            dev_builder_config: self.dev_builder_config,
            rpc_consensus_convert: Some(Arc::new(convert)),
            debug_block_provider_factory: self.debug_block_provider_factory,
        }
    }

    /// Provides a factory to build payload attributes for dev mining using runtime context.
    pub fn with_dev_payload_attributes_builder_factory(
        self,
        factory: DevPayloadAttributesBuilderFactory<N>,
    ) -> Self {
        Self {
            inner: self.inner,
            target: self.target,
            dev_builder_config: Some(DevBuilderConfig::Factory(factory)),
            rpc_consensus_convert: self.rpc_consensus_convert,
            debug_block_provider_factory: self.debug_block_provider_factory,
        }
    }

    /// Provides a factory to construct a custom block provider for the debug consensus client.
    pub fn with_debug_block_provider_factory(
        self,
        factory: DebugBlockProviderFactory<N, AddOns>,
    ) -> Self {
        Self {
            inner: self.inner,
            target: self.target,
            dev_builder_config: self.dev_builder_config,
            rpc_consensus_convert: self.rpc_consensus_convert,
            debug_block_provider_factory: Some(factory),
        }
    }

    /// Provides an already constructed block provider for the debug consensus client.
    pub fn with_debug_block_client(
        self,
        provider: impl BlockProvider<Block = BlockTy<<N as FullNodeTypes>::Types>> + 'static,
    ) -> Self {
        let handle = DynBlockProviderHandle::new(provider);
        self.with_debug_block_provider_factory(Box::new(move |_, _| Ok(handle.clone())))
    }

    async fn launch_node(self) -> eyre::Result<NodeHandle<N, AddOns>> {
        let Self {
            inner,
            target,
            dev_builder_config,
            rpc_consensus_convert,
            debug_block_provider_factory,
        } = self;

        let handle = inner.launch_node(target).await?;

        let config = &handle.node.config;
        if let Some(block_provider_factory) = debug_block_provider_factory {
            let provider = block_provider_factory(config, &handle)?;
            spawn_consensus_client(&handle, provider);
        } else if debug_block_provider_configured(config) {
            let convert_rpc = rpc_consensus_convert
                .unwrap_or_else(<N as FullNodeTypes>::Types::default_rpc_convert);

            let block_provider =
                <N as FullNodeTypes>::Types::block_provider(DebugBlockProviderContext {
                    config,
                    convert_rpc,
                })
                .await?;

            spawn_consensus_client(&handle, block_provider);
        } else {
            info!(target: "reth::cli", "Debug consensus client not configured; skipping");
        }

        if config.dev.dev {
            info!(target: "reth::cli", "Using local payload attributes builder for dev mode");

            let blockchain_db = handle.node.provider.clone();
            let chain_spec = config.chain.clone();
            let beacon_engine_handle = handle.node.add_ons_handle.beacon_engine_handle.clone();
            let pool = handle.node.pool.clone();
            let payload_builder_handle = handle.node.payload_builder_handle.clone();

            let builder: Box<
                dyn PayloadAttributesBuilder<
                    PayloadAttrTy<<N as FullNodeTypes>::Types>,
                    HeaderTy<<N as FullNodeTypes>::Types>,
                >,
            > = match dev_builder_config {
                Some(DevBuilderConfig::Factory(f)) => {
                    let ctx = DevMiningContext {
                        chain_spec: chain_spec.clone(),
                        provider: &blockchain_db,
                        pool: &pool,
                        payload_builder_handle: &payload_builder_handle,
                    };
                    f(ctx)
                }
                Some(DevBuilderConfig::Builder(b)) => b,
                Some(DevBuilderConfig::DefaultWithMap(map)) => {
                    let local =
                        <N as FullNodeTypes>::Types::local_payload_attributes_builder(&chain_spec);
                    Box::new(move |parent| map(local.build(&parent)))
                }
                None => {
                    let local =
                        <N as FullNodeTypes>::Types::local_payload_attributes_builder(&chain_spec);
                    Box::new(local)
                }
            };

            let dev_mining_mode = handle.node.config.dev_mining_mode(pool);
            handle.node.task_executor.spawn_critical("local engine", async move {
                LocalMiner::new(
                    blockchain_db,
                    builder,
                    beacon_engine_handle,
                    dev_mining_mode,
                    payload_builder_handle,
                )
                .run()
                .await
            });
        }

        Ok(handle)
    }
}

impl<L, Target, N, AddOns> IntoFuture for DebugNodeLauncherFuture<L, Target, N, AddOns>
where
    Target: Send + 'static,
    N: FullNodeComponents<Types: DebugNode<N>>,
    AddOns: RethRpcAddOns<N> + 'static,
    L: LaunchNode<Target, Node = NodeHandle<N, AddOns>> + 'static,
{
    type Output = eyre::Result<NodeHandle<N, AddOns>>;
    type IntoFuture = Pin<Box<dyn Future<Output = eyre::Result<NodeHandle<N, AddOns>>> + Send>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.launch_node())
    }
}

impl<L, Target, N, AddOns> LaunchNode<Target> for DebugNodeLauncher<L>
where
    Target: Send + 'static,
    N: FullNodeComponents<Types: DebugNode<N>>,
    AddOns: RethRpcAddOns<N> + 'static,
    L: LaunchNode<Target, Node = NodeHandle<N, AddOns>> + 'static,
{
    type Node = NodeHandle<N, AddOns>;
    type Future = DebugNodeLauncherFuture<L, Target, N, AddOns>;

    fn launch_node(self, target: Target) -> Self::Future {
        DebugNodeLauncherFuture {
            inner: self.inner,
            target,
            dev_builder_config: None,
            rpc_consensus_convert: None,
            debug_block_provider_factory: None,
        }
    }
}
