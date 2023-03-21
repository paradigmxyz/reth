use std::pin::Pin;

use crate::p2p::{download::DownloadClient, error::PeerRequestResult, priority::Priority};
use futures::Future;
use reth_primitives::{BlockBody, H256};

/// The bodies future type
pub type BodiesFut = Pin<Box<dyn Future<Output = PeerRequestResult<Vec<BlockBody>>> + Send + Sync>>;

/// A client capable of downloading block bodies.
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait BodiesClient: DownloadClient {
    /// The bodies type
    type Output: Future<Output = PeerRequestResult<Vec<BlockBody>>> + Sync + Send + Unpin;

    /// Fetches the block body for the requested block.
    fn get_block_bodies(&self, hashes: Vec<H256>) -> Self::Output {
        self.get_block_bodies_with_priority(hashes, Priority::Normal)
    }

    /// Fetches the block body for the requested block with priority
    fn get_block_bodies_with_priority(&self, hashes: Vec<H256>, priority: Priority)
        -> Self::Output;
}
