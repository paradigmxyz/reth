use crate::p2p::{downloader::DownloadClient, error::PeerRequestResult};
use async_trait::async_trait;
pub use reth_eth_wire::BlockHeaders;
use reth_primitives::{BlockHashOrNumber, HeadersDirection, H256, U256};
use std::fmt::Debug;

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

/// The block headers downloader client
#[async_trait]
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait HeadersClient: DownloadClient {
    /// Sends the header request to the p2p network and returns the header response received from a
    /// peer.
    async fn get_headers(&self, request: HeadersRequest) -> PeerRequestResult<BlockHeaders>;
}

/// The status updater for updating the status of the p2p node
pub trait StatusUpdater: Send + Sync {
    /// Updates the status of the p2p node
    fn update_status(&self, height: u64, hash: H256, total_difficulty: U256);
}
