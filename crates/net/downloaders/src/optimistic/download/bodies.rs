use futures::Future;
use reth_eth_wire::BlockBody;
use reth_interfaces::p2p::error::PeerRequestResult;
use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

type BodiesFut = Pin<Box<dyn Future<Output = PeerRequestResult<Vec<BlockBody>>> + Send>>;

/// TODO:
pub(crate) struct BodiesDownload<B, C> {
    client: Arc<B>,
    consensus: Arc<C>,
    fut: Option<BodiesFut>,
}

impl<H, C> std::fmt::Debug for BodiesDownload<H, C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BodiesDownload").finish()
    }
}

impl<B, C> BodiesDownload<B, C> {
    /// Create new instance of [BodiesDownload].
    pub(crate) fn new(client: Arc<B>, consensus: Arc<C>) -> Self {
        Self { client, consensus, fut: None }
    }

    /// Clear a pending future.
    pub(crate) fn clear(&mut self) {
        self.fut = None;
    }
}

impl<B, C> Future for BodiesDownload<B, C> {
    type Output = Vec<BlockBody>;

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        // TODO:
        Poll::Pending
    }
}
