//! Snap protocol request handling for serving state data to peers.

use crate::{budget::DEFAULT_BUDGET_TRY_DRAIN_DOWNLOADERS, metered_poll_nested_stream_with_budget};
use futures::StreamExt;
use reth_eth_wire_types::snap::{
    AccountRangeMessage, BlockAccessListsMessage, ByteCodesMessage, GetAccountRangeMessage,
    GetBlockAccessListsMessage, GetByteCodesMessage, GetStorageRangesMessage, StorageRangesMessage,
};
use reth_network_p2p::{
    error::RequestResult,
    snap::{client::SnapResponse, server::SnapStateProvider},
};
use reth_network_peers::PeerId;
use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use tokio::sync::{mpsc::Receiver, oneshot};
use tokio_stream::wrappers::ReceiverStream;

/// Manages incoming snap protocol requests from peers.
///
/// Each request is dispatched to [`tokio::task::spawn_blocking`] so heavy DB
/// reads, proof generation, and on-demand BAL replay can run concurrently
/// across the blocking thread pool. Without this, all peers' snap requests
/// would serialize through a single task, capping serve throughput at one
/// core regardless of in-flight client requests.
///
/// This should be spawned as a background task, similar to
/// [`EthRequestHandler`](crate::eth_requests::EthRequestHandler).
#[derive(Debug)]
#[must_use = "Handler does nothing unless polled."]
pub struct SnapRequestHandler<S> {
    snap_provider: Arc<S>,
    incoming_requests: ReceiverStream<IncomingSnapRequest>,
}

impl<S> SnapRequestHandler<S> {
    /// Creates a new handler with the given provider and receiver channel.
    pub fn new(snap_provider: S, incoming: Receiver<IncomingSnapRequest>) -> Self {
        Self {
            snap_provider: Arc::new(snap_provider),
            incoming_requests: ReceiverStream::new(incoming),
        }
    }
}

impl<S: SnapStateProvider> SnapRequestHandler<S> {
    fn dispatch(&self, incoming: IncomingSnapRequest) {
        let provider = Arc::clone(&self.snap_provider);
        tokio::task::spawn_blocking(move || handle_request(&*provider, incoming));
    }
}

fn handle_request<S: SnapStateProvider + ?Sized>(provider: &S, incoming: IncomingSnapRequest) {
    match incoming {
        IncomingSnapRequest::GetAccountRange { peer_id, request, response } => {
            on_account_range_request(provider, peer_id, request, response);
        }
        IncomingSnapRequest::GetStorageRanges { peer_id, request, response } => {
            on_storage_ranges_request(provider, peer_id, request, response);
        }
        IncomingSnapRequest::GetByteCodes { peer_id, request, response } => {
            on_byte_codes_request(provider, peer_id, request, response);
        }
        IncomingSnapRequest::GetBlockAccessLists { peer_id, request, response } => {
            on_block_access_lists_request(provider, peer_id, request, response);
        }
    }
}

fn on_account_range_request<S: SnapStateProvider + ?Sized>(
    provider: &S,
    _peer_id: PeerId,
    request: GetAccountRangeMessage,
    response: oneshot::Sender<RequestResult<SnapResponse>>,
) {
    let (accounts, proof) = provider.account_range(
        request.root_hash,
        request.starting_hash,
        request.limit_hash,
        request.response_bytes,
    );

    let _ = response.send(Ok(SnapResponse::AccountRange(AccountRangeMessage {
        request_id: request.request_id,
        accounts,
        proof,
    })));
}

fn on_storage_ranges_request<S: SnapStateProvider + ?Sized>(
    provider: &S,
    _peer_id: PeerId,
    request: GetStorageRangesMessage,
    response: oneshot::Sender<RequestResult<SnapResponse>>,
) {
    let (slots, proof) = provider.storage_ranges(
        request.root_hash,
        request.account_hashes,
        request.starting_hash,
        request.limit_hash,
        request.response_bytes,
    );

    let _ = response.send(Ok(SnapResponse::StorageRanges(StorageRangesMessage {
        request_id: request.request_id,
        slots,
        proof,
    })));
}

fn on_byte_codes_request<S: SnapStateProvider + ?Sized>(
    provider: &S,
    _peer_id: PeerId,
    request: GetByteCodesMessage,
    response: oneshot::Sender<RequestResult<SnapResponse>>,
) {
    let codes = provider.bytecodes(request.hashes, request.response_bytes);

    let _ = response.send(Ok(SnapResponse::ByteCodes(ByteCodesMessage {
        request_id: request.request_id,
        codes,
    })));
}

fn on_block_access_lists_request<S: SnapStateProvider + ?Sized>(
    provider: &S,
    _peer_id: PeerId,
    request: GetBlockAccessListsMessage,
    response: oneshot::Sender<RequestResult<SnapResponse>>,
) {
    let block_access_lists =
        provider.block_access_lists(request.block_hashes, request.response_bytes);

    let _ = response.send(Ok(SnapResponse::BlockAccessLists(BlockAccessListsMessage {
        request_id: request.request_id,
        block_access_lists,
    })));
}

impl<S: SnapStateProvider + Unpin> Future for SnapRequestHandler<S> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        let mut acc = Duration::ZERO;
        let maybe_more_incoming_requests = metered_poll_nested_stream_with_budget!(
            acc,
            "net::snap",
            "Incoming snap requests stream",
            DEFAULT_BUDGET_TRY_DRAIN_DOWNLOADERS,
            this.incoming_requests.poll_next_unpin(cx),
            |incoming| {
                this.dispatch(incoming);
            },
        );

        if maybe_more_incoming_requests {
            cx.waker().wake_by_ref();
        }

        Poll::Pending
    }
}

/// Incoming snap protocol requests delegated by the [`NetworkManager`](crate::NetworkManager).
#[derive(Debug)]
pub enum IncomingSnapRequest {
    /// Request for an account range.
    GetAccountRange {
        /// The ID of the peer requesting account range.
        peer_id: PeerId,
        /// The account range request.
        request: GetAccountRangeMessage,
        /// The channel sender for the response.
        response: oneshot::Sender<RequestResult<SnapResponse>>,
    },
    /// Request for storage ranges.
    GetStorageRanges {
        /// The ID of the peer requesting storage ranges.
        peer_id: PeerId,
        /// The storage ranges request.
        request: GetStorageRangesMessage,
        /// The channel sender for the response.
        response: oneshot::Sender<RequestResult<SnapResponse>>,
    },
    /// Request for bytecodes.
    GetByteCodes {
        /// The ID of the peer requesting bytecodes.
        peer_id: PeerId,
        /// The bytecodes request.
        request: GetByteCodesMessage,
        /// The channel sender for the response.
        response: oneshot::Sender<RequestResult<SnapResponse>>,
    },
    /// Request for block access lists.
    GetBlockAccessLists {
        /// The ID of the peer requesting BALs.
        peer_id: PeerId,
        /// The snap/2 BAL request.
        request: GetBlockAccessListsMessage,
        /// The channel sender for the response.
        response: oneshot::Sender<RequestResult<SnapResponse>>,
    },
}
