//! Snap protocol request handling for serving state data to peers.

use crate::{budget::DEFAULT_BUDGET_TRY_DRAIN_DOWNLOADERS, metered_poll_nested_stream_with_budget};
use alloy_primitives::{Bytes, B256};
use futures::StreamExt;
use reth_eth_wire_types::snap::{
    AccountData, AccountRangeMessage, ByteCodesMessage, GetAccountRangeMessage,
    GetByteCodesMessage, GetStorageRangesMessage, GetTrieNodesMessage, StorageData,
    StorageRangesMessage, TrieNodesMessage,
};
use reth_network_p2p::{error::RequestResult, snap::client::SnapResponse};
use reth_network_peers::PeerId;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::sync::{mpsc::Receiver, oneshot};
use tokio_stream::wrappers::ReceiverStream;

/// Provides access to state data for snap sync serving.
pub trait SnapStateProvider: Send + Sync + 'static {
    /// Returns the current state root (HEAD state root).
    fn current_state_root(&self) -> B256;

    /// Iterates accounts in hash-sorted order from `starting_hash` up to (but not including)
    /// `limit_hash`. Returns at most `response_bytes` worth of data.
    ///
    /// Returns `(accounts, proof)` — proof is empty for MVP.
    fn account_range(
        &self,
        root_hash: B256,
        starting_hash: B256,
        limit_hash: B256,
        response_bytes: u64,
    ) -> (Vec<AccountData>, Vec<Bytes>);

    /// Iterates storage slots for the given account hashes.
    ///
    /// Returns `(slots_per_account, proof)` — proof is empty for MVP.
    fn storage_ranges(
        &self,
        root_hash: B256,
        account_hashes: Vec<B256>,
        starting_hash: B256,
        limit_hash: B256,
        response_bytes: u64,
    ) -> (Vec<Vec<StorageData>>, Vec<Bytes>);

    /// Returns bytecodes for the given code hashes.
    fn bytecodes(&self, hashes: Vec<B256>, response_bytes: u64) -> Vec<Bytes>;
}

/// Manages incoming snap protocol requests from peers.
///
/// This should be spawned as a background task, similar to
/// [`EthRequestHandler`](crate::eth_requests::EthRequestHandler).
#[derive(Debug)]
#[must_use = "Handler does nothing unless polled."]
pub struct SnapRequestHandler<S> {
    snap_provider: S,
    incoming_requests: ReceiverStream<IncomingSnapRequest>,
}

impl<S> SnapRequestHandler<S> {
    /// Creates a new handler with the given provider and receiver channel.
    pub fn new(snap_provider: S, incoming: Receiver<IncomingSnapRequest>) -> Self {
        Self { snap_provider, incoming_requests: ReceiverStream::new(incoming) }
    }
}

impl<S: SnapStateProvider> SnapRequestHandler<S> {
    fn on_account_range_request(
        &self,
        _peer_id: PeerId,
        request: GetAccountRangeMessage,
        response: oneshot::Sender<RequestResult<SnapResponse>>,
    ) {
        let (accounts, proof) = self.snap_provider.account_range(
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

    fn on_storage_ranges_request(
        &self,
        _peer_id: PeerId,
        request: GetStorageRangesMessage,
        response: oneshot::Sender<RequestResult<SnapResponse>>,
    ) {
        let (slots, proof) = self.snap_provider.storage_ranges(
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

    fn on_byte_codes_request(
        &self,
        _peer_id: PeerId,
        request: GetByteCodesMessage,
        response: oneshot::Sender<RequestResult<SnapResponse>>,
    ) {
        let codes = self.snap_provider.bytecodes(request.hashes, request.response_bytes);

        let _ = response.send(Ok(SnapResponse::ByteCodes(ByteCodesMessage {
            request_id: request.request_id,
            codes,
        })));
    }

    fn on_trie_nodes_request(
        &self,
        _peer_id: PeerId,
        request: GetTrieNodesMessage,
        response: oneshot::Sender<RequestResult<SnapResponse>>,
    ) {
        let _ = response.send(Ok(SnapResponse::TrieNodes(TrieNodesMessage {
            request_id: request.request_id,
            nodes: Vec::new(),
        })));
    }
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
                match incoming {
                    IncomingSnapRequest::GetAccountRange { peer_id, request, response } => {
                        this.on_account_range_request(peer_id, request, response)
                    }
                    IncomingSnapRequest::GetStorageRanges { peer_id, request, response } => {
                        this.on_storage_ranges_request(peer_id, request, response)
                    }
                    IncomingSnapRequest::GetByteCodes { peer_id, request, response } => {
                        this.on_byte_codes_request(peer_id, request, response)
                    }
                    IncomingSnapRequest::GetTrieNodes { peer_id, request, response } => {
                        this.on_trie_nodes_request(peer_id, request, response)
                    }
                }
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
    /// Request for trie nodes.
    GetTrieNodes {
        /// The ID of the peer requesting trie nodes.
        peer_id: PeerId,
        /// The trie nodes request.
        request: GetTrieNodesMessage,
        /// The channel sender for the response.
        response: oneshot::Sender<RequestResult<SnapResponse>>,
    },
}
