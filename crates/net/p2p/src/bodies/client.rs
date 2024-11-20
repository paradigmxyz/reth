use std::{
    pin::Pin,
    task::{ready, Context, Poll},
};

use crate::{download::DownloadClient, error::PeerRequestResult, priority::Priority};
use alloy_primitives::B256;
use futures::{Future, FutureExt};
use reth_primitives::BlockBody;

/// The bodies future type
pub type BodiesFut<B = BlockBody> =
    Pin<Box<dyn Future<Output = PeerRequestResult<Vec<B>>> + Send + Sync>>;

/// A client capable of downloading block bodies.
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait BodiesClient: DownloadClient {
    /// The body type this client fetches.
    type Body: Send + Sync + Unpin + 'static;
    /// The output of the request future for querying block bodies.
    type Output: Future<Output = PeerRequestResult<Vec<Self::Body>>> + Sync + Send + Unpin;

    /// Fetches the block body for the requested block.
    fn get_block_bodies(&self, hashes: Vec<B256>) -> Self::Output {
        self.get_block_bodies_with_priority(hashes, Priority::Normal)
    }

    /// Fetches the block body for the requested block with priority
    fn get_block_bodies_with_priority(&self, hashes: Vec<B256>, priority: Priority)
        -> Self::Output;

    /// Fetches a single block body for the requested hash.
    fn get_block_body(&self, hash: B256) -> SingleBodyRequest<Self::Output> {
        self.get_block_body_with_priority(hash, Priority::Normal)
    }

    /// Fetches a single block body for the requested hash with priority
    fn get_block_body_with_priority(
        &self,
        hash: B256,
        priority: Priority,
    ) -> SingleBodyRequest<Self::Output> {
        let fut = self.get_block_bodies_with_priority(vec![hash], priority);
        SingleBodyRequest { fut }
    }
}

/// A Future that resolves to a single block body.
#[derive(Debug)]
#[must_use = "futures do nothing unless polled"]
pub struct SingleBodyRequest<Fut> {
    fut: Fut,
}

impl<Fut, B> Future for SingleBodyRequest<Fut>
where
    Fut: Future<Output = PeerRequestResult<Vec<B>>> + Sync + Send + Unpin,
{
    type Output = PeerRequestResult<Option<B>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let resp = ready!(self.get_mut().fut.poll_unpin(cx));
        let resp = resp.map(|res| res.map(|bodies| bodies.into_iter().next()));
        Poll::Ready(resp)
    }
}
