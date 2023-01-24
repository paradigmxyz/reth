use super::request::BodiesRequestFuture;
use futures::{stream::FuturesUnordered, Stream};
use futures_util::StreamExt;
use reth_interfaces::{
    consensus::Consensus as ConsensusTrait,
    p2p::bodies::{client::BodiesClient, response::BlockResponse},
};
use reth_primitives::{BlockNumber, SealedHeader};
use std::{
    collections::HashSet,
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
};

/// The wrapper around [FuturesUnordered] that keeps information
/// about the blocks currently being requested.
#[derive(Debug)]
pub(crate) struct BodiesRequestQueue<B, C> {
    /// Inner body request queue.
    inner: FuturesUnordered<BodiesRequestFuture<B, C>>,
    /// The block numbers being requested.
    block_numbers: HashSet<BlockNumber>,
    /// Last requested block number.
    pub(crate) last_requested_block_number: Option<BlockNumber>,
}

impl<B, C> Default for BodiesRequestQueue<B, C> {
    fn default() -> Self {
        Self {
            inner: Default::default(),
            block_numbers: Default::default(),
            last_requested_block_number: None,
        }
    }
}

impl<B, C> BodiesRequestQueue<B, C>
where
    B: BodiesClient + 'static,
    C: ConsensusTrait + 'static,
{
    pub(crate) fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub(crate) fn len(&self) -> usize {
        self.inner.len()
    }

    /// Add new request to the queue.
    /// Expects sorted collection of headers.
    pub(crate) fn push_new_request(
        &mut self,
        client: Arc<B>,
        consensus: Arc<C>,
        request: Vec<SealedHeader>,
    ) {
        // Set last max requested block number
        self.last_requested_block_number = request
            .last()
            .map(|last| match self.last_requested_block_number {
                Some(num) => last.number.max(num),
                None => last.number,
            })
            .or(self.last_requested_block_number);

        // Set requested block numbers
        self.block_numbers.extend(request.iter().map(|h| h.number));

        // Create request and push into the queue.
        self.inner.push(BodiesRequestFuture::new(client, consensus).with_headers(request))
    }

    /// Check if the block number is currently in progress
    pub(crate) fn contains_block(&self, number: BlockNumber) -> bool {
        self.block_numbers.contains(&number)
    }
}

impl<B, C> Stream for BodiesRequestQueue<B, C>
where
    B: BodiesClient + 'static,
    C: ConsensusTrait + 'static,
{
    type Item = Vec<BlockResponse>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        let result = ready!(this.inner.poll_next_unpin(cx));
        if let Some(ref response) = result {
            response.iter().for_each(|block| {
                this.block_numbers.remove(&block.block_number());
            });
        }
        Poll::Ready(result)
    }
}
