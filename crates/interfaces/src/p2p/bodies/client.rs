use reth_eth_wire::BlockBody;
use reth_primitives::H256;

use crate::p2p::bodies::error::BodiesClientError;
use async_trait::async_trait;
use std::fmt::Debug;

/// A client capable of downloading block bodies.
#[async_trait]
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait BodiesClient: Send + Sync + Debug {
    /// Fetches the block body for the requested block.
    async fn get_block_body(&self, hash: H256) -> Result<BlockBody, BodiesClientError>;
}
