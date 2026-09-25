//! Builder for configuring and launching test node setups.
//!
//! This module provides a flexible builder API for setting up test nodes with custom
//! configurations through closures that modify `NodeConfig` and `TreeConfig`.

use crate::{
    eth_payload_attributes, node::NodeTestContext, test_chain_spec, wallet::Wallet, Adapter,
    NodeBuilderHelper, NodeHelperType, TmpNodeAdapter,
};
use eyre::ensure;
use futures_util::future::{BoxFuture, TryJoinAll};
use reth_chainspec::{ChainSpec, EthChainSpec, EthereumHardfork};
use reth_node_api::{PayloadAttrTy, TreeConfig};
use reth_node_builder::{
    DebugNode, DebugNodeLauncher, EngineNodeLauncher, Node, NodeBuilder, NodeBuilderWithComponents,
    NodeConfig, NodeHandle,
};
use reth_node_core::{
    args::{DatadirArgs, DiscoveryArgs, NetworkArgs, PruningArgs, RpcServerArgs, StorageArgs},
    dirs::{ChainPath, DataDirPath, MaybePlatformPath},
};
use reth_primitives_traits::AlloyBlockHeader;
use reth_provider::providers::BlockchainProvider;
use reth_rpc_server_types::RpcModuleSelection;
use reth_tasks::Runtime;
use std::{path::PathBuf, sync::Arc, time::Duration};
use tracing::{span, Instrument, Level};

/// Builder for configuring and launching test node setups.
///
/// By default, the nodes:
/// - run on a shared [`Runtime::test`] runtime,
/// - build payloads with [`eth_payload_attributes`] for the hardforks active in the chain spec,
/// - have discovery disabled, use unused ports and serve all RPC modules except `testing` over
///   HTTP,
/// - report an idle sync state from startup, so they gossip transactions before their first block,
/// - are connected to each other.
///
/// Once launched, each node receives a forkchoice update that makes genesis the head, safe and
/// finalized block, unless [dev mining](Self::with_dev_mining) is enabled.
///
/// Configuration and tree configuration modifiers are applied in the order they are added.
///
/// Use [`E2ETestSetupExt::test_setup_for`] to set up nodes on the [`test_chain_spec`] at a
/// hardfork, or [`E2ETestSetupExt::test_setup`] for any other chain spec:
///
/// ```ignore
/// let (mut node, wallet) = EthereumNode::test_setup_for(EthereumHardfork::Cancun)
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
    storage_v2: bool,
    dev_launcher: Option<NodeLauncher<N>>,
    dev_payload_attributes: Option<PayloadAttributesMapper<N>>,
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
            storage_v2: StorageArgs::default().v2,
            dev_launcher: None,
            dev_payload_attributes: None,
        }
    }

    /// Sets the number of nodes to launch.
    pub const fn with_num_nodes(mut self, num_nodes: usize) -> Self {
        self.num_nodes = num_nodes;
        self
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

    /// Sets whether nodes use the v2 storage layout (`--storage.v2`), which routes tx hashes,
    /// history indices, etc. to `RocksDB` and changesets/senders to static files.
    ///
    /// Defaults to the node's `--storage.v2` default. Node config modifiers run afterwards and can
    /// still override it.
    pub const fn with_storage_v2(mut self, storage_v2: bool) -> Self {
        self.storage_v2 = storage_v2;
        self
    }

    /// Sets whether the nodes run in dev mode (`--dev`).
    ///
    /// Unlike [dev mining](Self::with_dev_mining), this only sets the dev flag and does not start a
    /// local miner.
    pub fn with_dev_mode(self, dev: bool) -> Self {
        self.with_node_config_modifier(move |config| config.set_dev(dev))
    }

    /// Launches the node in dev mode with a local miner that builds a block every `block_time`, or
    /// as soon as a transaction is pending if `None`.
    ///
    /// Dev mining is limited to a single node, since every node would mine its own chain. The local
    /// miner drives forkchoice and payload building, so the node does not receive the initial
    /// forkchoice update to genesis, and the block producing helpers of [`NodeTestContext`], e.g.
    /// [`NodeTestContext::advance_block`] and [`NodeTestContext::update_forkchoice`], must not be
    /// used. Use [`Self::map_dev_payload_attributes`] to customize the payload attributes of the
    /// mined blocks.
    pub fn with_dev_mining(mut self, block_time: Option<Duration>) -> Self
    where
        N: DebugNode<Adapter<N>>,
    {
        self.dev_launcher = Some(|args| Box::pin(launch_dev_node::<N>(args)));
        self.with_node_config_modifier(move |mut config| {
            config.dev.dev = true;
            config.dev.block_time = block_time;
            config
        })
    }

    /// Maps the payload attributes of the blocks built by the local miner of
    /// [dev mining](Self::with_dev_mining) nodes, e.g. to set the fee recipient.
    ///
    /// Mappers are applied in the order they are added. Has no effect unless dev mining is
    /// enabled.
    pub fn map_dev_payload_attributes<G>(mut self, map: G) -> Self
    where
        G: Fn(PayloadAttrTy<N>) -> PayloadAttrTy<N> + Send + Sync + 'static,
    {
        self.dev_payload_attributes = Some(match self.dev_payload_attributes.take() {
            Some(prev) => Arc::new(move |attributes| map(prev(attributes))),
            None => Arc::new(map),
        });
        self
    }

    /// Builds and launches the test nodes.
    pub async fn build(self) -> eyre::Result<(Vec<NodeHelperType<N>>, Wallet)> {
        ensure!(
            self.dev_launcher.is_none() || self.num_nodes == 1,
            "dev mining requires a single node setup, got {} nodes",
            self.num_nodes
        );
        let dev_mining = self.dev_launcher.is_some();
        let launch = self.dev_launcher.unwrap_or(|args| Box::pin(launch_test_node::<N>(args)));
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
                let node_config = self.node_config_modifiers.iter().fold(
                    test_node_config(self.chain_spec.clone())
                        .with_storage(StorageArgs { v2: self.storage_v2 }),
                    |config, modifier| modifier(config),
                );
                // The local miner of dev nodes drives forkchoice, unless a modifier disabled dev
                // mode.
                let mines = dev_mining && node_config.dev.dev;
                let node = launch(LaunchArgs {
                    node_config,
                    runtime: runtime.clone(),
                    tree_config: tree_config.clone(),
                    datadir: reth_db::test_utils::tempdir_path(),
                    attributes_generator: attributes_generator.clone(),
                    dev_payload_attributes: self.dev_payload_attributes.clone(),
                })
                .instrument(span!(Level::INFO, "node", idx))
                .await?;

                if !mines {
                    let genesis = node.block_hash(self.chain_spec.genesis_header().number());
                    node.update_forkchoice(genesis, genesis).await?;
                }

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
            .field("storage_v2", &self.storage_v2)
            .field("dev_mining", &self.dev_launcher.is_some())
            .finish_non_exhaustive()
    }
}

/// Extension trait to create an [`E2ETestSetupBuilder`] from a node type.
pub trait E2ETestSetupExt: NodeBuilderHelper {
    /// Returns an [`E2ETestSetupBuilder`] for `num_nodes` nodes of this type.
    fn test_setup(num_nodes: usize, chain_spec: Arc<Self::ChainSpec>) -> E2ETestSetupBuilder<Self> {
        E2ETestSetupBuilder::new(num_nodes, chain_spec)
    }

    /// Returns an [`E2ETestSetupBuilder`] for a single node of this type on the
    /// [`test_chain_spec`] with every hardfork up to and including `fork` active at genesis.
    ///
    /// Use [`E2ETestSetupBuilder::with_num_nodes`] to launch more nodes.
    fn test_setup_for(fork: EthereumHardfork) -> E2ETestSetupBuilder<Self>
    where
        Self::ChainSpec: From<ChainSpec>,
    {
        let chain_spec = Arc::unwrap_or_clone(test_chain_spec(fork));
        E2ETestSetupBuilder::new(1, Arc::new(chain_spec.into()))
    }
}

impl<N: NodeBuilderHelper> E2ETestSetupExt for N {}

/// Closure that modifies the tree configuration of the test nodes.
type TreeConfigModifier = Box<dyn Fn(TreeConfig) -> TreeConfig + Send + Sync>;

/// Closure that modifies the node configuration of each test node.
type NodeConfigModifier<C> = Box<dyn Fn(NodeConfig<C>) -> NodeConfig<C> + Send + Sync>;

/// Closure that generates the payload attributes for a given timestamp.
pub(crate) type AttributesGenerator<N> = Arc<dyn Fn(u64) -> PayloadAttrTy<N> + Send + Sync>;

/// Closure that maps payload attributes.
type PayloadAttributesMapper<N> = Arc<dyn Fn(PayloadAttrTy<N>) -> PayloadAttrTy<N> + Send + Sync>;

/// Builder of a test node that is ready to be launched.
type TestNodeBuilder<N> = NodeBuilderWithComponents<
    TmpNodeAdapter<N>,
    <N as Node<TmpNodeAdapter<N>>>::ComponentsBuilder,
    <N as Node<TmpNodeAdapter<N>>>::AddOns,
>;

/// Function that launches a single test node.
type NodeLauncher<N> = fn(LaunchArgs<N>) -> BoxFuture<'static, eyre::Result<NodeHelperType<N>>>;

/// Arguments for launching a single test node.
pub(crate) struct LaunchArgs<N: NodeBuilderHelper> {
    /// The node configuration.
    pub(crate) node_config: NodeConfig<N::ChainSpec>,
    /// The runtime to launch the node on.
    pub(crate) runtime: Runtime,
    /// The engine tree configuration.
    pub(crate) tree_config: TreeConfig,
    /// The datadir of the node.
    pub(crate) datadir: PathBuf,
    /// Generator for the payload attributes of the payloads built by the test context.
    pub(crate) attributes_generator: AttributesGenerator<N>,
    /// Mapper for the payload attributes of the local miner in dev mode.
    pub(crate) dev_payload_attributes: Option<PayloadAttributesMapper<N>>,
}

/// Returns the base tree configuration of test nodes.
pub(crate) fn test_tree_config() -> TreeConfig {
    TreeConfig::default().with_cross_block_cache_size(1024 * 1024)
}

/// Returns the base configuration of a test node.
///
/// Discovery is disabled, all ports are unused, all RPC modules except `testing` are served over
/// HTTP and the node reports an idle sync state from startup.
pub(crate) fn test_node_config<C>(chain_spec: Arc<C>) -> NodeConfig<C> {
    let mut config = NodeConfig::new(chain_spec)
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
        );
    // Nodes otherwise report that they are syncing until their first canonical block, which
    // e.g. stops transaction gossip.
    config.debug.startup_sync_state_idle = true;
    config
}

/// Launches a test node with the engine launcher.
pub(crate) async fn launch_test_node<N: NodeBuilderHelper>(
    args: LaunchArgs<N>,
) -> eyre::Result<NodeHelperType<N>> {
    let LaunchArgs { node_config, runtime, tree_config, datadir, attributes_generator, .. } = args;
    let (builder, datadir) = test_node_builder::<N>(node_config, datadir);
    let NodeHandle { node, node_exit_future: _ } =
        builder.launch_with(EngineNodeLauncher::new(runtime, datadir, tree_config)).await?;

    NodeTestContext::new(node, move |timestamp| attributes_generator(timestamp)).await
}

/// Launches a test node with the debug launcher, which runs a local miner in dev mode.
async fn launch_dev_node<N>(args: LaunchArgs<N>) -> eyre::Result<NodeHelperType<N>>
where
    N: NodeBuilderHelper + DebugNode<Adapter<N>>,
{
    let LaunchArgs {
        node_config,
        runtime,
        tree_config,
        datadir,
        attributes_generator,
        dev_payload_attributes,
    } = args;
    let (builder, datadir) = test_node_builder::<N>(node_config, datadir);
    let launch = builder.launch_with(DebugNodeLauncher::new(EngineNodeLauncher::new(
        runtime,
        datadir,
        tree_config,
    )));
    let launch = match dev_payload_attributes {
        Some(map) => launch.map_debug_payload_attributes(move |attributes| map(attributes)),
        None => launch,
    };
    let NodeHandle { node, node_exit_future: _ } = launch.await?;

    NodeTestContext::new(node, move |timestamp| attributes_generator(timestamp)).await
}

/// Returns the builder of a test node with a temporary database in `datadir`, and the resolved
/// datadir of the node.
///
/// The datadir is removed when the node is dropped.
fn test_node_builder<N: NodeBuilderHelper>(
    node_config: NodeConfig<N::ChainSpec>,
    datadir: PathBuf,
) -> (TestNodeBuilder<N>, ChainPath<DataDirPath>) {
    let datadir_args =
        DatadirArgs { datadir: MaybePlatformPath::from(datadir), ..node_config.datadir.clone() };
    let node_config = node_config.with_datadir_args(datadir_args);
    let datadir = node_config.datadir();
    let database = reth_db::test_utils::create_test_rw_db_with_datadir(datadir.data_dir());
    let node = N::default();
    let builder = NodeBuilder::new(node_config)
        .with_database(database)
        .with_types_and_provider::<N, BlockchainProvider<_>>()
        .with_components(node.components_builder())
        .with_add_ons(node.add_ons());
    (builder, datadir)
}
