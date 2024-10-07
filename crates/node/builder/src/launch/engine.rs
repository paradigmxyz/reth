//! Engine node related functionality.

use alloy_rpc_types::engine::ClientVersionV1;
use futures::{future::Either, stream, stream_select, StreamExt};
use reth_beacon_consensus::{
    hooks::{EngineHooks, StaticFileHook},
    BeaconConsensusEngineHandle,
};
use reth_blockchain_tree::BlockchainTreeConfig;
use reth_chainspec::EthChainSpec;
use reth_consensus_debug_client::{DebugConsensusClient, EtherscanBlockProvider};
use reth_engine_service::service::{ChainEvent, EngineService};
use reth_engine_tree::{
    engine::{EngineApiRequest, EngineRequestHandler},
    tree::TreeConfig,
};
use reth_engine_util::EngineMessageStreamExt;
use reth_exex::ExExManagerHandle;
use reth_network::{NetworkSyncUpdater, SyncState};
use reth_network_api::{BlockDownloaderProvider, NetworkEventListenerProvider};
use reth_node_api::{BuiltPayload, FullNodeTypes, NodeAddOns, NodeTypesWithEngine};
use reth_node_core::{
    dirs::{ChainPath, DataDirPath},
    exit::NodeExitFuture,
    primitives::Head,
    rpc::eth::{helpers::AddDevSigners, FullEthApiServer},
    version::{CARGO_PKG_VERSION, CLIENT_CODE, NAME_CLIENT, VERGEN_GIT_SHA},
};
use reth_node_events::{cl::ConsensusLayerHealthEvents, node};
use reth_payload_primitives::PayloadBuilder;
use reth_primitives::EthereumHardforks;
use reth_provider::providers::{BlockchainProvider2, ProviderNodeTypes};
use reth_rpc_engine_api::{capabilities::EngineCapabilities, EngineApi};
use reth_tasks::TaskExecutor;
use reth_tokio_util::EventSender;
use reth_tracing::tracing::{debug, error, info};
use std::sync::Arc;
use tokio::sync::{mpsc::unbounded_channel, oneshot};
use tokio_stream::wrappers::UnboundedReceiverStream;

use crate::{
    common::{Attached, LaunchContextWith, WithConfigs},
    hooks::NodeHooks,
    rpc::{launch_rpc_servers, EthApiBuilderProvider},
    setup::build_networked_pipeline,
    AddOns, ExExLauncher, FullNode, LaunchContext, LaunchNode, NodeAdapter,
    NodeBuilderWithComponents, NodeComponents, NodeComponentsBuilder, NodeHandle, NodeTypesAdapter,
};

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

impl<Types, T, CB, AO> LaunchNode<NodeBuilderWithComponents<T, CB, AO>> for EngineNodeLauncher
where
    Types: ProviderNodeTypes + NodeTypesWithEngine,
    T: FullNodeTypes<Types = Types, Provider = BlockchainProvider2<Types>>,
    CB: NodeComponentsBuilder<T>,
    AO: NodeAddOns<
        NodeAdapter<T, CB::Components>,
        EthApi: EthApiBuilderProvider<NodeAdapter<T, CB::Components>>
                    + FullEthApiServer
                    + AddDevSigners,
    >,
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
            add_ons: AddOns { hooks, rpc, exexs: installed_exex, .. },
            config,
        } = target;
        let NodeHooks { on_component_initialized, on_node_started, .. } = hooks;

        // TODO: move tree_config and canon_state_notification_sender
        // initialization to with_blockchain_db once the engine revamp is done
        // https://github.com/paradigmxyz/reth/issues/8742
        let tree_config = BlockchainTreeConfig::default();

        // NOTE: This is a temporary workaround to provide the canon state notification sender to the components builder because there's a cyclic dependency between the blockchain provider and the tree component. This will be removed once the Blockchain provider no longer depends on an instance of the tree: <https://github.com/paradigmxyz/reth/issues/7154>
        let (canon_state_notification_sender, _receiver) =
            tokio::sync::broadcast::channel(tree_config.max_reorg_depth() as usize * 2);

        // setup the launch context
        let ctx = ctx
            .with_configured_globals()
            // load the toml config
            .with_loaded_toml_config(config)?
            // add resolved peers
            .with_resolved_peers().await?
            // attach the database
            .attach(database.clone())
            // ensure certain settings take effect
            .with_adjusted_configs()
            // Create the provider factory
            .with_provider_factory().await?
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
                Ok(BlockchainProvider2::new(provider_factory)?)
            }, tree_config, canon_state_notification_sender)?
            .with_components(components_builder, on_component_initialized).await?;

        // spawn exexs
        let exex_manager_handle = ExExLauncher::new(
            ctx.head(),
            ctx.node_adapter().clone(),
            installed_exex,
            ctx.configs().clone(),
        )
        .launch()
        .await?;

        // create pipeline
        let network_client = ctx.components().network().fetch_client().await?;
        let (consensus_engine_tx, consensus_engine_rx) = unbounded_channel();

        let node_config = ctx.node_config();
        let consensus_engine_stream = UnboundedReceiverStream::from(consensus_engine_rx)
            .maybe_skip_fcu(node_config.debug.skip_fcu)
            .maybe_skip_new_payload(node_config.debug.skip_new_payload)
            .maybe_reorg(
                ctx.blockchain_db().clone(),
                ctx.components().evm_config().clone(),
                reth_payload_validator::ExecutionPayloadValidator::new(ctx.chain_spec()),
                node_config.debug.reorg_frequency,
                node_config.debug.reorg_depth,
            )
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
        let pipeline = build_networked_pipeline(
            &ctx.toml_config().stages,
            network_client.clone(),
            ctx.consensus(),
            ctx.provider_factory().clone(),
            ctx.task_executor(),
            ctx.sync_metrics_tx(),
            ctx.prune_config(),
            max_block,
            static_file_producer,
            ctx.components().block_executor().clone(),
            pipeline_exex_handle,
        )?;

        // The new engine writes directly to static files. This ensures that they're up to the tip.
        pipeline.move_to_static_files()?;

        let pipeline_events = pipeline.events();

        let mut pruner_builder = ctx.pruner_builder();
        if let Some(exex_manager_handle) = &exex_manager_handle {
            pruner_builder =
                pruner_builder.finished_exex_height(exex_manager_handle.finished_height());
        }
        let pruner = pruner_builder.build_with_provider_factory(ctx.provider_factory().clone());

        let pruner_events = pruner.events();
        info!(target: "reth::cli", prune_config=?ctx.prune_config().unwrap_or_default(), "Pruner initialized");

        // Configure the consensus engine
        let mut eth_service = EngineService::new(
            ctx.consensus(),
            ctx.components().block_executor().clone(),
            ctx.chain_spec(),
            network_client.clone(),
            Box::pin(consensus_engine_stream),
            pipeline,
            Box::new(ctx.task_executor().clone()),
            ctx.provider_factory().clone(),
            ctx.blockchain_db().clone(),
            pruner,
            ctx.components().payload_builder().clone(),
            engine_tree_config,
            ctx.invalid_block_hook()?,
            ctx.sync_metrics_tx(),
        );

        let event_sender = EventSender::default();

        let beacon_engine_handle =
            BeaconConsensusEngineHandle::new(consensus_engine_tx, event_sender.clone());

        info!(target: "reth::cli", "Consensus engine initialized");

        let events = stream_select!(
            ctx.components().network().event_listener().map(Into::into),
            beacon_engine_handle.event_listener().map(Into::into),
            pipeline_events.map(Into::into),
            if ctx.node_config().debug.tip.is_none() && !ctx.is_dev() {
                Either::Left(
                    ConsensusLayerHealthEvents::new(Box::new(ctx.blockchain_db().clone()))
                        .map(Into::into),
                )
            } else {
                Either::Right(stream::empty())
            },
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

        let client = ClientVersionV1 {
            code: CLIENT_CODE,
            name: NAME_CLIENT.to_string(),
            version: CARGO_PKG_VERSION.to_string(),
            commit: VERGEN_GIT_SHA.to_string(),
        };
        let engine_api = EngineApi::new(
            ctx.blockchain_db().clone(),
            ctx.chain_spec(),
            beacon_engine_handle,
            ctx.components().payload_builder().clone().into(),
            ctx.components().pool().clone(),
            Box::new(ctx.task_executor().clone()),
            client,
            EngineCapabilities::default(),
            ctx.components().engine_validator().clone(),
        );
        info!(target: "reth::cli", "Engine API handler initialized");

        // extract the jwt secret from the args if possible
        let jwt_secret = ctx.auth_jwt_secret()?;

        // Start RPC servers
        let (rpc_server_handles, rpc_registry) = launch_rpc_servers(
            ctx.node_adapter().clone(),
            engine_api,
            ctx.node_config(),
            jwt_secret,
            rpc,
        )
        .await?;

        // TODO: migrate to devmode with https://github.com/paradigmxyz/reth/issues/10104
        if let Some(maybe_custom_etherscan_url) = ctx.node_config().debug.etherscan.clone() {
            info!(target: "reth::cli", "Using etherscan as consensus client");

            let chain = ctx.node_config().chain.chain();
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
            );
            let rpc_consensus_client = DebugConsensusClient::new(
                rpc_server_handles.auth.clone(),
                Arc::new(block_provider),
            );
            ctx.task_executor().spawn_critical("etherscan consensus client", async move {
                rpc_consensus_client.run::<<Types as NodeTypesWithEngine>::Engine>().await
            });
        }

        // Run consensus engine to completion
        let initial_target = ctx.initial_backfill_target()?;
        let network_handle = ctx.components().network().clone();
        let mut built_payloads = ctx
            .components()
            .payload_builder()
            .subscribe()
            .await
            .map_err(|e| eyre::eyre!("Failed to subscribe to payload builder events: {:?}", e))?
            .into_built_payload_stream()
            .fuse();
        let chainspec = ctx.chain_spec();
        let (exit, rx) = oneshot::channel();
        info!(target: "reth::cli", "Starting consensus engine");
        ctx.task_executor().spawn_critical("consensus engine", async move {
            if let Some(initial_target) = initial_target {
                debug!(target: "reth::cli", %initial_target,  "start backfill sync");
                eth_service.orchestrator_mut().start_backfill_sync(initial_target);
            }

            let mut res = Ok(());

            // advance the chain and await payloads built locally to add into the engine api tree handler to prevent re-execution if that block is received as payload from the CL
            loop {
                tokio::select! {
                    payload = built_payloads.select_next_some() => {
                        if let Some(executed_block) = payload.executed_block() {
                            debug!(target: "reth::cli", block=?executed_block.block().num_hash(),  "inserting built payload");
                            eth_service.orchestrator_mut().handler_mut().handler_mut().on_event(EngineApiRequest::InsertExecutedBlock(executed_block).into());
                        }
                    }
                    event =  eth_service.next() => {
                        let Some(event) = event else { break };
                        debug!(target: "reth::cli", "Event: {event}");
                        match event {
                            ChainEvent::BackfillSyncFinished => {
                                network_handle.update_sync_state(SyncState::Idle);
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
                                    let head_block = Head {
                                        number: head.number,
                                        hash: head.hash(),
                                        difficulty: head.difficulty,
                                        timestamp: head.timestamp,
                                        total_difficulty: chainspec
                                            .final_paris_total_difficulty(head.number)
                                            .unwrap_or_default(),
                                    };
                                    network_handle.update_status(head_block);
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
            block_executor: ctx.components().block_executor().clone(),
            pool: ctx.components().pool().clone(),
            network: ctx.components().network().clone(),
            provider: ctx.node_adapter().provider.clone(),
            payload_builder: ctx.components().payload_builder().clone(),
            task_executor: ctx.task_executor().clone(),
            rpc_server_handles,
            rpc_registry,
            config: ctx.node_config().clone(),
            data_dir: ctx.data_dir().clone(),
        };
        // Notify on node started
        on_node_started.on_event(full_node.clone())?;

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
