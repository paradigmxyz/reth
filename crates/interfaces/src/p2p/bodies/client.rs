use std::pin::Pin;

use crate::p2p::{download::DownloadClient, error::PeerRequestResult, priority::Priority};
use futures::Future;
use reth_primitives::H256;
use tokio::sync::oneshot::error::RecvError;

/// The bodies future type
pub type BodiesFuture<T> =
    Pin<Box<dyn Future<Output = Result<PeerRequestResult<T>, RecvError>> + Send + Sync>>;

/// A client capable of downloading block bodies.
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait BodiesClient: DownloadClient {
    /// The bodies type
    type Output;

    /// Fetches the block body for the requested block.
    fn get_block_bodies(&self, hashes: Vec<H256>) -> BodiesFuture<Self::Output> {
        self.get_block_bodies_with_priority(hashes, Priority::Normal)
    }

    /// Fetches the block body for the requested block with priority
    fn get_block_bodies_with_priority(
        &self,
        hashes: Vec<H256>,
        priority: Priority,
    ) -> BodiesFuture<Self::Output>;
}
