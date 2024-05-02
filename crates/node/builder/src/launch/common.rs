//! Helper types that can be used by launchers.

use std::{cmp::max, sync::Arc, thread::available_parallelism};

use eyre::Context;
use rayon::ThreadPoolBuilder;
use tokio::sync::mpsc::Receiver;

use reth_auto_seal_consensus::MiningMode;
use reth_config::{config::EtlConfig, PruneConfig};
use reth_db::{database::Database, database_metrics::DatabaseMetrics};
use reth_interfaces::p2p::headers::client::HeadersClient;
use reth_node_core::{
    cli::config::RethRpcConfig,
    dirs::{ChainPath, DataDirPath},
    init::{init_genesis, InitDatabaseError},
    node_config::NodeConfig,
};
use reth_primitives::{BlockNumber, Chain, ChainSpec, Head, PruneModes, B256};
use reth_provider::{providers::StaticFileProvider, ProviderFactory, StaticFileProviderFactory};
use reth_prune::PrunerBuilder;
use reth_rpc::JwtSecret;
use reth_static_file::StaticFileProducer;
use reth_tasks::TaskExecutor;
use reth_tracing::tracing::{error, info, warn};

/// Reusable setup for launching a node.
///
/// This provides commonly used boilerplate for launching a node.
#[derive(Debug, Clone)]
pub struct LaunchContext {
    /// The task executor for the node.
    pub task_executor: TaskExecutor,
    /// The data directory for the node.
    pub data_dir: ChainPath<DataDirPath>,
}

impl LaunchContext {
    /// Create a new instance of the default node launcher.
    pub const fn new(task_executor: TaskExecutor, data_dir: ChainPath<DataDirPath>) -> Self {
        Self { task_executor, data_dir }
    }

    /// Attaches a database to the launch context.
    pub fn with<DB>(self, database: DB) -> LaunchContextWith<DB> {
        LaunchContextWith { inner: self, attachment: database }
    }

    /// Loads the reth config with the configured `data_dir` and overrides settings according to the
    /// `config`.
    ///
    /// Attaches both the `NodeConfig` and the loaded `reth.toml` config to the launch context.
    pub fn with_loaded_toml_config(
        self,
        config: NodeConfig,
    ) -> eyre::Result<LaunchContextWith<WithConfigs>> {
        let toml_config = self.load_toml_config(&config)?;
        Ok(self.with(WithConfigs { config, toml_config }))
    }

    /// Loads the reth config with the configured `data_dir` and overrides settings according to the
    /// `config`.
    pub fn load_toml_config(&self, config: &NodeConfig) -> eyre::Result<reth_config::Config> {
        let config_path = config.config.clone().unwrap_or_else(|| self.data_dir.config());

        let mut toml_config = confy::load_path::<reth_config::Config>(&config_path)
            .wrap_err_with(|| format!("Could not load config file {config_path:?}"))?;

        Self::save_pruning_config_if_full_node(&mut toml_config, config, &config_path)?;

        info!(target: "reth::cli", path = ?config_path, "Configuration loaded");

        // Update the config with the command line arguments
        toml_config.peers.trusted_nodes_only = config.network.trusted_only;

        if !config.network.trusted_peers.is_empty() {
            info!(target: "reth::cli", "Adding trusted nodes");
            config.network.trusted_peers.iter().for_each(|peer| {
                toml_config.peers.trusted_nodes.insert(*peer);
            });
        }

        Ok(toml_config)
    }

    /// Save prune config to the toml file if node is a full node.
    fn save_pruning_config_if_full_node(
        reth_config: &mut reth_config::Config,
        config: &NodeConfig,
        config_path: impl AsRef<std::path::Path>,
    ) -> eyre::Result<()> {
        if reth_config.prune.is_none() {
            if let Some(prune_config) = config.prune_config() {
                reth_config.update_prune_confing(prune_config);
                info!(target: "reth::cli", "Saving prune config to toml file");
                reth_config.save(config_path.as_ref())?;
            }
        } else if config.prune_config().is_none() {
            warn!(target: "reth::cli", "Prune configs present in config file but --full not provided. Running as a Full node");
        }
        Ok(())
    }

    /// Convenience function to [Self::configure_globals]
    pub fn with_configured_globals(self) -> Self {
        self.configure_globals();
        self
    }

    /// Configure global settings this includes:
    ///
    /// - Raising the file descriptor limit
    /// - Configuring the global rayon thread pool
    pub fn configure_globals(&self) {
        // Raise the fd limit of the process.
        // Does not do anything on windows.
        let _ = fdlimit::raise_fd_limit();

        // Limit the global rayon thread pool, reserving 2 cores for the rest of the system
        let _ = ThreadPoolBuilder::new()
            .num_threads(
                available_parallelism().map_or(25, |cpus| max(cpus.get().saturating_sub(2), 2)),
            )
            .build_global()
            .map_err(|e| error!("Failed to build global thread pool: {:?}", e));
    }
}

/// A [LaunchContext] along with an additional value.
///
/// This can be used to sequentially attach additional values to the type during the launch process.
///
/// The type provides common boilerplate for launching a node depending on the additional value.
#[derive(Debug, Clone)]
pub struct LaunchContextWith<T> {
    /// The wrapped launch context.
    pub inner: LaunchContext,
    /// The additional attached value.
    pub attachment: T,
}

impl<T> LaunchContextWith<T> {
    /// Configure global settings this includes:
    ///
    /// - Raising the file descriptor limit
    /// - Configuring the global rayon thread pool
    pub fn configure_globals(&self) {
        self.inner.configure_globals();
    }

    /// Returns the data directory.
    pub fn data_dir(&self) -> &ChainPath<DataDirPath> {
        &self.inner.data_dir
    }

    /// Returns the task executor.
    pub fn task_executor(&self) -> &TaskExecutor {
        &self.inner.task_executor
    }

    /// Attaches another value to the launch context.
    pub fn attach<A>(self, attachment: A) -> LaunchContextWith<Attached<T, A>> {
        LaunchContextWith {
            inner: self.inner,
            attachment: Attached::new(self.attachment, attachment),
        }
    }

    /// Consumes the type and calls a function with a reference to the context.
    // Returns the context again
    pub fn inspect<F>(self, f: F) -> Self
    where
        F: FnOnce(&Self),
    {
        f(&self);
        self
    }
}

impl<L, R> LaunchContextWith<Attached<L, R>> {
    /// Get a reference to the left value.
    pub const fn left(&self) -> &L {
        &self.attachment.left
    }

    /// Get a reference to the right value.
    pub const fn right(&self) -> &R {
        &self.attachment.right
    }

    /// Get a mutable reference to the right value.
    pub fn left_mut(&mut self) -> &mut L {
        &mut self.attachment.left
    }

    /// Get a mutable reference to the right value.
    pub fn right_mut(&mut self) -> &mut R {
        &mut self.attachment.right
    }
}
impl<R> LaunchContextWith<Attached<WithConfigs, R>> {
    /// Adjust certain settings in the config to make sure they are set correctly
    ///
    /// This includes:
    /// - Making sure the ETL dir is set to the datadir
    /// - RPC settings are adjusted to the correct port
    pub fn with_adjusted_configs(self) -> Self {
        self.ensure_etl_datadir().with_adjusted_rpc_instance_ports()
    }

    /// Make sure ETL doesn't default to /tmp/, but to whatever datadir is set to
    pub fn ensure_etl_datadir(mut self) -> Self {
        if self.toml_config_mut().stages.etl.dir.is_none() {
            self.toml_config_mut().stages.etl.dir =
                Some(EtlConfig::from_datadir(self.data_dir().data_dir()))
        }

        self
    }

    /// Change rpc port numbers based on the instance number.
    pub fn with_adjusted_rpc_instance_ports(mut self) -> Self {
        self.node_config_mut().adjust_instance_ports();
        self
    }

    /// Returns the attached [NodeConfig].
    pub const fn node_config(&self) -> &NodeConfig {
        &self.left().config
    }

    /// Returns the attached [NodeConfig].
    pub fn node_config_mut(&mut self) -> &mut NodeConfig {
        &mut self.left_mut().config
    }

    /// Returns the attached toml config [reth_config::Config].
    pub const fn toml_config(&self) -> &reth_config::Config {
        &self.left().toml_config
    }

    /// Returns the attached toml config [reth_config::Config].
    pub fn toml_config_mut(&mut self) -> &mut reth_config::Config {
        &mut self.left_mut().toml_config
    }

    /// Returns the configured chain spec.
    pub fn chain_spec(&self) -> Arc<ChainSpec> {
        self.node_config().chain.clone()
    }

    /// Get the hash of the genesis block.
    pub fn genesis_hash(&self) -> B256 {
        self.node_config().chain.genesis_hash()
    }

    /// Returns the chain identifier of the node.
    pub fn chain_id(&self) -> Chain {
        self.node_config().chain.chain
    }

    /// Returns true if the node is configured as --dev
    pub fn is_dev(&self) -> bool {
        self.node_config().dev.dev
    }

    /// Returns the configured [PruneConfig]
    pub fn prune_config(&self) -> Option<PruneConfig> {
        self.node_config().prune_config().or_else(|| self.toml_config().prune.clone())
    }

    /// Returns the configured [PruneModes]
    pub fn prune_modes(&self) -> Option<PruneModes> {
        self.prune_config().map(|config| config.segments)
    }

    /// Returns an initialized [PrunerBuilder] based on the configured [PruneConfig]
    pub fn pruner_builder(&self) -> PrunerBuilder {
        PrunerBuilder::new(self.prune_config().unwrap_or_default())
            .prune_delete_limit(self.chain_spec().prune_delete_limit)
            .timeout(PrunerBuilder::DEFAULT_TIMEOUT)
    }

    /// Returns the initial pipeline target, based on whether or not the node is running in
    /// `debug.tip` mode, `debug.continuous` mode, or neither.
    ///
    /// If running in `debug.tip` mode, the configured tip is returned.
    /// Otherwise, if running in `debug.continuous` mode, the genesis hash is returned.
    /// Otherwise, `None` is returned. This is what the node will do by default.
    pub fn initial_pipeline_target(&self) -> Option<B256> {
        self.node_config().initial_pipeline_target(self.genesis_hash())
    }

    /// Loads the JWT secret for the engine API
    pub fn auth_jwt_secret(&self) -> eyre::Result<JwtSecret> {
        let default_jwt_path = self.data_dir().jwt();
        let secret = self.node_config().rpc.auth_jwt_secret(default_jwt_path)?;
        Ok(secret)
    }

    /// Returns the [MiningMode] intended for --dev mode.
    pub fn dev_mining_mode(&self, pending_transactions_listener: Receiver<B256>) -> MiningMode {
        if let Some(interval) = self.node_config().dev.block_time {
            MiningMode::interval(interval)
        } else if let Some(max_transactions) = self.node_config().dev.block_max_transactions {
            MiningMode::instant(max_transactions, pending_transactions_listener)
        } else {
            MiningMode::instant(1, pending_transactions_listener)
        }
    }
}

impl<DB> LaunchContextWith<Attached<WithConfigs, DB>>
where
    DB: Clone,
{
    /// Returns the [ProviderFactory] for the attached database.
    pub fn create_provider_factory(&self) -> eyre::Result<ProviderFactory<DB>> {
        let factory = ProviderFactory::new(
            self.right().clone(),
            self.chain_spec(),
            self.data_dir().static_files(),
        )?
        .with_static_files_metrics();

        Ok(factory)
    }

    /// Creates a new [ProviderFactory] and attaches it to the launch context.
    pub fn with_provider_factory(
        self,
    ) -> eyre::Result<LaunchContextWith<Attached<WithConfigs, ProviderFactory<DB>>>> {
        let factory = self.create_provider_factory()?;
        let ctx = LaunchContextWith {
            inner: self.inner,
            attachment: self.attachment.map_right(|_| factory),
        };

        Ok(ctx)
    }
}

impl<DB> LaunchContextWith<Attached<WithConfigs, ProviderFactory<DB>>>
where
    DB: Database + DatabaseMetrics + Send + Sync + Clone + 'static,
{
    /// Returns access to the underlying database.
    pub fn database(&self) -> &DB {
        self.right().db_ref()
    }

    /// Returns the configured ProviderFactory.
    pub fn provider_factory(&self) -> &ProviderFactory<DB> {
        self.right()
    }

    /// Returns the static file provider to interact with the static files.
    pub fn static_file_provider(&self) -> StaticFileProvider {
        self.right().static_file_provider()
    }

    /// Creates a new [StaticFileProducer] with the attached database.
    pub fn static_file_producer(&self) -> StaticFileProducer<DB> {
        StaticFileProducer::new(
            self.provider_factory().clone(),
            self.static_file_provider(),
            self.prune_modes().unwrap_or_default(),
        )
    }

    /// Convenience function to [Self::init_genesis]
    pub fn with_genesis(self) -> Result<Self, InitDatabaseError> {
        init_genesis(self.provider_factory().clone())?;
        Ok(self)
    }

    /// Write the genesis block and state if it has not already been written
    pub fn init_genesis(&self) -> Result<B256, InitDatabaseError> {
        init_genesis(self.provider_factory().clone())
    }

    /// Returns the max block that the node should run to, looking it up from the network if
    /// necessary
    pub async fn max_block<C>(&self, client: C) -> eyre::Result<Option<BlockNumber>>
    where
        C: HeadersClient,
    {
        self.node_config().max_block(client, self.provider_factory().clone()).await
    }

    /// Convenience function to [Self::start_prometheus_endpoint]
    pub async fn with_prometheus(self) -> eyre::Result<Self> {
        self.start_prometheus_endpoint().await?;
        Ok(self)
    }

    /// Starts the prometheus endpoint.
    pub async fn start_prometheus_endpoint(&self) -> eyre::Result<()> {
        let prometheus_handle = self.node_config().install_prometheus_recorder()?;
        self.node_config()
            .start_metrics_endpoint(
                prometheus_handle,
                self.database().clone(),
                self.static_file_provider(),
                self.task_executor().clone(),
            )
            .await
    }

    /// Fetches the head block from the database.
    ///
    /// If the database is empty, returns the genesis block.
    pub fn lookup_head(&self) -> eyre::Result<Head> {
        self.node_config()
            .lookup_head(self.provider_factory().clone())
            .wrap_err("the head block is missing")
    }
}

/// Joins two attachments together.
#[derive(Clone, Copy, Debug)]
pub struct Attached<L, R> {
    left: L,
    right: R,
}

impl<L, R> Attached<L, R> {
    /// Creates a new `Attached` with the given values.
    pub const fn new(left: L, right: R) -> Self {
        Self { left, right }
    }

    /// Maps the left value to a new value.
    pub fn map_left<F, T>(self, f: F) -> Attached<T, R>
    where
        F: FnOnce(L) -> T,
    {
        Attached::new(f(self.left), self.right)
    }

    /// Maps the right value to a new value.
    pub fn map_right<F, T>(self, f: F) -> Attached<L, T>
    where
        F: FnOnce(R) -> T,
    {
        Attached::new(self.left, f(self.right))
    }

    /// Get a reference to the left value.
    pub const fn left(&self) -> &L {
        &self.left
    }

    /// Get a reference to the right value.
    pub const fn right(&self) -> &R {
        &self.right
    }

    /// Get a mutable reference to the right value.
    pub fn left_mut(&mut self) -> &mut R {
        &mut self.right
    }

    /// Get a mutable reference to the right value.
    pub fn right_mut(&mut self) -> &mut R {
        &mut self.right
    }
}

/// Helper container type to bundle the initial [NodeConfig] and the loaded settings from the
/// reth.toml config
#[derive(Debug, Clone)]
pub struct WithConfigs {
    /// The configured, usually derived from the CLI.
    pub config: NodeConfig,
    /// The loaded reth.toml config.
    pub toml_config: reth_config::Config,
}

#[cfg(test)]
mod tests {
    use super::{LaunchContext, NodeConfig};
    use reth_config::Config;
    use reth_node_core::args::PruningArgs;

    const EXTENSION: &str = "toml";

    fn with_tempdir(filename: &str, proc: fn(&std::path::Path)) {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join(filename).with_extension(EXTENSION);
        proc(&config_path);
        temp_dir.close().unwrap()
    }

    #[test]
    fn test_save_prune_config() {
        with_tempdir("prune-store-test", |config_path| {
            let mut reth_config = Config::default();
            let node_config =
                NodeConfig { pruning: PruningArgs { full: true }, ..NodeConfig::test() };
            LaunchContext::save_pruning_config_if_full_node(
                &mut reth_config,
                &node_config,
                config_path,
            )
            .unwrap();

            assert_eq!(
                reth_config.prune.as_ref().map(|p| p.block_interval),
                node_config.prune_config().map(|p| p.block_interval)
            );

            let loaded_config: Config = confy::load_path(config_path).unwrap();
            assert_eq!(reth_config, loaded_config);
        })
    }
}
