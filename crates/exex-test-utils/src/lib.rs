//! Test helpers for `reth-exex`

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

use futures_util::FutureExt;
use reth_blockchain_tree::noop::NoopBlockchainTree;
use reth_db::{test_utils::TempDatabase, DatabaseEnv};
use reth_db_common::init::init_genesis;
use reth_evm::test_utils::MockExecutorProvider;
use reth_exex::{ExExContext, ExExEvent, ExExNotification};
use reth_network::{config::SecretKey, NetworkConfigBuilder, NetworkManager};
use reth_node_api::{FullNodeTypes, FullNodeTypesAdapter, NodeTypes};
use reth_node_builder::{
    components::{
        Components, ComponentsBuilder, ExecutorBuilder, NodeComponentsBuilder, PoolBuilder,
    },
    BuilderContext, Node, NodeAdapter, RethFullAdapter,
};
use reth_node_core::node_config::NodeConfig;
use reth_node_ethereum::{
    node::{EthereumNetworkBuilder, EthereumPayloadBuilder},
    EthEngineTypes, EthEvmConfig,
};
use reth_payload_builder::noop::NoopPayloadBuilderService;
use reth_primitives::{Head, SealedBlockWithSenders};
use reth_provider::{
    providers::BlockchainProvider, test_utils::create_test_provider_factory, BlockReader,
    ProviderFactory,
};
use reth_tasks::TaskManager;
use reth_transaction_pool::test_utils::{testing_pool, TestPool};
use std::{
    future::{poll_fn, Future},
    sync::Arc,
    task::Poll,
};
use tokio::sync::mpsc::{Sender, UnboundedReceiver};

#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct TestPoolBuilder;

impl<Node> PoolBuilder<Node> for TestPoolBuilder
where
    Node: FullNodeTypes,
{
    type Pool = TestPool;

    async fn build_pool(self, _ctx: &BuilderContext<Node>) -> eyre::Result<Self::Pool> {
        Ok(testing_pool())
    }
}

#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct TestExecutorBuilder;

impl<Node> ExecutorBuilder<Node> for TestExecutorBuilder
where
    Node: FullNodeTypes,
{
    type EVM = EthEvmConfig;
    type Executor = MockExecutorProvider;

    async fn build_evm(
        self,
        _ctx: &BuilderContext<Node>,
    ) -> eyre::Result<(Self::EVM, Self::Executor)> {
        let evm_config = EthEvmConfig::default();
        let executor = MockExecutorProvider::default();

        Ok((evm_config, executor))
    }
}

#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct ExExNode;

impl ExExNode {
    fn components<Node>() -> ComponentsBuilder<
        Node,
        TestPoolBuilder,
        EthereumPayloadBuilder,
        EthereumNetworkBuilder,
        TestExecutorBuilder,
    >
    where
        Node: FullNodeTypes<Engine = EthEngineTypes>,
    {
        ComponentsBuilder::default()
            .node_types::<Node>()
            .pool(TestPoolBuilder::default())
            .payload(EthereumPayloadBuilder::default())
            .network(EthereumNetworkBuilder::default())
            .executor(TestExecutorBuilder::default())
    }
}

impl NodeTypes for ExExNode {
    type Primitives = ();
    type Engine = EthEngineTypes;
}

impl<N> Node<N> for ExExNode
where
    N: FullNodeTypes<Engine = EthEngineTypes>,
{
    type ComponentsBuilder = ComponentsBuilder<
        N,
        TestPoolBuilder,
        EthereumPayloadBuilder,
        EthereumNetworkBuilder,
        TestExecutorBuilder,
    >;

    fn components_builder(self) -> Self::ComponentsBuilder {
        Self::components()
    }
}

type TmpDB = Arc<TempDatabase<DatabaseEnv>>;
type Adapter = NodeAdapter<
    RethFullAdapter<TmpDB, ExExNode>,
    <<ExExNode as Node<FullNodeTypesAdapter<ExExNode, TmpDB, BlockchainProvider<TmpDB>>>>::ComponentsBuilder as NodeComponentsBuilder<
        RethFullAdapter<TmpDB, ExExNode>,
    >>::Components,
>;

pub struct TestExExContext {
    pub ctx: ExExContext<Adapter>,
    pub genesis: SealedBlockWithSenders,
    pub provider_factory: ProviderFactory<TmpDB>,
    pub events_rx: UnboundedReceiver<ExExEvent>,
    pub notifications_tx: Sender<ExExNotification>,
}

pub async fn test_exex_context() -> eyre::Result<TestExExContext> {
    let transaction_pool = testing_pool();
    let evm_config = EthEvmConfig::default();
    let executor = MockExecutorProvider::default();

    let provider_factory = create_test_provider_factory();
    let genesis_hash = init_genesis(provider_factory.clone())?;
    let provider =
        BlockchainProvider::new(provider_factory.clone(), Arc::new(NoopBlockchainTree::default()))?;

    let network_manager = NetworkManager::new(
        NetworkConfigBuilder::new(SecretKey::new(&mut rand::thread_rng()))
            .build(provider_factory.clone()),
    )
    .await?;
    let network = network_manager.handle().clone();

    let (_, payload_builder) = NoopPayloadBuilderService::<EthEngineTypes>::new();

    let tasks = TaskManager::current();
    let task_executor = tasks.executor();

    let components = NodeAdapter::<FullNodeTypesAdapter<ExExNode, _, _>, _> {
        components: Components { transaction_pool, evm_config, executor, network, payload_builder },
        task_executor,
        provider,
    };

    let genesis = provider_factory
        .block_by_hash(genesis_hash)?
        .ok_or(eyre::eyre!("genesis block not found"))?
        .seal_slow()
        .seal_with_senders()
        .ok_or(eyre::eyre!("failed to recover senders"))?;

    let head = Head {
        number: genesis.number,
        hash: genesis_hash,
        difficulty: genesis.difficulty,
        timestamp: genesis.timestamp,
        total_difficulty: Default::default(),
    };

    let (events_tx, events_rx) = tokio::sync::mpsc::unbounded_channel();
    let (notifications_tx, notifications_rx) = tokio::sync::mpsc::channel(1);

    let ctx = ExExContext {
        head,
        config: NodeConfig::test(),
        reth_config: reth_config::Config::default(),
        events: events_tx,
        notifications: notifications_rx,
        components,
    };

    Ok(TestExExContext { ctx, genesis, provider_factory, events_rx, notifications_tx })
}

/// An extension trait for polling an Execution Extension future.
pub trait PollOnce {
    /// Polls the given Execution Extension future __once__ and asserts that it is
    /// [`Poll::Pending`]. The future should be (pinned)[`std::pin::pin`].
    ///
    /// # Panics
    /// If the future returns [`Poll::Ready`], because Execution Extension future should never
    /// resolve.
    fn poll_once(&mut self) -> impl Future<Output = ()> + Send;
}

impl<F: Future<Output = eyre::Result<()>> + Unpin + Send> PollOnce for F {
    async fn poll_once(&mut self) {
        poll_fn(|cx| {
            assert!(self.poll_unpin(cx).is_pending());
            Poll::Ready(())
        })
        .await;
    }
}
