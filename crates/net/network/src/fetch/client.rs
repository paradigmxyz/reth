//! A client implementation that can interact with the network and download data.

use crate::fetch::DownloadRequest;

use reth_interfaces::p2p::{error::RequestResult, headers::client::HeadersRequest};
use reth_primitives::Header;
use tokio::sync::{mpsc::UnboundedSender, oneshot};

/// Front-end API for fetching data from the network.
#[derive(Debug)]
pub struct FetchClient {
    /// Sender half of the request channel.
    pub(crate) request_tx: UnboundedSender<DownloadRequest>,
}

impl FetchClient {
    /// Sends a `GetBlockHeaders` request to an available peer.
    pub async fn get_block_headers(&self, request: HeadersRequest) -> RequestResult<Vec<Header>> {
        let (response, rx) = oneshot::channel();
        self.request_tx.send(DownloadRequest::GetBlockHeaders { request, response })?;
        rx.await?
    }
}
