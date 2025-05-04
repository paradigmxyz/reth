use super::LaunchNode;
use crate::{rpc::RethRpcAddOns, EngineNodeLauncher, Node, NodeHandle};
use alloy_provider::network::AnyNetwork;
use jsonrpsee::core::{DeserializeOwned, Serialize};
use reth_chainspec::EthChainSpec;
use reth_consensus_debug_client::{DebugConsensusClient, EtherscanBlockProvider, RpcBlockProvider};
use reth_node_api::{BlockTy, FullNodeComponents};
use std::sync::Arc;
use tracing::info;
/// [`Node`] extension with support for debugging utilities, see [`DebugNodeLauncher`] for more
/// context.
pub trait DebugNode<N: FullNodeComponents>: Node<N> {
    /// RPC block type. Used by [`DebugConsensusClient`] to fetch blocks and submit them to the
    /// engine.
    type RpcBlock: Serialize + DeserializeOwned + 'static;

    /// Converts an RPC block to a primitive block.
    fn rpc_to_primitive_block(rpc_block: Self::RpcBlock) -> BlockTy<Self>;
}

/// Node launcher with support for launching various debugging utilities.
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

        // TODO: migrate to devmode with https://github.com/paradigmxyz/reth/issues/10104
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

        Ok(handle)
    }
}
