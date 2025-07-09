use super::LaunchNode;
use crate::{rpc::RethRpcAddOns, EngineNodeLauncher, Node, NodeHandle};
use alloy_provider::network::AnyNetwork;
use jsonrpsee::core::{DeserializeOwned, Serialize};
use reth_chainspec::EthChainSpec;
use reth_consensus_debug_client::{DebugConsensusClient, EtherscanBlockProvider, RpcBlockProvider};
use reth_engine_local::LocalMiner;
use reth_node_api::{BlockTy, FullNodeComponents, PayloadAttributesBuilder, PayloadTypes};
use std::sync::Arc;
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
    ) -> impl PayloadAttributesBuilder<
        <<Self as reth_node_api::NodeTypes>::Payload as PayloadTypes>::PayloadAttributes,
    >;
}

/// Node launcher with support for launching various debugging utilities.
///
/// This launcher wraps an existing launcher and adds debugging capabilities when
/// certain debug flags are enabled. It provides two main debugging features:
///
/// ## RPC Consensus Client
///
/// When `--debug.rpc-consensus-ws <URL>` is provided, the launcher will:
/// - Connect to an external RPC `WebSocket` endpoint
/// - Fetch blocks from that endpoint
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

impl<L, Target, N, AddOns> LaunchNode<Target> for DebugNodeLauncher<L>
where
    N: FullNodeComponents<Types: DebugNode<N>>,
    AddOns: RethRpcAddOns<N>,
    L: LaunchNode<Target, Node = NodeHandle<N, AddOns>>,
{
    type Node = NodeHandle<N, AddOns>;

    async fn launch_node(self, target: Target) -> eyre::Result<Self::Node> {
        let handle = self.inner.launch_node(target).await?;

        let config = &handle.node.config;
        if let Some(ws_url) = config.debug.rpc_consensus_ws.clone() {
            info!(target: "reth::cli", "Using RPC WebSocket consensus client: {}", ws_url);

            let block_provider =
                RpcBlockProvider::<AnyNetwork, _>::new(ws_url.as_str(), |block_response| {
                    let json = serde_json::to_value(block_response)
                        .expect("Block serialization cannot fail");
                    let rpc_block =
                        serde_json::from_value(json).expect("Block deserialization cannot fail");
                    N::Types::rpc_to_primitive_block(rpc_block)
                })
                .await?;

            let rpc_consensus_client = DebugConsensusClient::new(
                handle.node.add_ons_handle.beacon_engine_handle.clone(),
                Arc::new(block_provider),
            );

            handle.node.task_executor.spawn_critical("rpc-ws consensus client", async move {
                rpc_consensus_client.run().await
            });
        }

        if let Some(maybe_custom_etherscan_url) = config.debug.etherscan.clone() {
            info!(target: "reth::cli", "Using etherscan as consensus client");

            let chain = config.chain.chain();
            let etherscan_url = maybe_custom_etherscan_url.map(Ok).unwrap_or_else(|| {
                // If URL isn't provided, use default Etherscan URL for the chain if it is known
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
            let rpc_consensus_client = DebugConsensusClient::new(
                handle.node.add_ons_handle.beacon_engine_handle.clone(),
                Arc::new(block_provider),
            );
            handle.node.task_executor.spawn_critical("etherscan consensus client", async move {
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

            let dev_mining_mode = handle.node.config.dev_mining_mode(pool);
            handle.node.task_executor.spawn_critical("local engine", async move {
                LocalMiner::new(
                    blockchain_db,
                    N::Types::local_payload_attributes_builder(&chain_spec),
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
