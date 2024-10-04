//! Test helpers for `reth-exex`

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

use std::{
    fmt::Debug,
    future::{poll_fn, Future},
    sync::Arc,
    task::Poll,
};

use futures_util::FutureExt;
use reth_blockchain_tree::noop::NoopBlockchainTree;
use reth_chainspec::{ChainSpec, MAINNET};
use reth_consensus::test_utils::TestConsensus;
use reth_db::{
    test_utils::{create_test_rw_db, create_test_static_files_dir, TempDatabase},
    DatabaseEnv,
};
use reth_db_common::init::init_genesis;
use reth_ethereum_engine_primitives::EthereumEngineValidator;
use reth_evm::test_utils::MockExecutorProvider;
use reth_execution_types::Chain;
use reth_exex::{ExExContext, ExExEvent, ExExNotification, ExExNotifications, Wal};
use reth_network::{config::SecretKey, NetworkConfigBuilder, NetworkManager};
use reth_node_api::{
    FullNodeTypes, FullNodeTypesAdapter, NodeTypes, NodeTypesWithDBAdapter, NodeTypesWithEngine,
};
use reth_node_builder::{
    components::{
        Components, ComponentsBuilder, ConsensusBuilder, ExecutorBuilder, NodeComponentsBuilder,
        PoolBuilder,
    },
    BuilderContext, Node, NodeAdapter, RethFullAdapter,
};
use reth_node_core::node_config::NodeConfig;
use reth_node_ethereum::{
    node::{
        EthereumAddOns, EthereumEngineValidatorBuilder, EthereumNetworkBuilder,
        EthereumPayloadBuilder,
    },
    EthEngineTypes, EthEvmConfig,
};
use reth_payload_builder::noop::NoopPayloadBuilderService;
use reth_primitives::{BlockNumHash, Head, SealedBlockWithSenders};
use reth_provider::{
    providers::{BlockchainProvider, StaticFileProvider},
    BlockReader, ProviderFactory,
};
use reth_tasks::TaskManager;
use reth_transaction_pool::test_utils::{testing_pool, TestPool};

use tempfile::TempDir;
use thiserror::Error;
use tokio::sync::mpsc::{Sender, UnboundedReceiver};

/// A test [`PoolBuilder`] that builds a [`TestPool`].
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

/// A test [`ExecutorBuilder`] that builds a [`MockExecutorProvider`].
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct TestExecutorBuilder;

impl<Node> ExecutorBuilder<Node> for TestExecutorBuilder
where
    Node: FullNodeTypes<Types: NodeTypes<ChainSpec = ChainSpec>>,
{
    type EVM = EthEvmConfig;
    type Executor = MockExecutorProvider;

    async fn build_evm(
        self,
        ctx: &BuilderContext<Node>,
    ) -> eyre::Result<(Self::EVM, Self::Executor)> {
        let evm_config = EthEvmConfig::new(ctx.chain_spec());
        let executor = MockExecutorProvider::default();

        Ok((evm_config, executor))
    }
}

/// A test [`ConsensusBuilder`] that builds a [`TestConsensus`].
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct TestConsensusBuilder;

impl<Node> ConsensusBuilder<Node> for TestConsensusBuilder
where
    Node: FullNodeTypes,
{
    type Consensus = Arc<TestConsensus>;

    async fn build_consensus(self, _ctx: &BuilderContext<Node>) -> eyre::Result<Self::Consensus> {
        Ok(Arc::new(TestConsensus::default()))
    }
}

/// A test [`Node`].
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct TestNode;

impl NodeTypes for TestNode {
    type Primitives = ();
    type ChainSpec = ChainSpec;
}

impl NodeTypesWithEngine for TestNode {
    type Engine = EthEngineTypes;
}

impl<N> Node<N> for TestNode
where
    N: FullNodeTypes<Types: NodeTypesWithEngine<Engine = EthEngineTypes, ChainSpec = ChainSpec>>,
{
    type ComponentsBuilder = ComponentsBuilder<
        N,
        TestPoolBuilder,
        EthereumPayloadBuilder,
        EthereumNetworkBuilder,
        TestExecutorBuilder,
        TestConsensusBuilder,
        EthereumEngineValidatorBuilder,
    >;
    type AddOns = EthereumAddOns;

    fn components_builder(&self) -> Self::ComponentsBuilder {
        ComponentsBuilder::default()
            .node_types::<N>()
            .pool(TestPoolBuilder::default())
            .payload(EthereumPayloadBuilder::default())
            .network(EthereumNetworkBuilder::default())
            .executor(TestExecutorBuilder::default())
            .consensus(TestConsensusBuilder::default())
            .engine_validator(EthereumEngineValidatorBuilder::default())
    }

    fn add_ons(&self) -> Self::AddOns {
        EthereumAddOns::default()
    }
}

/// A shared [`TempDatabase`] used for testing
pub type TmpDB = Arc<TempDatabase<DatabaseEnv>>;
/// The [`NodeAdapter`] for the [`TestExExContext`]. Contains type necessary to
/// boot the testing environment
pub type Adapter = NodeAdapter<
    RethFullAdapter<TmpDB, TestNode>,
    <<TestNode as Node<
        FullNodeTypesAdapter<
            NodeTypesWithDBAdapter<TestNode, TmpDB>,
            BlockchainProvider<NodeTypesWithDBAdapter<TestNode, TmpDB>>,
        >,
    >>::ComponentsBuilder as NodeComponentsBuilder<RethFullAdapter<TmpDB, TestNode>>>::Components,
>;
/// An [`ExExContext`] using the [`Adapter`] type.
pub type TestExExContext = ExExContext<Adapter>;

/// A helper type for testing Execution Extensions.
#[derive(Debug)]
pub struct TestExExHandle {
    /// Genesis block that was inserted into the storage
    pub genesis: SealedBlockWithSenders,
    /// Provider Factory for accessing the emphemeral storage of the host node
    pub provider_factory: ProviderFactory<NodeTypesWithDBAdapter<TestNode, TmpDB>>,
    /// Channel for receiving events from the Execution Extension
    pub events_rx: UnboundedReceiver<ExExEvent>,
    /// Channel for sending notifications to the Execution Extension
    pub notifications_tx: Sender<ExExNotification>,
    /// Node task manager
    pub tasks: TaskManager,
    /// WAL temp directory handle
    _wal_directory: TempDir,
}

impl TestExExHandle {
    /// Send a notification to the Execution Extension that the chain has been committed
    pub async fn send_notification_chain_committed(&self, chain: Chain) -> eyre::Result<()> {
        self.notifications_tx
            .send(ExExNotification::ChainCommitted { new: Arc::new(chain) })
            .await?;
        Ok(())
    }

    /// Send a notification to the Execution Extension that the chain has been reorged
    pub async fn send_notification_chain_reorged(
        &self,
        old: Chain,
        new: Chain,
    ) -> eyre::Result<()> {
        self.notifications_tx
            .send(ExExNotification::ChainReorged { old: Arc::new(old), new: Arc::new(new) })
            .await?;
        Ok(())
    }

    /// Send a notification to the Execution Extension that the chain has been reverted
    pub async fn send_notification_chain_reverted(&self, chain: Chain) -> eyre::Result<()> {
        self.notifications_tx
            .send(ExExNotification::ChainReverted { old: Arc::new(chain) })
            .await?;
        Ok(())
    }

    /// Asserts that the Execution Extension did not emit any events.
    #[track_caller]
    pub fn assert_events_empty(&self) {
        assert!(self.events_rx.is_empty());
    }

    /// Asserts that the Execution Extension emitted a `FinishedHeight` event with the correct
    /// height.
    #[track_caller]
    pub fn assert_event_finished_height(&mut self, height: BlockNumHash) -> eyre::Result<()> {
        let event = self.events_rx.try_recv()?;
        assert_eq!(event, ExExEvent::FinishedHeight(height));
        Ok(())
    }
}

/// Creates a new [`ExExContext`].
///
/// This is a convenience function that does the following:
/// 1. Sets up an [`ExExContext`] with all dependencies.
/// 2. Inserts the genesis block of the provided (chain spec)[`ChainSpec`] into the storage.
/// 3. Creates a channel for receiving events from the Execution Extension.
/// 4. Creates a channel for sending notifications to the Execution Extension.
///
/// # Warning
/// The genesis block is not sent to the notifications channel. The caller is responsible for
/// doing this.
pub async fn test_exex_context_with_chain_spec(
    chain_spec: Arc<ChainSpec>,
) -> eyre::Result<(ExExContext<Adapter>, TestExExHandle)> {
    let transaction_pool = testing_pool();
    let evm_config = EthEvmConfig::new(chain_spec.clone());
    let executor = MockExecutorProvider::default();
    let consensus = Arc::new(TestConsensus::default());

    let (static_dir, _) = create_test_static_files_dir();
    let db = create_test_rw_db();
    let provider_factory = ProviderFactory::new(
        db,
        chain_spec.clone(),
        StaticFileProvider::read_write(static_dir.into_path()).expect("static file provider"),
    );

    let genesis_hash = init_genesis(&provider_factory)?;
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

    let engine_validator = EthereumEngineValidator::new(chain_spec.clone());

    let components = NodeAdapter::<FullNodeTypesAdapter<NodeTypesWithDBAdapter<TestNode, _>, _>, _> {
        components: Components {
            transaction_pool,
            evm_config,
            executor,
            consensus,
            network,
            payload_builder,
            engine_validator,
        },
        task_executor,
        provider,
    };

    let genesis = provider_factory
        .block_by_hash(genesis_hash)?
        .ok_or_else(|| eyre::eyre!("genesis block not found"))?
        .seal_slow()
        .seal_with_senders()
        .ok_or_else(|| eyre::eyre!("failed to recover senders"))?;

    let head = Head {
        number: genesis.number,
        hash: genesis_hash,
        difficulty: genesis.difficulty,
        timestamp: genesis.timestamp,
        total_difficulty: Default::default(),
    };

    let wal_directory = tempfile::tempdir()?;
    let wal = Wal::new(wal_directory.path())?;

    let (events_tx, events_rx) = tokio::sync::mpsc::unbounded_channel();
    let (notifications_tx, notifications_rx) = tokio::sync::mpsc::channel(1);
    let notifications = ExExNotifications::new(
        head,
        components.provider.clone(),
        components.components.executor.clone(),
        notifications_rx,
        wal.handle(),
    );

    let ctx = ExExContext {
        head,
        config: NodeConfig::test(),
        reth_config: reth_config::Config::default(),
        events: events_tx,
        notifications,
        components,
    };

    Ok((
        ctx,
        TestExExHandle {
            genesis,
            provider_factory,
            events_rx,
            notifications_tx,
            tasks,
            _wal_directory: wal_directory,
        },
    ))
}

/// Creates a new [`ExExContext`] with (mainnet)[`MAINNET`] chain spec.
///
/// For more information see [`test_exex_context_with_chain_spec`].
pub async fn test_exex_context() -> eyre::Result<(ExExContext<Adapter>, TestExExHandle)> {
    test_exex_context_with_chain_spec(MAINNET.clone()).await
}

/// An extension trait for polling an Execution Extension future.
pub trait PollOnce {
    /// Polls the given Execution Extension future __once__. The future should be
    /// (pinned)[`std::pin::pin`].
    ///
    /// # Returns
    /// - `Ok(())` if the future returned [`Poll::Pending`]. The future can be polled again.
    /// - `Err(PollOnceError::FutureIsReady)` if the future returned [`Poll::Ready`] without an
    ///   error. The future should never resolve.
    /// - `Err(PollOnceError::FutureError(err))` if the future returned [`Poll::Ready`] with an
    ///   error. Something went wrong.
    fn poll_once(&mut self) -> impl Future<Output = Result<(), PollOnceError>> + Send;
}

/// An Execution Extension future polling error.
#[derive(Error, Debug)]
pub enum PollOnceError {
    /// The future returned [`Poll::Ready`] without an error, but it should never resolve.
    #[error("Execution Extension future returned Ready, but it should never resolve")]
    FutureIsReady,
    /// The future returned [`Poll::Ready`] with an error.
    #[error(transparent)]
    FutureError(#[from] eyre::Error),
}

impl<F: Future<Output = eyre::Result<()>> + Unpin + Send> PollOnce for F {
    async fn poll_once(&mut self) -> Result<(), PollOnceError> {
        poll_fn(|cx| match self.poll_unpin(cx) {
            Poll::Ready(Ok(())) => Poll::Ready(Err(PollOnceError::FutureIsReady)),
            Poll::Ready(Err(err)) => Poll::Ready(Err(PollOnceError::FutureError(err))),
            Poll::Pending => Poll::Ready(Ok(())),
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn check_test_context_creation() {
        let _ = test_exex_context().await.unwrap();
    }
}
