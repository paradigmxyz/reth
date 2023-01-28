//! Configuration files.
use reth_consensus::BeaconConsensus;
use reth_db::database::Database;
use reth_downloaders::{
    bodies, bodies::concurrent::ConcurrentDownloaderBuilder, headers,
    headers::linear::LinearDownloadBuilder,
};
use reth_interfaces::consensus::Consensus;
use reth_network::{NetworkConfigBuilder, NetworkHandle, NetworkManager};
use reth_primitives::{ChainSpec, NodeRecord, MAINNET};
use reth_provider::ShareableDatabase;
use reth_stages::{sets::DefaultStages, Pipeline};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub struct NodeHandle {
    network_handle: NetworkHandle,
}

// todo: shutdown
// todo: wait
impl NodeHandle {
    pub fn network_handle(&self) -> NetworkHandle {
        self.network_handle.clone()
    }
}

#[derive(Default)]
#[must_use]
pub struct NodeBuilder {
    config: Config,
}

impl NodeBuilder {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    pub fn modify_config<F>(mut self, mut f: F) -> Self
    where
        F: FnMut(&mut Config),
    {
        f(&mut self.config);
        self
    }

    pub fn modify_network_config<F>(mut self, mut f: F) -> Self
    where
        F: FnMut(&mut NetworkConfigBuilder),
    {
        f(&mut self.config.network);
        self
    }

    pub async fn run<DB: Database + 'static>(self, db: Arc<DB>) -> NodeHandle {
        // Build consensus
        // TODO: Consensus engine factory
        let consensus: Arc<dyn Consensus> = Arc::new(BeaconConsensus::new(MAINNET.clone()));

        // Build network
        let Config { network, downloaders } = self.config;
        let network_handle = network
            .build(Arc::new(ShareableDatabase::new(db.clone())))
            .start_network()
            .await
            .unwrap();
        let fetch_client = Arc::new(network_handle.fetch_client().await.unwrap());

        // Build downloaders
        let Downloaders { headers, bodies } = downloaders;
        let headers_downloader = headers::task::TaskDownloader::spawn(
            headers.build(consensus.clone(), fetch_client.clone()),
        );
        let bodies_downloader = bodies::task::TaskDownloader::spawn(bodies.build(
            fetch_client.clone(),
            consensus.clone(),
            db.clone(),
        ));

        // Build pipeline
        let pipeline: Pipeline<DB, _> = Pipeline::builder()
            .with_sync_state_updater(network_handle.clone())
            .add_stages(DefaultStages::new(
                consensus.clone(),
                headers_downloader,
                bodies_downloader,
            ))
            .build();

        NodeHandle { network_handle }
    }
}

/// Configuration for the reth node.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    pub network: NetworkConfigBuilder,
    pub downloaders: Downloaders,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Downloaders {
    pub headers: LinearDownloadBuilder,
    pub bodies: ConcurrentDownloaderBuilder,
}

#[cfg(test)]
mod tests {
    use super::Config;
    #[test]
    fn can_serde_config() {
        let _: Config = confy::load("test", None).unwrap();
    }
}
