//! Configuration files.
use std::sync::Arc;

use reth_db::database::Database;
use reth_network::{
    config::{mainnet_nodes, rng_secret_key},
    NetworkConfig, PeersConfig,
};
use reth_primitives::{ChainSpec, NodeRecord};
use reth_provider::ProviderImpl;
use serde::{Deserialize, Serialize};

/// Configuration for the reth node.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    /// Configuration for each stage in the pipeline.
    // TODO(onbjerg): Can we make this easier to maintain when we add/remove stages?
    pub stages: StageConfig,
    /// Configuration for the discovery service.
    pub peers: PeersConfig,
}

impl Config {
    /// Initializes network config from read data
    pub fn network_config<DB: Database>(
        &self,
        db: Arc<DB>,
        chain_spec: ChainSpec,
        disable_discovery: bool,
        bootnodes: Option<Vec<NodeRecord>>,
    ) -> NetworkConfig<ProviderImpl<DB>> {
        let peer_config = reth_network::PeersConfig::default()
            .with_trusted_nodes(self.peers.trusted_nodes.clone())
            .with_connect_trusted_nodes_only(self.peers.connect_trusted_nodes_only);
        NetworkConfig::builder(Arc::new(ProviderImpl::new(db)), rng_secret_key())
            .boot_nodes(bootnodes.unwrap_or_else(mainnet_nodes))
            .peer_config(peer_config)
            .chain_spec(chain_spec)
            .set_discovery(disable_discovery)
            .build()
    }
}

/// Configuration for each stage in the pipeline.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct StageConfig {
    /// Header stage configuration.
    pub headers: HeadersConfig,
    /// Total difficulty stage configuration
    pub total_difficulty: TotalDifficultyConfig,
    /// Body stage configuration.
    pub bodies: BodiesConfig,
    /// Sender recovery stage configuration.
    pub sender_recovery: SenderRecoveryConfig,
    /// Execution stage configuration.
    pub execution: ExecutionConfig,
}

/// Header stage configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HeadersConfig {
    /// The maximum number of headers to download before committing progress to the database.
    pub commit_threshold: u64,
    /// The maximum number of headers to request from a peer at a time.
    pub downloader_batch_size: u64,
    /// The number of times to retry downloading a set of headers.
    pub downloader_retries: usize,
}

impl Default for HeadersConfig {
    fn default() -> Self {
        Self { commit_threshold: 10_000, downloader_batch_size: 1000, downloader_retries: 5 }
    }
}

/// Total difficulty stage configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TotalDifficultyConfig {
    /// The maximum number of total difficulty entries to sum up before committing progress to the
    /// database.
    pub commit_threshold: u64,
}

impl Default for TotalDifficultyConfig {
    fn default() -> Self {
        Self { commit_threshold: 100_000 }
    }
}

/// Body stage configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BodiesConfig {
    /// The maximum number of bodies to download before committing progress to the database.
    pub commit_threshold: u64,
    /// The maximum number of bodies to request from a peer at a time.
    pub downloader_batch_size: usize,
    /// The number of times to retry downloading a set of bodies.
    pub downloader_retries: usize,
    /// The maximum number of body requests to have in flight at a time.
    ///
    /// The maximum number of bodies downloaded at the same time is `downloader_batch_size *
    /// downloader_concurrency`.
    pub downloader_concurrency: usize,
}

impl Default for BodiesConfig {
    fn default() -> Self {
        Self {
            commit_threshold: 5_000,
            downloader_batch_size: 100,
            downloader_retries: 5,
            downloader_concurrency: 10,
        }
    }
}

/// Sender recovery stage configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SenderRecoveryConfig {
    /// The maximum number of blocks to process before committing progress to the database.
    pub commit_threshold: u64,
    /// The maximum number of transactions to recover senders for concurrently.
    pub batch_size: usize,
}

impl Default for SenderRecoveryConfig {
    fn default() -> Self {
        Self { commit_threshold: 5_000, batch_size: 1000 }
    }
}

/// Execution stage configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ExecutionConfig {
    /// The maximum number of blocks to execution before committing progress to the database.
    pub commit_threshold: u64,
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self { commit_threshold: 5_000 }
    }
}

#[cfg(test)]
mod tests {
    use super::Config;
    #[test]
    fn can_serde_config() {
        let _: Config = confy::load("test", None).unwrap();
    }
}
