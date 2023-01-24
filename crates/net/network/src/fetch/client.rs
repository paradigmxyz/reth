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
    priority::Priority,
};
use reth_primitives::{PeerId, WithPeerId, H256};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::sync::{mpsc::UnboundedSender, oneshot};

/// Front-end API for fetching data from the network.
///
/// Following diagram illustrates how a request, See [`HeadersClient::get_headers`] and
/// [`BodiesClient::get_block_bodies`] is handled internally.
#[cfg_attr(doc, aquamarine::aquamarine)]
/// ```mermaid
/// sequenceDiagram
//     participant Client as FetchClient
//     participant Fetcher as StateFetcher
//     participant State as NetworkState
//     participant Session as Active Peer Session
//     participant Peers as PeerManager
//     loop Send Request, retry if retriable and remaining retries
//         Client->>Fetcher: DownloadRequest{GetHeaders, GetBodies}
//         Note over Client,Fetcher: Request and oneshot Sender sent via `request_tx` channel
//         loop Process buffered requests
//             State->>Fetcher: poll action
//             Fetcher->>Fetcher: Select Available Peer
//             Note over Fetcher: Peer is available if it's currently idle, no inflight requests
//             Fetcher->>State: FetchAction::BlockDownloadRequest
//             State->>Session: Delegate Request
//             Note over State,Session: Request and oneshot Sender sent via `to_session_tx` channel
//         end
//         Session->>Session: Send Request to remote
//         Session->>Session: Enforce Request timeout
//         Session-->>State: Send Response Result via channel
//         State->>Fetcher: Delegate Response
//         Fetcher-->>Client: Send Response via channel
//         opt Bad Response
//             Client->>Peers: Penalize Peer
//         end
//         Peers->>Peers: Apply Reputation Change
//         opt reputation dropped below threshold
//             Peers->>State: Disconnect Session
//             State->>Session: Delegate Disconnect
//         end
//     end
/// ```
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

#[async_trait::async_trait]
impl HeadersClient for FetchClient {
    /// Sends a `GetBlockHeaders` request to an available peer.
    async fn get_headers_with_priority(
        &self,
        request: HeadersRequest,
        priority: Priority,
    ) -> PeerRequestResult<BlockHeaders> {
        let (response, rx) = oneshot::channel();
        self.request_tx.send(DownloadRequest::GetBlockHeaders { request, response, priority })?;
        rx.await?.map(WithPeerId::transform)
    }
}

#[async_trait::async_trait]
impl BodiesClient for FetchClient {
    /// Sends a `GetBlockBodies` request to an available peer.
    async fn get_block_bodies_with_priority(
        &self,
        request: Vec<H256>,
        priority: Priority,
    ) -> PeerRequestResult<Vec<BlockBody>> {
        let (response, rx) = oneshot::channel();
        self.request_tx.send(DownloadRequest::GetBlockBodies { request, response, priority })?;
        rx.await?
    }
}
