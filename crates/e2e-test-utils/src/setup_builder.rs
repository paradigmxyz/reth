//! Builder for configuring and launching test node setups.
//!
//! This module provides a flexible builder API for setting up test nodes with custom
//! configurations through closures that modify `NodeConfig` and `TreeConfig`.

use crate::{
    eth_payload_attributes,
    node::{NodeRestart, NodeTestContext, Relaunch},
    test_chain_spec,
    wallet::Wallet,
    Adapter, NodeBuilderHelper, NodeHelperType, TmpDB, TmpNodeAdapter,
};
use alloy_primitives::B256;
use alloy_rpc_types_engine::{ForkchoiceState, PayloadAttributes};
use alloy_rpc_types_eth::BlockNumberOrTag;
use eyre::{ensure, eyre, WrapErr};
use futures_util::future::{BoxFuture, TryJoinAll};
use reth_chainspec::{ChainSpec, EthChainSpec, EthereumHardfork};
use reth_db::{init_db, mdbx::DatabaseArguments, test_utils::TempDatabase};
use reth_network_api::BlockDownloaderProvider;
use reth_node_api::{PayloadAttrTy, TreeConfig};
use reth_node_builder::{
    sync::{BackfillSyncBuilder, PipelineBackfill},
    DebugNode, DebugNodeLauncher, EngineNodeLauncher, Node, NodeBuilder, NodeBuilderWithComponents,
    NodeComponents, NodeComponentsBuilder, NodeConfig, NodeHandle, NodeTypesWithDBAdapter,
};
use reth_node_core::{
    args::{DatadirArgs, DiscoveryArgs, NetworkArgs, PruningArgs, RpcServerArgs, StorageArgs},
    dirs::{ChainPath, DataDirPath, MaybePlatformPath},
};
use reth_primitives_traits::AlloyBlockHeader;
use reth_provider::{providers::BlockchainProvider, BlockReaderIdExt};
use reth_rpc_server_types::RpcModuleSelection;
use reth_tasks::Runtime;
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tracing::{span, Instrument, Level};

/// Builder for configuring and launching test node setups.
///
/// By default, the nodes:
/// - are [`Default`] instances of `N`, see [`Self::with_node`],
/// - run on a shared [`Runtime::test`] runtime, unless they are
///   [restartable](Self::with_restartable_nodes),
/// - build payloads with [`eth_payload_attributes`] for the hardforks active in the chain spec,
///   unless created with [`Self::new_with_attributes_generator`],
/// - have discovery disabled, use unused ports and serve all RPC modules except `testing` over
///   HTTP,
/// - report an idle sync state from startup, so they gossip transactions before their first block,
/// - do not persist their peers, so a restarted node only connects to the peers a test connects it
///   to,
/// - are connected to each other.
///
/// Once launched, each node receives a forkchoice update that makes genesis the head, safe and
/// finalized block, unless [dev mining](Self::with_dev_mining) is enabled.
///
/// Configuration and tree configuration modifiers are applied in the order they are added. Like
/// for a node launched from the command line, the tree configuration of a node is derived from its
/// final node configuration, see [`NodeConfig::tree_config`], so the engine arguments and
/// `--debug.skip-state-root` set by node configuration modifiers take effect. Tree configuration
/// modifiers are applied last and take precedence over them.
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
    attributes_generator: AttributesGenerator<N>,
    connect_nodes: bool,
    tree_config_modifiers: Vec<TreeConfigModifier>,
    node_config_modifiers: Vec<NodeConfigModifier<N::ChainSpec>>,
    storage_v2: bool,
    dev_mining: bool,
    launcher: Option<NodeLauncher<N>>,
    dev_payload_attributes: Option<PayloadAttributesMapper<N>>,
    node_factory: NodeFactory<N>,
    node_builder_modifiers: Vec<NodeBuilderModifier<N>>,
    restartable: bool,
}

impl<N: NodeBuilderHelper> E2ETestSetupBuilder<N> {
    /// Creates a new builder for `num_nodes` nodes of the given chain.
    ///
    /// The nodes build payloads with [`eth_payload_attributes`] for the chain spec, which requires
    /// the payload attributes of the node to be convertible from Ethereum's. Use
    /// [`Self::new_with_attributes_generator`] for other nodes.
    pub fn new(num_nodes: usize, chain_spec: Arc<N::ChainSpec>) -> Self
    where
        PayloadAttrTy<N>: From<PayloadAttributes>,
    {
        let attributes_chain_spec = chain_spec.clone();
        Self::new_with_attributes_generator(num_nodes, chain_spec, move |timestamp| {
            eth_payload_attributes(&attributes_chain_spec, timestamp).into()
        })
    }

    /// Creates a new builder for `num_nodes` nodes of the given chain, whose payloads are built
    /// with the attributes returned by `attributes_generator`, see
    /// [`Self::with_attributes_generator`].
    ///
    /// Unlike [`Self::new`], this does not require the payload attributes of the node to be
    /// convertible from Ethereum's.
    pub fn new_with_attributes_generator<G>(
        num_nodes: usize,
        chain_spec: Arc<N::ChainSpec>,
        attributes_generator: G,
    ) -> Self
    where
        G: Fn(u64) -> PayloadAttrTy<N> + Send + Sync + 'static,
    {
        Self {
            num_nodes,
            chain_spec,
            runtime: None,
            attributes_generator: Arc::new(attributes_generator),
            connect_nodes: true,
            tree_config_modifiers: Vec::new(),
            node_config_modifiers: Vec::new(),
            storage_v2: StorageArgs::default().v2,
            dev_mining: false,
            launcher: None,
            dev_payload_attributes: None,
            node_factory: Arc::new(|_| N::default()),
            node_builder_modifiers: Vec::new(),
            restartable: false,
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
    /// runtime are dropped, not when the nodes are dropped. Restartable nodes, see
    /// [`Self::with_restartable_nodes`], can not share a runtime.
    pub fn with_runtime(mut self, runtime: Runtime) -> Self {
        self.runtime = Some(runtime);
        self
    }

    /// Sets the generator for the payload attributes of the payloads built by the test nodes.
    ///
    /// The generator is called with the timestamp of the next payload. It replaces the generator
    /// of the constructor, which is [`eth_payload_attributes`] for the chain spec of the setup for
    /// [`Self::new`].
    pub fn with_attributes_generator<G>(mut self, generator: G) -> Self
    where
        G: Fn(u64) -> PayloadAttrTy<N> + Send + Sync + 'static,
    {
        self.attributes_generator = Arc::new(generator);
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
    /// config is derived from the node configuration after all node configuration modifiers ran,
    /// see [`NodeConfig::tree_config`]. Unless a node configuration modifier changes
    /// `engine.cross_block_cache_size`, it uses a small cross block cache of 1 MiB.
    pub fn with_tree_config_modifier<G>(mut self, modifier: G) -> Self
    where
        G: Fn(TreeConfig) -> TreeConfig + Send + Sync + 'static,
    {
        self.tree_config_modifiers.push(Box::new(modifier));
        self
    }

    /// Adds a modifier for the node configuration.
    ///
    /// The closure receives the current node config and returns a modified version. Its engine
    /// arguments and `debug.skip_state_root` configure the engine tree, unless a
    /// [tree configuration modifier](Self::with_tree_config_modifier) overrides them.
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
        self.dev_mining = true;
        self.launcher.get_or_insert_with(|| Self::backfill_launcher(|_| PipelineBackfill));
        self.with_node_config_modifier(move |mut config| {
            config.dev.dev = true;
            config.dev.block_time = block_time;
            config
        })
    }

    /// Launches nodes with the engine's backfill that `backfill` builds from each node's
    /// configuration, instead of the staged pipeline, e.g. `EthereumBackfill::new`.
    pub fn with_backfill<F, B>(mut self, backfill: F) -> Self
    where
        N: DebugNode<Adapter<N>>,
        F: Fn(&NodeConfig<N::ChainSpec>) -> B + Send + Sync + 'static,
        B: BackfillSyncBuilder<NodeTypesWithDBAdapter<N, TmpDB>, BackfillClient<N>>
            + Send
            + 'static,
    {
        self.launcher = Some(Self::backfill_launcher(backfill));
        self
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

    /// Sets the factory of the node instances, which is called with the index of each node when it
    /// is launched.
    ///
    /// Defaults to [`Default::default`]. Use this to configure the node type, or to keep a handle
    /// of the node instance, e.g. one that reads state of the node once it is launched.
    pub fn with_node<F>(mut self, node: F) -> Self
    where
        F: Fn(usize) -> N + Send + Sync + 'static,
    {
        self.node_factory = Arc::new(node);
        self
    }

    /// Adds a modifier for the node builder of each test node.
    ///
    /// The closure receives the [`TestNodeBuilder`] after the node types, components and add-ons
    /// are configured, and returns it to be launched. It can e.g. install an `ExEx` with
    /// [`install_exex`](NodeBuilderWithComponents::install_exex), extend the RPC modules with
    /// [`extend_rpc_modules`](NodeBuilderWithComponents::extend_rpc_modules), set the
    /// `on_component_initialized`, `on_node_started` and `on_rpc_started` hooks, or modify the
    /// add-ons with [`map_add_ons`](NodeBuilderWithComponents::map_add_ons).
    ///
    /// Modifiers are applied to every node in the order they are added. The node builder keeps a
    /// single hook of each kind, so a modifier that sets e.g. the `extend_rpc_modules` hook
    /// replaces the one set by an earlier modifier. The datadir and tree configuration of the node
    /// are derived before the modifiers run, so change the node configuration with
    /// [`Self::with_node_config_modifier`] instead of `builder.config`.
    pub fn with_node_builder_modifier<G>(mut self, modifier: G) -> Self
    where
        G: Fn(TestNodeBuilder<N>) -> TestNodeBuilder<N> + Send + Sync + 'static,
    {
        self.node_builder_modifiers.push(Arc::new(modifier));
        self
    }

    /// Makes the nodes restartable, so they can be stopped and launched again on the same datadir
    /// with [`NodeTestContext::stop`] and [`NodeTestContext::restart`].
    ///
    /// Each restartable node runs on a [`Runtime::test`] runtime of its own instead of the shared
    /// one, so stopping it shuts down only its tasks. This costs a few threads per node, which is
    /// why nodes are not restartable by default. Conflicts with [`Self::with_runtime`].
    pub const fn with_restartable_nodes(mut self) -> Self {
        self.restartable = true;
        self
    }

    /// Builds and launches the test nodes.
    pub async fn build(self) -> eyre::Result<(Vec<NodeHelperType<N>>, Wallet)> {
        ensure!(
            !self.dev_mining || self.num_nodes == 1,
            "dev mining requires a single node setup, got {} nodes",
            self.num_nodes
        );
        ensure!(
            !self.restartable || self.runtime.is_none(),
            "restartable nodes run on a runtime of their own and can not use the runtime set with \
             `with_runtime`"
        );
        let dev_mining = self.dev_mining;
        let launch = self.launcher.clone().unwrap_or_else(|| {
            Arc::new(|args, database, _| {
                Box::pin(launch_test_node(args, database, PipelineBackfill))
            })
        });
        // Restartable nodes create their own runtime when they are launched.
        let runtime =
            (!self.restartable).then(|| self.runtime.clone().unwrap_or_else(Runtime::test));

        let mut nodes = (0..self.num_nodes)
            .map(async |idx| {
                let (node_config, tree_config) = self.node_and_tree_config();
                // The local miner of dev nodes drives forkchoice, unless a modifier disabled dev
                // mode.
                let mines = dev_mining && node_config.dev.dev;
                let args = LaunchArgs {
                    idx,
                    node_factory: self.node_factory.clone(),
                    node_builder_modifiers: self.node_builder_modifiers.clone(),
                    node_config,
                    runtime: runtime.clone(),
                    tree_config,
                    datadir: reth_db::test_utils::tempdir_path(),
                    attributes_generator: self.attributes_generator.clone(),
                    dev_payload_attributes: self.dev_payload_attributes.clone(),
                };
                let node = launch_node(launch.clone(), args, self.restartable, mines)
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

    /// Returns the configuration of a test node and the tree configuration derived from it.
    ///
    /// The node configuration modifiers run first, the tree configuration modifiers are applied to
    /// the tree configuration of the resulting node configuration.
    fn node_and_tree_config(&self) -> (NodeConfig<N::ChainSpec>, TreeConfig) {
        let node_config = self.node_config_modifiers.iter().fold(
            test_node_config(self.chain_spec.clone())
                .with_storage(StorageArgs { v2: self.storage_v2 }),
            |config, modifier| modifier(config),
        );
        let tree_config = self
            .tree_config_modifiers
            .iter()
            .fold(node_config.tree_config(), |config, modifier| modifier(config));
        (node_config, tree_config)
    }

    /// Returns a launcher whose nodes sync with the backfill `backfill` builds from their
    /// configuration, and that launches mining nodes with the dev launcher.
    fn backfill_launcher<F, B>(backfill: F) -> NodeLauncher<N>
    where
        N: DebugNode<Adapter<N>>,
        F: Fn(&NodeConfig<N::ChainSpec>) -> B + Send + Sync + 'static,
        B: BackfillSyncBuilder<NodeTypesWithDBAdapter<N, TmpDB>, BackfillClient<N>>
            + Send
            + 'static,
    {
        Arc::new(move |args, database, mines| {
            let backfill = backfill(&args.node_config);
            if mines {
                Box::pin(launch_dev_node(args, database, backfill))
            } else {
                Box::pin(launch_test_node(args, database, backfill))
            }
        })
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
            .field("dev_mining", &self.dev_mining)
            .field("node_builder_modifiers", &self.node_builder_modifiers.len())
            .field("restartable", &self.restartable)
            .finish_non_exhaustive()
    }
}

/// Extension trait to create an [`E2ETestSetupBuilder`] from a node type.
pub trait E2ETestSetupExt: NodeBuilderHelper {
    /// Returns an [`E2ETestSetupBuilder`] for `num_nodes` nodes of this type, see
    /// [`E2ETestSetupBuilder::new`].
    fn test_setup(num_nodes: usize, chain_spec: Arc<Self::ChainSpec>) -> E2ETestSetupBuilder<Self>
    where
        PayloadAttrTy<Self>: From<PayloadAttributes>,
    {
        E2ETestSetupBuilder::new(num_nodes, chain_spec)
    }

    /// Returns an [`E2ETestSetupBuilder`] for a single node of this type on the
    /// [`test_chain_spec`] with every hardfork up to and including `fork` active at genesis, see
    /// [`E2ETestSetupBuilder::new`].
    ///
    /// Use [`E2ETestSetupBuilder::with_num_nodes`] to launch more nodes.
    fn test_setup_for(fork: EthereumHardfork) -> E2ETestSetupBuilder<Self>
    where
        Self::ChainSpec: From<ChainSpec>,
        PayloadAttrTy<Self>: From<PayloadAttributes>,
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

/// Builder of a test node with the node types, components and add-ons of `N`, which is ready to
/// be launched.
///
/// This is what [`E2ETestSetupBuilder::with_node_builder_modifier`] receives.
pub type TestNodeBuilder<N> = NodeBuilderWithComponents<
    TmpNodeAdapter<N>,
    <N as Node<TmpNodeAdapter<N>>>::ComponentsBuilder,
    <N as Node<TmpNodeAdapter<N>>>::AddOns,
>;

/// Closure that returns the node instance of the test node with the given index.
type NodeFactory<N> = Arc<dyn Fn(usize) -> N + Send + Sync>;

/// Closure that modifies the node builder of each test node.
type NodeBuilderModifier<N> = Arc<dyn Fn(TestNodeBuilder<N>) -> TestNodeBuilder<N> + Send + Sync>;

/// Closure that launches a single test node on the given database in its datadir, with the dev
/// launcher if the node mines.
type NodeLauncher<N> = Arc<
    dyn Fn(LaunchArgs<N>, TmpDB, bool) -> BoxFuture<'static, eyre::Result<NodeHelperType<N>>>
        + Send
        + Sync,
>;

/// Client the network of a test node hands its backfill.
type BackfillClient<N> = <<<<N as Node<TmpNodeAdapter<N>>>::ComponentsBuilder as NodeComponentsBuilder<
    TmpNodeAdapter<N>,
>>::Components as NodeComponents<TmpNodeAdapter<N>>>::Network as BlockDownloaderProvider>::Client;

/// Arguments for launching a single test node.
pub(crate) struct LaunchArgs<N: NodeBuilderHelper> {
    /// The index of the node in the setup.
    pub(crate) idx: usize,
    /// Factory of the node instance, called with the index of the node.
    pub(crate) node_factory: NodeFactory<N>,
    /// Modifiers for the node builder, applied in order.
    pub(crate) node_builder_modifiers: Vec<NodeBuilderModifier<N>>,
    /// The node configuration.
    pub(crate) node_config: NodeConfig<N::ChainSpec>,
    /// The runtime to launch the node on, or `None` to launch it on a new [`Runtime::test`]
    /// runtime of its own.
    pub(crate) runtime: Option<Runtime>,
    /// The engine tree configuration.
    pub(crate) tree_config: TreeConfig,
    /// The datadir of the node.
    pub(crate) datadir: PathBuf,
    /// Generator for the payload attributes of the payloads built by the test context.
    pub(crate) attributes_generator: AttributesGenerator<N>,
    /// Mapper for the payload attributes of the local miner in dev mode.
    pub(crate) dev_payload_attributes: Option<PayloadAttributesMapper<N>>,
}

// Derived `Clone` would require the chain spec to be `Clone`.
impl<N: NodeBuilderHelper> Clone for LaunchArgs<N> {
    fn clone(&self) -> Self {
        Self {
            idx: self.idx,
            node_factory: self.node_factory.clone(),
            node_builder_modifiers: self.node_builder_modifiers.clone(),
            node_config: self.node_config.clone(),
            runtime: self.runtime.clone(),
            tree_config: self.tree_config.clone(),
            datadir: self.datadir.clone(),
            attributes_generator: self.attributes_generator.clone(),
            dev_payload_attributes: self.dev_payload_attributes.clone(),
        }
    }
}

/// Returns the base configuration of a test node.
///
/// Discovery is disabled, all ports are unused, all RPC modules except `testing` are served over
/// HTTP, the node reports an idle sync state from startup, does not persist its peers and the
/// engine uses a cross block cache of 1 MiB.
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
    // A stopped node would otherwise save its peers and dial them on its own once it is started
    // again, racing the test that connects it.
    config.network.no_persist_peers = true;
    // The cross block cache is allocated up front, and the default of 4 GiB is far more than tests
    // need. The size is in MiB.
    config.engine.cross_block_cache_size = 1;
    config
}

/// Launches a test node with a custom backfill.
pub(crate) async fn launch_test_node<N, B>(
    args: LaunchArgs<N>,
    database: TmpDB,
    backfill: B,
) -> eyre::Result<NodeHelperType<N>>
where
    N: NodeBuilderHelper,
    B: BackfillSyncBuilder<NodeTypesWithDBAdapter<N, TmpDB>, BackfillClient<N>> + 'static,
{
    let LaunchArgs {
        idx,
        node_factory,
        node_builder_modifiers,
        node_config,
        runtime,
        tree_config,
        datadir,
        attributes_generator,
        ..
    } = args;
    let (builder, datadir) = test_node_builder(
        node_factory(idx),
        node_config,
        datadir,
        database,
        &node_builder_modifiers,
    );
    let runtime = runtime.unwrap_or_else(Runtime::test);
    let NodeHandle { node, node_exit_future } = builder
        .launch_with(EngineNodeLauncher::new(runtime, datadir, tree_config).with_backfill(backfill))
        .await?;

    let mut node =
        NodeTestContext::new(node, move |timestamp| attributes_generator(timestamp)).await?;
    node.exit_future = Mutex::new(Some(node_exit_future));
    Ok(node)
}

/// Launches a test node with the debug launcher, which runs a local miner in dev mode, and a
/// custom backfill.
async fn launch_dev_node<N, B>(
    args: LaunchArgs<N>,
    database: TmpDB,
    backfill: B,
) -> eyre::Result<NodeHelperType<N>>
where
    N: NodeBuilderHelper + DebugNode<Adapter<N>>,
    B: BackfillSyncBuilder<NodeTypesWithDBAdapter<N, TmpDB>, BackfillClient<N>> + 'static,
{
    let LaunchArgs {
        idx,
        node_factory,
        node_builder_modifiers,
        node_config,
        runtime,
        tree_config,
        datadir,
        attributes_generator,
        dev_payload_attributes,
    } = args;
    let (builder, datadir) = test_node_builder(
        node_factory(idx),
        node_config,
        datadir,
        database,
        &node_builder_modifiers,
    );
    let runtime = runtime.unwrap_or_else(Runtime::test);
    let launch = builder.launch_with(DebugNodeLauncher::new(
        EngineNodeLauncher::new(runtime, datadir, tree_config).with_backfill(backfill),
    ));
    let launch = match dev_payload_attributes {
        Some(map) => launch.map_debug_payload_attributes(move |attributes| map(attributes)),
        None => launch,
    };
    let NodeHandle { node, node_exit_future } = launch.await?;

    let mut node =
        NodeTestContext::new(node, move |timestamp| attributes_generator(timestamp)).await?;
    node.exit_future = Mutex::new(Some(node_exit_future));
    Ok(node)
}

/// Returns the builder of the test node `node` with the database in `datadir` and the node
/// builder modifiers applied, and the resolved datadir of the node.
fn test_node_builder<N: NodeBuilderHelper>(
    node: N,
    node_config: NodeConfig<N::ChainSpec>,
    datadir: PathBuf,
    database: TmpDB,
    node_builder_modifiers: &[NodeBuilderModifier<N>],
) -> (TestNodeBuilder<N>, ChainPath<DataDirPath>) {
    let datadir_args =
        DatadirArgs { datadir: MaybePlatformPath::from(datadir), ..node_config.datadir.clone() };
    let node_config = node_config.with_datadir_args(datadir_args);
    let datadir = node_config.datadir();
    let builder = NodeBuilder::new(node_config)
        .with_database(database)
        .with_types_and_provider::<N, BlockchainProvider<_>>()
        .with_components(node.components_builder())
        .with_add_ons(node.add_ons());
    let builder =
        node_builder_modifiers.iter().fold(builder, |builder, modifier| modifier(builder));
    (builder, datadir)
}

/// Launches a test node with `launch` on the database in the datadir of `args`, which is created
/// if it does not exist yet.
///
/// A `restartable` node keeps what [`NodeTestContext::stop`] needs to stop it and launch it again
/// with the same arguments. Unless the node `mines` in dev mode, the relaunched node receives a
/// forkchoice update that restates the forkchoice state it loaded from disk.
fn launch_node<N: NodeBuilderHelper>(
    launch: NodeLauncher<N>,
    args: LaunchArgs<N>,
    restartable: bool,
    mines: bool,
) -> BoxFuture<'static, eyre::Result<NodeHelperType<N>>> {
    Box::pin(async move {
        let database = open_test_database(&args.datadir)?;
        let relaunch_args = restartable.then(|| args.clone());
        let mut node = launch(args, database.clone(), mines).await?;
        if let Some(args) = relaunch_args {
            let relaunch: Relaunch<_> =
                Arc::new(move || relaunch_node(launch.clone(), args.clone(), mines));
            node.restart = Some(NodeRestart { database, relaunch });
        }
        Ok(node)
    })
}

/// Launches a stopped restartable node again, see [`launch_node`].
fn relaunch_node<N: NodeBuilderHelper>(
    launch: NodeLauncher<N>,
    args: LaunchArgs<N>,
    mines: bool,
) -> BoxFuture<'static, eyre::Result<NodeHelperType<N>>> {
    let span = span!(Level::INFO, "node", idx = args.idx);
    Box::pin(
        async move {
            let node = launch_node(launch, args, true, mines).await?;
            if !mines {
                restate_forkchoice(&node).await?;
            }
            Ok(node)
        }
        .instrument(span),
    )
}

/// Sends a forkchoice update to a restarted node that restates the head, safe and finalized block
/// it loaded from disk.
///
/// Returns an error if the engine does not report the update valid.
async fn restate_forkchoice<N: NodeBuilderHelper>(node: &NodeHelperType<N>) -> eyre::Result<()> {
    let provider = &node.inner.provider;
    let hash = |tag| -> eyre::Result<Option<B256>> {
        Ok(provider.sealed_header_by_number_or_tag(tag)?.map(|header| header.hash()))
    };
    let head = hash(BlockNumberOrTag::Latest)?
        .ok_or_else(|| eyre!("the restarted node has no latest block"))?;
    // A zero hash leaves the safe or finalized block unset if the node did not persist one.
    let state = ForkchoiceState {
        head_block_hash: head,
        safe_block_hash: hash(BlockNumberOrTag::Safe)?.unwrap_or_default(),
        finalized_block_hash: hash(BlockNumberOrTag::Finalized)?.unwrap_or_default(),
    };
    let updated =
        node.inner.add_ons_handle.beacon_engine_handle.fork_choice_updated(state, None).await?;
    ensure!(
        updated.is_valid(),
        "forkchoice update to the head {head} the restarted node loaded from disk is not valid: {}",
        updated.payload_status.status
    );
    Ok(())
}

/// Opens the database in `datadir` like [`create_test_rw_db_with_datadir`], creating it if it does
/// not exist yet.
///
/// The datadir is removed when the returned database is dropped.
///
/// [`create_test_rw_db_with_datadir`]: reth_db::test_utils::create_test_rw_db_with_datadir
pub(crate) fn open_test_database(datadir: &Path) -> eyre::Result<TmpDB> {
    let path = datadir.join("db");
    let database = init_db(&path, DatabaseArguments::test())
        .wrap_err_with(|| format!("failed to open the database at {}", path.display()))?;
    Ok(Arc::new(TempDatabase::new(database, datadir.to_path_buf())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_node_ethereum::EthereumNode;

    #[test]
    fn tree_config_uses_small_cross_block_cache() {
        let (_, tree_config) =
            EthereumNode::test_setup_for(EthereumHardfork::Cancun).node_and_tree_config();
        assert_eq!(tree_config.cross_block_cache_size(), 1024 * 1024);
    }

    #[test]
    fn tree_config_follows_node_config() {
        let (_, tree_config) = EthereumNode::test_setup_for(EthereumHardfork::Cancun)
            .with_node_config_modifier(|mut config| {
                config.engine.cross_block_cache_size = 2;
                config.engine.persistence_threshold = 0;
                config.debug.skip_state_root = true;
                config
            })
            .node_and_tree_config();
        assert_eq!(tree_config.cross_block_cache_size(), 2 * 1024 * 1024);
        assert_eq!(tree_config.persistence_threshold(), 0);
        assert!(tree_config.skip_state_root());
    }

    #[test]
    fn tree_config_modifiers_override_node_config() {
        // Tree config modifiers apply last, even if they are added before node config modifiers.
        let (_, tree_config) = EthereumNode::test_setup_for(EthereumHardfork::Cancun)
            .with_tree_config_modifier(|config| config.with_state_root_fallback(false))
            .with_node_config_modifier(|mut config| {
                config.engine.state_root_fallback = true;
                config
            })
            .node_and_tree_config();
        assert!(!tree_config.state_root_fallback());
    }

    /// Nodes whose payload attributes are not convertible from Ethereum's can be set up with an
    /// explicit attributes generator.
    #[expect(dead_code)]
    async fn build_with_attributes_generator<N: NodeBuilderHelper>(
        chain_spec: Arc<N::ChainSpec>,
        attributes_generator: fn(u64) -> PayloadAttrTy<N>,
    ) -> eyre::Result<(Vec<NodeHelperType<N>>, Wallet)> {
        E2ETestSetupBuilder::new_with_attributes_generator(1, chain_spec, attributes_generator)
            .build()
            .await
    }

    /// The node builder modifier can use the hooks of the node builder for any node.
    #[expect(dead_code)]
    fn node_builder_modifier_can_use_builder_hooks<N: NodeBuilderHelper>(
        setup: E2ETestSetupBuilder<N>,
    ) -> E2ETestSetupBuilder<N> {
        setup.with_node_builder_modifier(|builder| {
            builder
                .on_component_initialized(|_| Ok(()))
                .on_node_started(|_| Ok(()))
                .on_rpc_started(|_, _| Ok(()))
                .extend_rpc_modules(|_| Ok(()))
                .map_add_ons(|add_ons| add_ons)
                .install_exex("exex", |_| async { Ok(async { Ok(()) }) })
        })
    }
}
