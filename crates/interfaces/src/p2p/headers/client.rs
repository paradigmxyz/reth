use crate::p2p::error::RequestResult;
use async_trait::async_trait;
pub use reth_eth_wire::BlockHeaders;
use reth_primitives::{BlockHashOrNumber, H256};
use std::fmt::Debug;

/// The header request struct to be sent to connected peers, which
/// will proceed to ask them to stream the requested headers to us.
#[derive(Clone, Debug)]
pub struct HeadersRequest {
    /// The starting block
    pub start: BlockHashOrNumber,
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
    fn update_status(&self, height: u64, hash: H256, td: H256);

    /// Sends the header request to the p2p network and returns the header response received from a
    /// peer.
    async fn get_headers(&self, request: HeadersRequest) -> RequestResult<BlockHeaders>;
}
