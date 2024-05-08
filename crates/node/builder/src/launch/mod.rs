//! Abstraction for launching a node.

use crate::{
    builder::{NodeAdapter, NodeAddOns, NodeTypesAdapter},
    components::{NodeComponents, NodeComponentsBuilder},
    hooks::NodeHooks,
    node::FullNode,
    BuilderContext, NodeBuilderWithComponents, NodeHandle,
};
use futures::{future, future::Either, stream, stream_select, StreamExt};
use reth_auto_seal_consensus::AutoSealConsensus;
use reth_beacon_consensus::{
    hooks::{EngineHooks, PruneHook, StaticFileHook},
    BeaconConsensusEngine, EthBeaconConsensus,
};
use reth_blockchain_tree::{
    noop::NoopBlockchainTree, BlockchainTree, BlockchainTreeConfig, ShareableBlockchainTree,
    TreeExternals,
};
use reth_consensus::Consensus;
use reth_exex::{ExExContext, ExExHandle, ExExManager, ExExManagerHandle};
use reth_interfaces::p2p::either::EitherDownloader;
use reth_network::NetworkEvents;
use reth_node_api::{FullNodeComponents, FullNodeTypes};
use reth_node_core::{
    dirs::{ChainPath, DataDirPath},
    engine::EngineMessageStreamExt,
    exit::NodeExitFuture,
};
use reth_node_events::{cl::ConsensusLayerHealthEvents, node};
use reth_primitives::format_ether;
use reth_provider::{providers::BlockchainProvider, CanonStateSubscriptions};
use reth_rpc_engine_api::EngineApi;
use reth_tasks::TaskExecutor;
use reth_tracing::tracing::{debug, info};
use reth_transaction_pool::TransactionPool;
use std::{future::Future, sync::Arc};
use tokio::sync::{mpsc::unbounded_channel, oneshot};
use tokio_stream::wrappers::UnboundedReceiverStream;

pub mod common;
pub use common::LaunchContext;

/// A general purpose trait that launches a new node of any kind.
///
/// Acts as a node factory.
///
/// This is essentially the launch logic for a node.
///
/// See also [DefaultNodeLauncher] and [NodeBuilderWithComponents::launch_with]
pub trait LaunchNode<Target> {
    /// The node type that is created.
    type Node;

    /// Create and return a new node asynchronously.
    fn launch_node(self, target: Target) -> impl Future<Output = eyre::Result<Self::Node>> + Send;
}

/// The default launcher for a node.
#[derive(Debug)]
pub struct DefaultNodeLauncher {
    /// The task executor for the node.
    pub ctx: LaunchContext,
}

impl DefaultNodeLauncher {
    /// Create a new instance of the default node launcher.
    pub fn new(task_executor: TaskExecutor, data_dir: ChainPath<DataDirPath>) -> Self {
        Self { ctx: LaunchContext::new(task_executor, data_dir) }
    }
}

impl<T, CB> LaunchNode<NodeBuilderWithComponents<T, CB>> for DefaultNodeLauncher
where
    T: FullNodeTypes<Provider = BlockchainProvider<<T as FullNodeTypes>::DB>>,
    CB: NodeComponentsBuilder<T>,
{
    type Node = NodeHandle<NodeAdapter<T, CB::Components>>;

    async fn launch_node(
        self,
        target: NodeBuilderWithComponents<T, CB>,
    ) -> eyre::Result<Self::Node> {
        let Self { ctx } = self;
        let NodeBuilderWithComponents {
            adapter: NodeTypesAdapter { database },
            components_builder,
            add_ons: NodeAddOns { hooks, rpc, exexs: installed_exex },
            config,
        } = target;

        // setup the launch context
        let ctx = ctx
            .with_configured_globals()
            // load the toml config
            .with_loaded_toml_config(config)?
            // attach the database
            .attach(database.clone())
            // ensure certain settings take effect
            .with_adjusted_configs()
            // Create the provider factory
            .with_provider_factory()?
            .inspect(|_| {
                info!(target: "reth::cli", "Database opened");
            })
            .with_prometheus().await?
            .inspect(|this| {
                debug!(target: "reth::cli", chain=%this.chain_id(), genesis=?this.genesis_hash(), "Initializing genesis");
            })
            .with_genesis()?
            .inspect(|this| {
                info!(target: "reth::cli", "\n{}", this.chain_spec().display_hardforks());
            });

        // setup the consensus instance
        let consensus: Arc<dyn Consensus> = if ctx.is_dev() {
            Arc::new(AutoSealConsensus::new(ctx.chain_spec()))
        } else {
            Arc::new(EthBeaconConsensus::new(ctx.chain_spec()))
        };

        debug!(target: "reth::cli", "Spawning stages metrics listener task");
        let (sync_metrics_tx, sync_metrics_rx) = unbounded_channel();
        let sync_metrics_listener = reth_stages::MetricsListener::new(sync_metrics_rx);
        ctx.task_executor().spawn_critical("stages metrics listener task", sync_metrics_listener);

        // fetch the head block from the database
        let head = ctx.lookup_head()?;

        // Configure the blockchain tree for the node
        let tree_config = BlockchainTreeConfig::default();

        // NOTE: This is a temporary workaround to provide the canon state notification sender to the components builder because there's a cyclic dependency between the blockchain provider and the tree component. This will be removed once the Blockchain provider no longer depends on an instance of the tree: <https://github.com/paradigmxyz/reth/issues/7154>
        let (canon_state_notification_sender, _receiver) =
            tokio::sync::broadcast::channel(tree_config.max_reorg_depth() as usize * 2);

        let blockchain_db = BlockchainProvider::new(
            ctx.provider_factory().clone(),
            Arc::new(NoopBlockchainTree::with_canon_state_notifications(
                canon_state_notification_sender.clone(),
            )),
        )?;

        let builder_ctx = BuilderContext::new(
            head,
            blockchain_db.clone(),
            ctx.task_executor().clone(),
            ctx.data_dir().clone(),
            ctx.node_config().clone(),
            ctx.toml_config().clone(),
        );

        debug!(target: "reth::cli", "creating components");
        let components = components_builder.build_components(&builder_ctx).await?;

        let tree_externals = TreeExternals::new(
            ctx.provider_factory().clone(),
            consensus.clone(),
            components.block_executor().clone(),
        );
        let tree = BlockchainTree::new(tree_externals, tree_config, ctx.prune_modes())?
            .with_sync_metrics_tx(sync_metrics_tx.clone())
            // Note: This is required because we need to ensure that both the components and the
            // tree are using the same channel for canon state notifications. This will be removed
            // once the Blockchain provider no longer depends on an instance of the tree
            .with_canon_state_notification_sender(canon_state_notification_sender);

        let canon_state_notification_sender = tree.canon_state_notification_sender();
        let blockchain_tree = Arc::new(ShareableBlockchainTree::new(tree));

        // Replace the tree component with the actual tree
        let blockchain_db = blockchain_db.with_tree(blockchain_tree);

        debug!(target: "reth::cli", "configured blockchain tree");

        let NodeHooks { on_component_initialized, on_node_started, .. } = hooks;

        let node_adapter = NodeAdapter {
            components,
            task_executor: ctx.task_executor().clone(),
            provider: blockchain_db.clone(),
        };

        debug!(target: "reth::cli", "calling on_component_initialized hook");
        on_component_initialized.on_event(node_adapter.clone())?;

        // spawn exexs
        let mut exex_handles = Vec::with_capacity(installed_exex.len());
        let mut exexs = Vec::with_capacity(installed_exex.len());
        for (id, exex) in installed_exex {
            // create a new exex handle
            let (handle, events, notifications) = ExExHandle::new(id.clone());
            exex_handles.push(handle);

            // create the launch context for the exex
            let context = ExExContext {
                head,
                data_dir: ctx.data_dir().clone(),
                config: ctx.node_config().clone(),
                reth_config: ctx.toml_config().clone(),
                components: node_adapter.clone(),
                events,
                notifications,
            };

            let executor = ctx.task_executor().clone();
            exexs.push(async move {
                debug!(target: "reth::cli", id, "spawning exex");
                let span = reth_tracing::tracing::info_span!("exex", id);
                let _enter = span.enter();

                // init the exex
                let exex = exex.launch(context).await.unwrap();

                // spawn it as a crit task
                executor.spawn_critical("exex", async move {
                    info!(target: "reth::cli", "ExEx started");
                    match exex.await {
                        Ok(_) => panic!("ExEx {id} finished. ExEx's should run indefinitely"),
                        Err(err) => panic!("ExEx {id} crashed: {err}"),
                    }
                });
            });
        }

        future::join_all(exexs).await;

        // spawn exex manager
        let exex_manager_handle = if !exex_handles.is_empty() {
            debug!(target: "reth::cli", "spawning exex manager");
            // todo(onbjerg): rm magic number
            let exex_manager = ExExManager::new(exex_handles, 1024);
            let exex_manager_handle = exex_manager.handle();
            ctx.task_executor().spawn_critical("exex manager", async move {
                exex_manager.await.expect("exex manager crashed");
            });

            // send notifications from the blockchain tree to exex manager
            let mut canon_state_notifications = blockchain_db.subscribe_to_canonical_state();
            let mut handle = exex_manager_handle.clone();
            ctx.task_executor().spawn_critical(
                "exex manager blockchain tree notifications",
                async move {
                    while let Ok(notification) = canon_state_notifications.recv().await {
                        handle.send_async(notification.into()).await.expect(
                            "blockchain tree notification could not be sent to exex manager",
                        );
                    }
                },
            );

            info!(target: "reth::cli", "ExEx Manager started");

            Some(exex_manager_handle)
        } else {
            None
        };

        // create pipeline
        let network_client = node_adapter.network().fetch_client().await?;
        let (consensus_engine_tx, consensus_engine_rx) = unbounded_channel();

        let node_config = ctx.node_config();
        let consensus_engine_stream = UnboundedReceiverStream::from(consensus_engine_rx)
            .maybe_skip_fcu(node_config.debug.skip_fcu)
            .maybe_skip_new_payload(node_config.debug.skip_new_payload)
            // Store messages _after_ skipping so that `replay-engine` command
            // would replay only the messages that were observed by the engine
            // during this run.
            .maybe_store_messages(node_config.debug.engine_api_store.clone());

        let max_block = ctx.max_block(network_client.clone()).await?;
        let mut hooks = EngineHooks::new();

        let static_file_producer = ctx.static_file_producer();
        let static_file_producer_events = static_file_producer.lock().events();
        hooks.add(StaticFileHook::new(
            static_file_producer.clone(),
            Box::new(ctx.task_executor().clone()),
        ));
        info!(target: "reth::cli", "StaticFileProducer initialized");

        // Configure the pipeline
        let pipeline_exex_handle =
            exex_manager_handle.clone().unwrap_or_else(ExExManagerHandle::empty);
        let (mut pipeline, client) = if ctx.is_dev() {
            info!(target: "reth::cli", "Starting Reth in dev mode");

            for (idx, (address, alloc)) in ctx.chain_spec().genesis.alloc.iter().enumerate() {
                info!(target: "reth::cli", "Allocated Genesis Account: {:02}. {} ({} ETH)", idx, address.to_string(), format_ether(alloc.balance));
            }

            // install auto-seal
            let mining_mode =
                ctx.dev_mining_mode(node_adapter.components.pool().pending_transactions_listener());
            info!(target: "reth::cli", mode=%mining_mode, "configuring dev mining mode");

            let (_, client, mut task) = reth_auto_seal_consensus::AutoSealBuilder::new(
                ctx.chain_spec(),
                blockchain_db.clone(),
                node_adapter.components.pool().clone(),
                consensus_engine_tx.clone(),
                canon_state_notification_sender,
                mining_mode,
                node_adapter.components.block_executor().clone(),
            )
            .build();

            let mut pipeline = crate::setup::build_networked_pipeline(
                ctx.node_config(),
                &ctx.toml_config().stages,
                client.clone(),
                Arc::clone(&consensus),
                ctx.provider_factory().clone(),
                ctx.task_executor(),
                sync_metrics_tx,
                ctx.prune_config(),
                max_block,
                static_file_producer,
                node_adapter.components.block_executor().clone(),
                pipeline_exex_handle,
            )
            .await?;

            let pipeline_events = pipeline.events();
            task.set_pipeline_events(pipeline_events);
            debug!(target: "reth::cli", "Spawning auto mine task");
            ctx.task_executor().spawn(Box::pin(task));

            (pipeline, EitherDownloader::Left(client))
        } else {
            let pipeline = crate::setup::build_networked_pipeline(
                ctx.node_config(),
                &ctx.toml_config().stages,
                network_client.clone(),
                Arc::clone(&consensus),
                ctx.provider_factory().clone(),
                ctx.task_executor(),
                sync_metrics_tx,
                ctx.prune_config(),
                max_block,
                static_file_producer,
                node_adapter.components.block_executor().clone(),
                pipeline_exex_handle,
            )
            .await?;

            (pipeline, EitherDownloader::Right(network_client.clone()))
        };

        let pipeline_events = pipeline.events();

        let initial_target = ctx.initial_pipeline_target();

        let mut pruner_builder =
            ctx.pruner_builder().max_reorg_depth(tree_config.max_reorg_depth() as usize);
        if let Some(exex_manager_handle) = &exex_manager_handle {
            pruner_builder =
                pruner_builder.finished_exex_height(exex_manager_handle.finished_height());
        }

        let mut pruner = pruner_builder.build(ctx.provider_factory().clone());

        let pruner_events = pruner.events();
        info!(target: "reth::cli", prune_config=?ctx.prune_config().unwrap_or_default(), "Pruner initialized");
        hooks.add(PruneHook::new(pruner, Box::new(ctx.task_executor().clone())));

        // Configure the consensus engine
        let (beacon_consensus_engine, beacon_engine_handle) = BeaconConsensusEngine::with_channel(
            client,
            pipeline,
            blockchain_db.clone(),
            Box::new(ctx.task_executor().clone()),
            Box::new(node_adapter.components.network().clone()),
            max_block,
            ctx.node_config().debug.continuous,
            node_adapter.components.payload_builder().clone(),
            initial_target,
            reth_beacon_consensus::MIN_BLOCKS_FOR_PIPELINE_RUN,
            consensus_engine_tx,
            Box::pin(consensus_engine_stream),
            hooks,
        )?;
        info!(target: "reth::cli", "Consensus engine initialized");

        let events = stream_select!(
            node_adapter.components.network().event_listener().map(Into::into),
            beacon_engine_handle.event_listener().map(Into::into),
            pipeline_events.map(Into::into),
            if ctx.node_config().debug.tip.is_none() && !ctx.is_dev() {
                Either::Left(
                    ConsensusLayerHealthEvents::new(Box::new(blockchain_db.clone()))
                        .map(Into::into),
                )
            } else {
                Either::Right(stream::empty())
            },
            pruner_events.map(Into::into),
            static_file_producer_events.map(Into::into)
        );
        ctx.task_executor().spawn_critical(
            "events task",
            node::handle_events(
                Some(node_adapter.components.network().clone()),
                Some(head.number),
                events,
                database.clone(),
            ),
        );

        let engine_api = EngineApi::new(
            blockchain_db.clone(),
            ctx.chain_spec(),
            beacon_engine_handle,
            node_adapter.components.payload_builder().clone().into(),
            Box::new(ctx.task_executor().clone()),
        );
        info!(target: "reth::cli", "Engine API handler initialized");

        // extract the jwt secret from the args if possible
        let jwt_secret = ctx.auth_jwt_secret()?;

        // Start RPC servers
        let (rpc_server_handles, mut rpc_registry) = crate::rpc::launch_rpc_servers(
            node_adapter.clone(),
            engine_api,
            ctx.node_config(),
            jwt_secret,
            rpc,
        )
        .await?;

        // in dev mode we generate 20 random dev-signer accounts
        if ctx.is_dev() {
            rpc_registry.eth_api().with_dev_accounts();
        }

        // Run consensus engine to completion
        let (tx, rx) = oneshot::channel();
        info!(target: "reth::cli", "Starting consensus engine");
        ctx.task_executor().spawn_critical_blocking("consensus engine", async move {
            let res = beacon_consensus_engine.await;
            let _ = tx.send(res);
        });

        let full_node = FullNode {
            evm_config: node_adapter.components.evm_config().clone(),
            pool: node_adapter.components.pool().clone(),
            network: node_adapter.components.network().clone(),
            provider: node_adapter.provider.clone(),
            payload_builder: node_adapter.components.payload_builder().clone(),
            task_executor: ctx.task_executor().clone(),
            rpc_server_handles,
            rpc_registry,
            config: ctx.node_config().clone(),
            data_dir: ctx.data_dir().clone(),
        };
        // Notify on node started
        on_node_started.on_event(full_node.clone())?;

        let handle = NodeHandle {
            node_exit_future: NodeExitFuture::new(rx, full_node.config.debug.terminate),
            node: full_node,
        };

        Ok(handle)
    }
}
