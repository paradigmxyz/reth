//! Blocks/Headers management for the p2p network.

use crate::{
    budget::DEFAULT_BUDGET_TRY_DRAIN_DOWNLOADERS, metered_poll_nested_stream_with_budget,
    metrics::EthRequestHandlerMetrics,
};
use alloy_consensus::{BlockHeader, ReceiptWithBloom};
use alloy_eips::BlockHashOrNumber;
use alloy_primitives::{Bytes, B256};
use alloy_rlp::Encodable;
use futures::StreamExt;
use reth_eth_wire::{
    snap::{
        AccountData, AccountRangeMessage, BlockAccessListsMessage, ByteCodesMessage,
        GetAccountRangeMessage, GetStorageRangesMessage, SnapProtocolMessage, StorageData,
        StorageRangesMessage,
    },
    BlockAccessLists, BlockBodies, BlockHeaders, Cells, EthNetworkPrimitives, GetBlockAccessLists,
    GetBlockBodies, GetBlockHeaders, GetCells, GetNodeData, GetReceipts, GetReceipts70,
    HeadersDirection, NetworkPrimitives, NodeData, Receipts, Receipts69, Receipts70,
};
use reth_network_api::test_utils::PeersHandle;
use reth_network_p2p::{
    error::{RequestError, RequestResult},
    snap::client::SnapResponse,
};
use reth_network_peers::PeerId;
use reth_primitives_traits::Block;
use reth_storage_api::{
    BalProvider, BlockReader, BytecodeReader, GetBlockAccessListLimit, HeaderProvider,
    StateProviderFactory, StateRangeProvider,
};
use reth_transaction_pool::{blobstore::NoopBlobStore, BlobStore};
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::sync::{mpsc::Receiver, oneshot};
use tokio_stream::wrappers::ReceiverStream;

// Limits: <https://github.com/ethereum/go-ethereum/blob/b0d44338bbcefee044f1f635a84487cbbd8f0538/eth/protocols/eth/handler.go#L34-L56>

/// Maximum number of receipts to serve.
///
/// Used to limit lookups.
pub const MAX_RECEIPTS_SERVE: usize = 1024;

/// Maximum number of block headers to serve.
///
/// Used to limit lookups.
pub const MAX_HEADERS_SERVE: usize = 1024;

/// Maximum number of block headers to serve.
///
/// Used to limit lookups. With 24KB block sizes nowadays, the practical limit will always be
/// `SOFT_RESPONSE_LIMIT`.
pub const MAX_BODIES_SERVE: usize = 1024;

/// Maximum number of block access lists to serve.
///
/// Used to limit lookups.
pub const MAX_BLOCK_ACCESS_LISTS_SERVE: usize = 1024;

/// Maximum number of cell lookups to serve.
///
/// Used to limit lookups.
pub const MAX_CELLS_SERVE: usize = 1024;

/// Maximum number of bytecode lookups to serve.
///
/// Used to limit lookups.
pub const MAX_BYTE_CODES_SERVE: usize = 1024;

/// Maximum size of replies to data retrievals: 2MB
pub const SOFT_RESPONSE_LIMIT: usize = 2 * 1024 * 1024;

/// Manages eth related requests on top of the p2p network.
///
/// This can be spawned to another task and is supposed to be run as background service.
#[derive(Debug)]
#[must_use = "Manager does nothing unless polled."]
pub struct EthRequestHandler<C, N: NetworkPrimitives = EthNetworkPrimitives> {
    /// The client type that can interact with the chain.
    client: C,
    /// Blob store used for serving blob cell requests.
    blob_store: Box<dyn BlobStore>,
    /// Used for reporting peers.
    // TODO use to report spammers
    #[expect(dead_code)]
    peers: PeersHandle,
    /// Incoming request from the [`NetworkManager`](crate::NetworkManager).
    incoming_requests: ReceiverStream<IncomingEthRequest<N>>,
    /// Metrics for the eth request handler.
    metrics: EthRequestHandlerMetrics,
}

// === impl EthRequestHandler ===
impl<C, N: NetworkPrimitives> EthRequestHandler<C, N> {
    /// Create a new instance
    pub fn new(client: C, peers: PeersHandle, incoming: Receiver<IncomingEthRequest<N>>) -> Self {
        Self {
            client,
            blob_store: Box::<NoopBlobStore>::default(),
            peers,
            incoming_requests: ReceiverStream::new(incoming),
            metrics: Default::default(),
        }
    }

    /// Set blob store for the request handler
    pub fn with_blob_store(mut self, blob_store: Box<dyn BlobStore>) -> Self {
        self.blob_store = blob_store;
        self
    }
}

impl<C, N> EthRequestHandler<C, N>
where
    N: NetworkPrimitives,
    C: BlockReader,
{
    /// Returns the list of requested headers
    fn get_headers_response(&self, request: GetBlockHeaders) -> Vec<C::Header> {
        let GetBlockHeaders { start_block, limit, skip, direction } = request;

        let mut headers = Vec::new();

        let mut block: BlockHashOrNumber = match start_block {
            BlockHashOrNumber::Hash(start) => start.into(),
            BlockHashOrNumber::Number(num) => {
                let Some(hash) = self.client.block_hash(num).unwrap_or_default() else {
                    return headers
                };
                hash.into()
            }
        };

        let skip = skip as u64;
        let mut total_bytes = 0;

        for _ in 0..limit {
            if let Some(header) = self.client.header_by_hash_or_number(block).unwrap_or_default() {
                let number = header.number();
                let parent_hash = header.parent_hash();

                total_bytes += header.length();
                headers.push(header);

                if headers.len() >= MAX_HEADERS_SERVE || total_bytes > SOFT_RESPONSE_LIMIT {
                    break
                }

                match direction {
                    HeadersDirection::Rising => {
                        if let Some(next) = number.checked_add(1).and_then(|n| n.checked_add(skip))
                        {
                            block = next.into()
                        } else {
                            break
                        }
                    }
                    HeadersDirection::Falling => {
                        if skip > 0 {
                            // prevent under flows for block.number == 0 and `block.number - skip <
                            // 0`
                            if let Some(next) =
                                number.checked_sub(1).and_then(|num| num.checked_sub(skip))
                            {
                                block = next.into()
                            } else {
                                break
                            }
                        } else {
                            block = parent_hash.into()
                        }
                    }
                }
            } else {
                break
            }
        }

        headers
    }

    fn on_headers_request(
        &self,
        _peer_id: PeerId,
        request: GetBlockHeaders,
        response: oneshot::Sender<RequestResult<BlockHeaders<C::Header>>>,
    ) {
        self.metrics.eth_headers_requests_received_total.increment(1);
        let headers = self.get_headers_response(request);
        let _ = response.send(Ok(BlockHeaders(headers)));
    }

    fn on_bodies_request(
        &self,
        _peer_id: PeerId,
        request: GetBlockBodies,
        response: oneshot::Sender<RequestResult<BlockBodies<<C::Block as Block>::Body>>>,
    ) {
        self.metrics.eth_bodies_requests_received_total.increment(1);
        let mut bodies = Vec::new();

        let mut total_bytes = 0;

        for hash in request {
            if let Some(block) = self.client.block_by_hash(hash).unwrap_or_default() {
                let body = block.into_body();
                total_bytes += body.length();
                bodies.push(body);

                if bodies.len() >= MAX_BODIES_SERVE || total_bytes > SOFT_RESPONSE_LIMIT {
                    break
                }
            } else {
                break
            }
        }

        let _ = response.send(Ok(BlockBodies(bodies)));
    }

    fn on_receipts_request(
        &self,
        _peer_id: PeerId,
        request: GetReceipts,
        response: oneshot::Sender<RequestResult<Receipts<C::Receipt>>>,
    ) {
        self.metrics.eth_receipts_requests_received_total.increment(1);

        let receipts = self.get_receipts_response(request, |receipts_by_block| {
            receipts_by_block.into_iter().map(ReceiptWithBloom::from).collect::<Vec<_>>()
        });

        let _ = response.send(Ok(Receipts(receipts)));
    }

    fn on_receipts69_request(
        &self,
        _peer_id: PeerId,
        request: GetReceipts,
        response: oneshot::Sender<RequestResult<Receipts69<C::Receipt>>>,
    ) {
        self.metrics.eth_receipts_requests_received_total.increment(1);

        let receipts = self.get_receipts_response(request, |receipts_by_block| {
            // skip bloom filter for eth69
            receipts_by_block
        });

        let _ = response.send(Ok(Receipts69(receipts)));
    }

    /// Handles partial responses for [`GetReceipts70`] queries.
    ///
    /// This will adhere to the soft limit but allow filling the last vec partially.
    fn on_receipts70_request(
        &self,
        _peer_id: PeerId,
        request: GetReceipts70,
        response: oneshot::Sender<RequestResult<Receipts70<C::Receipt>>>,
    ) {
        self.metrics.eth_receipts_requests_received_total.increment(1);

        let GetReceipts70 { first_block_receipt_index, block_hashes } = request;

        let mut receipts = Vec::new();
        let mut total_bytes = 0usize;
        let mut last_block_incomplete = false;

        for (idx, hash) in block_hashes.into_iter().enumerate() {
            if idx >= MAX_RECEIPTS_SERVE {
                break
            }

            let Some(mut block_receipts) =
                self.client.receipts_by_block(BlockHashOrNumber::Hash(hash)).unwrap_or_default()
            else {
                break
            };

            if idx == 0 && first_block_receipt_index > 0 {
                let skip = first_block_receipt_index as usize;
                if skip >= block_receipts.len() {
                    block_receipts.clear();
                } else {
                    block_receipts.drain(0..skip);
                }
            }

            let block_size = block_receipts.length();

            if total_bytes + block_size <= SOFT_RESPONSE_LIMIT {
                total_bytes += block_size;
                receipts.push(block_receipts);
                continue;
            }

            let mut partial_block = Vec::new();
            for receipt in block_receipts {
                let receipt_size = receipt.length();
                if total_bytes + receipt_size > SOFT_RESPONSE_LIMIT {
                    break;
                }
                total_bytes += receipt_size;
                partial_block.push(receipt);
            }

            receipts.push(partial_block);
            last_block_incomplete = true;
            break;
        }

        let _ = response.send(Ok(Receipts70 { last_block_incomplete, receipts }));
    }

    #[inline]
    fn get_receipts_response<T, F>(&self, request: GetReceipts, transform_fn: F) -> Vec<Vec<T>>
    where
        F: Fn(Vec<C::Receipt>) -> Vec<T>,
        T: Encodable,
    {
        let mut receipts = Vec::new();
        let mut total_bytes = 0;

        for hash in request {
            if let Some(receipts_by_block) =
                self.client.receipts_by_block(BlockHashOrNumber::Hash(hash)).unwrap_or_default()
            {
                let transformed_receipts = transform_fn(receipts_by_block);
                total_bytes += transformed_receipts.length();
                receipts.push(transformed_receipts);

                if receipts.len() >= MAX_RECEIPTS_SERVE || total_bytes > SOFT_RESPONSE_LIMIT {
                    break
                }
            } else {
                break
            }
        }

        receipts
    }

    fn on_cells_request(
        &self,
        _peer_id: PeerId,
        request: GetCells,
        response: oneshot::Sender<RequestResult<Cells>>,
    ) {
        let mut cells_response = Cells { cell_mask: request.cell_mask, ..Default::default() };

        for hash in request.hashes.into_iter().take(MAX_CELLS_SERVE) {
            let Some(cells) =
                self.blob_store.get_cells(hash, request.cell_mask).unwrap_or_default()
            else {
                continue;
            };

            cells_response.hashes.push(hash);
            cells_response.cells.push(cells);

            if cells_response.length() > SOFT_RESPONSE_LIMIT {
                break
            }
        }

        let _ = response.send(Ok(cells_response));
    }
}

impl<C, N> EthRequestHandler<C, N>
where
    N: NetworkPrimitives,
    C: BalProvider,
{
    /// Handles [`GetBlockAccessLists`] queries.
    ///
    /// EIP-8159 defines the final `BlockAccessLists` response semantics:
    /// <https://eips.ethereum.org/EIPS/eip-8159>
    fn on_block_access_lists_request(
        &self,
        _peer_id: PeerId,
        mut request: GetBlockAccessLists,
        response: oneshot::Sender<RequestResult<BlockAccessLists>>,
    ) {
        self.metrics.eth_block_access_lists_requests_received_total.increment(1);
        request.0.truncate(MAX_BLOCK_ACCESS_LISTS_SERVE);

        let limit = GetBlockAccessListLimit::ResponseSizeSoftLimit(SOFT_RESPONSE_LIMIT);
        let access_lists =
            self.client.bal_store().get_by_hashes_with_limit(&request.0, limit).unwrap_or_default();
        let _ = response.send(Ok(BlockAccessLists(access_lists)));
    }
}

impl<C, N> EthRequestHandler<C, N>
where
    N: NetworkPrimitives,
    C: BalProvider + StateProviderFactory + StateRangeProvider,
{
    /// Handles `snap/2` (EIP-8189) requests.
    ///
    /// `GetAccountRange`/`GetStorageRanges` are hash-native throughout and served from
    /// [`StateRangeProvider`], for the latest state only. `GetByteCodes` is content-addressed
    /// and independent of any particular state root, so it's served directly.
    /// `GetBlockAccessLists` is answered from the same [`BalProvider`] store eth71's
    /// `GetBlockAccessLists` uses, since both serve the same underlying data.
    fn on_snap_request(
        &self,
        _peer_id: PeerId,
        request: SnapProtocolMessage,
        response: oneshot::Sender<RequestResult<SnapResponse>>,
    ) {
        self.metrics.snap_requests_received_total.increment(1);

        let result = match request {
            SnapProtocolMessage::GetAccountRange(req) => {
                Ok(SnapResponse::AccountRange(self.get_account_range_response(req)))
            }
            SnapProtocolMessage::GetStorageRanges(req) => {
                Ok(SnapResponse::StorageRanges(self.get_storage_ranges_response(req)))
            }
            SnapProtocolMessage::GetByteCodes(req) => {
                let codes = self.get_byte_codes_response(&req.hashes, req.response_bytes as usize);
                Ok(SnapResponse::ByteCodes(ByteCodesMessage { request_id: req.request_id, codes }))
            }
            SnapProtocolMessage::GetBlockAccessLists(mut req) => {
                req.block_hashes.truncate(MAX_BLOCK_ACCESS_LISTS_SERVE);
                let limit = GetBlockAccessListLimit::ResponseSizeSoftLimit(
                    (req.response_bytes as usize).min(SOFT_RESPONSE_LIMIT),
                );
                let block_access_lists = self
                    .client
                    .bal_store()
                    .get_by_hashes_with_limit(&req.block_hashes, limit)
                    .unwrap_or_default();
                Ok(SnapResponse::BlockAccessLists(BlockAccessListsMessage {
                    request_id: req.request_id,
                    block_access_lists: BlockAccessLists(block_access_lists),
                }))
            }
            // The peer sent us a response-shaped message instead of a request; not something we
            // asked for.
            _ => Err(RequestError::BadResponse),
        };

        let _ = response.send(result);
    }

    /// Returns the bytecode for each of `hashes`, skipping hashes with no known code, stopping
    /// once `response_bytes` (capped at [`SOFT_RESPONSE_LIMIT`]) is exceeded.
    fn get_byte_codes_response(&self, hashes: &[B256], response_bytes: usize) -> Vec<Bytes> {
        let Ok(state) = self.client.latest() else { return Vec::new() };
        let response_bytes = response_bytes.min(SOFT_RESPONSE_LIMIT);

        let mut codes = Vec::new();
        let mut total_bytes = 0;
        for hash in hashes.iter().take(MAX_BYTE_CODES_SERVE) {
            let Ok(Some(bytecode)) = state.bytecode_by_hash(hash) else { continue };
            let bytes = bytecode.original_bytes();
            total_bytes += bytes.len();
            codes.push(bytes);

            if total_bytes > response_bytes {
                break
            }
        }
        codes
    }

    /// Serves a `GetAccountRange` request via [`StateRangeProvider`], for the latest state only.
    ///
    /// Skips (rather than serves with a wrong root) any account whose storage root can't be
    /// computed, e.g. because the in-memory reorg overlay is active. A complete range (nothing
    /// left beyond what's returned) needs no proof; a truncated one is proven via a boundary
    /// proof over its first and last account.
    fn get_account_range_response(&self, req: GetAccountRangeMessage) -> AccountRangeMessage {
        let empty = AccountRangeMessage {
            request_id: req.request_id,
            accounts: Vec::new(),
            proof: Vec::new(),
        };

        // We only ever serve the current state, so a request for any other root can't be
        // answered correctly.
        let Ok(Some(state_root)) = self.client.current_state_root() else { return empty };
        if state_root != req.root_hash {
            return empty
        }

        let response_bytes = (req.response_bytes as usize).min(SOFT_RESPONSE_LIMIT);
        let Ok(Some((accounts, complete))) =
            self.client.account_range(req.starting_hash, req.limit_hash, response_bytes)
        else {
            return empty
        };

        let boundary_keys = match (complete, accounts.first(), accounts.last()) {
            (false, Some((first, _)), Some((last, _))) => vec![*first, *last],
            _ => Vec::new(),
        };

        let accounts = accounts
            .into_iter()
            .filter_map(|(hash, account)| {
                let storage_root = self.client.storage_root_by_hash(hash).ok()??;
                let body = alloy_rlp::encode(account.into_trie_account(storage_root)).into();
                Some(AccountData { hash, body })
            })
            .collect();

        let proof = if boundary_keys.is_empty() {
            Vec::new()
        } else {
            self.client.account_range_proof(&boundary_keys).ok().flatten().unwrap_or_default()
        };

        AccountRangeMessage { request_id: req.request_id, accounts, proof }
    }

    /// Serves a `GetStorageRanges` request via [`StateRangeProvider`], for the latest state only.
    ///
    /// `starting_hash` only applies to the first account; later accounts start from zero.
    /// `limit_hash` isn't applied per-account here — the shared `response_bytes` budget across
    /// all accounts is what actually bounds the response, and iteration stops at the first
    /// account whose range doesn't fully complete within that budget.
    fn get_storage_ranges_response(&self, req: GetStorageRangesMessage) -> StorageRangesMessage {
        let empty = StorageRangesMessage {
            request_id: req.request_id,
            slots: Vec::new(),
            proof: Vec::new(),
        };

        // We only ever serve the current state, so a request for any other root can't be
        // answered correctly.
        let Ok(Some(state_root)) = self.client.current_state_root() else { return empty };
        if state_root != req.root_hash {
            return empty
        }

        let mut slots = Vec::new();
        let mut remaining_bytes = (req.response_bytes as usize).min(SOFT_RESPONSE_LIMIT);
        // Boundary keys for the last account processed, unless its range fully completed and it
        // was the last one requested, in which case no proof is needed at all.
        let mut proof_target: Option<(B256, Vec<B256>)> = None;

        for (i, &hashed_address) in req.account_hashes.iter().enumerate() {
            if remaining_bytes == 0 {
                break
            }
            let start = if i == 0 { req.starting_hash } else { B256::ZERO };
            let Ok(Some((account_slots, complete))) = self.client.storage_range(
                hashed_address,
                start,
                B256::repeat_byte(0xff),
                remaining_bytes,
            ) else {
                break
            };

            remaining_bytes = remaining_bytes.saturating_sub(account_slots.len() * 64);
            let is_last_requested = i == req.account_hashes.len() - 1;
            proof_target = if complete && is_last_requested {
                None
            } else {
                match (account_slots.first(), account_slots.last()) {
                    (Some((first, _)), Some((last, _))) => {
                        Some((hashed_address, vec![*first, *last]))
                    }
                    _ => None,
                }
            };

            slots.push(
                account_slots
                    .into_iter()
                    .map(|(hash, value)| StorageData {
                        hash,
                        data: value.to_be_bytes_trimmed_vec().into(),
                    })
                    .collect(),
            );

            if !complete {
                break
            }
        }

        let proof = proof_target
            .and_then(|(hashed_address, keys)| {
                self.client.storage_range_proof(hashed_address, &keys).ok().flatten()
            })
            .unwrap_or_default();

        StorageRangesMessage { request_id: req.request_id, slots, proof }
    }
}

/// An endless future.
///
/// This should be spawned or used as part of `tokio::select!`.
impl<C, N> Future for EthRequestHandler<C, N>
where
    N: NetworkPrimitives,
    C: BalProvider
        + StateProviderFactory
        + StateRangeProvider
        + BlockReader<Block = N::Block, Receipt = N::Receipt>
        + HeaderProvider<Header = N::BlockHeader>
        + Unpin,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        let mut acc = Duration::ZERO;
        let maybe_more_incoming_requests = metered_poll_nested_stream_with_budget!(
            acc,
            "net::eth",
            "Incoming eth requests stream",
            DEFAULT_BUDGET_TRY_DRAIN_DOWNLOADERS,
            this.incoming_requests.poll_next_unpin(cx),
            |incoming| {
                match incoming {
                    IncomingEthRequest::GetBlockHeaders { peer_id, request, response } => {
                        this.on_headers_request(peer_id, request, response)
                    }
                    IncomingEthRequest::GetBlockBodies { peer_id, request, response } => {
                        this.on_bodies_request(peer_id, request, response)
                    }
                    IncomingEthRequest::GetNodeData { .. } => {
                        this.metrics.eth_node_data_requests_received_total.increment(1);
                    }
                    IncomingEthRequest::GetReceipts { peer_id, request, response } => {
                        this.on_receipts_request(peer_id, request, response)
                    }
                    IncomingEthRequest::GetReceipts69 { peer_id, request, response } => {
                        this.on_receipts69_request(peer_id, request, response)
                    }
                    IncomingEthRequest::GetReceipts70 { peer_id, request, response } => {
                        this.on_receipts70_request(peer_id, request, response)
                    }
                    IncomingEthRequest::GetBlockAccessLists { peer_id, request, response } => {
                        this.on_block_access_lists_request(peer_id, request, response)
                    }
                    IncomingEthRequest::GetCells { peer_id, request, response } => {
                        this.on_cells_request(peer_id, request, response)
                    }
                    IncomingEthRequest::GetSnap { peer_id, request, response } => {
                        this.on_snap_request(peer_id, request, response)
                    }
                }
            },
        );

        this.metrics.acc_duration_poll_eth_req_handler.set(acc.as_secs_f64());

        // stream is fully drained and import futures pending
        if maybe_more_incoming_requests {
            // make sure we're woken up again
            cx.waker().wake_by_ref();
        }

        Poll::Pending
    }
}

/// All `eth` request related to blocks delegated by the network.
#[derive(Debug)]
pub enum IncomingEthRequest<N: NetworkPrimitives = EthNetworkPrimitives> {
    /// Request Block headers from the peer.
    ///
    /// The response should be sent through the channel.
    GetBlockHeaders {
        /// The ID of the peer to request block headers from.
        peer_id: PeerId,
        /// The specific block headers requested.
        request: GetBlockHeaders,
        /// The channel sender for the response containing block headers.
        response: oneshot::Sender<RequestResult<BlockHeaders<N::BlockHeader>>>,
    },
    /// Request Block bodies from the peer.
    ///
    /// The response should be sent through the channel.
    GetBlockBodies {
        /// The ID of the peer to request block bodies from.
        peer_id: PeerId,
        /// The specific block bodies requested.
        request: GetBlockBodies,
        /// The channel sender for the response containing block bodies.
        response: oneshot::Sender<RequestResult<BlockBodies<N::BlockBody>>>,
    },
    /// Request Node Data from the peer.
    ///
    /// The response should be sent through the channel.
    GetNodeData {
        /// The ID of the peer to request node data from.
        peer_id: PeerId,
        /// The specific node data requested.
        request: GetNodeData,
        /// The channel sender for the response containing node data.
        response: oneshot::Sender<RequestResult<NodeData>>,
    },
    /// Request Receipts from the peer.
    ///
    /// The response should be sent through the channel.
    GetReceipts {
        /// The ID of the peer to request receipts from.
        peer_id: PeerId,
        /// The specific receipts requested.
        request: GetReceipts,
        /// The channel sender for the response containing receipts.
        response: oneshot::Sender<RequestResult<Receipts<N::Receipt>>>,
    },
    /// Request Receipts from the peer without bloom filter.
    ///
    /// The response should be sent through the channel.
    GetReceipts69 {
        /// The ID of the peer to request receipts from.
        peer_id: PeerId,
        /// The specific receipts requested.
        request: GetReceipts,
        /// The channel sender for the response containing Receipts69.
        response: oneshot::Sender<RequestResult<Receipts69<N::Receipt>>>,
    },
    /// Request Receipts from the peer using eth/70.
    ///
    /// The response should be sent through the channel.
    GetReceipts70 {
        /// The ID of the peer to request receipts from.
        peer_id: PeerId,
        /// The specific receipts requested including the `firstBlockReceiptIndex`.
        request: GetReceipts70,
        /// The channel sender for the response containing Receipts70.
        response: oneshot::Sender<RequestResult<Receipts70<N::Receipt>>>,
    },
    /// Request Block Access Lists from the peer.
    ///
    /// The response should be sent through the channel.
    GetBlockAccessLists {
        /// The ID of the peer to request block access lists from.
        peer_id: PeerId,
        /// The requested block hashes.
        request: GetBlockAccessLists,
        /// The channel sender for the response containing block access lists.
        response: oneshot::Sender<RequestResult<BlockAccessLists>>,
    },
    /// Request Cells from the peer.
    ///
    /// The response should be sent through the channel.
    GetCells {
        /// The ID of the peer to request cells from.
        peer_id: PeerId,
        /// The requested block hashes.
        request: GetCells,
        /// The channel sender for the response containing cells.
        response: oneshot::Sender<RequestResult<Cells>>,
    },
    /// Request a `snap/2` message from the peer.
    ///
    /// The response should be sent through the channel.
    GetSnap {
        /// The ID of the peer to request from.
        peer_id: PeerId,
        /// The `snap/2` request.
        request: SnapProtocolMessage,
        /// The channel sender for the response.
        response: oneshot::Sender<RequestResult<SnapResponse>>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eips::{
        eip4844::{BlobAndProofV1, BlobAndProofV2, BlobCellsAndProofsV1},
        eip7594::{BlobTransactionSidecarVariant, Cell},
    };
    use alloy_primitives::{TxHash, B128};
    use reth_network_api::test_utils::PeersHandle;
    use reth_storage_api::noop::NoopProvider;
    use reth_transaction_pool::blobstore::{BlobStoreCleanupStat, BlobStoreError};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use tokio::sync::mpsc;

    #[derive(Debug, Default)]
    struct CountingBlobStore {
        get_cells_calls: Arc<AtomicUsize>,
    }

    impl BlobStore for CountingBlobStore {
        fn insert(
            &self,
            _tx: B256,
            _data: BlobTransactionSidecarVariant,
        ) -> Result<(), BlobStoreError> {
            Ok(())
        }

        fn insert_all(
            &self,
            _txs: Vec<(B256, BlobTransactionSidecarVariant)>,
        ) -> Result<(), BlobStoreError> {
            Ok(())
        }

        fn delete(&self, _tx: B256) -> Result<(), BlobStoreError> {
            Ok(())
        }

        fn delete_all(&self, _txs: Vec<B256>) -> Result<(), BlobStoreError> {
            Ok(())
        }

        fn cleanup(&self) -> BlobStoreCleanupStat {
            BlobStoreCleanupStat::default()
        }

        fn get(
            &self,
            _tx: B256,
        ) -> Result<Option<Arc<BlobTransactionSidecarVariant>>, BlobStoreError> {
            Ok(None)
        }

        fn contains(&self, _tx: B256) -> Result<bool, BlobStoreError> {
            Ok(false)
        }

        fn get_all(
            &self,
            _txs: Vec<B256>,
        ) -> Result<Vec<(B256, Arc<BlobTransactionSidecarVariant>)>, BlobStoreError> {
            Ok(vec![])
        }

        fn get_exact(
            &self,
            txs: Vec<B256>,
        ) -> Result<Vec<Arc<BlobTransactionSidecarVariant>>, BlobStoreError> {
            if txs.is_empty() {
                return Ok(vec![])
            }

            Err(BlobStoreError::MissingSidecar(txs[0]))
        }

        fn get_by_versioned_hashes_v1(
            &self,
            versioned_hashes: &[B256],
        ) -> Result<Vec<Option<BlobAndProofV1>>, BlobStoreError> {
            Ok(vec![None; versioned_hashes.len()])
        }

        fn get_by_versioned_hashes_v2(
            &self,
            _versioned_hashes: &[B256],
        ) -> Result<Option<Vec<BlobAndProofV2>>, BlobStoreError> {
            Ok(None)
        }

        fn get_by_versioned_hashes_v3(
            &self,
            versioned_hashes: &[B256],
        ) -> Result<Vec<Option<BlobAndProofV2>>, BlobStoreError> {
            Ok(vec![None; versioned_hashes.len()])
        }

        fn get_by_versioned_hashes_v4(
            &self,
            versioned_hashes: &[B256],
            _indices_bitarray: B128,
        ) -> Result<Vec<Option<BlobCellsAndProofsV1>>, BlobStoreError> {
            Ok(vec![None; versioned_hashes.len()])
        }

        fn has_versioned_hashes(
            &self,
            versioned_hashes: &[B256],
        ) -> Result<Vec<bool>, BlobStoreError> {
            Ok(vec![false; versioned_hashes.len()])
        }

        fn get_cells(
            &self,
            _tx_hash: TxHash,
            _indices_bitarray: B128,
        ) -> Result<Option<Vec<Cell>>, BlobStoreError> {
            self.get_cells_calls.fetch_add(1, Ordering::Relaxed);
            Ok(None)
        }

        fn data_size_hint(&self) -> Option<usize> {
            Some(0)
        }

        fn blobs_len(&self) -> usize {
            0
        }
    }

    #[tokio::test]
    async fn get_cells_request_limits_blob_store_lookups() {
        let (peers_tx, _) = mpsc::unbounded_channel();
        let (_incoming_tx, incoming_rx) = mpsc::channel(1);
        let get_cells_calls = Arc::new(AtomicUsize::new(0));
        let blob_store = CountingBlobStore { get_cells_calls: Arc::clone(&get_cells_calls) };
        let handler = EthRequestHandler::<NoopProvider>::new(
            NoopProvider::default(),
            PeersHandle::new(peers_tx),
            incoming_rx,
        )
        .with_blob_store(Box::new(blob_store));
        let (response, rx) = oneshot::channel();
        let request =
            GetCells { hashes: vec![B256::ZERO; MAX_CELLS_SERVE + 1], cell_mask: B128::default() };

        handler.on_cells_request(PeerId::default(), request, response);

        let cells = rx.await.unwrap().unwrap();
        assert!(cells.hashes.is_empty());
        assert_eq!(get_cells_calls.load(Ordering::Relaxed), MAX_CELLS_SERVE);
    }
}
