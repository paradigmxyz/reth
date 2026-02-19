use super::LaunchNode;
use crate::{rpc::RethRpcAddOns, EngineNodeLauncher, Node, NodeHandle};
use alloy_consensus::transaction::Either;
use alloy_provider::network::AnyNetwork;
use jsonrpsee::core::{DeserializeOwned, Serialize};
use reth_chainspec::EthChainSpec;
use reth_consensus_debug_client::{
    BlockProvider, DebugConsensusClient, EtherscanBlockProvider, ForkchoiceMode,
    ForkchoiceProvider, RpcBlockProvider,
};
use reth_engine_local::{LocalMiner, MiningMode};
use reth_node_api::{
    BlockTy, FullNodeComponents, FullNodeTypes, HeaderTy, PayloadAttrTy, PayloadAttributesBuilder,
    PayloadTypes,
};
use std::{
    future::{Future, IntoFuture},
    pin::Pin,
    sync::Arc,
};
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
/// impl<N: FullNodeComponents<Types = Self>> DebugNode<N> for MyNode {
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

/// Type alias for the default debug block provider. We use etherscan provider to satisfy the
/// bounds.
pub type DefaultDebugBlockProvider<N> = ForkchoiceProvider<
    EtherscanBlockProvider<
        <<N as FullNodeTypes>::Types as DebugNode<N>>::RpcBlock,
        BlockTy<<N as FullNodeTypes>::Types>,
    >,
>;

/// Future for the [`DebugNodeLauncher`].
#[expect(missing_debug_implementations, clippy::type_complexity)]
pub struct DebugNodeLauncherFuture<L, Target, N, B = DefaultDebugBlockProvider<N>>
where
    N: FullNodeComponents<Types: DebugNode<N>>,
{
    inner: L,
    target: Target,
    local_payload_attributes_builder:
        Option<Box<dyn PayloadAttributesBuilder<PayloadAttrTy<N::Types>, HeaderTy<N::Types>>>>,
    map_attributes:
        Option<Box<dyn Fn(PayloadAttrTy<N::Types>) -> PayloadAttrTy<N::Types> + Send + Sync>>,
    debug_block_provider: Option<B>,
    forkchoice_mode: ForkchoiceMode,
    mining_mode: Option<MiningMode<N::Pool>>,
}

impl<L, Target, N, AddOns, B> DebugNodeLauncherFuture<L, Target, N, B>
where
    N: FullNodeComponents<Types: DebugNode<N>>,
    AddOns: RethRpcAddOns<N>,
    L: LaunchNode<Target, Node = NodeHandle<N, AddOns>>,
    B: BlockProvider<Block = BlockTy<N::Types>> + Clone,
{
    /// Sets a custom payload attributes builder for local mining in dev mode.
    pub fn with_payload_attributes_builder(
        self,
        builder: impl PayloadAttributesBuilder<PayloadAttrTy<N::Types>, HeaderTy<N::Types>>,
    ) -> Self {
        Self {
            inner: self.inner,
            target: self.target,
            local_payload_attributes_builder: Some(Box::new(builder)),
            map_attributes: None,
            debug_block_provider: self.debug_block_provider,
            forkchoice_mode: self.forkchoice_mode,
            mining_mode: self.mining_mode,
        }
    }

    /// Sets a function to map payload attributes before building.
    pub fn map_debug_payload_attributes(
        self,
        f: impl Fn(PayloadAttrTy<N::Types>) -> PayloadAttrTy<N::Types> + Send + Sync + 'static,
    ) -> Self {
        Self {
            inner: self.inner,
            target: self.target,
            local_payload_attributes_builder: None,
            map_attributes: Some(Box::new(f)),
            debug_block_provider: self.debug_block_provider,
            forkchoice_mode: self.forkchoice_mode,
            mining_mode: self.mining_mode,
        }
    }

    /// Sets the [`ForkchoiceMode`] used by the default RPC and Etherscan block providers.
    ///
    /// This has no effect when a custom block provider is set via
    /// [`with_debug_block_provider`](Self::with_debug_block_provider).
    pub const fn with_forkchoice_mode(mut self, mode: ForkchoiceMode) -> Self {
        self.forkchoice_mode = mode;
        self
    }

    /// Sets a custom [`MiningMode`] for the local miner in dev mode.
    ///
    /// This overrides the default mining mode that is derived from the node configuration
    /// (instant or interval). This can be used to provide a custom trigger-based mining mode.
    pub fn with_mining_mode(mut self, mode: MiningMode<N::Pool>) -> Self {
        self.mining_mode = Some(mode);
        self
    }

    /// Sets a custom block provider for the debug consensus client.
    ///
    /// When set, this provider will be used instead of creating an `EtherscanBlockProvider`
    /// or `RpcBlockProvider` from CLI arguments.
    pub fn with_debug_block_provider<B2>(
        self,
        provider: B2,
    ) -> DebugNodeLauncherFuture<L, Target, N, B2>
    where
        B2: BlockProvider<Block = BlockTy<N::Types>> + Clone,
    {
        DebugNodeLauncherFuture {
            inner: self.inner,
            target: self.target,
            local_payload_attributes_builder: self.local_payload_attributes_builder,
            map_attributes: self.map_attributes,
            debug_block_provider: Some(provider),
            forkchoice_mode: self.forkchoice_mode,
            mining_mode: self.mining_mode,
        }
    }

    async fn launch_node(self) -> eyre::Result<NodeHandle<N, AddOns>> {
        let Self {
            inner,
            target,
            local_payload_attributes_builder,
            map_attributes,
            debug_block_provider,
            forkchoice_mode,
            mining_mode,
        } = self;

        let handle = inner.launch_node(target).await?;

        let config = &handle.node.config;

        if let Some(provider) = debug_block_provider {
            info!(target: "reth::cli", "Using custom debug block provider");

            let rpc_consensus_client = DebugConsensusClient::new(
                handle.node.add_ons_handle.beacon_engine_handle.clone(),
                Arc::new(provider),
            );

            handle
                .node
                .task_executor
                .spawn_critical_task("custom debug block provider consensus client", async move {
                    rpc_consensus_client.run().await
                });
        } else if let Some(url) = config.debug.rpc_consensus_url.clone() {
            info!(target: "reth::cli", "Using RPC consensus client: {}", url);

            let block_provider =
                RpcBlockProvider::<AnyNetwork, _>::new(url.as_str(), |block_response| {
                    let json = serde_json::to_value(block_response)
                        .expect("Block serialization cannot fail");
                    let rpc_block =
                        serde_json::from_value(json).expect("Block deserialization cannot fail");
                    N::Types::rpc_to_primitive_block(rpc_block)
                })
                .await?;
            let block_provider = ForkchoiceProvider::with_mode(block_provider, forkchoice_mode);

            let rpc_consensus_client = DebugConsensusClient::new(
                handle.node.add_ons_handle.beacon_engine_handle.clone(),
                Arc::new(block_provider),
            );

            handle.node.task_executor.spawn_critical_task("rpc-ws consensus client", async move {
                rpc_consensus_client.run().await
            });
        } else if let Some(maybe_custom_etherscan_url) = config.debug.etherscan.clone() {
            info!(target: "reth::cli", "Using etherscan as consensus client");

            let chain = config.chain.chain();
            let etherscan_url = maybe_custom_etherscan_url.map(Ok).unwrap_or_else(|| {
                chain
                    .etherscan_urls()
                    .map(|urls| urls.0.to_string())
                    .ok_or_else(|| eyre::eyre!("failed to get etherscan url for chain: {chain}"))
            })?;

            let block_provider = EtherscanBlockProvider::new(
                etherscan_url,
                chain.etherscan_api_key().ok_or_else(|| {
                    eyre::eyre!(
                        "etherscan api key not found for rpc consensus client for chain: {chain}"
                    )
                })?,
                chain.id(),
                N::Types::rpc_to_primitive_block,
            );
            let block_provider = ForkchoiceProvider::with_mode(block_provider, forkchoice_mode);
            let rpc_consensus_client = DebugConsensusClient::new(
                handle.node.add_ons_handle.beacon_engine_handle.clone(),
                Arc::new(block_provider),
            );
            handle
                .node
                .task_executor
                .spawn_critical_task("etherscan consensus client", async move {
                    rpc_consensus_client.run().await
                });
        }

        if config.dev.dev {
            info!(target: "reth::cli", "Using local payload attributes builder for dev mode");

            let blockchain_db = handle.node.provider.clone();
            let chain_spec = config.chain.clone();
            let beacon_engine_handle = handle.node.add_ons_handle.beacon_engine_handle.clone();
            let pool = handle.node.pool.clone();
            let payload_builder_handle = handle.node.payload_builder_handle.clone();

            let builder = if let Some(builder) = local_payload_attributes_builder {
                Either::Left(builder)
            } else {
                let local = N::Types::local_payload_attributes_builder(&chain_spec);
                let builder = if let Some(f) = map_attributes {
                    Either::Left(move |parent| f(local.build(&parent)))
                } else {
                    Either::Right(local)
                };
                Either::Right(builder)
            };

            let dev_mining_mode =
                mining_mode.unwrap_or_else(|| handle.node.config.dev_mining_mode(pool));
            handle.node.task_executor.spawn_critical_task("local engine", async move {
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

impl<L, Target, N, AddOns, B> IntoFuture for DebugNodeLauncherFuture<L, Target, N, B>
where
    Target: Send + 'static,
    N: FullNodeComponents<Types: DebugNode<N>>,
    AddOns: RethRpcAddOns<N> + 'static,
    L: LaunchNode<Target, Node = NodeHandle<N, AddOns>> + 'static,
    B: BlockProvider<Block = BlockTy<N::Types>> + Clone + 'static,
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
    DefaultDebugBlockProvider<N>: BlockProvider<Block = BlockTy<N::Types>> + Clone,
{
    type Node = NodeHandle<N, AddOns>;
    type Future = DebugNodeLauncherFuture<L, Target, N>;

    fn launch_node(self, target: Target) -> Self::Future {
        DebugNodeLauncherFuture {
            inner: self.inner,
            target,
            local_payload_attributes_builder: None,
            map_attributes: None,
            debug_block_provider: None,
            forkchoice_mode: ForkchoiceMode::default(),
            mining_mode: None,
        }
    }
}
