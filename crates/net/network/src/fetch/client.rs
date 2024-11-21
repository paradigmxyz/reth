//! A client implementation that can interact with the network and download data.

use crate::{fetch::DownloadRequest, flattened_response::FlattenedResponse};
use alloy_primitives::B256;
use futures::{future, future::Either};
use reth_eth_wire::{EthNetworkPrimitives, NetworkPrimitives};
use reth_network_api::test_utils::PeersHandle;
use reth_network_p2p::{
    bodies::client::{BodiesClient, BodiesFut},
    download::DownloadClient,
    error::{PeerRequestResult, RequestError},
    headers::client::{HeadersClient, HeadersRequest},
    priority::Priority,
};
use reth_network_peers::PeerId;
use reth_network_types::ReputationChangeKind;
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
pub struct FetchClient<N: NetworkPrimitives = EthNetworkPrimitives> {
    /// Sender half of the request channel.
    pub(crate) request_tx: UnboundedSender<DownloadRequest<N>>,
    /// The handle to the peers
    pub(crate) peers_handle: PeersHandle,
    /// Number of active peer sessions the node's currently handling.
    pub(crate) num_active_peers: Arc<AtomicUsize>,
}

impl<N: NetworkPrimitives> DownloadClient for FetchClient<N> {
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

impl<N: NetworkPrimitives> HeadersClient for FetchClient<N> {
    type Header = N::BlockHeader;
    type Output = HeadersClientFuture<PeerRequestResult<Vec<N::BlockHeader>>>;

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

impl<N: NetworkPrimitives> BodiesClient for FetchClient<N> {
    type Body = N::BlockBody;
    type Output = BodiesFut<N::BlockBody>;

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
