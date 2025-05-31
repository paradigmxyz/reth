use clap::Args;
use reth_chainspec::NamedChain;

/// Hyperliquid-specific command line arguments
#[derive(Debug, Args)]
pub struct HyperliquidArgs {
    /// Use Hyperliquid mainnet
    #[arg(long, conflicts_with = "hyperliquid_testnet")]
    pub hyperliquid: bool,
    
    /// Use Hyperliquid testnet
    #[arg(long, conflicts_with = "hyperliquid")]
    pub hyperliquid_testnet: bool,
    
    /// Block data source configuration
    #[arg(long, default_value = "http")]
    pub data_source: String,
    
    /// HTTP endpoint for block data (when using http data source)
    #[arg(long, default_value = "https://api.hyperliquid.xyz")]
    pub http_endpoint: String,
    
    /// Local directory for block files (when using local data source)
    #[arg(long)]
    pub block_dir: Option<String>,
    
    /// P2P bootstrap nodes (when using p2p data source)
    #[arg(long)]
    pub bootstrap_nodes: Vec<String>,
}

impl HyperliquidArgs {
    pub fn chain(&self) -> Option<NamedChain> {
        if self.hyperliquid {
            Some(NamedChain::Hyperliquid)
        } else if self.hyperliquid_testnet {
            Some(NamedChain::HyperliquidTestnet)
        } else {
            None
        }
    }
    
    pub fn data_source_config(&self) -> crate::datasource::DataSourceConfig {
        match self.data_source.as_str() {
            "http" => crate::datasource::DataSourceConfig::Http {
                url: self.http_endpoint.clone(),
            },
            "local" => crate::datasource::DataSourceConfig::LocalFiles {
                path: self.block_dir.clone().unwrap_or_else(|| "./blocks".to_string()),
            },
            "p2p" => crate::datasource::DataSourceConfig::P2P {
                bootstrap_nodes: self.bootstrap_nodes.clone(),
            },
            _ => panic!("Invalid data source: {}", self.data_source),
        }
    }
}