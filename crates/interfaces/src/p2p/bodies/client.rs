use crate::p2p::{downloader::DownloadClient, error::PeerRequestResult, priority::Priority};
use async_trait::async_trait;
use reth_eth_wire::BlockBody;
use reth_primitives::H256;

/// A client capable of downloading block bodies.
#[async_trait]
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait BodiesClient: DownloadClient {
    /// Fetches the block body for the requested block.
    async fn get_block_bodies(&self, hashes: Vec<H256>) -> PeerRequestResult<Vec<BlockBody>> {
        self.get_block_bodies_with_priority(hashes, Priority::Normal).await
    }

    /// Fetches the block body for the requested block with priority
    async fn get_block_bodies_with_priority(
        &self,
        hashes: Vec<H256>,
        priority: Priority,
    ) -> PeerRequestResult<Vec<BlockBody>>;
}
