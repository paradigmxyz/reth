//! A client implementation that can interact with the network and download data.

use crate::fetch::{DownloadRequest, StatusUpdate};
use reth_eth_wire::{BlockBody, BlockHeaders};
use reth_interfaces::p2p::{
    bodies::client::BodiesClient,
    error::RequestResult,
    headers::client::{HeadersClient, HeadersRequest},
};
use reth_primitives::{H256, U256};
use tokio::sync::{mpsc::UnboundedSender, oneshot};

/// Front-end API for fetching data from the network.
#[derive(Debug)]
pub struct FetchClient {
    /// Sender half of the request channel.
    pub(crate) request_tx: UnboundedSender<DownloadRequest>,
    /// Sender for sending Status updates
    pub(crate) status_tx: UnboundedSender<StatusUpdate>,
}

#[async_trait::async_trait]
impl HeadersClient for FetchClient {
    fn update_status(&self, height: u64, hash: H256, total_difficulty: U256) {
        let _ = self.status_tx.send(StatusUpdate { height, hash, total_difficulty });
    }

    /// Sends a `GetBlockHeaders` request to an available peer.
    async fn get_headers(&self, request: HeadersRequest) -> RequestResult<BlockHeaders> {
        let (response, rx) = oneshot::channel();
        self.request_tx.send(DownloadRequest::GetBlockHeaders { request, response })?;
        rx.await?.map(BlockHeaders::from)
    }
}

#[async_trait::async_trait]
impl BodiesClient for FetchClient {
    async fn get_block_body(&self, request: Vec<H256>) -> RequestResult<Vec<BlockBody>> {
        let (response, rx) = oneshot::channel();
        self.request_tx.send(DownloadRequest::GetBlockBodies { request, response })?;
        rx.await?
    }
}
