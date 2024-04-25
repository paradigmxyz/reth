//! Helper types that can be used by launchers.

use eyre::Context;
use rayon::ThreadPoolBuilder;
use reth_config::PruneConfig;
use reth_db::{database::Database, database_metrics::DatabaseMetrics};
use reth_node_core::{
    cli::config::RethRpcConfig,
    dirs::{ChainPath, DataDirPath},
    node_config::NodeConfig,
};
use reth_primitives::{Chain, ChainSpec, Head, B256};
use reth_provider::{providers::StaticFileProvider, ProviderFactory};
use reth_rpc::JwtSecret;
use reth_tasks::TaskExecutor;
use reth_tracing::tracing::{error, info};
use std::{cmp::max, sync::Arc, thread::available_parallelism};

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
        let config_path = config.config.clone().unwrap_or_else(|| self.data_dir.config_path());

        let mut toml_config = confy::load_path::<reth_config::Config>(&config_path)
            .wrap_err_with(|| format!("Could not load config file {config_path:?}"))?;

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
    pub fn prune_config(&self) -> eyre::Result<Option<PruneConfig>> {
        Ok(self.node_config().prune_config()?.or_else(|| self.toml_config().prune.clone()))
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
        let default_jwt_path = self.data_dir().jwt_path();
        let secret = self.node_config().rpc.auth_jwt_secret(default_jwt_path)?;
        Ok(secret)
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
            self.data_dir().static_files_path(),
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
