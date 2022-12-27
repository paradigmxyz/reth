//! A client implementation that can interact with the network and download data.

use crate::{
    fetch::DownloadRequest,
    peers::{PeersHandle, ReputationChangeKind},
};
use reth_eth_wire::{BlockBody, BlockHeaders};
use reth_interfaces::p2p::{
    bodies::client::BodiesClient,
    downloader::DownloadClient,
    error::PeerRequestResult,
    headers::client::{HeadersClient, HeadersRequest},
};
use reth_primitives::{PeerId, WithPeerId, H256};
use tokio::sync::{mpsc::UnboundedSender, oneshot};

/// Front-end API for fetching data from the network.
// ANCHOR: struct-FetchClient
#[derive(Debug)]
pub struct FetchClient {
    /// Sender half of the request channel.
    pub(crate) request_tx: UnboundedSender<DownloadRequest>,
    /// The handle to the peers
    pub(crate) peers_handle: PeersHandle,
}
// ANCHOR_END: struct-FetchClient

impl DownloadClient for FetchClient {
    fn report_bad_message(&self, peer_id: PeerId) {
        self.peers_handle.reputation_change(peer_id, ReputationChangeKind::BadMessage);
    }
}

// ANCHOR: trait-HeadersClient-BodiesClient
#[async_trait::async_trait]
impl HeadersClient for FetchClient {
    /// Sends a `GetBlockHeaders` request to an available peer.
    async fn get_headers(&self, request: HeadersRequest) -> PeerRequestResult<BlockHeaders> {
        let (response, rx) = oneshot::channel();
        self.request_tx.send(DownloadRequest::GetBlockHeaders { request, response })?;
        rx.await?.map(WithPeerId::transform)
    }
}

#[async_trait::async_trait]
impl BodiesClient for FetchClient {
    async fn get_block_bodies(&self, request: Vec<H256>) -> PeerRequestResult<Vec<BlockBody>> {
        let (response, rx) = oneshot::channel();
        self.request_tx.send(DownloadRequest::GetBlockBodies { request, response })?;
        rx.await?
    }
}
// ANCHOR_END: trait-HeadersClient-BodiesClient
