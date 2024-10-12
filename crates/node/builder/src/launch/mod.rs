//! Abstraction for launching a node.

pub mod common;
mod exex;

pub(crate) mod engine;

pub use common::LaunchContext;
use common::{Attached, LaunchContextWith, WithConfigs};
pub use exex::ExExLauncher;

use std::{future::Future, sync::Arc};

use alloy_primitives::utils::format_ether;
use alloy_rpc_types::engine::ClientVersionV1;
use futures::{future::Either, stream, stream_select, StreamExt};
use reth_beacon_consensus::{
    hooks::{EngineHooks, PruneHook, StaticFileHook},
    BeaconConsensusEngine,
};
use reth_blockchain_tree::{noop::NoopBlockchainTree, BlockchainTreeConfig};
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_consensus_debug_client::{DebugConsensusClient, EtherscanBlockProvider, RpcBlockProvider};
use reth_engine_util::EngineMessageStreamExt;
use reth_exex::ExExManagerHandle;
use reth_network::{BlockDownloaderProvider, NetworkEventListenerProvider};
use reth_node_api::{
    FullNodeComponents, FullNodeTypes, NodeAddOns, NodeTypesWithDB, NodeTypesWithEngine,
};
use reth_node_core::{
    dirs::{ChainPath, DataDirPath},
    exit::NodeExitFuture,
    rpc::eth::{helpers::AddDevSigners, FullEthApiServer},
    version::{CARGO_PKG_VERSION, CLIENT_CODE, NAME_CLIENT, VERGEN_GIT_SHA},
};
use reth_node_events::{cl::ConsensusLayerHealthEvents, node};
use reth_provider::providers::BlockchainProvider;
use reth_rpc_engine_api::{capabilities::EngineCapabilities, EngineApi};
use reth_tasks::TaskExecutor;
use reth_tracing::tracing::{debug, info};
use reth_transaction_pool::TransactionPool;
use tokio::sync::{mpsc::unbounded_channel, oneshot};
use tokio_stream::wrappers::UnboundedReceiverStream;

use crate::{
    builder::{NodeAdapter, NodeTypesAdapter},
    components::{NodeComponents, NodeComponentsBuilder},
    hooks::NodeHooks,
    node::FullNode,
    rpc::EthApiBuilderProvider,
    AddOns, NodeBuilderWithComponents, NodeHandle,
};

/// Alias for [`reth_rpc_eth_types::EthApiBuilderCtx`], adapter for [`FullNodeComponents`].
pub type EthApiBuilderCtx<N, Eth> = reth_rpc_eth_types::EthApiBuilderCtx<
    <N as FullNodeTypes>::Provider,
    <N as FullNodeComponents>::Pool,
    <N as FullNodeComponents>::Evm,
    <N as FullNodeComponents>::Network,
    TaskExecutor,
    <N as FullNodeTypes>::Provider,
    Eth,
>;

/// A general purpose trait that launches a new node of any kind.
///
/// Acts as a node factory that targets a certain node configuration and returns a handle to the
/// node.
///
/// This is essentially the launch logic for a node.
///
/// See also [`DefaultNodeLauncher`] and [`NodeBuilderWithComponents::launch_with`]
pub trait LaunchNode<Target> {
    /// The node type that is created.
    type Node;

    /// Create and return a new node asynchronously.
    fn launch_node(self, target: Target) -> impl Future<Output = eyre::Result<Self::Node>> + Send;
}

impl<F, Target, Fut, Node> LaunchNode<Target> for F
where
    F: FnOnce(Target) -> Fut + Send,
    Fut: Future<Output = eyre::Result<Node>> + Send,
{
    type Node = Node;

    fn launch_node(self, target: Target) -> impl Future<Output = eyre::Result<Self::Node>> + Send {
        self(target)
    }
}

/// The default launcher for a node.
#[derive(Debug)]
pub struct DefaultNodeLauncher {
    /// The task executor for the node.
    pub ctx: LaunchContext,
}

impl DefaultNodeLauncher {
    /// Create a new instance of the default node launcher.
    pub const fn new(task_executor: TaskExecutor, data_dir: ChainPath<DataDirPath>) -> Self {
        Self { ctx: LaunchContext::new(task_executor, data_dir) }
    }
}

impl<Types, T, CB, AO> LaunchNode<NodeBuilderWithComponents<T, CB, AO>> for DefaultNodeLauncher
where
    Types: NodeTypesWithDB<ChainSpec: EthereumHardforks + EthChainSpec> + NodeTypesWithEngine,
    T: FullNodeTypes<Provider = BlockchainProvider<Types>, Types = Types>,
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
        let Self { ctx } = self;
        let NodeBuilderWithComponents {
            adapter: NodeTypesAdapter { database },
            components_builder,
            add_ons: AddOns { hooks, rpc, exexs: installed_exex, .. },
            config,
        } = target;
        let NodeHooks { on_component_initialized, on_node_started, .. } = hooks;

        // TODO: remove tree and move tree_config and canon_state_notification_sender
        // initialization to with_blockchain_db once the engine revamp is done
        // https://github.com/paradigmxyz/reth/issues/8742
        let tree_config = BlockchainTreeConfig::default();

        // NOTE: This is a temporary workaround to provide the canon state notification sender to the components builder because there's a cyclic dependency between the blockchain provider and the tree component. This will be removed once the Blockchain provider no longer depends on an instance of the tree: <https://github.com/paradigmxyz/reth/issues/7154>
        let (canon_state_notification_sender, _receiver) =
            tokio::sync::broadcast::channel(tree_config.max_reorg_depth() as usize * 2);

        let tree = Arc::new(NoopBlockchainTree::with_canon_state_notifications(
            canon_state_notification_sender.clone(),
        ));

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
                Ok(BlockchainProvider::new(provider_factory, tree)?)
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
        let (pipeline, client) = if ctx.is_dev() {
            info!(target: "reth::cli", "Starting Reth in dev mode");

            for (idx, (address, alloc)) in ctx.chain_spec().genesis().alloc.iter().enumerate() {
                info!(target: "reth::cli", "Allocated Genesis Account: {:02}. {} ({} ETH)", idx, address.to_string(), format_ether(alloc.balance));
            }

            // install auto-seal
            let mining_mode =
                ctx.dev_mining_mode(ctx.components().pool().pending_transactions_listener());
            info!(target: "reth::cli", mode=%mining_mode, "configuring dev mining mode");

            let (_, client, mut task) = reth_auto_seal_consensus::AutoSealBuilder::new(
                ctx.chain_spec(),
                ctx.blockchain_db().clone(),
                ctx.components().pool().clone(),
                consensus_engine_tx.clone(),
                mining_mode,
                ctx.components().block_executor().clone(),
            )
            .build();

            let pipeline = crate::setup::build_networked_pipeline(
                &ctx.toml_config().stages,
                client.clone(),
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

            let pipeline_events = pipeline.events();
            task.set_pipeline_events(pipeline_events);
            debug!(target: "reth::cli", "Spawning auto mine task");
            ctx.task_executor().spawn(Box::pin(task));

            (pipeline, Either::Left(client))
        } else {
            let pipeline = crate::setup::build_networked_pipeline(
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

            (pipeline, Either::Right(network_client.clone()))
        };

        let pipeline_events = pipeline.events();

        let initial_target = ctx.node_config().debug.tip;

        let mut pruner_builder = ctx.pruner_builder();
        if let Some(exex_manager_handle) = &exex_manager_handle {
            pruner_builder =
                pruner_builder.finished_exex_height(exex_manager_handle.finished_height());
        }
        let pruner = pruner_builder.build_with_provider_factory(ctx.provider_factory().clone());

        let pruner_events = pruner.events();
        info!(target: "reth::cli", prune_config=?ctx.prune_config().unwrap_or_default(), "Pruner initialized");
        hooks.add(PruneHook::new(pruner, Box::new(ctx.task_executor().clone())));

        // Configure the consensus engine
        let (beacon_consensus_engine, beacon_engine_handle) = BeaconConsensusEngine::with_channel(
            client,
            pipeline,
            ctx.blockchain_db().clone(),
            Box::new(ctx.task_executor().clone()),
            Box::new(ctx.components().network().clone()),
            max_block,
            ctx.components().payload_builder().clone(),
            initial_target,
            reth_beacon_consensus::MIN_BLOCKS_FOR_PIPELINE_RUN,
            consensus_engine_tx,
            Box::pin(consensus_engine_stream),
            hooks,
        )?;
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
        let (rpc_server_handles, rpc_registry) = crate::rpc::launch_rpc_servers(
            ctx.node_adapter().clone(),
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
                rpc_consensus_client.run::<Types::Engine>().await
            });
        }

        if let Some(rpc_ws_url) = ctx.node_config().debug.rpc_consensus_ws.clone() {
            info!(target: "reth::cli", "Using rpc provider as consensus client");

            let block_provider = RpcBlockProvider::new(rpc_ws_url);
            let rpc_consensus_client = DebugConsensusClient::new(
                rpc_server_handles.auth.clone(),
                Arc::new(block_provider),
            );
            ctx.task_executor().spawn_critical("rpc consensus client", async move {
                rpc_consensus_client.run::<Types::Engine>().await
            });
        }

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
                async { Ok(rx.await??) },
                full_node.config.debug.terminate,
            ),
            node: full_node,
        };

        Ok(handle)
    }
}
