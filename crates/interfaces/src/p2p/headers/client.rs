use crate::p2p::MessageStream;

use reth_primitives::{rpc::BlockId, Header, H256, H512};

use async_trait::async_trait;
use std::{collections::HashSet, fmt::Debug};

/// Each peer returns a list of headers and the request id corresponding
/// to these headers. This allows clients to make multiple requests in parallel
/// and multiplex the responses accordingly.
pub type HeadersStream = MessageStream<HeadersResponse>;

/// The item contained in each [`MessageStream`] when used to fetch [`Header`]s via
/// [`HeadersClient`].
#[derive(Clone, Debug)]
pub struct HeadersResponse {
    /// The request id associated with this response.
    pub id: u64,
    /// The headers the peer replied with.
    pub headers: Vec<Header>,
}

impl From<(u64, Vec<Header>)> for HeadersResponse {
    fn from((id, headers): (u64, Vec<Header>)) -> Self {
        HeadersResponse { id, headers }
    }
}

/// The header request struct to be sent to connected peers, which
/// will proceed to ask them to stream the requested headers to us.
#[derive(Clone, Debug)]
pub struct HeadersRequest {
    /// The starting block
    pub start: BlockId,
    /// The response max size
    pub limit: u64,
    /// Flag indicating whether the blocks should
    /// arrive in reverse
    pub reverse: bool,
}

/// The block headers downloader client
#[async_trait]
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait HeadersClient: Send + Sync + Debug {
    /// Update the node's Status message.
    ///
    /// The updated Status message will be used during any new eth/65 handshakes.
    async fn update_status(&self, height: u64, hash: H256, td: H256);

    /// Sends the header request to the p2p network.
    // TODO: What does this return?
    async fn send_header_request(&self, id: u64, request: HeadersRequest) -> HashSet<H512>;

    /// Stream the header response messages
    async fn stream_headers(&self) -> HeadersStream;
}
