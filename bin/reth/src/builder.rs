//! Contains types and methods that can be used to launch a node based off of a [NodeConfig].

use crate::commands::debug_cmd::engine_api_store::EngineApiStore;
use eyre::Context;
use fdlimit::raise_fd_limit;
use futures::{future::Either, stream, stream_select, StreamExt};
use reth_auto_seal_consensus::AutoSealBuilder;
use reth_beacon_consensus::{
    hooks::{EngineHooks, PruneHook},
    BeaconConsensusEngine, MIN_BLOCKS_FOR_PIPELINE_RUN,
};
use reth_blockchain_tree::{config::BlockchainTreeConfig, ShareableBlockchainTree};
use reth_config::Config;
use reth_db::{
    database::Database,
    database_metrics::{DatabaseMetadata, DatabaseMetrics},
};
use reth_interfaces::p2p::either::EitherDownloader;
use reth_network::NetworkEvents;
use reth_network_api::{NetworkInfo, PeersInfo};
use reth_node_core::{
    cli::{
        components::{RethNodeComponentsImpl, RethRpcServerHandles},
        config::RethRpcConfig,
        db_type::DatabaseInstance,
        ext::{DefaultRethNodeCommandConfig, RethCliExt, RethNodeCommandConfig},
    },
    dirs::{ChainPath, DataDirPath},
    events::cl::ConsensusLayerHealthEvents,
    exit::NodeExitFuture,
    init::init_genesis,
    version::SHORT_VERSION,
};
#[cfg(not(feature = "optimism"))]
use reth_node_ethereum::{EthEngineTypes, EthEvmConfig};
#[cfg(feature = "optimism")]
use reth_node_optimism::{OptimismEngineTypes, OptimismEvmConfig};
use reth_payload_builder::PayloadBuilderHandle;
use reth_primitives::format_ether;
use reth_provider::{providers::BlockchainProvider, ProviderFactory};
use reth_prune::PrunerBuilder;
use reth_rpc_engine_api::EngineApi;
use reth_tasks::{TaskExecutor, TaskManager};
use reth_transaction_pool::TransactionPool;
use std::{path::PathBuf, sync::Arc};
use tokio::sync::{mpsc::unbounded_channel, oneshot};
use tracing::*;

/// Re-export `NodeConfig` from `reth_node_core`.
pub use reth_node_core::node_config::NodeConfig;

/// Launches the node, also adding any RPC extensions passed.
///
/// # Example
/// ```rust
/// # use reth_tasks::{TaskManager, TaskSpawner};
/// # use reth_node_core::node_config::NodeConfig;
/// # use reth_node_core::cli::{
/// #     ext::DefaultRethNodeCommandConfig,
/// # };
/// # use tokio::runtime::Handle;
/// # use reth::builder::launch_from_config;
///
/// async fn t() {
///     let handle = Handle::current();
///     let manager = TaskManager::new(handle);
///     let executor = manager.executor();
///     let builder = NodeConfig::default();
///     let ext = DefaultRethNodeCommandConfig::default();
///     let handle = launch_from_config::<()>(builder, ext, executor).await.unwrap();
/// }
/// ```
pub async fn launch_from_config<E: RethCliExt>(
    mut config: NodeConfig,
    ext: E::Node,
    executor: TaskExecutor,
) -> eyre::Result<NodeHandle> {
    info!(target: "reth::cli", "reth {} starting", SHORT_VERSION);

    // Register the prometheus recorder before creating the database,
    // because database init needs it to register metrics.
    config.install_prometheus_recorder()?;

    let database = std::mem::take(&mut config.database);
    let db_instance = database.init_db(config.db.log_level, config.chain.chain)?;
    info!(target: "reth::cli", "Database opened");

    match db_instance {
        DatabaseInstance::Real { db, data_dir } => {
            let builder = NodeBuilderWithDatabase { config, db, data_dir };
            builder.launch::<E>(ext, executor).await
        }
        DatabaseInstance::Test { db, data_dir } => {
            let builder = NodeBuilderWithDatabase { config, db, data_dir };
            builder.launch::<E>(ext, executor).await
        }
    }
}

/// A version of the [NodeConfig] that has an installed database. This is used to construct the
/// [NodeHandle].
///
/// This also contains a path to a data dir that cannot be changed.
#[derive(Debug)]
pub struct NodeBuilderWithDatabase<DB> {
    /// The node config
    pub config: NodeConfig,
    /// The database
    pub db: Arc<DB>,
    /// The data dir
    pub data_dir: ChainPath<DataDirPath>,
}

impl<DB: Database + DatabaseMetrics + DatabaseMetadata + 'static> NodeBuilderWithDatabase<DB> {
    /// Launch the node with the given extensions and executor
    pub async fn launch<E: RethCliExt>(
        mut self,
        mut ext: E::Node,
        executor: TaskExecutor,
    ) -> eyre::Result<NodeHandle> {
        // Raise the fd limit of the process.
        // Does not do anything on windows.
        raise_fd_limit()?;

        // get config
        let config = self.load_config()?;

        let prometheus_handle = self.config.install_prometheus_recorder()?;

        let mut provider_factory =
            ProviderFactory::new(Arc::clone(&self.db), Arc::clone(&self.config.chain));

        // configure snapshotter
        let snapshotter = reth_snapshot::Snapshotter::new(
            provider_factory.clone(),
            self.data_dir.snapshots_path(),
            self.config.chain.snapshot_block_interval,
        )?;

        provider_factory = provider_factory.with_snapshots(
            self.data_dir.snapshots_path(),
            snapshotter.highest_snapshot_receiver(),
        )?;

        self.config.start_metrics_endpoint(prometheus_handle, Arc::clone(&self.db)).await?;

        debug!(target: "reth::cli", chain=%self.config.chain.chain, genesis=?self.config.chain.genesis_hash(), "Initializing genesis");

        let genesis_hash = init_genesis(Arc::clone(&self.db), self.config.chain.clone())?;

        info!(target: "reth::cli", "{}", self.config.chain.display_hardforks());

        let consensus = self.config.consensus();

        debug!(target: "reth::cli", "Spawning stages metrics listener task");
        let (sync_metrics_tx, sync_metrics_rx) = unbounded_channel();
        let sync_metrics_listener = reth_stages::MetricsListener::new(sync_metrics_rx);
        executor.spawn_critical("stages metrics listener task", sync_metrics_listener);

        let prune_config = self
            .config
            .pruning
            .prune_config(Arc::clone(&self.config.chain))?
            .or(config.prune.clone());

        // TODO: stateful node builder should be able to remove cfgs here
        #[cfg(feature = "optimism")]
        let evm_config = OptimismEvmConfig::default();

        // The default payload builder is implemented on the unit type.
        #[cfg(not(feature = "optimism"))]
        let evm_config = EthEvmConfig::default();

        // configure blockchain tree
        let tree_config = BlockchainTreeConfig::default();
        let tree = self.config.build_blockchain_tree(
            provider_factory.clone(),
            consensus.clone(),
            prune_config.clone(),
            sync_metrics_tx.clone(),
            tree_config,
            evm_config,
        )?;
        let canon_state_notification_sender = tree.canon_state_notification_sender();
        let blockchain_tree = ShareableBlockchainTree::new(tree);
        debug!(target: "reth::cli", "configured blockchain tree");

        // fetch the head block from the database
        let head = self
            .config
            .lookup_head(provider_factory.clone())
            .wrap_err("the head block is missing")?;

        // setup the blockchain provider
        let blockchain_db =
            BlockchainProvider::new(provider_factory.clone(), blockchain_tree.clone())?;

        // build transaction pool
        let transaction_pool =
            self.config.build_and_spawn_txpool(&blockchain_db, head, &executor, &self.data_dir)?;

        // build network
        let mut network_builder = self
            .config
            .build_network(
                &config,
                provider_factory.clone(),
                executor.clone(),
                head,
                &self.data_dir,
            )
            .await?;

        let components = RethNodeComponentsImpl::new(
            blockchain_db.clone(),
            transaction_pool.clone(),
            network_builder.handle(),
            executor.clone(),
            blockchain_db.clone(),
            evm_config,
        );

        // allow network modifications
        ext.configure_network(network_builder.network_mut(), &components)?;

        // launch network
        let network = self.config.start_network(
            network_builder,
            &executor,
            transaction_pool.clone(),
            provider_factory.clone(),
            &self.data_dir,
        );

        info!(target: "reth::cli", peer_id = %network.peer_id(), local_addr = %network.local_addr(), enode = %network.local_node_record(), "Connected to P2P network");
        debug!(target: "reth::cli", peer_id = ?network.peer_id(), "Full peer ID");
        let network_client = network.fetch_client().await?;

        ext.on_components_initialized(&components)?;

        debug!(target: "reth::cli", "Spawning payload builder service");

        // TODO: stateful node builder should handle this in with_payload_builder
        // Optimism's payload builder is implemented on the OptimismPayloadBuilder type.
        #[cfg(feature = "optimism")]
        let payload_builder = reth_optimism_payload_builder::OptimismPayloadBuilder::default()
            .set_compute_pending_block(self.config.builder.compute_pending_block);

        #[cfg(feature = "optimism")]
        let payload_builder: PayloadBuilderHandle<OptimismEngineTypes> =
            ext.spawn_payload_builder_service(&self.config.builder, &components, payload_builder)?;

        // The default payload builder is implemented on the unit type.
        #[cfg(not(feature = "optimism"))]
        let payload_builder = reth_ethereum_payload_builder::EthereumPayloadBuilder::default();

        #[cfg(not(feature = "optimism"))]
        let payload_builder: PayloadBuilderHandle<EthEngineTypes> =
            ext.spawn_payload_builder_service(&self.config.builder, &components, payload_builder)?;

        let (consensus_engine_tx, mut consensus_engine_rx) = unbounded_channel();
        if let Some(store_path) = self.config.debug.engine_api_store.clone() {
            let (engine_intercept_tx, engine_intercept_rx) = unbounded_channel();
            let engine_api_store = EngineApiStore::new(store_path);
            executor.spawn_critical(
                "engine api interceptor",
                engine_api_store.intercept(consensus_engine_rx, engine_intercept_tx),
            );
            consensus_engine_rx = engine_intercept_rx;
        };
        let max_block = self.config.max_block(&network_client, provider_factory.clone()).await?;

        // Configure the pipeline
        let (mut pipeline, client) = if self.config.dev.dev {
            info!(target: "reth::cli", "Starting Reth in dev mode");
            for (idx, (address, alloc)) in self.config.chain.genesis.alloc.iter().enumerate() {
                info!(target: "reth::cli", "Allocated Genesis Account: {:02}. {} ({} ETH)", idx, address.to_string(), format_ether(alloc.balance));
            }
            let mining_mode =
                self.config.mining_mode(transaction_pool.pending_transactions_listener());

            let (_, client, mut task) = AutoSealBuilder::new(
                Arc::clone(&self.config.chain),
                blockchain_db.clone(),
                transaction_pool.clone(),
                consensus_engine_tx.clone(),
                canon_state_notification_sender,
                mining_mode,
                evm_config,
            )
            .build();

            let mut pipeline = self
                .config
                .build_networked_pipeline(
                    &config.stages,
                    client.clone(),
                    Arc::clone(&consensus),
                    provider_factory.clone(),
                    &executor,
                    sync_metrics_tx,
                    prune_config.clone(),
                    max_block,
                    evm_config,
                )
                .await?;

            let pipeline_events = pipeline.events();
            task.set_pipeline_events(pipeline_events);
            debug!(target: "reth::cli", "Spawning auto mine task");
            executor.spawn(Box::pin(task));

            (pipeline, EitherDownloader::Left(client))
        } else {
            let pipeline = self
                .config
                .build_networked_pipeline(
                    &config.stages,
                    network_client.clone(),
                    Arc::clone(&consensus),
                    provider_factory.clone(),
                    &executor.clone(),
                    sync_metrics_tx,
                    prune_config.clone(),
                    max_block,
                    evm_config,
                )
                .await?;

            (pipeline, EitherDownloader::Right(network_client))
        };

        let pipeline_events = pipeline.events();

        let initial_target = self.config.initial_pipeline_target(genesis_hash);
        let mut hooks = EngineHooks::new();

        let pruner_events = if let Some(prune_config) = prune_config {
            let mut pruner = PrunerBuilder::new(prune_config.clone())
                .max_reorg_depth(tree_config.max_reorg_depth() as usize)
                .prune_delete_limit(self.config.chain.prune_delete_limit)
                .build(provider_factory, snapshotter.highest_snapshot_receiver());

            let events = pruner.events();
            hooks.add(PruneHook::new(pruner, Box::new(executor.clone())));

            info!(target: "reth::cli", ?prune_config, "Pruner initialized");
            Either::Left(events)
        } else {
            Either::Right(stream::empty())
        };

        // Configure the consensus engine
        let (beacon_consensus_engine, beacon_engine_handle) = BeaconConsensusEngine::with_channel(
            client,
            pipeline,
            blockchain_db.clone(),
            Box::new(executor.clone()),
            Box::new(network.clone()),
            max_block,
            self.config.debug.continuous,
            payload_builder.clone(),
            initial_target,
            MIN_BLOCKS_FOR_PIPELINE_RUN,
            consensus_engine_tx,
            consensus_engine_rx,
            hooks,
        )?;
        info!(target: "reth::cli", "Consensus engine initialized");

        let events = stream_select!(
            network.event_listener().map(Into::into),
            beacon_engine_handle.event_listener().map(Into::into),
            pipeline_events.map(Into::into),
            if self.config.debug.tip.is_none() {
                Either::Left(
                    ConsensusLayerHealthEvents::new(Box::new(blockchain_db.clone()))
                        .map(Into::into),
                )
            } else {
                Either::Right(stream::empty())
            },
            pruner_events.map(Into::into)
        );
        executor.spawn_critical(
            "events task",
            reth_node_core::events::node::handle_events(
                Some(network.clone()),
                Some(head.number),
                events,
                self.db.clone(),
            ),
        );

        let engine_api = EngineApi::new(
            blockchain_db.clone(),
            self.config.chain.clone(),
            beacon_engine_handle,
            payload_builder.into(),
            Box::new(executor.clone()),
        );
        info!(target: "reth::cli", "Engine API handler initialized");

        // extract the jwt secret from the args if possible
        let default_jwt_path = self.data_dir.jwt_path();
        let jwt_secret = self.config.rpc.auth_jwt_secret(default_jwt_path)?;

        // adjust rpc port numbers based on instance number
        self.config.adjust_instance_ports();

        // Start RPC servers
        let rpc_server_handles =
            self.config.rpc.start_servers(&components, engine_api, jwt_secret, &mut ext).await?;

        // Run consensus engine to completion
        let (tx, rx) = oneshot::channel();
        info!(target: "reth::cli", "Starting consensus engine");
        executor.spawn_critical_blocking("consensus engine", async move {
            let res = beacon_consensus_engine.await;
            let _ = tx.send(res);
        });

        ext.on_node_started(&components)?;

        // If `enable_genesis_walkback` is set to true, the rollup client will need to
        // perform the derivation pipeline from genesis, validating the data dir.
        // When set to false, set the finalized, safe, and unsafe head block hashes
        // on the rollup client using a fork choice update. This prevents the rollup
        // client from performing the derivation pipeline from genesis, and instead
        // starts syncing from the current tip in the DB.
        #[cfg(feature = "optimism")]
        if self.config.chain.is_optimism() && !self.config.rollup.enable_genesis_walkback {
            let client = rpc_server_handles.auth.http_client();
            reth_rpc_api::EngineApiClient::<OptimismEngineTypes>::fork_choice_updated_v2(
                &client,
                reth_rpc_types::engine::ForkchoiceState {
                    head_block_hash: head.hash,
                    safe_block_hash: head.hash,
                    finalized_block_hash: head.hash,
                },
                None,
            )
            .await?;
        }

        // construct node handle and return
        let node_handle = NodeHandle {
            rpc_server_handles,
            node_exit_future: NodeExitFuture::new(rx, self.config.debug.terminate),
        };
        Ok(node_handle)
    }

    /// Returns the path to the config file.
    fn config_path(&self) -> PathBuf {
        self.config.config.clone().unwrap_or_else(|| self.data_dir.config_path())
    }

    /// Loads the reth config with the given datadir root
    fn load_config(&self) -> eyre::Result<Config> {
        let config_path = self.config_path();

        let mut config = confy::load_path::<Config>(&config_path)
            .wrap_err_with(|| format!("Could not load config file {:?}", config_path))?;

        info!(target: "reth::cli", path = ?config_path, "Configuration loaded");

        // Update the config with the command line arguments
        config.peers.connect_trusted_nodes_only = self.config.network.trusted_only;

        if !self.config.network.trusted_peers.is_empty() {
            info!(target: "reth::cli", "Adding trusted nodes");
            self.config.network.trusted_peers.iter().for_each(|peer| {
                config.peers.trusted_nodes.insert(*peer);
            });
        }

        Ok(config)
    }
}

/// The [NodeHandle] contains the [RethRpcServerHandles] returned by the reth initialization
/// process, as well as a method for waiting for the node exit.
#[derive(Debug)]
pub struct NodeHandle {
    /// The handles to the RPC servers
    rpc_server_handles: RethRpcServerHandles,

    /// A Future which waits node exit
    /// See [`NodeExitFuture`]
    node_exit_future: NodeExitFuture,
}

impl NodeHandle {
    /// Returns the [RethRpcServerHandles] for this node.
    pub fn rpc_server_handles(&self) -> &RethRpcServerHandles {
        &self.rpc_server_handles
    }

    /// Waits for the node to exit, if it was configured to exit.
    pub async fn wait_for_node_exit(self) -> eyre::Result<()> {
        self.node_exit_future.await
    }
}

/// A simple function to launch a node with the specified [NodeConfig], spawning tasks on the
/// [TaskExecutor] constructed from [TaskManager::current].
///
/// # Example
/// ```
/// # use reth_node_core::{
/// #     node_config::NodeConfig,
/// #     args::RpcServerArgs,
/// # };
/// # use reth::builder::spawn_node;
/// async fn t() {
///     // Create a node builder with an http rpc server enabled
///     let rpc_args = RpcServerArgs::default().with_http();
///
///     let builder = NodeConfig::test().with_rpc(rpc_args);
///
///     // Spawn the builder, returning a handle to the node
///     let (_handle, _manager) = spawn_node(builder).await.unwrap();
/// }
/// ```
pub async fn spawn_node(config: NodeConfig) -> eyre::Result<(NodeHandle, TaskManager)> {
    let task_manager = TaskManager::current();
    let ext = DefaultRethNodeCommandConfig::default();
    Ok((launch_from_config::<()>(config, ext, task_manager.executor()).await?, task_manager))
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_node_core::args::RpcServerArgs;
    use reth_primitives::U256;
    use reth_rpc_api::EthApiClient;

    #[tokio::test]
    async fn block_number_node_config_test() {
        // this launches a test node with http
        let rpc_args = RpcServerArgs::default().with_http();

        let (handle, _manager) = spawn_node(NodeConfig::test().with_rpc(rpc_args)).await.unwrap();

        // call a function on the node
        let client = handle.rpc_server_handles().rpc.http_client().unwrap();
        let block_number = client.block_number().await.unwrap();

        // it should be zero, since this is an ephemeral test node
        assert_eq!(block_number, U256::ZERO);
    }

    #[tokio::test]
    async fn rpc_handles_none_without_http() {
        // this launches a test node _without_ http
        let (handle, _manager) = spawn_node(NodeConfig::test()).await.unwrap();

        // ensure that the `http_client` is none
        let maybe_client = handle.rpc_server_handles().rpc.http_client();
        assert!(maybe_client.is_none());
    }

    #[tokio::test]
    async fn launch_multiple_nodes() {
        // spawn_test_node takes roughly 1 second per node, so this test takes ~4 seconds
        let num_nodes = 4;

        // contains handles and managers
        let mut handles = Vec::new();
        for _ in 0..num_nodes {
            let handle = spawn_node(NodeConfig::test()).await.unwrap();
            handles.push(handle);
        }
    }

    #[cfg(feature = "optimism")]
    #[tokio::test]
    async fn optimism_pre_canyon_no_withdrawals_valid() {
        reth_tracing::init_test_tracing();
        use alloy_chains::Chain;
        use jsonrpsee::http_client::HttpClient;
        use reth_primitives::{ChainSpec, Genesis};
        use reth_rpc_api::EngineApiClient;
        use reth_rpc_types::engine::{
            ForkchoiceState, OptimismPayloadAttributes, PayloadAttributes,
        };

        // this launches a test node with http
        let rpc_args = RpcServerArgs::default().with_http();

        // create optimism genesis with canyon at block 2
        let spec = ChainSpec::builder()
            .chain(Chain::optimism_mainnet())
            .genesis(Genesis::default())
            .regolith_activated()
            .build();

        let genesis_hash = spec.genesis_hash();

        // create node config
        let node_config = NodeConfig::test().with_rpc(rpc_args).with_chain(spec);

        let (handle, _manager) = spawn_node(node_config).await.unwrap();

        // call a function on the node
        let client = handle.rpc_server_handles().auth.http_client();
        let block_number = client.block_number().await.unwrap();

        // it should be zero, since this is an ephemeral test node
        assert_eq!(block_number, U256::ZERO);

        // call the engine_forkchoiceUpdated function with payload attributes
        let forkchoice_state = ForkchoiceState {
            head_block_hash: genesis_hash,
            safe_block_hash: genesis_hash,
            finalized_block_hash: genesis_hash,
        };

        let payload_attributes = OptimismPayloadAttributes {
            payload_attributes: PayloadAttributes {
                timestamp: 1,
                prev_randao: Default::default(),
                suggested_fee_recipient: Default::default(),
                // canyon is _not_ in the chain spec, so this should cause the engine call to fail
                withdrawals: None,
                parent_beacon_block_root: None,
            },
            no_tx_pool: None,
            gas_limit: Some(1),
            transactions: None,
        };

        // call the engine_forkchoiceUpdated function with payload attributes
        let res = <HttpClient as EngineApiClient<OptimismEngineTypes>>::fork_choice_updated_v2(
            &client,
            forkchoice_state,
            Some(payload_attributes),
        )
        .await;
        res.expect("pre-canyon engine call without withdrawals should succeed");
    }

    #[cfg(feature = "optimism")]
    #[tokio::test]
    async fn optimism_pre_canyon_withdrawals_invalid() {
        reth_tracing::init_test_tracing();
        use alloy_chains::Chain;
        use assert_matches::assert_matches;
        use jsonrpsee::{core::Error, http_client::HttpClient, types::error::INVALID_PARAMS_CODE};
        use reth_primitives::{ChainSpec, Genesis};
        use reth_rpc_api::EngineApiClient;
        use reth_rpc_types::engine::{
            ForkchoiceState, OptimismPayloadAttributes, PayloadAttributes,
        };

        // this launches a test node with http
        let rpc_args = RpcServerArgs::default().with_http();

        // create optimism genesis with canyon at block 2
        let spec = ChainSpec::builder()
            .chain(Chain::optimism_mainnet())
            .genesis(Genesis::default())
            .regolith_activated()
            .build();

        let genesis_hash = spec.genesis_hash();

        // create node config
        let node_config = NodeConfig::test().with_rpc(rpc_args).with_chain(spec);

        let (handle, _manager) = spawn_node(node_config).await.unwrap();

        // call a function on the node
        let client = handle.rpc_server_handles().auth.http_client();
        let block_number = client.block_number().await.unwrap();

        // it should be zero, since this is an ephemeral test node
        assert_eq!(block_number, U256::ZERO);

        // call the engine_forkchoiceUpdated function with payload attributes
        let forkchoice_state = ForkchoiceState {
            head_block_hash: genesis_hash,
            safe_block_hash: genesis_hash,
            finalized_block_hash: genesis_hash,
        };

        let payload_attributes = OptimismPayloadAttributes {
            payload_attributes: PayloadAttributes {
                timestamp: 1,
                prev_randao: Default::default(),
                suggested_fee_recipient: Default::default(),
                // canyon is _not_ in the chain spec, so this should cause the engine call to fail
                withdrawals: Some(vec![]),
                parent_beacon_block_root: None,
            },
            no_tx_pool: None,
            gas_limit: Some(1),
            transactions: None,
        };

        // call the engine_forkchoiceUpdated function with payload attributes
        let res = <HttpClient as EngineApiClient<OptimismEngineTypes>>::fork_choice_updated_v2(
            &client,
            forkchoice_state,
            Some(payload_attributes),
        )
        .await;
        let err = res.expect_err("pre-canyon engine call with withdrawals should fail");
        assert_matches!(err, Error::Call(ref object) if object.code() == INVALID_PARAMS_CODE);
    }

    #[cfg(feature = "optimism")]
    #[tokio::test]
    async fn optimism_post_canyon_no_withdrawals_invalid() {
        reth_tracing::init_test_tracing();
        use alloy_chains::Chain;
        use assert_matches::assert_matches;
        use jsonrpsee::{core::Error, http_client::HttpClient, types::error::INVALID_PARAMS_CODE};
        use reth_primitives::{ChainSpec, Genesis};
        use reth_rpc_api::EngineApiClient;
        use reth_rpc_types::engine::{
            ForkchoiceState, OptimismPayloadAttributes, PayloadAttributes,
        };

        // this launches a test node with http
        let rpc_args = RpcServerArgs::default().with_http();

        // create optimism genesis with canyon at block 2
        let spec = ChainSpec::builder()
            .chain(Chain::optimism_mainnet())
            .genesis(Genesis::default())
            .canyon_activated()
            .build();

        let genesis_hash = spec.genesis_hash();

        // create node config
        let node_config = NodeConfig::test().with_rpc(rpc_args).with_chain(spec);

        let (handle, _manager) = spawn_node(node_config).await.unwrap();

        // call a function on the node
        let client = handle.rpc_server_handles().auth.http_client();
        let block_number = client.block_number().await.unwrap();

        // it should be zero, since this is an ephemeral test node
        assert_eq!(block_number, U256::ZERO);

        // call the engine_forkchoiceUpdated function with payload attributes
        let forkchoice_state = ForkchoiceState {
            head_block_hash: genesis_hash,
            safe_block_hash: genesis_hash,
            finalized_block_hash: genesis_hash,
        };

        let payload_attributes = OptimismPayloadAttributes {
            payload_attributes: PayloadAttributes {
                timestamp: 1,
                prev_randao: Default::default(),
                suggested_fee_recipient: Default::default(),
                // canyon is _not_ in the chain spec, so this should cause the engine call to fail
                withdrawals: None,
                parent_beacon_block_root: None,
            },
            no_tx_pool: None,
            gas_limit: Some(1),
            transactions: None,
        };

        // call the engine_forkchoiceUpdated function with payload attributes
        let res = <HttpClient as EngineApiClient<OptimismEngineTypes>>::fork_choice_updated_v2(
            &client,
            forkchoice_state,
            Some(payload_attributes),
        )
        .await;
        let err = res.expect_err("post-canyon engine call with no withdrawals should fail");
        assert_matches!(err, Error::Call(ref object) if object.code() == INVALID_PARAMS_CODE);
    }

    #[cfg(feature = "optimism")]
    #[tokio::test]
    async fn optimism_post_canyon_withdrawals_valid() {
        reth_tracing::init_test_tracing();
        use alloy_chains::Chain;
        use jsonrpsee::http_client::HttpClient;
        use reth_primitives::{ChainSpec, Genesis};
        use reth_rpc_api::EngineApiClient;
        use reth_rpc_types::engine::{
            ForkchoiceState, OptimismPayloadAttributes, PayloadAttributes,
        };

        // this launches a test node with http
        let rpc_args = RpcServerArgs::default().with_http();

        // create optimism genesis with canyon at block 2
        let spec = ChainSpec::builder()
            .chain(Chain::optimism_mainnet())
            .genesis(Genesis::default())
            .canyon_activated()
            .build();

        let genesis_hash = spec.genesis_hash();

        // create node config
        let node_config = NodeConfig::test().with_rpc(rpc_args).with_chain(spec);

        let (handle, _manager) = spawn_node(node_config).await.unwrap();

        // call a function on the node
        let client = handle.rpc_server_handles().auth.http_client();
        let block_number = client.block_number().await.unwrap();

        // it should be zero, since this is an ephemeral test node
        assert_eq!(block_number, U256::ZERO);

        // call the engine_forkchoiceUpdated function with payload attributes
        let forkchoice_state = ForkchoiceState {
            head_block_hash: genesis_hash,
            safe_block_hash: genesis_hash,
            finalized_block_hash: genesis_hash,
        };

        let payload_attributes = OptimismPayloadAttributes {
            payload_attributes: PayloadAttributes {
                timestamp: 1,
                prev_randao: Default::default(),
                suggested_fee_recipient: Default::default(),
                // canyon is _not_ in the chain spec, so this should cause the engine call to fail
                withdrawals: Some(vec![]),
                parent_beacon_block_root: None,
            },
            no_tx_pool: None,
            gas_limit: Some(1),
            transactions: None,
        };

        // call the engine_forkchoiceUpdated function with payload attributes
        let res = <HttpClient as EngineApiClient<OptimismEngineTypes>>::fork_choice_updated_v2(
            &client,
            forkchoice_state,
            Some(payload_attributes),
        )
        .await;
        res.expect("post-canyon engine call with withdrawals should succeed");
    }
}
