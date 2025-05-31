pub mod http;
pub mod p2p;

use reth_primitives::{Block, BlockNumber};
use std::future::Future;

/// Trait for block data sources
pub trait BlockDataSource: Send + Sync {
    /// Fetch block by number
    fn fetch_block(&self, block_number: BlockNumber) -> impl Future<Output = Result<Block, Box<dyn std::error::Error>>> + Send;
    
    /// Get latest block number
    fn get_latest_block_number(&self) -> impl Future<Output = Result<BlockNumber, Box<dyn std::error::Error>>> + Send;
}

/// Factory for creating block data sources
pub enum DataSourceConfig {
    Http { url: String },
    P2P { bootstrap_nodes: Vec<String> },
    LocalFiles { path: String },
}

impl DataSourceConfig {
    pub fn create_data_source(&self) -> Box<dyn BlockDataSource> {
        match self {
            DataSourceConfig::Http { url } => {
                Box::new(http::HttpBlockDataSource::new(url.clone()))
            }
            DataSourceConfig::P2P { bootstrap_nodes } => {
                Box::new(p2p::P2PBlockDataSource::new(bootstrap_nodes.clone()))
            }
            DataSourceConfig::LocalFiles { path } => {
                Box::new(local::LocalFileDataSource::new(path.clone()))
            }
        }
    }
}