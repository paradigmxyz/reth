//! Configuration files.
use reth_discv4::Discv4Config;
use reth_network::{NetworkConfigBuilder, PeersConfig, SessionsConfig};
use reth_stages::{PruneConfig, StageConfig};
use secp256k1::SecretKey;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Configuration for the reth node.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct Config {
    /// Configuration for each stage in the pipeline.
    // TODO(onbjerg): Can we make this easier to maintain when we add/remove stages?
    pub stages: StageConfig,
    /// Configuration for stage pruning.
    pub prune: PruneConfig,
    /// Configuration for the discovery service.
    pub peers: PeersConfig,
    /// Configuration for peer sessions.
    pub sessions: SessionsConfig,
}

impl Config {
    /// Initializes network config from read data
    pub fn network_config(
        &self,
        nat_resolution_method: reth_net_nat::NatResolver,
        peers_file: Option<PathBuf>,
        secret_key: SecretKey,
    ) -> NetworkConfigBuilder {
        let peer_config = self
            .peers
            .clone()
            .with_basic_nodes_from_file(peers_file)
            .unwrap_or_else(|_| self.peers.clone());

        let discv4 =
            Discv4Config::builder().external_ip_resolver(Some(nat_resolution_method)).clone();
        NetworkConfigBuilder::new(secret_key)
            .sessions_config(self.sessions.clone())
            .peer_config(peer_config)
            .discovery(discv4)
    }
}

#[cfg(test)]
mod tests {
    use super::Config;

    const EXTENSION: &str = "toml";

    fn with_tempdir(filename: &str, proc: fn(&std::path::Path)) {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join(filename).with_extension(EXTENSION);

        proc(&config_path);

        temp_dir.close().unwrap()
    }

    #[test]
    fn test_store_config() {
        with_tempdir("config-store-test", |config_path| {
            let config = Config::default();
            confy::store_path(config_path, config).unwrap();
        })
    }

    #[test]
    fn test_load_config() {
        with_tempdir("config-load-test", |config_path| {
            let config = Config::default();
            confy::store_path(config_path, &config).unwrap();

            let loaded_config: Config = confy::load_path(config_path).unwrap();
            assert_eq!(config, loaded_config);
        })
    }
}
