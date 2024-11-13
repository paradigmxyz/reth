use crate::{download::DownloadClient, error::PeerRequestResult, priority::Priority};
use alloy_consensus::Header;
use alloy_eips::BlockHashOrNumber;
use futures::{Future, FutureExt};
pub use reth_eth_wire_types::{BlockHeaders, HeadersDirection};
use std::{
    fmt::Debug,
    pin::Pin,
    task::{ready, Context, Poll},
};

/// The header request struct to be sent to connected peers, which
/// will proceed to ask them to stream the requested headers to us.
#[derive(Clone, Debug)]
pub struct HeadersRequest {
    /// The starting block
    pub start: BlockHashOrNumber,
    /// The response max size
    pub limit: u64,
    /// The direction in which headers should be returned.
    pub direction: HeadersDirection,
}

impl HeadersRequest {
    /// Creates a request for a single header (direction doesn't matter).
    ///
    /// # Arguments
    /// * `start` - The block hash or number to start from
    pub const fn one(start: BlockHashOrNumber) -> Self {
        Self { direction: HeadersDirection::Rising, limit: 1, start }
    }

    /// Creates a request for headers in rising direction (ascending block numbers).
    ///
    /// # Arguments
    /// * `start` - The block hash or number to start from
    /// * `limit` - Maximum number of headers to retrieve
    pub const fn rising(start: BlockHashOrNumber, limit: u64) -> Self {
        Self { direction: HeadersDirection::Rising, limit, start }
    }

    /// Creates a request for headers in falling direction (descending block numbers).
    ///
    /// # Arguments
    /// * `start` - The block hash or number to start from
    /// * `limit` - Maximum number of headers to retrieve
    pub const fn falling(start: BlockHashOrNumber, limit: u64) -> Self {
        Self { direction: HeadersDirection::Falling, limit, start }
    }
}

/// The headers future type
pub type HeadersFut = Pin<Box<dyn Future<Output = PeerRequestResult<Vec<Header>>> + Send + Sync>>;

/// The block headers downloader client
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait HeadersClient: DownloadClient {
    /// The header type this client fetches.
    type Header: Send + Sync + Unpin;
    /// The headers future type
    type Output: Future<Output = PeerRequestResult<Vec<Self::Header>>> + Sync + Send + Unpin;

    /// Sends the header request to the p2p network and returns the header response received from a
    /// peer.
    fn get_headers(&self, request: HeadersRequest) -> Self::Output {
        self.get_headers_with_priority(request, Priority::Normal)
    }

    /// Sends the header request to the p2p network with priority set and returns the header
    /// response received from a peer.
    fn get_headers_with_priority(
        &self,
        request: HeadersRequest,
        priority: Priority,
    ) -> Self::Output;

    /// Fetches a single header for the requested number or hash.
    fn get_header(&self, start: BlockHashOrNumber) -> SingleHeaderRequest<Self::Output> {
        self.get_header_with_priority(start, Priority::Normal)
    }

    /// Fetches a single header for the requested number or hash with priority
    fn get_header_with_priority(
        &self,
        start: BlockHashOrNumber,
        priority: Priority,
    ) -> SingleHeaderRequest<Self::Output> {
        let req = HeadersRequest::one(start);
        let fut = self.get_headers_with_priority(req, priority);
        SingleHeaderRequest { fut }
    }
}

/// A Future that resolves to a single block body.
#[derive(Debug)]
#[must_use = "futures do nothing unless polled"]
pub struct SingleHeaderRequest<Fut> {
    fut: Fut,
}

impl<Fut, H> Future for SingleHeaderRequest<Fut>
where
    Fut: Future<Output = PeerRequestResult<Vec<H>>> + Sync + Send + Unpin,
{
    type Output = PeerRequestResult<Option<H>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let resp = ready!(self.get_mut().fut.poll_unpin(cx));
        let resp = resp.map(|res| res.map(|headers| headers.into_iter().next()));
        Poll::Ready(resp)
    }
}
