use reth_eth_wire::BlockBody;
use reth_primitives::H256;

use async_trait::async_trait;
use std::fmt::Debug;
use thiserror::Error;

/// Body client errors.
#[derive(Error, Debug)]
pub enum BodiesClientError {
    /// Timed out while waiting for a response.
    #[error("Timed out while getting bodies for block {header_hash}.")]
    Timeout {
        /// The header hash of the block that timed out.
        header_hash: H256,
    },
    /// The client encountered an internal error.
    #[error(transparent)]
    Internal(Box<dyn std::error::Error + Send + Sync>),
}

/// The block bodies downloader client
#[async_trait]
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait BodiesClient: Send + Sync + Debug {
    /// Fetches the block body for the requested block.
    async fn get_block_body(&self, hash: H256) -> Result<BlockBody, BodiesClientError>;
}
