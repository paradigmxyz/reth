//! Helper types that can be used by launchers.

use std::cmp::max;
use std::thread::available_parallelism;
use eyre::Context;
use rayon::ThreadPoolBuilder;
use reth_node_core::dirs::{ChainPath, DataDirPath};
use reth_node_core::node_config::NodeConfig;
use reth_tasks::TaskExecutor;
use reth_tracing::tracing::{error, info};

/// Reusable setup for launching a node.
#[derive(Debug)]
pub struct LaunchContext {
    /// The task executor for the node.
    pub task_executor: TaskExecutor,
    /// The data directory for the node.
    pub data_dir: ChainPath<DataDirPath>,
}

impl LaunchContext {

    /// Create a new instance of the default node launcher.
    pub fn new(task_executor: TaskExecutor, data_dir: ChainPath<DataDirPath>) -> Self {
        Self { task_executor, data_dir }
    }

    /// Loads the reth config with the configured `data_dir` and overrides settings according to the `config`.
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

    /// Configure global settings
     pub fn configure_globals(&self) {
         // Raise the fd limit of the process.
         // Does not do anything on windows.
         fdlimit::raise_fd_limit()?;

         // Limit the global rayon thread pool, reserving 2 cores for the rest of the system
         let _ = ThreadPoolBuilder::new()
             .num_threads(
                 available_parallelism().map_or(25, |cpus| max(cpus.get().saturating_sub(2), 2)),
             )
             .build_global()
             .map_err(|e| error!("Failed to build global thread pool: {:?}", e));
     }


}