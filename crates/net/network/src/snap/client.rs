//! Network-facing `snap/2` sync client.

use reth_eth_wire_types::snap::{
    GetAccountRangeMessage, GetBlockAccessListsMessage, GetByteCodesMessage,
    GetStorageRangesMessage, SnapProtocolMessage,
};
use reth_network_api::PeerId;
use reth_network_p2p::{
    download::DownloadClient,
    error::{PeerRequestResult, RequestError},
    priority::Priority,
    snap::client::{SnapClient as SnapClientTrait, SnapResponse},
};
use std::{
    fmt,
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    task::{Context, Poll},
};
use tokio::sync::{mpsc, oneshot};

/// A [`SnapClient`](SnapClientTrait) that dispatches requests to a snap-capable session through the
/// [`SessionManager`](crate::session::SessionManager).
///
/// The client does not own any peer state: it forwards each request to the manager, which routes it
/// to a session that negotiated `snap/2` (see [`EthSnapStream`](reth_eth_wire::EthSnapStream)) and
/// correlates the response. Cloning shares the same routing channel and peer count.
#[derive(Clone, Debug)]
pub struct SnapClient {
    /// Routes requests to the session manager, which forwards them to a snap-capable session.
    to_manager: mpsc::UnboundedSender<SnapPeerRequest>,
    /// Number of connected snap-capable peers, maintained by the session manager.
    peer_count: Arc<AtomicUsize>,
}

impl SnapClient {
    /// Creates a new client backed by the manager's routing channel and shared peer count.
    pub(crate) const fn new(
        to_manager: mpsc::UnboundedSender<SnapPeerRequest>,
        peer_count: Arc<AtomicUsize>,
    ) -> Self {
        Self { to_manager, peer_count }
    }

    /// Routes a request to a snap-capable session via the manager.
    fn request(&self, request: SnapProtocolMessage) -> SnapResponseFuture {
        let (tx, rx) = oneshot::channel();
        match self.to_manager.send(SnapPeerRequest { request, response: tx }) {
            Ok(()) => SnapResponseFuture::pending(rx),
            // The session manager is gone; no peer can serve the request.
            Err(_) => SnapResponseFuture::ready_err(RequestError::UnsupportedCapability),
        }
    }
}

impl DownloadClient for SnapClient {
    fn report_bad_message(&self, _peer_id: PeerId) {
        // TODO(snap/2 client): wire peer reputation once the network handle is threaded in.
    }

    fn num_connected_peers(&self) -> usize {
        self.peer_count.load(Ordering::Relaxed)
    }
}

impl SnapClientTrait for SnapClient {
    type Output = SnapResponseFuture;

    fn get_account_range_with_priority(
        &self,
        request: GetAccountRangeMessage,
        _priority: Priority,
    ) -> Self::Output {
        self.request(SnapProtocolMessage::GetAccountRange(request))
    }

    fn get_storage_ranges(&self, request: GetStorageRangesMessage) -> Self::Output {
        self.get_storage_ranges_with_priority(request, Priority::Normal)
    }

    fn get_storage_ranges_with_priority(
        &self,
        request: GetStorageRangesMessage,
        _priority: Priority,
    ) -> Self::Output {
        self.request(SnapProtocolMessage::GetStorageRanges(request))
    }

    fn get_byte_codes(&self, request: GetByteCodesMessage) -> Self::Output {
        self.get_byte_codes_with_priority(request, Priority::Normal)
    }

    fn get_byte_codes_with_priority(
        &self,
        request: GetByteCodesMessage,
        _priority: Priority,
    ) -> Self::Output {
        self.request(SnapProtocolMessage::GetByteCodes(request))
    }

    fn get_block_access_lists_with_priority(
        &self,
        request: GetBlockAccessListsMessage,
        _priority: Priority,
    ) -> Self::Output {
        self.request(SnapProtocolMessage::GetBlockAccessLists(request))
    }
}

/// A `snap/2` client request routed to a snap-capable session by the
/// [`SessionManager`](crate::session::SessionManager).
#[derive(Debug)]
pub struct SnapPeerRequest {
    /// The request to send to the peer.
    pub(crate) request: SnapProtocolMessage,
    /// Channel the session uses to return the correlated response.
    pub(crate) response: oneshot::Sender<PeerRequestResult<SnapResponse>>,
}

/// Future resolving to a `snap/2` peer response.
///
/// Either awaits a session's correlated reply or resolves immediately with an error (e.g. when no
/// snap/2 peer is available).
pub struct SnapResponseFuture {
    rx: Option<oneshot::Receiver<PeerRequestResult<SnapResponse>>>,
    err: Option<RequestError>,
}

impl SnapResponseFuture {
    const fn pending(rx: oneshot::Receiver<PeerRequestResult<SnapResponse>>) -> Self {
        Self { rx: Some(rx), err: None }
    }

    const fn ready_err(err: RequestError) -> Self {
        Self { rx: None, err: Some(err) }
    }
}

impl fmt::Debug for SnapResponseFuture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SnapResponseFuture").finish_non_exhaustive()
    }
}

impl Future for SnapResponseFuture {
    type Output = PeerRequestResult<SnapResponse>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if let Some(err) = this.err.take() {
            return Poll::Ready(Err(err));
        }
        match this.rx.as_mut() {
            Some(rx) => match Pin::new(rx).poll(cx) {
                Poll::Ready(Ok(res)) => Poll::Ready(res),
                Poll::Ready(Err(_)) => Poll::Ready(Err(RequestError::ChannelClosed)),
                Poll::Pending => Poll::Pending,
            },
            None => Poll::Ready(Err(RequestError::ChannelClosed)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_eth_wire_types::{snap::BlockAccessListsMessage, BlockAccessLists};
    use reth_network_peers::WithPeerId;

    fn client() -> (SnapClient, mpsc::UnboundedReceiver<SnapPeerRequest>, Arc<AtomicUsize>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let count = Arc::new(AtomicUsize::new(0));
        (SnapClient::new(tx, Arc::clone(&count)), rx, count)
    }

    fn block_access_lists_request(request_id: u64) -> GetBlockAccessListsMessage {
        GetBlockAccessListsMessage { request_id, block_hashes: Vec::new(), response_bytes: 0 }
    }

    #[tokio::test]
    async fn request_without_manager_is_unsupported() {
        let (client, rx, _count) = client();
        drop(rx);
        let res = client.get_block_access_lists(block_access_lists_request(1)).await;
        assert!(matches!(res, Err(RequestError::UnsupportedCapability)));
    }

    #[tokio::test]
    async fn request_is_routed_and_response_is_correlated() {
        let (client, mut rx, _count) = client();

        let response = client.get_block_access_lists(block_access_lists_request(9));
        let routed = rx.recv().await.expect("request routed to manager");
        assert!(matches!(
            routed.request,
            SnapProtocolMessage::GetBlockAccessLists(GetBlockAccessListsMessage {
                request_id: 9,
                ..
            })
        ));

        let peer = PeerId::random();
        routed
            .response
            .send(Ok(WithPeerId::new(
                peer,
                SnapResponse::BlockAccessLists(BlockAccessListsMessage {
                    request_id: 9,
                    block_access_lists: BlockAccessLists(Vec::new()),
                }),
            )))
            .unwrap();

        let response = response.await.unwrap();
        assert_eq!(response.peer_id(), peer);
        assert!(matches!(
            response.into_data(),
            SnapResponse::BlockAccessLists(BlockAccessListsMessage { request_id: 9, .. })
        ));
    }

    #[tokio::test]
    async fn response_future_returns_channel_closed_if_session_drops() {
        let (client, mut rx, _count) = client();
        let response = client.get_block_access_lists(block_access_lists_request(1));
        // The session takes the request but drops the response channel (e.g. disconnect).
        drop(rx.recv().await.unwrap().response);
        assert_eq!(response.await.unwrap_err(), RequestError::ChannelClosed);
        let _ = rx;
    }

    #[test]
    fn num_connected_peers_reflects_shared_count() {
        let (client, _rx, count) = client();
        assert_eq!(client.num_connected_peers(), 0);
        count.store(3, Ordering::Relaxed);
        assert_eq!(client.num_connected_peers(), 3);
    }
}
