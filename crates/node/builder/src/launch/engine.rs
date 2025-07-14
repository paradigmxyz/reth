//! Engine node related functionality.

use crate::{
    common::{Attached, LaunchContextWith, WithConfigs},
    hooks::NodeHooks,
    rpc::{EngineValidatorAddOn, RethRpcAddOns, RpcHandle},
    setup::build_networked_pipeline,
    AddOns, AddOnsContext, FullNode, LaunchContext, LaunchNode, NodeAdapter,
    NodeBuilderWithComponents, NodeComponents, NodeComponentsBuilder, NodeHandle, NodeTypesAdapter,
};
use alloy_consensus::BlockHeader;
use futures::{stream_select, StreamExt};
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_db_api::{database_metrics::DatabaseMetrics, Database};
use reth_engine_service::service::{ChainEvent, EngineService};
use reth_engine_tree::{
    engine::{EngineApiRequest, EngineRequestHandler},
    tree::TreeConfig,
};
use reth_engine_util::EngineMessageStreamExt;
use reth_exex::ExExManagerHandle;
use reth_network::{types::BlockRangeUpdate, NetworkSyncUpdater, SyncState};
use reth_network_api::BlockDownloaderProvider;
use reth_node_api::{
    BeaconConsensusEngineHandle, BuiltPayload, FullNodeTypes, NodeTypes, NodeTypesWithDBAdapter,
};
use reth_node_core::{
    dirs::{ChainPath, DataDirPath},
    exit::NodeExitFuture,
    primitives::Head,
};
use reth_node_events::node;
use reth_provider::{
    providers::{BlockchainProvider, NodeTypesForProvider},
    BlockNumReader,
};
use reth_tasks::TaskExecutor;
use reth_tokio_util::EventSender;
use reth_tracing::tracing::{debug, error, info};
use std::sync::Arc;
use tokio::sync::{mpsc::unbounded_channel, oneshot};
use tokio_stream::wrappers::UnboundedReceiverStream;

/// The engine node launcher.
#[derive(Debug)]
pub struct EngineNodeLauncher {
    /// The task executor for the node.
    pub ctx: LaunchContext,

    /// Temporary configuration for engine tree.
    /// After engine is stabilized, this should be configured through node builder.
    pub engine_tree_config: TreeConfig,
}

impl EngineNodeLauncher {
    /// Create a new instance of the ethereum node launcher.
    pub const fn new(
        task_executor: TaskExecutor,
        data_dir: ChainPath<DataDirPath>,
        engine_tree_config: TreeConfig,
    ) -> Self {
        Self { ctx: LaunchContext::new(task_executor, data_dir), engine_tree_config }
    }
}

impl<Types, DB, T, CB, AO> LaunchNode<NodeBuilderWithComponents<T, CB, AO>> for EngineNodeLauncher
where
    Types: NodeTypesForProvider + NodeTypes,
    DB: Database + DatabaseMetrics + Clone + Unpin + 'static,
    T: FullNodeTypes<
        Types = Types,
        DB = DB,
        Provider = BlockchainProvider<NodeTypesWithDBAdapter<Types, DB>>,
    >,
    CB: NodeComponentsBuilder<T>,
    AO: RethRpcAddOns<NodeAdapter<T, CB::Components>>
        + EngineValidatorAddOn<NodeAdapter<T, CB::Components>>,
{
    type Node = NodeHandle<NodeAdapter<T, CB::Components>, AO>;

    async fn launch_node(
        self,
        target: NodeBuilderWithComponents<T, CB, AO>,
    ) -> eyre::Result<Self::Node> {
        let Self { ctx, engine_tree_config } = self;
        let NodeBuilderWithComponents {
            adapter: NodeTypesAdapter { database },
            components_builder,
            add_ons: AddOns { hooks, exexs: installed_exex, add_ons },
            config,
        } = target;
        let NodeHooks { on_component_initialized, on_node_started, .. } = hooks;

        // setup the launch context
        let ctx = ctx
            .with_configured_globals(engine_tree_config.reserved_cpu_cores())
            // load the toml config
            .with_loaded_toml_config(config)?
            // add resolved peers
            .with_resolved_peers()?
            // attach the database
            .attach(database.clone())
            // ensure certain settings take effect
            .with_adjusted_configs()
            // Create the provider factory
            .with_provider_factory::<_, <CB::Components as NodeComponents<T>>::Evm>().await?
            .inspect(|_| {
                info!(target: "reth::cli", "Database opened");
            })
            .with_prometheus_server().await?
            .inspect(|this| {
                debug!(target: "reth::cli", chain=%this.chain_id(), genesis=?this.genesis_hash(), "Initializing genesis");
            })
            .with_genesis()?
            .inspect(|this: &LaunchContextWith<Attached<WithConfigs<Types::ChainSpec>, _>>| {
                info!(target: "reth::cli", "\n{}", this.chain_spec().display_hardforks());
            })
            .with_metrics_task()
            // passing FullNodeTypes as type parameter here so that we can build
            // later the components.
            .with_blockchain_db::<T, _>(move |provider_factory| {
                Ok(BlockchainProvider::new(provider_factory)?)
            })?
            .with_components(components_builder, on_component_initialized).await?;

        // Try to expire pre-merge transaction history if configured
        ctx.expire_pre_merge_transactions()?;

        // spawn exexs if any
        let maybe_exex_manager_handle = ctx.launch_exex(installed_exex).await?;

        // create pipeline
        let network_handle = ctx.components().network().clone();
        let network_client = network_handle.fetch_client().await?;
        let (consensus_engine_tx, consensus_engine_rx) = unbounded_channel();

        let node_config = ctx.node_config();

        // We always assume that node is syncing after a restart
        network_handle.update_sync_state(SyncState::Syncing);

        let max_block = ctx.max_block(network_client.clone()).await?;

        let static_file_producer = ctx.static_file_producer();
        let static_file_producer_events = static_file_producer.lock().events();
        info!(target: "reth::cli", "StaticFileProducer initialized");

        let consensus = Arc::new(ctx.components().consensus().clone());

        let pipeline = build_networked_pipeline(
            &ctx.toml_config().stages,
            network_client.clone(),
            consensus.clone(),
            ctx.provider_factory().clone(),
            ctx.task_executor(),
            ctx.sync_metrics_tx(),
            ctx.prune_config(),
            max_block,
            static_file_producer,
            ctx.components().evm_config().clone(),
            maybe_exex_manager_handle.clone().unwrap_or_else(ExExManagerHandle::empty),
            ctx.era_import_source(),
        )?;

        // The new engine writes directly to static files. This ensures that they're up to the tip.
        pipeline.move_to_static_files()?;

        let pipeline_events = pipeline.events();

        let mut pruner_builder = ctx.pruner_builder();
        if let Some(exex_manager_handle) = &maybe_exex_manager_handle {
            pruner_builder =
                pruner_builder.finished_exex_height(exex_manager_handle.finished_height());
        }
        let pruner = pruner_builder.build_with_provider_factory(ctx.provider_factory().clone());
        let pruner_events = pruner.events();
        info!(target: "reth::cli", prune_config=?ctx.prune_config().unwrap_or_default(), "Pruner initialized");

        let event_sender = EventSender::default();

        let beacon_engine_handle = BeaconConsensusEngineHandle::new(consensus_engine_tx.clone());

        // extract the jwt secret from the args if possible
        let jwt_secret = ctx.auth_jwt_secret()?;

        let add_ons_ctx = AddOnsContext {
            node: ctx.node_adapter().clone(),
            config: ctx.node_config(),
            beacon_engine_handle: beacon_engine_handle.clone(),
            jwt_secret,
            engine_events: event_sender.clone(),
        };
        let engine_payload_validator = add_ons.engine_validator(&add_ons_ctx).await?;

        let consensus_engine_stream = UnboundedReceiverStream::from(consensus_engine_rx)
            .maybe_skip_fcu(node_config.debug.skip_fcu)
            .maybe_skip_new_payload(node_config.debug.skip_new_payload)
            .maybe_reorg(
                ctx.blockchain_db().clone(),
                ctx.components().evm_config().clone(),
                engine_payload_validator.clone(),
                node_config.debug.reorg_frequency,
                node_config.debug.reorg_depth,
            )
            // Store messages _after_ skipping so that `replay-engine` command
            // would replay only the messages that were observed by the engine
            // during this run.
            .maybe_store_messages(node_config.debug.engine_api_store.clone());

        let mut engine_service = EngineService::new(
            consensus.clone(),
            ctx.chain_spec(),
            network_client.clone(),
            Box::pin(consensus_engine_stream),
            pipeline,
            Box::new(ctx.task_executor().clone()),
            ctx.provider_factory().clone(),
            ctx.blockchain_db().clone(),
            pruner,
            ctx.components().payload_builder_handle().clone(),
            engine_payload_validator,
            engine_tree_config,
            ctx.invalid_block_hook().await?,
            ctx.sync_metrics_tx(),
            ctx.components().evm_config().clone(),
        );

        info!(target: "reth::cli", "Consensus engine initialized");

        let events = stream_select!(
            event_sender.new_listener().map(Into::into),
            pipeline_events.map(Into::into),
            ctx.consensus_layer_events(),
            pruner_events.map(Into::into),
            static_file_producer_events.map(Into::into),
        );

        ctx.task_executor().spawn_critical(
            "events task",
            node::handle_events(
                Some(Box::new(ctx.components().network().clone())),
                Some(ctx.head().number),
                events,
            ),
        );

        let RpcHandle { rpc_server_handles, rpc_registry, engine_events, beacon_engine_handle } =
            add_ons.launch_add_ons(add_ons_ctx).await?;

        // Run consensus engine to completion
        let initial_target = ctx.initial_backfill_target()?;
        let mut built_payloads = ctx
            .components()
            .payload_builder_handle()
            .subscribe()
            .await
            .map_err(|e| eyre::eyre!("Failed to subscribe to payload builder events: {:?}", e))?
            .into_built_payload_stream()
            .fuse();

        let chainspec = ctx.chain_spec();
        let provider = ctx.blockchain_db().clone();
        let (exit, rx) = oneshot::channel();
        let terminate_after_backfill = ctx.terminate_after_initial_backfill();

        info!(target: "reth::cli", "Starting consensus engine");
        ctx.task_executor().spawn_critical("consensus engine", async move {
            if let Some(initial_target) = initial_target {
                debug!(target: "reth::cli", %initial_target,  "start backfill sync");
                engine_service.orchestrator_mut().start_backfill_sync(initial_target);
            }

            let mut res = Ok(());

            // advance the chain and await payloads built locally to add into the engine api tree handler to prevent re-execution if that block is received as payload from the CL
            loop {
                tokio::select! {
                    payload = built_payloads.select_next_some() => {
                        if let Some(executed_block) = payload.executed_block() {
                            debug!(target: "reth::cli", block=?executed_block.recovered_block().num_hash(),  "inserting built payload");
                            engine_service.orchestrator_mut().handler_mut().handler_mut().on_event(EngineApiRequest::InsertExecutedBlock(executed_block).into());
                        }
                    }
                    event = engine_service.next() => {
                        let Some(event) = event else { break };
                        debug!(target: "reth::cli", "Event: {event}");
                        match event {
                            ChainEvent::BackfillSyncFinished => {
                                if terminate_after_backfill {
                                    debug!(target: "reth::cli", "Terminating after initial backfill");
                                    break
                                }
                            }
                            ChainEvent::BackfillSyncStarted => {
                                network_handle.update_sync_state(SyncState::Syncing);
                            }
                            ChainEvent::FatalError => {
                                error!(target: "reth::cli", "Fatal error in consensus engine");
                                res = Err(eyre::eyre!("Fatal error in consensus engine"));
                                break
                            }
                            ChainEvent::Handler(ev) => {
                                if let Some(head) = ev.canonical_header() {
                                    // Once we're progressing via live sync, we can consider the node is not syncing anymore
                                    network_handle.update_sync_state(SyncState::Idle);
                                                                        let head_block = Head {
                                        number: head.number(),
                                        hash: head.hash(),
                                        difficulty: head.difficulty(),
                                        timestamp: head.timestamp(),
                                        total_difficulty: chainspec.final_paris_total_difficulty().filter(|_| chainspec.is_paris_active_at_block(head.number())).unwrap_or_default(),
                                    };
                                    network_handle.update_status(head_block);

                                    let updated = BlockRangeUpdate {
                                        earliest: provider.earliest_block_number().unwrap_or_default(),
                                        latest:head.number(),
                                        latest_hash:head.hash()
                                    };
                                    network_handle.update_block_range(updated);
                                }
                                event_sender.notify(ev);
                            }
                        }
                    }
                }
            }

            let _ = exit.send(res);
        });

        let full_node = FullNode {
            evm_config: ctx.components().evm_config().clone(),
            pool: ctx.components().pool().clone(),
            network: ctx.components().network().clone(),
            provider: ctx.node_adapter().provider.clone(),
            payload_builder_handle: ctx.components().payload_builder_handle().clone(),
            task_executor: ctx.task_executor().clone(),
            config: ctx.node_config().clone(),
            data_dir: ctx.data_dir().clone(),
            add_ons_handle: RpcHandle {
                rpc_server_handles,
                rpc_registry,
                engine_events,
                beacon_engine_handle,
            },
        };
        // Notify on node started
        on_node_started.on_event(FullNode::clone(&full_node))?;

        let handle = NodeHandle {
            node_exit_future: NodeExitFuture::new(
                async { rx.await? },
                full_node.config.debug.terminate,
            ),
            node: full_node,
        };

        Ok(handle)
    }
}
