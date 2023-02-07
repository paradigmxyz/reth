use crate::p2p::{download::DownloadClient, error::PeerRequestResult, priority::Priority};
use futures::Future;
pub use reth_eth_wire::BlockHeaders;
use reth_primitives::{BlockHashOrNumber, Head, Header, HeadersDirection};
use std::{fmt::Debug, pin::Pin};

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

/// The headers future type
pub type HeadersFut = Pin<Box<dyn Future<Output = PeerRequestResult<Vec<Header>>> + Send + Sync>>;

/// The block headers downloader client
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait HeadersClient: DownloadClient {
    /// The headers type
    type Output: Future<Output = PeerRequestResult<Vec<Header>>> + Sync + Send + Unpin;

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
}

/// The status updater for updating the status of the p2p node
pub trait StatusUpdater: Send + Sync {
    /// Updates the status of the p2p node
    fn update_status(&self, head: Head);
}
