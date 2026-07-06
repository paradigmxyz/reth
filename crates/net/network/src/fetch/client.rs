//! A client implementation that can interact with the network and download data.

use crate::{fetch::DownloadRequest, flattened_response::FlattenedResponse};
use alloy_primitives::B256;
use futures::{future, future::Either};
use reth_eth_wire::{BlockAccessLists, EthNetworkPrimitives, NetworkPrimitives};
use reth_eth_wire_types::snap::{
    GetAccountRangeMessage, GetBlockAccessListsMessage, GetByteCodesMessage,
    GetStorageRangesMessage, SnapProtocolMessage,
};
use reth_network_api::test_utils::PeersHandle;
use reth_network_p2p::{
    block_access_lists::client::{BalRequirement, BlockAccessListsClient},
    bodies::client::{BodiesClient, BodiesFut},
    download::DownloadClient,
    error::{PeerRequestResult, RequestError},
    headers::client::{HeadersClient, HeadersRequest},
    priority::Priority,
    receipts::client::{ReceiptsClient, ReceiptsFut},
    snap::client::{SnapClient, SnapResponse},
    BlockClient,
};
use reth_network_peers::PeerId;
use reth_network_types::ReputationChangeKind;
use std::{
    ops::RangeInclusive,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
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

impl<N: NetworkPrimitives> FetchClient<N> {
    /// Sends a `snap/2` request to an available peer.
    fn send_snap_request(
        &self,
        request: SnapProtocolMessage,
        priority: Priority,
    ) -> std::pin::Pin<Box<dyn Future<Output = PeerRequestResult<SnapResponse>> + Send + Sync>>
    {
        let (response, rx) = oneshot::channel();
        if self.request_tx.send(DownloadRequest::GetSnap { request, response, priority }).is_ok() {
            Box::pin(FlattenedResponse::from(rx))
        } else {
            Box::pin(future::err(RequestError::ChannelClosed))
        }
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
    fn get_block_bodies_with_priority_and_range_hint(
        &self,
        request: Vec<B256>,
        priority: Priority,
        range_hint: Option<RangeInclusive<u64>>,
    ) -> Self::Output {
        let (response, rx) = oneshot::channel();
        if self
            .request_tx
            .send(DownloadRequest::GetBlockBodies { request, response, priority, range_hint })
            .is_ok()
        {
            Box::pin(FlattenedResponse::from(rx))
        } else {
            Box::pin(future::err(RequestError::ChannelClosed))
        }
    }
}

impl<N: NetworkPrimitives> ReceiptsClient for FetchClient<N> {
    type Receipt = N::Receipt;
    type Output = ReceiptsFut<N::Receipt>;

    fn get_receipts_with_priority(&self, request: Vec<B256>, priority: Priority) -> Self::Output {
        let (response, rx) = oneshot::channel();
        if self
            .request_tx
            .send(DownloadRequest::GetReceipts { request, response, priority })
            .is_ok()
        {
            Box::pin(FlattenedResponse::from(rx))
        } else {
            Box::pin(future::err(RequestError::ChannelClosed))
        }
    }
}

impl<N: NetworkPrimitives> BlockClient for FetchClient<N> {
    type Block = N::Block;
}

impl<N: NetworkPrimitives> BlockAccessListsClient for FetchClient<N> {
    type Output =
        std::pin::Pin<Box<dyn Future<Output = PeerRequestResult<BlockAccessLists>> + Send + Sync>>;

    fn get_block_access_lists_with_priority_and_requirement(
        &self,
        hashes: Vec<B256>,
        priority: Priority,
        requirement: BalRequirement,
    ) -> Self::Output {
        let (response, rx) = oneshot::channel();
        if self
            .request_tx
            .send(DownloadRequest::GetBlockAccessLists {
                request: hashes,
                response,
                priority,
                requirement,
            })
            .is_ok()
        {
            Box::pin(FlattenedResponse::from(rx))
        } else {
            Box::pin(future::err(RequestError::ChannelClosed))
        }
    }
}

/// A [`SnapClient`] backed by a [`FetchClient`], reporting the number of *snap-capable* connected
/// peers rather than [`FetchClient`]'s shared, capability-agnostic peer count.
#[derive(Debug, Clone)]
pub struct SnapFetchClient<N: NetworkPrimitives = EthNetworkPrimitives> {
    /// Sends the request and reports bad peers the same way [`FetchClient`] does.
    pub(crate) inner: FetchClient<N>,
    /// Number of connected peers that negotiated `snap/2`.
    pub(crate) num_snap_peers: Arc<AtomicUsize>,
}

impl<N: NetworkPrimitives> DownloadClient for SnapFetchClient<N> {
    fn report_bad_message(&self, peer_id: PeerId) {
        self.inner.report_bad_message(peer_id);
    }

    fn num_connected_peers(&self) -> usize {
        self.num_snap_peers.load(Ordering::Relaxed)
    }
}

impl<N: NetworkPrimitives> SnapClient for SnapFetchClient<N> {
    type Output =
        std::pin::Pin<Box<dyn Future<Output = PeerRequestResult<SnapResponse>> + Send + Sync>>;

    /// Sends a `GetAccountRange` (`snap/2`) request to an available peer.
    fn get_account_range_with_priority(
        &self,
        request: GetAccountRangeMessage,
        priority: Priority,
    ) -> Self::Output {
        self.inner.send_snap_request(SnapProtocolMessage::GetAccountRange(request), priority)
    }

    /// Sends a `GetStorageRanges` (`snap/2`) request to an available peer.
    fn get_storage_ranges(&self, request: GetStorageRangesMessage) -> Self::Output {
        self.get_storage_ranges_with_priority(request, Priority::Normal)
    }

    /// Sends a `GetStorageRanges` (`snap/2`) request to an available peer.
    fn get_storage_ranges_with_priority(
        &self,
        request: GetStorageRangesMessage,
        priority: Priority,
    ) -> Self::Output {
        self.inner.send_snap_request(SnapProtocolMessage::GetStorageRanges(request), priority)
    }

    /// Sends a `GetByteCodes` (`snap/2`) request to an available peer.
    fn get_byte_codes(&self, request: GetByteCodesMessage) -> Self::Output {
        self.get_byte_codes_with_priority(request, Priority::Normal)
    }

    /// Sends a `GetByteCodes` (`snap/2`) request to an available peer.
    fn get_byte_codes_with_priority(
        &self,
        request: GetByteCodesMessage,
        priority: Priority,
    ) -> Self::Output {
        self.inner.send_snap_request(SnapProtocolMessage::GetByteCodes(request), priority)
    }

    /// Sends a `GetBlockAccessLists` (`snap/2`) request to an available peer.
    fn get_block_access_lists_with_priority(
        &self,
        request: GetBlockAccessListsMessage,
        priority: Priority,
    ) -> Self::Output {
        self.inner.send_snap_request(SnapProtocolMessage::GetBlockAccessLists(request), priority)
    }
}
