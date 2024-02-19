//! A client implementation that can interact with the network and download data.

use crate::{fetch::DownloadRequest, flattened_response::FlattenedResponse, peers::PeersHandle};
use futures::{future, future::Either};

use reth_interfaces::p2p::{
    bodies::client::{BodiesClient, BodiesFut},
    download::DownloadClient,
    error::{PeerRequestResult, RequestError},
    headers::client::{HeadersClient, HeadersRequest},
    priority::Priority,
};
use reth_network_api::ReputationChangeKind;
use reth_primitives::{Header, PeerId, B256};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::sync::{mpsc::UnboundedSender, oneshot};

#[cfg_attr(doc, aquamarine::aquamarine)]
/// Front-end API for fetching data from the network.
///
/// Following diagram illustrates how a request, See [`HeadersClient::get_headers`] and
/// [`BodiesClient::get_block_bodies`] is handled internally.
///
/// include_mmd!("docs/mermaid/fetch-client.mmd")
#[derive(Debug, Clone)]
pub struct FetchClient {
    /// Sender half of the request channel.
    pub(crate) request_tx: UnboundedSender<DownloadRequest>,
    /// The handle to the peers
    pub(crate) peers_handle: PeersHandle,
    /// Number of active peer sessions the node's currently handling.
    pub(crate) num_active_peers: Arc<AtomicUsize>,
}

impl DownloadClient for FetchClient {
    fn report_bad_message(&self, peer_id: PeerId) {
        self.peers_handle.reputation_change(peer_id, ReputationChangeKind::BadMessage);
    }

    fn num_connected_peers(&self) -> usize {
        self.num_active_peers.load(Ordering::Relaxed)
    }
}

// The `Output` future of the [HeadersClient] impl of [FetchClient] that either returns a response
// or an error.
type HeadersClientFuture<T> = Either<FlattenedResponse<T>, future::Ready<T>>;

impl HeadersClient for FetchClient {
    type Output = HeadersClientFuture<PeerRequestResult<Vec<Header>>>;

    /// Sends a `GetBlockHeaders` request to an available peer.
    fn get_headers_with_priority(
        &self,
        request: HeadersRequest,
        priority: Priority,
    ) -> Self::Output {
        let (response, rx) = oneshot::channel();
        if self
            .request_tx
            .send(DownloadRequest::GetBlockHeaders { request, response, priority })
            .is_ok()
        {
            Either::Left(FlattenedResponse::from(rx))
        } else {
            Either::Right(future::err(RequestError::ChannelClosed))
        }
    }
}

impl BodiesClient for FetchClient {
    type Output = BodiesFut;

    /// Sends a `GetBlockBodies` request to an available peer.
    fn get_block_bodies_with_priority(
        &self,
        request: Vec<B256>,
        priority: Priority,
    ) -> Self::Output {
        let (response, rx) = oneshot::channel();
        if self
            .request_tx
            .send(DownloadRequest::GetBlockBodies { request, response, priority })
            .is_ok()
        {
            Box::pin(FlattenedResponse::from(rx))
        } else {
            Box::pin(future::err(RequestError::ChannelClosed))
        }
    }
}
