//! Builder for configuring and launching test node setups.
//!
//! This module provides a flexible builder API for setting up test nodes with custom
//! configurations through closures that modify `NodeConfig` and `TreeConfig`.

use crate::{
    eth_payload_attributes, node::NodeTestContext, wallet::Wallet, NodeBuilderHelper,
    NodeHelperType,
};
use eyre::ensure;
use futures_util::future::TryJoinAll;
use reth_chainspec::EthChainSpec;
use reth_node_api::{PayloadAttrTy, TreeConfig};
use reth_node_builder::{EngineNodeLauncher, NodeBuilder, NodeConfig, NodeHandle};
use reth_node_core::args::{DiscoveryArgs, NetworkArgs, PruningArgs, RpcServerArgs};
use reth_primitives_traits::AlloyBlockHeader;
use reth_provider::providers::BlockchainProvider;
use reth_rpc_server_types::RpcModuleSelection;
use reth_tasks::Runtime;
use std::{path::PathBuf, sync::Arc};
use tracing::{span, Instrument, Level};

/// Builder for configuring and launching test node setups.
///
/// By default, the nodes:
/// - run on a shared [`Runtime::test`] runtime,
/// - build payloads with [`eth_payload_attributes`] for the hardforks active in the chain spec,
/// - have discovery disabled, use unused ports and serve all RPC modules except `testing` over
///   HTTP,
/// - are connected to each other.
///
/// Once launched, each node receives a forkchoice update that makes genesis the head, safe and
/// finalized block.
///
/// Configuration and tree configuration modifiers are applied in the order they are added.
///
/// Use [`E2ETestSetupExt::test_setup`] to create the builder without naming the node type twice:
///
/// ```ignore
/// let (mut node, wallet) = EthereumNode::test_setup(1, test_chain_spec(EthereumHardfork::Cancun))
///     .with_tree_config_modifier(|config| config.with_persistence_threshold(0))
///     .build_single()
///     .await?;
/// ```
pub struct E2ETestSetupBuilder<N: NodeBuilderHelper> {
    num_nodes: usize,
    chain_spec: Arc<N::ChainSpec>,
    runtime: Option<Runtime>,
    attributes_generator: Option<AttributesGenerator<N>>,
    connect_nodes: bool,
    tree_config_modifiers: Vec<TreeConfigModifier>,
    node_config_modifiers: Vec<NodeConfigModifier<N::ChainSpec>>,
}

impl<N: NodeBuilderHelper> E2ETestSetupBuilder<N> {
    /// Creates a new builder for `num_nodes` nodes of the given chain.
    pub fn new(num_nodes: usize, chain_spec: Arc<N::ChainSpec>) -> Self {
        Self {
            num_nodes,
            chain_spec,
            runtime: None,
            attributes_generator: None,
            connect_nodes: true,
            tree_config_modifiers: Vec::new(),
            node_config_modifiers: Vec::new(),
        }
    }

    /// Launches the nodes on the given runtime instead of a new [`Runtime::test`].
    ///
    /// This lets multiple setups, or other components of a test, share the same tokio handle and
    /// rayon pools. Note that the tasks of the nodes are only shut down once all handles to the
    /// runtime are dropped, not when the nodes are dropped.
    pub fn with_runtime(mut self, runtime: Runtime) -> Self {
        self.runtime = Some(runtime);
        self
    }

    /// Sets the generator for the payload attributes of the payloads built by the test nodes.
    ///
    /// The generator is called with the timestamp of the next payload. Defaults to
    /// [`eth_payload_attributes`] for the chain spec of the setup.
    pub fn with_attributes_generator<G>(mut self, generator: G) -> Self
    where
        G: Fn(u64) -> PayloadAttrTy<N> + Send + Sync + 'static,
    {
        self.attributes_generator = Some(Arc::new(generator));
        self
    }

    /// Sets whether nodes should be interconnected (default: true).
    pub const fn with_connect_nodes(mut self, connect_nodes: bool) -> Self {
        self.connect_nodes = connect_nodes;
        self
    }

    /// Adds a modifier for the tree configuration.
    ///
    /// The closure receives the current tree config and returns a modified version. The base
    /// config is the default config with a small cross block cache.
    pub fn with_tree_config_modifier<G>(mut self, modifier: G) -> Self
    where
        G: Fn(TreeConfig) -> TreeConfig + Send + Sync + 'static,
    {
        self.tree_config_modifiers.push(Box::new(modifier));
        self
    }

    /// Adds a modifier for the node configuration.
    ///
    /// The closure receives the current node config and returns a modified version.
    pub fn with_node_config_modifier<G>(mut self, modifier: G) -> Self
    where
        G: Fn(NodeConfig<N::ChainSpec>) -> NodeConfig<N::ChainSpec> + Send + Sync + 'static,
    {
        self.node_config_modifiers.push(Box::new(modifier));
        self
    }

    /// Adds a modifier for the RPC server arguments.
    ///
    /// The closure receives the current arguments, which serve all modules except `testing` over
    /// HTTP on an unused port.
    pub fn with_rpc_modifier<G>(self, modifier: G) -> Self
    where
        G: Fn(RpcServerArgs) -> RpcServerArgs + Send + Sync + 'static,
    {
        self.with_node_config_modifier(move |mut config| {
            config.rpc = modifier(config.rpc);
            config
        })
    }

    /// Sets the pruning arguments for the test nodes.
    pub fn with_pruning(self, pruning: PruningArgs) -> Self {
        self.with_node_config_modifier(move |config| config.with_pruning(pruning.clone()))
    }

    /// Enables v2 storage defaults (`--storage.v2`), routing tx hashes, history
    /// indices, etc. to `RocksDB` and changesets/senders to static files.
    ///
    /// Note that v2 storage is currently also the default for new databases.
    pub fn with_storage_v2(self) -> Self {
        self.with_node_config_modifier(|mut config| {
            config.storage.v2 = true;
            config
        })
    }

    /// Builds and launches the test nodes.
    pub async fn build(self) -> eyre::Result<(Vec<NodeHelperType<N>>, Wallet)> {
        let runtime = self.runtime.unwrap_or_else(Runtime::test);
        let attributes_generator = self.attributes_generator.unwrap_or_else(|| {
            let chain_spec = self.chain_spec.clone();
            Arc::new(move |timestamp| eth_payload_attributes(&chain_spec, timestamp).into())
        });
        let tree_config = self
            .tree_config_modifiers
            .iter()
            .fold(test_tree_config(), |config, modifier| modifier(config));

        let mut nodes = (0..self.num_nodes)
            .map(async |idx| {
                let node_config = self
                    .node_config_modifiers
                    .iter()
                    .fold(test_node_config(self.chain_spec.clone()), |config, modifier| {
                        modifier(config)
                    });
                let attributes_generator = attributes_generator.clone();
                let node = launch_test_node::<N>(
                    node_config,
                    runtime.clone(),
                    tree_config.clone(),
                    reth_db::test_utils::tempdir_path(),
                    move |timestamp| attributes_generator(timestamp),
                )
                .instrument(span!(Level::INFO, "node", idx))
                .await?;

                let genesis = node.block_hash(self.chain_spec.genesis_header().number());
                node.update_forkchoice(genesis, genesis).await?;

                eyre::Ok(node)
            })
            .collect::<TryJoinAll<_>>()
            .await?;

        if self.connect_nodes {
            for idx in 1..self.num_nodes {
                let (prev, current) = nodes.split_at_mut(idx);
                prev[idx - 1].connect(&mut current[0]).await;
            }

            // Connect the last node with the first if there are more than two.
            if self.num_nodes > 2 {
                let (first, rest) = nodes.split_at_mut(1);
                rest.last_mut().unwrap().connect(&mut first[0]).await;
            }
        }

        Ok((nodes, Wallet::default().with_chain_id(self.chain_spec.chain().into())))
    }

    /// Builds and launches a single test node.
    ///
    /// Returns an error if the builder was not configured with exactly one node.
    pub async fn build_single(self) -> eyre::Result<(NodeHelperType<N>, Wallet)> {
        ensure!(self.num_nodes == 1, "expected a single node setup, got {} nodes", self.num_nodes);
        let (mut nodes, wallet) = self.build().await?;
        Ok((nodes.pop().expect("one node was launched"), wallet))
    }
}

impl<N: NodeBuilderHelper> std::fmt::Debug for E2ETestSetupBuilder<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("E2ETestSetupBuilder")
            .field("num_nodes", &self.num_nodes)
            .field("runtime", &self.runtime)
            .field("connect_nodes", &self.connect_nodes)
            .field("tree_config_modifiers", &self.tree_config_modifiers.len())
            .field("node_config_modifiers", &self.node_config_modifiers.len())
            .finish_non_exhaustive()
    }
}

/// Extension trait to create an [`E2ETestSetupBuilder`] from a node type.
pub trait E2ETestSetupExt: NodeBuilderHelper {
    /// Returns an [`E2ETestSetupBuilder`] for `num_nodes` nodes of this type.
    fn test_setup(num_nodes: usize, chain_spec: Arc<Self::ChainSpec>) -> E2ETestSetupBuilder<Self> {
        E2ETestSetupBuilder::new(num_nodes, chain_spec)
    }
}

impl<N: NodeBuilderHelper> E2ETestSetupExt for N {}

/// Closure that modifies the tree configuration of the test nodes.
type TreeConfigModifier = Box<dyn Fn(TreeConfig) -> TreeConfig + Send + Sync>;

/// Closure that modifies the node configuration of each test node.
type NodeConfigModifier<C> = Box<dyn Fn(NodeConfig<C>) -> NodeConfig<C> + Send + Sync>;

/// Closure that generates the payload attributes for a given timestamp.
type AttributesGenerator<N> = Arc<dyn Fn(u64) -> PayloadAttrTy<N> + Send + Sync>;

/// Returns the base tree configuration of test nodes.
pub(crate) fn test_tree_config() -> TreeConfig {
    TreeConfig::default().with_cross_block_cache_size(1024 * 1024)
}

/// Returns the base configuration of a test node.
///
/// Discovery is disabled, all ports are unused and all RPC modules except `testing` are served
/// over HTTP.
pub(crate) fn test_node_config<C>(chain_spec: Arc<C>) -> NodeConfig<C> {
    NodeConfig::new(chain_spec)
        .with_network(NetworkArgs {
            discovery: DiscoveryArgs { disable_discovery: true, ..DiscoveryArgs::default() },
            ..NetworkArgs::default()
        })
        .with_unused_ports()
        .with_rpc(
            RpcServerArgs::default()
                .with_unused_ports()
                .with_http()
                .with_http_api(RpcModuleSelection::All),
        )
}

/// Launches a test node with the engine launcher on the given runtime and datadir.
pub(crate) async fn launch_test_node<N: NodeBuilderHelper>(
    node_config: NodeConfig<N::ChainSpec>,
    runtime: Runtime,
    tree_config: TreeConfig,
    datadir: PathBuf,
    attributes_generator: impl Fn(u64) -> PayloadAttrTy<N> + Send + Sync + 'static,
) -> eyre::Result<NodeHelperType<N>> {
    let node = N::default();
    let NodeHandle { node, node_exit_future: _ } = NodeBuilder::new(node_config)
        .testing_node_with_datadir(runtime, datadir)
        .with_types_and_provider::<N, BlockchainProvider<_>>()
        .with_components(node.components_builder())
        .with_add_ons(node.add_ons())
        .launch_with_fn(|builder| {
            let launcher = EngineNodeLauncher::new(
                builder.task_executor().clone(),
                builder.config().datadir(),
                tree_config,
            );
            builder.launch_with(launcher)
        })
        .await?;

    NodeTestContext::new(node, attributes_generator).await
}
