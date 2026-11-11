//! Blocks/Headers management for the p2p network.

use crate::{
    budget::DEFAULT_BUDGET_TRY_DRAIN_DOWNLOADERS, metered_poll_nested_stream_with_budget,
    metrics::EthRequestHandlerMetrics,
};
use alloy_consensus::{constants::KECCAK_EMPTY, BlockHeader, ReceiptWithBloom};
use alloy_eips::BlockHashOrNumber;
use alloy_primitives::{Bytes, B256};
use alloy_rlp::{length_of_length, Encodable};
use futures::StreamExt;
use reth_eth_wire::{
    message::MAX_MESSAGE_SIZE,
    snap::{
        AccountData, AccountRangeMessage, BlockAccessListsMessage, ByteCodesMessage,
        GetAccountRangeMessage, GetStorageRangesMessage, SnapProtocolMessage, StorageData,
        StorageRangesMessage,
    },
    BlockAccessLists, BlockBodies, BlockHeaders, Cells, EthNetworkPrimitives, GetBlockAccessLists,
    GetBlockBodies, GetBlockHeaders, GetCells, GetNodeData, GetReceipts, GetReceipts70,
    HeadersDirection, NetworkPrimitives, NodeData, Receipts, Receipts69, Receipts70,
};
use reth_network_p2p::{
    error::{RequestError, RequestResult},
    snap::client::SnapResponse,
};
use reth_network_peers::PeerId;
use reth_primitives_traits::Block;
use reth_storage_api::{
    errors::provider::ProviderResult, BalProvider, BlockReader, BytecodeReader,
    GetBlockAccessListLimit, HeaderProvider, RangeEnd, RangeResponse, StateProviderFactory,
    StateRangeProviderFactory,
};
use reth_transaction_pool::{blobstore::NoopBlobStore, BlobStore};
use std::{
    collections::VecDeque,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    sync::{mpsc::Receiver, oneshot},
    task::JoinHandle,
    time::{sleep_until, Instant, Sleep},
};
use tokio_stream::wrappers::ReceiverStream;
use tracing::{debug, error, Span};

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

/// Maximum number of storage range account lookups to serve.
pub const MAX_STORAGE_RANGE_ACCOUNTS_SERVE: usize = 1024;

/// Maximum size of replies to data retrievals: 2MB
pub const SOFT_RESPONSE_LIMIT: usize = 2 * 1024 * 1024;

/// Maximum number of `snap/2` account and storage range requests served at once.
///
/// Serving an old root can rewind the trie block by block, so these requests run off the handler
/// task and are capped to keep them from starving other blocking work.
const MAX_CONCURRENT_STATE_RANGE_REQUESTS: usize = 2;

/// Maximum number of `snap/2` range requests waiting for a serving slot, one per peer.
///
/// They wait because snap clients read an empty response as the root being unavailable, and then
/// leave the peer alone until they retry.
const MAX_QUEUED_STATE_RANGE_REQUESTS: usize = 16;

/// How long a `snap/2` range request may wait for a serving slot before it is answered empty.
///
/// A requester's timeout adapts down to two seconds and also covers transit and serving, so the
/// wait takes only a fraction of it. A slot is otherwise spent on a response nobody awaits.
const STATE_RANGE_REQUEST_QUEUE_TIMEOUT: Duration = Duration::from_millis(500);

/// Upper bound on the headers reserved once the first requested header resolves.
///
/// Header responses are bound by [`MAX_HEADERS_SERVE`] rather than by size, so a full batch reaches
/// its final capacity within a few doublings while a batch that runs past the local tip wastes at
/// most this many slots.
const MAX_HEADERS_RESERVE: usize = 64;

/// Upper bound on the block bodies or receipt lists reserved once the first requested block
/// resolves.
///
/// These responses are bound by [`SOFT_RESPONSE_LIMIT`], so the first block's size predicts how
/// many fit and this only caps the estimate for tiny blocks.
const MAX_BLOCKS_RESERVE: usize = 16;

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
    /// Incoming request from the [`NetworkManager`](crate::NetworkManager).
    incoming_requests: ReceiverStream<IncomingEthRequest<N>>,
    /// Metrics for the eth request handler.
    metrics: EthRequestHandlerMetrics,
    /// `snap/2` range requests running or waiting off the handler task.
    state_range_requests: StateRangeRequests,
}

// === impl EthRequestHandler ===
impl<C, N: NetworkPrimitives> EthRequestHandler<C, N> {
    /// Create a new instance
    pub fn new(client: C, incoming: Receiver<IncomingEthRequest<N>>) -> Self {
        Self {
            client,
            blob_store: Box::<NoopBlobStore>::default(),
            incoming_requests: ReceiverStream::new(incoming),
            metrics: Default::default(),
            state_range_requests: Default::default(),
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
                let len = header.length();

                if headers.is_empty() {
                    let count = limit.min(MAX_HEADERS_SERVE as u64) as usize;
                    headers.reserve(response_reserve_hint(count, len, MAX_HEADERS_RESERVE));
                }

                total_bytes += len;
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
        let count = request.0.len();
        let mut bodies = Vec::new();

        let mut total_bytes = 0;

        for hash in request {
            if let Some(block) = self.client.block_by_hash(hash).unwrap_or_default() {
                let body = block.into_body();
                let len = body.length();
                if bodies.is_empty() {
                    bodies.reserve(response_reserve_hint(count, len, MAX_BLOCKS_RESERVE));
                }
                total_bytes += len;
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

    /// Replies to `GetNodeData`.
    ///
    /// State serving via eth `GetNodeData` was removed; answer with an empty payload so eth/66
    /// peers receive a response instead of hanging until the request timeout when the oneshot is
    /// dropped unanswered.
    fn on_node_data_request(
        &self,
        _peer_id: PeerId,
        _request: GetNodeData,
        response: oneshot::Sender<RequestResult<NodeData>>,
    ) {
        self.metrics.eth_node_data_requests_received_total.increment(1);
        let _ = response.send(Ok(NodeData(vec![])));
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
    /// This will adhere to the soft limit but allow filling the last vec partially. A response
    /// never ends with an empty incomplete block: if not even the next receipt fits, the response
    /// ends after the last complete block, except when it would otherwise be empty, in which case
    /// the first receipt is served even if it exceeds the soft limit. If it cannot fit within
    /// [`MAX_MESSAGE_SIZE`], including the response framing, the response is empty and complete.
    fn on_receipts70_request(
        &self,
        _peer_id: PeerId,
        request: GetReceipts70,
        response: oneshot::Sender<RequestResult<Receipts70<C::Receipt>>>,
    ) {
        self.metrics.eth_receipts_requests_received_total.increment(1);

        let GetReceipts70 { first_block_receipt_index, block_hashes } = request;

        let count = block_hashes.len();
        let mut receipts = Vec::new();
        let mut total_bytes = 0usize;
        let mut last_block_incomplete = false;

        // The request ID is unavailable here. Reserve the message code, the largest request ID,
        // the incomplete flag, and the three RLP list headers to bound the full encoded response.
        let max_first_receipt_size = MAX_MESSAGE_SIZE -
            (1 + u64::MAX.length() + false.length() + 3 * length_of_length(MAX_MESSAGE_SIZE));

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
            if receipts.is_empty() {
                receipts.reserve(response_reserve_hint(count, block_size, MAX_BLOCKS_RESERVE));
            }

            if total_bytes + block_size <= SOFT_RESPONSE_LIMIT {
                total_bytes += block_size;
                receipts.push(block_receipts);
                continue;
            }

            // If nothing has been added to the response yet, serve the first receipt even if it
            // exceeds the soft limit, provided the framed response fits within the hard limit.
            let always_serve_first = receipts.is_empty();
            let remaining = block_receipts.len();
            let mut partial_block = Vec::new();
            for receipt in block_receipts {
                let receipt_size = receipt.length();
                if total_bytes + receipt_size > SOFT_RESPONSE_LIMIT &&
                    !(always_serve_first &&
                        partial_block.is_empty() &&
                        receipt_size <= max_first_receipt_size)
                {
                    break;
                }
                total_bytes += receipt_size;
                partial_block.push(receipt);
            }

            // An empty partial block delivers nothing, so it is never sent. The requester can
            // simply continue with this block in its next request.
            if !partial_block.is_empty() {
                last_block_incomplete = partial_block.len() < remaining;
                receipts.push(partial_block);
            }
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
        let count = request.0.len();
        let mut receipts = Vec::new();
        let mut total_bytes = 0;

        for hash in request {
            if let Some(receipts_by_block) =
                self.client.receipts_by_block(BlockHashOrNumber::Hash(hash)).unwrap_or_default()
            {
                let transformed_receipts = transform_fn(receipts_by_block);
                let len = transformed_receipts.length();
                if receipts.is_empty() {
                    receipts.reserve(response_reserve_hint(count, len, MAX_BLOCKS_RESERVE));
                }
                total_bytes += len;
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
        let mut total_bytes = 0;
        let cell_mask = request.cell_mask();

        for hash in request.hashes.into_iter().take(MAX_CELLS_SERVE) {
            let Some(cells) = self.blob_store.get_cells(hash, cell_mask).unwrap_or_default() else {
                continue;
            };

            total_bytes += hash.length() + cells.length();
            cells_response.hashes.push(hash);
            cells_response.cells.push(cells);

            if total_bytes > SOFT_RESPONSE_LIMIT {
                break
            }
        }

        let _ = response.send(Ok(cells_response));
    }
}

/// Number of response items to reserve once the first requested item has resolved.
///
/// Assumes the remaining items are about as large as the first one and reserves as many as fit into
/// [`SOFT_RESPONSE_LIMIT`], bounded by the requested `count` and `max`.
fn response_reserve_hint(count: usize, first_len: usize, max: usize) -> usize {
    SOFT_RESPONSE_LIMIT.div_ceil(first_len.max(1)).min(count).min(max)
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
            self.client.get_bals_by_hashes_with_limit(&request.0, limit).unwrap_or_default();
        let _ = response.send(Ok(BlockAccessLists(access_lists)));
    }
}

impl<C, N> EthRequestHandler<C, N>
where
    N: NetworkPrimitives,
    C: BalProvider + StateProviderFactory + StateRangeProviderFactory + Clone + 'static,
{
    /// Handles `snap/2` (EIP-8189) requests.
    ///
    /// `GetAccountRange`/`GetStorageRanges` are hash-native throughout and served from retained
    /// canonical roots via [`StateRangeProviderFactory`], on blocking tasks so that rewinding an
    /// old root doesn't stall other requests. `GetByteCodes` is content-addressed and
    /// independent of any particular state root, so it's served directly.
    /// `GetBlockAccessLists` is answered from the same [`BalProvider`] store eth71's
    /// `GetBlockAccessLists` uses, since both serve the same underlying data.
    fn on_snap_request(
        &mut self,
        peer_id: PeerId,
        request: SnapProtocolMessage,
        response: oneshot::Sender<RequestResult<SnapResponse>>,
    ) {
        self.metrics.snap_requests_received_total.increment(1);

        let result = match request {
            SnapProtocolMessage::GetAccountRange(req) => {
                return self.on_state_range_request(
                    peer_id,
                    StateRangeRequest::Account(req),
                    response,
                )
            }
            SnapProtocolMessage::GetStorageRanges(mut req) => {
                // Accounts past the serving limit are never read, so a queued request releases
                // them along with the allocation the peer sized.
                req.account_hashes.truncate(MAX_STORAGE_RANGE_ACCOUNTS_SERVE);
                req.account_hashes.shrink_to_fit();
                return self.on_state_range_request(
                    peer_id,
                    StateRangeRequest::Storage(req),
                    response,
                )
            }
            SnapProtocolMessage::GetByteCodes(req) => {
                let codes = self
                    .get_byte_codes_response(&req.hashes, req.response_bytes as usize)
                    .unwrap_or_else(|error| {
                        tracing::debug!(target: "net::snap", %error, "failed to serve bytecodes");
                        Vec::new()
                    });
                Ok(SnapResponse::ByteCodes(ByteCodesMessage { request_id: req.request_id, codes }))
            }
            SnapProtocolMessage::GetBlockAccessLists(mut req) => {
                req.block_hashes.truncate(MAX_BLOCK_ACCESS_LISTS_SERVE);
                let limit = GetBlockAccessListLimit::ResponseSizeSoftLimit(
                    (req.response_bytes as usize).min(SOFT_RESPONSE_LIMIT),
                );
                let block_access_lists = self
                    .client
                    .get_bals_by_hashes_with_limit(&req.block_hashes, limit)
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

    /// Hands a `snap/2` range request to [`StateRangeRequests`].
    fn on_state_range_request(
        &mut self,
        peer_id: PeerId,
        request: StateRangeRequest,
        response: oneshot::Sender<RequestResult<SnapResponse>>,
    ) {
        let incoming =
            IncomingStateRangeRequest { peer_id, request, response, received: Instant::now() };
        self.state_range_requests.admit(incoming, &self.client, &self.metrics);
    }

    /// Returns the bytecode for each of `hashes`, skipping hashes with no known code, stopping
    /// once `response_bytes` (capped at [`SOFT_RESPONSE_LIMIT`]) is exceeded.
    fn get_byte_codes_response(
        &self,
        hashes: &[B256],
        response_bytes: usize,
    ) -> ProviderResult<Vec<Bytes>> {
        let state = self.client.latest()?;
        let response_bytes = response_bytes.min(SOFT_RESPONSE_LIMIT);

        let mut codes = Vec::new();
        let mut total_bytes = 0;
        for hash in hashes.iter().take(MAX_BYTE_CODES_SERVE) {
            let bytes = if *hash == KECCAK_EMPTY {
                Bytes::new()
            } else {
                match state.bytecode_by_hash(hash)? {
                    Some(bytecode) => bytecode.original_bytes(),
                    None => continue,
                }
            };
            total_bytes += bytes.len();
            codes.push(bytes);

            if total_bytes > response_bytes {
                break
            }
        }
        Ok(codes)
    }
}

/// Boundary-proof keys for a range reply: `origin`, plus the last returned item's key if any.
fn boundary_proof_keys<T>(origin: B256, last: Option<&(B256, T)>) -> Vec<B256> {
    match last {
        Some((last, _)) => vec![origin, *last],
        None => vec![origin],
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
        + StateRangeProviderFactory
        + BlockReader<Block = N::Block, Receipt = N::Receipt>
        + HeaderProvider<Header = N::BlockHeader>
        + Clone
        + Unpin
        + 'static,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        // Frees the slots of finished state range tasks first, so the requests admitted below see
        // them and queued requests keep their turn.
        this.state_range_requests.poll(cx, &this.client, &this.metrics);

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
                    IncomingEthRequest::GetNodeData { peer_id, request, response } => {
                        this.on_node_data_request(peer_id, request, response)
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

        // Polled again so that the tasks and the queue timeout of requests admitted above wake this
        // handler.
        this.state_range_requests.poll(cx, &this.client, &this.metrics);

        // stream is fully drained and import futures pending
        if maybe_more_incoming_requests {
            // make sure we're woken up again
            cx.waker().wake_by_ref();
        }

        Poll::Pending
    }
}

/// `snap/2` state range requests served off the handler task.
///
/// Unlike other requests, serving a range for an old root rewinds the state to that block, which
/// can take seconds and would stall every peer if it ran on the handler task.
///
/// At most [`MAX_CONCURRENT_STATE_RANGE_REQUESTS`] run at once, one per peer. A request that can't
/// start waits in a queue of at most [`MAX_QUEUED_STATE_RANGE_REQUESTS`], one place per peer, for
/// at most [`STATE_RANGE_REQUEST_QUEUE_TIMEOUT`].
#[derive(Debug, Default)]
struct StateRangeRequests {
    /// Requests being served on blocking tasks, with the peer each belongs to.
    tasks: Vec<(PeerId, JoinHandle<()>)>,
    /// Requests waiting for a slot in `tasks`, oldest first.
    queue: VecDeque<IncomingStateRangeRequest>,
    /// Wakes the handler when the oldest queued request times out.
    queue_timeout: Option<Pin<Box<Sleep>>>,
}

impl StateRangeRequests {
    /// Starts `incoming` or queues it until a slot is free.
    ///
    /// A request whose peer has already gone is dropped. One that finds the queue full, or its
    /// peer already holding a place in it, is answered empty.
    fn admit<C>(
        &mut self,
        incoming: IncomingStateRangeRequest,
        client: &C,
        metrics: &EthRequestHandlerMetrics,
    ) where
        C: StateRangeProviderFactory + Clone + Send + 'static,
    {
        if incoming.response.is_closed() {
            return
        }
        if self.can_start(incoming.peer_id) {
            return self.spawn(incoming, client)
        }

        self.remove_stale(metrics);
        if self.queue.len() >= MAX_QUEUED_STATE_RANGE_REQUESTS ||
            self.queue.iter().any(|queued| queued.peer_id == incoming.peer_id)
        {
            debug!(
                target: "net::snap",
                peer_id=?incoming.peer_id,
                queued=self.queue.len(),
                "no room to queue state range request"
            );
            return incoming.reply_empty(metrics)
        }
        self.queue.push_back(incoming);
    }

    /// Returns whether a request from `peer_id` can start now.
    fn can_start(&self, peer_id: PeerId) -> bool {
        self.tasks.len() < MAX_CONCURRENT_STATE_RANGE_REQUESTS &&
            self.tasks.iter().all(|(running, _)| *running != peer_id)
    }

    /// Serves `incoming` on a blocking task, which holds its peer's slot until it finishes.
    fn spawn<C>(&mut self, incoming: IncomingStateRangeRequest, client: &C)
    where
        C: StateRangeProviderFactory + Clone + Send + 'static,
    {
        let IncomingStateRangeRequest { peer_id, request, response, .. } = incoming;
        let client = client.clone();
        let span = Span::current();
        let task = tokio::task::spawn_blocking(move || {
            let _guard = span.enter();
            let empty = request.empty_response();
            let result = request.serve(&client).unwrap_or_else(|error| {
                debug!(target: "net::snap", %error, "failed to serve state range");
                empty
            });
            let _ = response.send(Ok(result));
        });
        self.tasks.push((peer_id, task));
    }

    /// Drops queued requests whose peer has gone, and answers empty those that waited longer than
    /// [`STATE_RANGE_REQUEST_QUEUE_TIMEOUT`].
    fn remove_stale(&mut self, metrics: &EthRequestHandlerMetrics) {
        let mut index = 0;
        while index < self.queue.len() {
            let queued = &self.queue[index];
            if queued.response.is_closed() {
                self.queue.remove(index);
            } else if queued.received.elapsed() < STATE_RANGE_REQUEST_QUEUE_TIMEOUT {
                index += 1;
            } else if let Some(incoming) = self.queue.remove(index) {
                debug!(
                    target: "net::snap",
                    peer_id=?incoming.peer_id,
                    "state range request timed out in queue"
                );
                incoming.reply_empty(metrics);
            }
        }
    }

    /// Frees the slots of finished tasks and starts queued requests, oldest first, in their place.
    ///
    /// Registers `cx` to be woken when a task finishes or the oldest queued request times out.
    fn poll<C>(&mut self, cx: &mut Context<'_>, client: &C, metrics: &EthRequestHandlerMetrics)
    where
        C: StateRangeProviderFactory + Clone + Send + 'static,
    {
        // Runs again after starting queued requests so that their tasks wake this context, and
        // after the queue timeout fires so that it is armed for the next request.
        loop {
            self.remove_stale(metrics);
            self.tasks.retain_mut(|(_, task)| match Pin::new(task).poll(cx) {
                Poll::Pending => true,
                Poll::Ready(result) => {
                    if let Err(error) = result {
                        error!(target: "net::snap", %error, "state range task failed");
                    }
                    false
                }
            });

            let mut spawned = false;
            let mut index = 0;
            while index < self.queue.len() && self.tasks.len() < MAX_CONCURRENT_STATE_RANGE_REQUESTS
            {
                // A peer with a request still running keeps its place and is passed over.
                if !self.can_start(self.queue[index].peer_id) {
                    index += 1;
                } else if let Some(incoming) = self.queue.remove(index) {
                    self.spawn(incoming, client);
                    spawned = true;
                }
            }
            if !spawned && !self.poll_queue_timeout(cx) {
                break
            }
        }
    }

    /// Arms the timeout of the oldest queued request, returning whether it already fired.
    fn poll_queue_timeout(&mut self, cx: &mut Context<'_>) -> bool {
        let Some(oldest) = self.queue.front() else {
            self.queue_timeout = None;
            return false
        };
        let deadline = oldest.received + STATE_RANGE_REQUEST_QUEUE_TIMEOUT;
        let timeout = self.queue_timeout.get_or_insert_with(|| Box::pin(sleep_until(deadline)));
        timeout.as_mut().reset(deadline);
        timeout.as_mut().poll(cx).is_ready()
    }
}

/// A `snap/2` range request waiting for, or holding, a serving slot.
#[derive(Debug)]
struct IncomingStateRangeRequest {
    /// Peer that sent the request.
    peer_id: PeerId,
    /// The range request the peer sent.
    request: StateRangeRequest,
    /// Channel the response is sent on.
    response: oneshot::Sender<RequestResult<SnapResponse>>,
    /// When the request arrived, which bounds how long it may wait in the queue.
    received: Instant,
}

impl IncomingStateRangeRequest {
    /// Answers the request empty without serving it.
    fn reply_empty(self, metrics: &EthRequestHandlerMetrics) {
        metrics.snap_state_range_requests_unserved_total.increment(1);
        let _ = self.response.send(Ok(self.request.empty_response()));
    }
}

/// A `snap/2` request for a range of state at a given root.
#[derive(Debug)]
enum StateRangeRequest {
    /// A `GetAccountRange` request.
    Account(GetAccountRangeMessage),
    /// A `GetStorageRanges` request.
    Storage(GetStorageRangesMessage),
}

impl StateRangeRequest {
    /// Returns the response for a request that isn't served, which snap clients read as the root
    /// being unavailable.
    const fn empty_response(&self) -> SnapResponse {
        match self {
            Self::Account(req) => SnapResponse::AccountRange(AccountRangeMessage {
                request_id: req.request_id,
                accounts: Vec::new(),
                proof: Vec::new(),
            }),
            Self::Storage(req) => SnapResponse::StorageRanges(StorageRangesMessage {
                request_id: req.request_id,
                slots: Vec::new(),
                proof: Vec::new(),
            }),
        }
    }

    /// Serves the range from `client`.
    fn serve(self, client: &impl StateRangeProviderFactory) -> ProviderResult<SnapResponse> {
        match self {
            Self::Account(req) => {
                Self::serve_account_range(client, req).map(SnapResponse::AccountRange)
            }
            Self::Storage(req) => {
                Self::serve_storage_ranges(client, req).map(SnapResponse::StorageRanges)
            }
        }
    }

    /// Serves a `GetAccountRange` request via [`StateRangeProviderFactory`].
    ///
    /// Fails the request if a storage root or proof becomes unavailable after the range lookup.
    /// Proves the boundary between `starting_hash` and the last returned account, per snap/2's
    /// boundary-proof requirement, unless the range is a zero-origin range that exhausted the
    /// trie, in which case no proof is needed.
    fn serve_account_range(
        client: &impl StateRangeProviderFactory,
        req: GetAccountRangeMessage,
    ) -> ProviderResult<AccountRangeMessage> {
        let empty = AccountRangeMessage {
            request_id: req.request_id,
            accounts: Vec::new(),
            proof: Vec::new(),
        };

        let response_bytes = (req.response_bytes as usize).min(SOFT_RESPONSE_LIMIT);
        let Some(state) = client.state_range_provider(req.root_hash)? else { return Ok(empty) };
        let RangeResponse { items: accounts, end } =
            state.account_range(req.starting_hash, req.limit_hash, response_bytes)?;

        let proof = if req.starting_hash.is_zero() && end == RangeEnd::Exhausted {
            Vec::new()
        } else {
            let boundary_keys = boundary_proof_keys(req.starting_hash, accounts.last());
            state.account_range_proof(&boundary_keys)?
        };

        let mut account_data = Vec::with_capacity(accounts.len());
        for (hash, account) in accounts {
            let storage_root = state.storage_root_by_hash(hash)?;
            account_data.push(AccountData::from_trie_account(
                hash,
                &account.into_trie_account(storage_root),
            ));
        }

        Ok(AccountRangeMessage { request_id: req.request_id, accounts: account_data, proof })
    }

    /// Serves a `GetStorageRanges` request via [`StateRangeProviderFactory`].
    ///
    /// `starting_hash`/`limit_hash` apply only to the first account. An account unavailable at
    /// this root makes the whole response empty, rather than skipping it and shifting later
    /// accounts' positions. A proof stops the response at the first account that isn't a
    /// complete, zero-origin range.
    fn serve_storage_ranges(
        client: &impl StateRangeProviderFactory,
        req: GetStorageRangesMessage,
    ) -> ProviderResult<StorageRangesMessage> {
        let empty = StorageRangesMessage {
            request_id: req.request_id,
            slots: Vec::new(),
            proof: Vec::new(),
        };
        let Some(state) = client.state_range_provider(req.root_hash)? else { return Ok(empty) };
        let mut slots: Vec<Vec<StorageData>> = Vec::new();
        let mut proof = Vec::new();
        let mut remaining_bytes = (req.response_bytes as usize).min(SOFT_RESPONSE_LIMIT);

        for (i, &hashed_address) in
            req.account_hashes.iter().take(MAX_STORAGE_RANGE_ACCOUNTS_SERVE).enumerate()
        {
            // Keeps traversing storage-empty accounts under a zero budget so a real slot still
            // gets served; worst case is a lookup per account, bounded by the take() above.
            if remaining_bytes == 0 && slots.iter().any(|range| !range.is_empty()) {
                break
            }
            let (origin, limit) = if i == 0 {
                (
                    req.starting_hash.unwrap_or(B256::ZERO),
                    req.limit_hash.unwrap_or(B256::repeat_byte(0xff)),
                )
            } else {
                (B256::ZERO, B256::repeat_byte(0xff))
            };
            let Some(RangeResponse { items: account_slots, end }) =
                state.storage_range(hashed_address, origin, limit, remaining_bytes)?
            else {
                return Ok(empty)
            };

            remaining_bytes = remaining_bytes.saturating_sub(account_slots.len() * 64);
            let last = account_slots.last().map(|(hash, _)| *hash);
            let needs_proof = !origin.is_zero() || end != RangeEnd::Exhausted;
            slots.push(
                account_slots
                    .into_iter()
                    // snap clients verify proofs against RLP-encoded storage trie leaves.
                    .map(|(hash, value)| StorageData::from_value(hash, value))
                    .collect(),
            );

            if needs_proof {
                let boundary_keys = match last {
                    Some(last) => vec![origin, last],
                    None => vec![origin],
                };
                proof = state.storage_range_proof(hashed_address, &boundary_keys)?;
                break
            }
        }

        Ok(StorageRangesMessage { request_id: req.request_id, slots, proof })
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
    use alloy_consensus::{constants::EMPTY_ROOT_HASH, Header, TxType};
    use alloy_eips::{
        eip4844::{BlobAndProofV1, BlobAndProofV2, BlobCellsAndProofsV1},
        eip7594::{BlobCellMask, BlobTransactionSidecarVariant, Cell},
    };
    use alloy_primitives::{keccak256, Address, Log, LogData, TxHash, B128, U256};
    use alloy_rlp::{Decodable, Encodable};
    use reth_eth_wire::{message::RequestPair, EthMessage, EthStreamInner, EthVersion};
    use reth_ethereum_primitives::Receipt;
    use reth_primitives_traits::Account;
    use reth_provider::test_utils::{ExtendedAccount, MockEthProvider};
    use reth_storage_api::noop::NoopProvider;
    use reth_transaction_pool::blobstore::{
        BlobStoreCleanupStat, BlobStoreError, PooledBlobSidecar,
    };
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use test_case::test_case;
    use tokio::sync::mpsc;

    #[derive(Debug, Default)]
    struct CountingBlobStore {
        get_cells_calls: Arc<AtomicUsize>,
        expected_cell_mask: Option<BlobCellMask>,
    }

    impl BlobStore for CountingBlobStore {
        fn insert(&self, _tx: B256, _data: PooledBlobSidecar) -> Result<(), BlobStoreError> {
            Ok(())
        }

        fn insert_all(&self, _txs: Vec<(B256, PooledBlobSidecar)>) -> Result<(), BlobStoreError> {
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
            _cell_mask: BlobCellMask,
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
            cell_mask: BlobCellMask,
        ) -> Result<Option<Vec<Cell>>, BlobStoreError> {
            self.get_cells_calls.fetch_add(1, Ordering::Relaxed);
            if let Some(expected) = self.expected_cell_mask {
                assert_eq!(cell_mask, expected);
                return Ok(Some(vec![Cell::default()]))
            }
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
        let (_incoming_tx, incoming_rx) = mpsc::channel(1);
        let get_cells_calls = Arc::new(AtomicUsize::new(0));
        let blob_store = CountingBlobStore {
            get_cells_calls: Arc::clone(&get_cells_calls),
            ..Default::default()
        };
        let handler = EthRequestHandler::<NoopProvider>::new(NoopProvider::default(), incoming_rx)
            .with_blob_store(Box::new(blob_store));
        let (response, rx) = oneshot::channel();
        let request =
            GetCells { hashes: vec![B256::ZERO; MAX_CELLS_SERVE + 1], cell_mask: B128::default() };

        handler.on_cells_request(PeerId::default(), request, response);

        let cells = rx.await.unwrap().unwrap();
        assert!(cells.hashes.is_empty());
        assert_eq!(get_cells_calls.load(Ordering::Relaxed), MAX_CELLS_SERVE);
    }

    #[test_case(0)]
    #[test_case(7)]
    #[test_case(8)]
    #[test_case(63)]
    #[test_case(64)]
    #[test_case(127)]
    #[tokio::test]
    async fn get_cells_request_uses_little_endian_wire_mask(index: u32) {
        let (_incoming_tx, incoming_rx) = mpsc::channel(1);
        let numeric_mask = 1u128 << index;
        let blob_store = CountingBlobStore {
            expected_cell_mask: Some(BlobCellMask::from_bits(numeric_mask)),
            ..Default::default()
        };
        let handler = EthRequestHandler::<NoopProvider>::new(NoopProvider::default(), incoming_rx)
            .with_blob_store(Box::new(blob_store));
        let wire_mask = B128::from(numeric_mask.to_le_bytes());
        let request = GetCells { hashes: vec![B256::ZERO], cell_mask: wire_mask };
        let mut encoded = Vec::new();
        request.encode(&mut encoded);
        let request = GetCells::decode(&mut encoded.as_slice()).unwrap();
        let (response, rx) = oneshot::channel();

        handler.on_cells_request(PeerId::default(), request, response);

        let cells = rx.await.unwrap().unwrap();
        assert_eq!(cells.hashes, vec![B256::ZERO]);
        assert_eq!(cells.cells, vec![vec![Cell::default()]]);
        assert_eq!(cells.cell_mask, wire_mask);
        encoded.clear();
        cells.encode(&mut encoded);
        assert_eq!(Cells::decode(&mut encoded.as_slice()).unwrap(), cells);
    }

    #[tokio::test]
    async fn get_node_data_responds_with_empty_payload() {
        let (_incoming_tx, incoming_rx) = mpsc::channel(1);
        let handler = EthRequestHandler::<NoopProvider>::new(NoopProvider::default(), incoming_rx);
        let (response, rx) = oneshot::channel();

        handler.on_node_data_request(PeerId::default(), GetNodeData(vec![B256::ZERO]), response);

        let node_data = rx.await.expect("response channel must not be dropped").unwrap();
        assert!(node_data.0.is_empty(), "GetNodeData should reply with empty NodeData");
    }

    /// Creates a request handler backed by the mock provider for snap response tests.
    fn snap_handler(
        provider: MockEthProvider,
    ) -> EthRequestHandler<MockEthProvider, EthNetworkPrimitives> {
        let (_incoming_tx, incoming_rx) = mpsc::channel(1);
        EthRequestHandler::new(provider, incoming_rx)
    }

    #[test_case(
        SnapProtocolMessage::GetAccountRange(GetAccountRangeMessage {
            request_id: 1,
            root_hash: B256::ZERO,
            starting_hash: B256::ZERO,
            limit_hash: B256::repeat_byte(0xff),
            response_bytes: SOFT_RESPONSE_LIMIT as u64,
        }),
        SnapResponse::AccountRange(AccountRangeMessage {
            request_id: 1,
            accounts: Vec::new(),
            proof: Vec::new(),
        }); "account range"
    )]
    #[test_case(
        SnapProtocolMessage::GetStorageRanges(GetStorageRangesMessage {
            request_id: 2,
            root_hash: B256::ZERO,
            account_hashes: vec![B256::ZERO],
            starting_hash: B256::ZERO.into(),
            limit_hash: B256::repeat_byte(0xff).into(),
            response_bytes: SOFT_RESPONSE_LIMIT as u64,
        }),
        SnapResponse::StorageRanges(StorageRangesMessage {
            request_id: 2,
            slots: Vec::new(),
            proof: Vec::new(),
        }); "storage ranges"
    )]
    #[test_case(
        SnapProtocolMessage::GetByteCodes(reth_eth_wire::snap::GetByteCodesMessage {
            request_id: 3,
            hashes: vec![B256::repeat_byte(0x11)],
            response_bytes: SOFT_RESPONSE_LIMIT as u64,
        }),
        SnapResponse::ByteCodes(ByteCodesMessage { request_id: 3, codes: Vec::new() }); "bytecodes"
    )]
    #[tokio::test]
    async fn snap_requests_return_empty_responses_on_provider_errors(
        request: SnapProtocolMessage,
        expected: SnapResponse,
    ) {
        let provider = MockEthProvider::default();
        provider.set_snap_state_reads_fail(true);
        let mut handler = snap_handler(provider);
        let (response, rx) = oneshot::channel();

        handler.on_snap_request(PeerId::default(), request, response);

        assert_eq!(rx.await.unwrap(), Ok(expected));
    }

    #[tokio::test]
    async fn unavailable_snap_state_returns_empty_response() {
        let provider = MockEthProvider::default();
        let mut handler = snap_handler(provider.clone());
        let (response, rx) = oneshot::channel();
        handler.on_snap_request(
            PeerId::default(),
            SnapProtocolMessage::GetAccountRange(GetAccountRangeMessage {
                request_id: 1,
                root_hash: B256::ZERO,
                starting_hash: B256::ZERO,
                limit_hash: B256::repeat_byte(0xff),
                response_bytes: SOFT_RESPONSE_LIMIT as u64,
            }),
            response,
        );

        assert_eq!(
            rx.await.unwrap(),
            Ok(SnapResponse::AccountRange(AccountRangeMessage {
                request_id: 1,
                accounts: Vec::new(),
                proof: Vec::new(),
            }))
        );
        assert_eq!(provider.snap_state_range_resolutions(), 1);
    }

    fn account_range_request() -> SnapProtocolMessage {
        SnapProtocolMessage::GetAccountRange(GetAccountRangeMessage {
            request_id: 1,
            root_hash: B256::ZERO,
            starting_hash: B256::ZERO,
            limit_hash: B256::repeat_byte(0xff),
            response_bytes: SOFT_RESPONSE_LIMIT as u64,
        })
    }

    fn empty_account_range() -> SnapResponse {
        SnapResponse::AccountRange(AccountRangeMessage {
            request_id: 1,
            accounts: Vec::new(),
            proof: Vec::new(),
        })
    }

    #[tokio::test]
    async fn busy_state_range_request_waits_for_a_slot() {
        let provider = MockEthProvider::default();
        let mut handler = snap_handler(provider.clone());
        // Every slot is taken, and the requesting peer holds the one that finishes on `release`.
        let peer_id = PeerId::default();
        let (release, released) = oneshot::channel::<()>();
        handler.state_range_requests.tasks.push((
            peer_id,
            tokio::spawn(async move {
                let _ = released.await;
            }),
        ));
        let busy_peer = PeerId::repeat_byte(1);
        for peer in std::iter::once(busy_peer).chain((2..).map(PeerId::repeat_byte)) {
            if handler.state_range_requests.tasks.len() == MAX_CONCURRENT_STATE_RANGE_REQUESTS {
                break
            }
            handler
                .state_range_requests
                .tasks
                .push((peer, tokio::spawn(futures::future::pending())));
        }
        // A peer whose request keeps running is queued first, and must be passed over.
        let (response, mut passed_over) = oneshot::channel();
        handler.on_snap_request(busy_peer, account_range_request(), response);
        let (response, mut rx) = oneshot::channel();
        handler.on_snap_request(peer_id, account_range_request(), response);

        assert_eq!(handler.state_range_requests.queue.len(), 2);
        assert_eq!(provider.snap_state_range_resolutions(), 0);

        release.send(()).unwrap();
        // Only the queue is polled, so the request it starts is served and its slot freed only if
        // that task wakes this one.
        let EthRequestHandler { state_range_requests, client, metrics, .. } = &mut handler;
        let wait = Duration::from_secs(5);
        let started = Instant::now();
        let mut served = None;
        tokio::time::timeout(
            wait,
            std::future::poll_fn(|cx| {
                state_range_requests.poll(cx, client, metrics);
                if served.is_none() {
                    served = rx.try_recv().ok();
                }
                if served.is_some() &&
                    state_range_requests.tasks.len() < MAX_CONCURRENT_STATE_RANGE_REQUESTS
                {
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            }),
        )
        .await
        .expect("queued state range request must be served once a slot frees");

        assert!(started.elapsed() < wait, "task started from the queue must wake the poller");
        assert_eq!(served, Some(Ok(empty_account_range())));
        assert_eq!(provider.snap_state_range_resolutions(), 1);
        assert_eq!(handler.state_range_requests.queue.len(), 1);
        assert_eq!(passed_over.try_recv(), Err(oneshot::error::TryRecvError::Empty));
    }

    #[tokio::test]
    async fn stale_queued_state_range_requests_are_not_served() {
        let provider = MockEthProvider::default();
        let mut handler = snap_handler(provider.clone());
        for byte in 0..MAX_CONCURRENT_STATE_RANGE_REQUESTS as u8 {
            handler
                .state_range_requests
                .tasks
                .push((PeerId::repeat_byte(byte), tokio::spawn(futures::future::pending())));
        }
        // One peer goes away while its request is queued.
        let (response, gone) = oneshot::channel();
        handler.on_snap_request(PeerId::repeat_byte(0x80), account_range_request(), response);
        drop(gone);
        // Another request is about to time out, with nothing else left to wake the handler.
        let SnapProtocolMessage::GetAccountRange(request) = account_range_request() else {
            unreachable!("fixture is an account range request")
        };
        let (response, mut expired) = oneshot::channel();
        handler.state_range_requests.queue.push_back(IncomingStateRangeRequest {
            peer_id: PeerId::repeat_byte(0x81),
            request: StateRangeRequest::Account(request),
            response,
            received: Instant::now() + Duration::from_millis(50) -
                STATE_RANGE_REQUEST_QUEUE_TIMEOUT,
        });

        // Only the queue is polled, so nothing but its timeout can wake this task before the wait
        // below gives up.
        let EthRequestHandler { state_range_requests, client, metrics, .. } = &mut handler;
        let wait = Duration::from_secs(5);
        let started = Instant::now();
        let answer = tokio::time::timeout(
            wait,
            std::future::poll_fn(|cx| {
                state_range_requests.poll(cx, client, metrics);
                Pin::new(&mut expired).poll(cx)
            }),
        )
        .await
        .expect("queued state range request must be answered when it times out");

        assert!(started.elapsed() < wait, "queue timeout must wake the poller");
        assert_eq!(answer.unwrap(), Ok(empty_account_range()));
        assert!(handler.state_range_requests.queue.is_empty());
        assert_eq!(provider.snap_state_range_resolutions(), 0);
    }

    #[tokio::test]
    async fn incoming_state_range_request_sees_slots_freed_since_the_last_poll() {
        let provider = MockEthProvider::default();
        let (incoming_tx, incoming_rx) = mpsc::channel(1);
        let mut handler = snap_handler(provider.clone());
        handler.incoming_requests = ReceiverStream::new(incoming_rx);
        // The peer's task finished after the last poll, and it has another request queued.
        let peer_id = PeerId::default();
        let finished = tokio::spawn(async {});
        while !finished.is_finished() {
            tokio::task::yield_now().await;
        }
        handler.state_range_requests.tasks.push((peer_id, finished));
        for byte in 1..MAX_CONCURRENT_STATE_RANGE_REQUESTS as u8 {
            handler
                .state_range_requests
                .tasks
                .push((PeerId::repeat_byte(byte), tokio::spawn(futures::future::pending())));
        }
        let (response, _queued) = oneshot::channel();
        handler.on_snap_request(peer_id, account_range_request(), response);
        assert_eq!(handler.state_range_requests.queue.len(), 1);

        // Its next request arrives before the handler has noticed the finished task.
        let (response, mut rx) = oneshot::channel();
        incoming_tx
            .send(IncomingEthRequest::GetSnap {
                peer_id,
                request: account_range_request(),
                response,
            })
            .await
            .unwrap();
        tokio::time::timeout(
            Duration::from_secs(5),
            std::future::poll_fn(|cx| {
                let _ = Pin::new(&mut handler).poll(cx);
                Pin::new(&mut rx).poll(cx)
            }),
        )
        .await
        .expect("incoming state range request must be answered")
        .unwrap()
        .unwrap();

        // Both requests were served in turn, neither was turned away.
        assert_eq!(provider.snap_state_range_resolutions(), 2);
    }

    #[tokio::test]
    async fn overflowing_state_range_requests_return_empty_responses() {
        let provider = MockEthProvider::default();
        let mut handler = snap_handler(provider.clone());
        for byte in 0..MAX_CONCURRENT_STATE_RANGE_REQUESTS as u8 {
            handler
                .state_range_requests
                .tasks
                .push((PeerId::repeat_byte(byte), tokio::spawn(futures::future::pending())));
        }
        let queued_peer = |index: usize| PeerId::repeat_byte(0x80 + index as u8);
        let (response, queued) = oneshot::channel();
        handler.on_snap_request(queued_peer(0), account_range_request(), response);

        // The peer already holds a place in the queue.
        let (response, rx) = oneshot::channel();
        handler.on_snap_request(queued_peer(0), account_range_request(), response);
        assert_eq!(rx.await.unwrap(), Ok(empty_account_range()));

        // The queue is full.
        let mut waiting = vec![queued];
        for index in 1..MAX_QUEUED_STATE_RANGE_REQUESTS {
            let (response, rx) = oneshot::channel();
            handler.on_snap_request(queued_peer(index), account_range_request(), response);
            waiting.push(rx);
        }
        let (response, rx) = oneshot::channel();
        handler.on_snap_request(PeerId::repeat_byte(0xff), account_range_request(), response);
        assert_eq!(rx.await.unwrap(), Ok(empty_account_range()));

        // Queued requests stay unanswered until a slot frees.
        assert!(waiting
            .iter_mut()
            .all(|rx| rx.try_recv() == Err(oneshot::error::TryRecvError::Empty)));
        assert_eq!(provider.snap_state_range_resolutions(), 0);
    }

    #[tokio::test]
    async fn state_range_request_from_gone_peer_is_dropped() {
        let mut handler = snap_handler(MockEthProvider::default());
        let (response, rx) = oneshot::channel();
        drop(rx);

        handler.on_snap_request(PeerId::default(), account_range_request(), response);

        assert!(handler.state_range_requests.tasks.is_empty());
    }

    #[tokio::test]
    async fn finished_state_range_request_frees_its_peer_slot() {
        let mut handler = snap_handler(MockEthProvider::default());
        let (response, rx) = oneshot::channel();

        handler.on_snap_request(PeerId::default(), account_range_request(), response);

        assert_eq!(handler.state_range_requests.tasks.len(), 1);
        rx.await.unwrap().unwrap();
        tokio::time::timeout(
            Duration::from_secs(5),
            std::future::poll_fn(|cx| {
                let _ = Pin::new(&mut handler).poll(cx);
                if handler.state_range_requests.tasks.is_empty() {
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            }),
        )
        .await
        .expect("finished state range task must free its peer slot");
    }

    #[tokio::test]
    async fn snap_requests_return_empty_responses_on_inconsistent_provider_results() {
        let missing_storage_root = MockEthProvider::default();
        missing_storage_root
            .set_snap_account_range(vec![(B256::ZERO, Account::default())], RangeEnd::Exhausted);

        let missing_account_proof = MockEthProvider::default();
        missing_account_proof.set_snap_account_range(Vec::new(), RangeEnd::Exhausted);

        let missing_storage_proof = MockEthProvider::default();
        missing_storage_proof
            .push_snap_storage_range(vec![(B256::ZERO, U256::ONE)], RangeEnd::ByteLimit);

        let storage_disappears = MockEthProvider::default();
        storage_disappears.push_snap_storage_range(Vec::new(), RangeEnd::Exhausted);
        storage_disappears.push_unavailable_snap_storage_range();

        let account_request = || {
            SnapProtocolMessage::GetAccountRange(GetAccountRangeMessage {
                request_id: 1,
                root_hash: B256::ZERO,
                starting_hash: B256::ZERO,
                limit_hash: B256::repeat_byte(0xff),
                response_bytes: SOFT_RESPONSE_LIMIT as u64,
            })
        };
        let storage_request = |account_hashes| {
            SnapProtocolMessage::GetStorageRanges(GetStorageRangesMessage {
                request_id: 2,
                root_hash: B256::ZERO,
                account_hashes,
                starting_hash: B256::ZERO.into(),
                limit_hash: B256::repeat_byte(0xff).into(),
                response_bytes: SOFT_RESPONSE_LIMIT as u64,
            })
        };
        let empty_accounts = SnapResponse::AccountRange(AccountRangeMessage {
            request_id: 1,
            accounts: Vec::new(),
            proof: Vec::new(),
        });
        let empty_storage = SnapResponse::StorageRanges(StorageRangesMessage {
            request_id: 2,
            slots: Vec::new(),
            proof: Vec::new(),
        });
        let cases = [
            (missing_storage_root, account_request(), empty_accounts.clone()),
            (missing_account_proof, account_request(), empty_accounts),
            (missing_storage_proof, storage_request(vec![B256::ZERO]), empty_storage.clone()),
            (storage_disappears, storage_request(vec![B256::ZERO, B256::ZERO]), empty_storage),
        ];

        for (provider, request, expected) in cases {
            let mut handler = snap_handler(provider.clone());
            let (response, rx) = oneshot::channel();
            handler.on_snap_request(PeerId::default(), request, response);
            assert_eq!(rx.await.unwrap(), Ok(expected));
            assert_eq!(provider.snap_state_range_resolutions(), 1);
        }
    }

    #[tokio::test]
    #[allow(clippy::needless_update)]
    async fn snap_account_range_response_encodes_accounts_and_proof() {
        let provider = MockEthProvider::default();
        let first_hash = B256::repeat_byte(0x01);
        let second_hash = B256::repeat_byte(0x02);
        let storage_root = B256::repeat_byte(0x11);
        let code_hash = B256::repeat_byte(0x22);
        let proof = vec![Bytes::from_static(&[0xaa])];
        provider.set_snap_account_range(
            vec![
                (
                    first_hash,
                    Account {
                        nonce: 1,
                        balance: U256::from(2),
                        bytecode_hash: Some(code_hash),
                        ..Default::default()
                    },
                ),
                (second_hash, Account { nonce: 3, balance: U256::from(4), ..Default::default() }),
            ],
            // A hash-limit stop (not an exhausted trie) so a boundary proof is still expected,
            // matching the mocked `proof` below.
            RangeEnd::HashLimit,
        );
        provider.set_snap_storage_root(first_hash, storage_root);
        provider.set_snap_storage_root(second_hash, EMPTY_ROOT_HASH);
        provider.set_snap_account_proof(Some(proof.clone()));

        let mut full_body = vec![0xf8, 0x44, 0x01, 0x02, 0xa0];
        full_body.extend_from_slice(storage_root.as_slice());
        full_body.push(0xa0);
        full_body.extend_from_slice(code_hash.as_slice());
        let empty_body = Bytes::from_static(&[0xc4, 0x03, 0x04, 0x80, 0x80]);

        let mut handler = snap_handler(provider.clone());
        let (response, rx) = oneshot::channel();
        handler.on_snap_request(
            PeerId::default(),
            SnapProtocolMessage::GetAccountRange(GetAccountRangeMessage {
                request_id: 1,
                root_hash: B256::ZERO,
                starting_hash: B256::ZERO,
                limit_hash: B256::repeat_byte(0xff),
                response_bytes: SOFT_RESPONSE_LIMIT as u64,
            }),
            response,
        );

        assert_eq!(
            rx.await.unwrap(),
            Ok(SnapResponse::AccountRange(AccountRangeMessage {
                request_id: 1,
                accounts: vec![
                    AccountData {
                        hash: first_hash,
                        body: alloy_rlp::decode_exact(&full_body).unwrap()
                    },
                    AccountData {
                        hash: second_hash,
                        body: alloy_rlp::decode_exact(&empty_body).unwrap()
                    },
                ],
                proof,
            }))
        );
        assert_eq!(provider.snap_state_range_resolutions(), 1);
    }

    #[tokio::test]
    async fn snap_account_range_response_skips_proof_for_exhausted_zero_origin_range() {
        let provider = MockEthProvider::default();
        let hash = B256::repeat_byte(0x01);
        provider.set_snap_account_range(
            vec![(hash, Account { nonce: 1, balance: U256::from(2), ..Default::default() })],
            RangeEnd::Exhausted,
        );
        provider.set_snap_storage_root(hash, EMPTY_ROOT_HASH);
        // A non-empty mocked proof: if the skip-proof optimization regressed to "never skip",
        // this wrongly-included proof would surface in the response and fail the assertion below.
        provider.set_snap_account_proof(Some(vec![Bytes::from_static(&[0xaa])]));

        let mut handler = snap_handler(provider.clone());
        let (response, rx) = oneshot::channel();
        handler.on_snap_request(
            PeerId::default(),
            SnapProtocolMessage::GetAccountRange(GetAccountRangeMessage {
                request_id: 1,
                root_hash: B256::ZERO,
                starting_hash: B256::ZERO,
                limit_hash: B256::repeat_byte(0xff),
                response_bytes: SOFT_RESPONSE_LIMIT as u64,
            }),
            response,
        );

        let Ok(SnapResponse::AccountRange(AccountRangeMessage { accounts, proof, .. })) =
            rx.await.unwrap()
        else {
            panic!("expected an account range response");
        };
        assert_eq!(accounts.len(), 1);
        assert!(proof.is_empty(), "a zero-origin, exhausted range must not carry a boundary proof");
    }

    #[tokio::test]
    async fn snap_storage_range_response_encodes_values_and_proof() {
        let provider = MockEthProvider::default();
        let first_hash = B256::repeat_byte(0x01);
        let second_hash = B256::repeat_byte(0x02);
        let origin = B256::repeat_byte(0x10);
        let proof = vec![Bytes::from_static(&[0xbb])];
        provider.push_snap_storage_range(
            vec![(first_hash, U256::from(0x0102)), (second_hash, U256::from(0xff))],
            RangeEnd::Exhausted,
        );
        provider.set_snap_storage_proof(Some(proof.clone()));

        let mut handler = snap_handler(provider.clone());
        let (response, rx) = oneshot::channel();
        handler.on_snap_request(
            PeerId::default(),
            SnapProtocolMessage::GetStorageRanges(GetStorageRangesMessage {
                request_id: 2,
                root_hash: B256::ZERO,
                account_hashes: vec![B256::repeat_byte(0x03)],
                starting_hash: origin.into(),
                limit_hash: B256::repeat_byte(0xff).into(),
                response_bytes: SOFT_RESPONSE_LIMIT as u64,
            }),
            response,
        );

        assert_eq!(
            rx.await.unwrap(),
            Ok(SnapResponse::StorageRanges(StorageRangesMessage {
                request_id: 2,
                slots: vec![vec![
                    StorageData { hash: first_hash, data: Bytes::from_static(&[0x82, 0x01, 0x02]) },
                    StorageData { hash: second_hash, data: Bytes::from_static(&[0x81, 0xff]) },
                ]],
                proof,
            }))
        );
        assert_eq!(provider.snap_storage_range_requests()[0].1, origin);
        assert_eq!(provider.snap_state_range_resolutions(), 1);
    }

    #[tokio::test]
    async fn snap_storage_ranges_only_bound_the_first_account() {
        let provider = MockEthProvider::default();
        provider.push_snap_storage_range(Vec::new(), RangeEnd::Exhausted);
        provider.push_snap_storage_range(Vec::new(), RangeEnd::Exhausted);
        let first_account = B256::repeat_byte(0x01);
        let second_account = B256::repeat_byte(0x02);
        let origin = B256::ZERO;
        let limit = B256::repeat_byte(0x22);

        let mut handler = snap_handler(provider.clone());
        let (response, rx) = oneshot::channel();
        handler.on_snap_request(
            PeerId::default(),
            SnapProtocolMessage::GetStorageRanges(GetStorageRangesMessage {
                request_id: 3,
                root_hash: B256::ZERO,
                account_hashes: vec![first_account, second_account],
                starting_hash: origin.into(),
                limit_hash: limit.into(),
                response_bytes: 1_000,
            }),
            response,
        );

        assert_eq!(
            rx.await.unwrap(),
            Ok(SnapResponse::StorageRanges(StorageRangesMessage {
                request_id: 3,
                slots: vec![Vec::new(), Vec::new()],
                proof: Vec::new(),
            }))
        );
        assert_eq!(
            provider.snap_storage_range_requests(),
            vec![
                (first_account, origin, limit, 1_000),
                (second_account, B256::ZERO, B256::repeat_byte(0xff), 1_000),
            ]
        );
        assert_eq!(provider.snap_state_range_resolutions(), 1);
    }

    #[tokio::test]
    async fn snap_byte_codes_response_preserves_found_code_order() {
        let provider = MockEthProvider::default();
        let code = Bytes::from_static(&[0x60, 0x00]);
        let code_hash = keccak256(&code);
        let later_code = Bytes::from_static(&[0x60, 0x01]);
        let later_code_hash = keccak256(&later_code);
        provider.add_account(
            Address::repeat_byte(0x01),
            ExtendedAccount::new(1, U256::ZERO).with_bytecode(code.clone()),
        );
        provider.add_account(
            Address::repeat_byte(0x02),
            ExtendedAccount::new(1, U256::ZERO).with_bytecode(later_code),
        );

        let mut handler = snap_handler(provider);
        let (response, rx) = oneshot::channel();
        handler.on_snap_request(
            PeerId::default(),
            SnapProtocolMessage::GetByteCodes(reth_eth_wire::snap::GetByteCodesMessage {
                request_id: 3,
                hashes: vec![KECCAK_EMPTY, B256::repeat_byte(0xff), code_hash, later_code_hash],
                response_bytes: 1,
            }),
            response,
        );

        assert_eq!(
            rx.await.unwrap(),
            Ok(SnapResponse::ByteCodes(ByteCodesMessage {
                request_id: 3,
                codes: vec![Bytes::new(), code],
            }))
        );
    }

    #[tokio::test]
    async fn snap_storage_ranges_limit_account_lookups() {
        let provider = MockEthProvider::default();
        for _ in 0..=MAX_STORAGE_RANGE_ACCOUNTS_SERVE {
            provider.push_snap_storage_range(Vec::new(), RangeEnd::Exhausted);
        }
        let mut handler = snap_handler(provider.clone());
        let (response, rx) = oneshot::channel();
        handler.on_snap_request(
            PeerId::default(),
            SnapProtocolMessage::GetStorageRanges(GetStorageRangesMessage {
                request_id: 4,
                root_hash: B256::ZERO,
                account_hashes: vec![B256::ZERO; MAX_STORAGE_RANGE_ACCOUNTS_SERVE + 1],
                starting_hash: B256::ZERO.into(),
                limit_hash: B256::repeat_byte(0xff).into(),
                response_bytes: SOFT_RESPONSE_LIMIT as u64,
            }),
            response,
        );

        assert_eq!(
            rx.await.unwrap(),
            Ok(SnapResponse::StorageRanges(StorageRangesMessage {
                request_id: 4,
                slots: vec![Vec::new(); MAX_STORAGE_RANGE_ACCOUNTS_SERVE],
                proof: Vec::new(),
            }))
        );
        assert_eq!(provider.snap_storage_ranges_remaining(), 1);
        assert_eq!(provider.snap_state_range_resolutions(), 1);
    }

    #[tokio::test]
    async fn snap_storage_range_proves_finite_limit_from_zero_origin() {
        let provider = MockEthProvider::default();
        let hash = B256::repeat_byte(0x01);
        let proof = vec![Bytes::from_static(&[0xcc])];
        // More entries exist beyond `limit_hash`, so the cursor stopped at the hash limit
        // rather than exhausting the trie -- a proof is required even though origin is zero.
        provider.push_snap_storage_range(vec![(hash, U256::ONE)], RangeEnd::HashLimit);
        provider.set_snap_storage_proof(Some(proof.clone()));

        let mut handler = snap_handler(provider.clone());
        let (response, rx) = oneshot::channel();
        handler.on_snap_request(
            PeerId::default(),
            SnapProtocolMessage::GetStorageRanges(GetStorageRangesMessage {
                request_id: 5,
                root_hash: B256::ZERO,
                account_hashes: vec![B256::repeat_byte(0x03)],
                starting_hash: B256::ZERO.into(),
                limit_hash: B256::repeat_byte(0x20).into(),
                response_bytes: SOFT_RESPONSE_LIMIT as u64,
            }),
            response,
        );

        assert_eq!(
            rx.await.unwrap(),
            Ok(SnapResponse::StorageRanges(StorageRangesMessage {
                request_id: 5,
                slots: vec![vec![StorageData { hash, data: Bytes::from_static(&[0x01]) }]],
                proof,
            }))
        );
    }

    #[tokio::test]
    async fn snap_storage_ranges_are_entirely_empty_when_an_account_is_missing() {
        let provider = MockEthProvider::default();
        provider.push_missing_snap_storage_account();
        provider.push_snap_storage_range(
            vec![(B256::repeat_byte(0x01), U256::ONE)],
            RangeEnd::Exhausted,
        );
        let missing_account = B256::repeat_byte(0x01);
        let valid_account = B256::repeat_byte(0x02);

        let mut handler = snap_handler(provider.clone());
        let (response, rx) = oneshot::channel();
        handler.on_snap_request(
            PeerId::default(),
            SnapProtocolMessage::GetStorageRanges(GetStorageRangesMessage {
                request_id: 6,
                root_hash: B256::ZERO,
                account_hashes: vec![missing_account, valid_account],
                starting_hash: B256::ZERO.into(),
                limit_hash: B256::repeat_byte(0xff).into(),
                response_bytes: SOFT_RESPONSE_LIMIT as u64,
            }),
            response,
        );

        assert_eq!(
            rx.await.unwrap(),
            Ok(SnapResponse::StorageRanges(StorageRangesMessage {
                request_id: 6,
                slots: Vec::new(),
                proof: Vec::new(),
            }))
        );
        // The valid account's queued range is never consumed: the response bails out at the
        // first missing account instead of skipping it and shifting later positions.
        assert_eq!(provider.snap_storage_ranges_remaining(), 1);
    }

    /// Creates a receipt whose single log carries `data_len` bytes of data.
    fn receipt_with_data(data_len: usize) -> Receipt {
        Receipt {
            tx_type: TxType::Legacy,
            success: true,
            cumulative_gas_used: 21_000,
            logs: vec![Log {
                address: Address::ZERO,
                data: LogData::new_unchecked(Vec::new(), vec![0u8; data_len].into()),
            }],
        }
    }

    /// Stores the receipts as block `number` and returns the block hash.
    fn insert_receipts_block(
        provider: &MockEthProvider,
        number: u64,
        receipts: Vec<Receipt>,
    ) -> B256 {
        let hash = B256::with_last_byte(number as u8);
        provider.add_header(hash, Header { number, ..Default::default() });
        provider.add_receipts(number, receipts);
        hash
    }

    /// Sends a [`GetReceipts70`] request to the handler and returns the response.
    async fn get_receipts70(
        provider: MockEthProvider,
        first_block_receipt_index: u64,
        block_hashes: Vec<B256>,
    ) -> Receipts70 {
        let handler = snap_handler(provider);
        let (response, rx) = oneshot::channel();
        handler.on_receipts70_request(
            PeerId::default(),
            GetReceipts70 { first_block_receipt_index, block_hashes },
            response,
        );
        rx.await.unwrap().unwrap()
    }

    #[tokio::test]
    async fn receipts70_serves_only_oversized_first_receipt_of_partial_block() {
        let provider = MockEthProvider::default();
        let oversized = receipt_with_data(SOFT_RESPONSE_LIMIT + 1024);
        let hash =
            insert_receipts_block(&provider, 1, vec![oversized.clone(), receipt_with_data(16)]);

        let resp = get_receipts70(provider, 0, vec![hash]).await;

        // The oversized receipt exhausts the budget, so the rest of the block is left out.
        assert_eq!(
            resp,
            Receipts70 { last_block_incomplete: true, receipts: vec![vec![oversized]] }
        );
    }

    #[tokio::test]
    async fn receipts70_omits_empty_incomplete_block() {
        let provider = MockEthProvider::default();
        let first = vec![receipt_with_data(SOFT_RESPONSE_LIMIT - 1024)];
        let first_hash = insert_receipts_block(&provider, 1, first.clone());
        // The first receipt of the second block does not fit into the remaining budget.
        let second_hash = insert_receipts_block(&provider, 2, vec![receipt_with_data(4096)]);

        let resp = get_receipts70(provider, 0, vec![first_hash, second_hash]).await;

        assert_eq!(resp, Receipts70 { last_block_incomplete: false, receipts: vec![first] });
    }

    #[tokio::test]
    async fn receipts70_serves_partial_block() {
        let provider = MockEthProvider::default();
        let receipts = vec![receipt_with_data(16 * 1024); 200];
        let hash = insert_receipts_block(&provider, 1, receipts.clone());

        let resp = get_receipts70(provider, 0, vec![hash]).await;

        assert!(resp.last_block_incomplete);
        assert_eq!(resp.receipts.len(), 1);
        let partial = &resp.receipts[0];
        assert!(!partial.is_empty() && partial.len() < receipts.len());
        assert_eq!(partial.as_slice(), &receipts[..partial.len()]);
        assert!(partial.iter().map(Encodable::length).sum::<usize>() <= SOFT_RESPONSE_LIMIT);
    }

    #[test_case(0, 0; "first receipt at hard limit")]
    #[test_case(1, 0; "continuation receipt at hard limit")]
    #[test_case(0, 1; "first receipt framing exceeds hard limit")]
    #[test_case(1, 1; "continuation receipt framing exceeds hard limit")]
    #[tokio::test]
    async fn receipts70_enforces_framed_response_limit(first_receipt_index: u64, excess: usize) {
        // Measure the actual wire encoding with the largest request ID. At these sizes, changing
        // the log data length does not change the width of any RLP length prefix.
        let data_len = MAX_MESSAGE_SIZE - 1024;
        let encoded = encode_receipts70_response(Receipts70 {
            last_block_incomplete: false,
            receipts: vec![vec![receipt_with_data(data_len)]],
        });
        let receipt = receipt_with_data(data_len + MAX_MESSAGE_SIZE - encoded.len() + excess);
        assert!(receipt.length() < MAX_MESSAGE_SIZE);
        let expected =
            Receipts70 { last_block_incomplete: false, receipts: vec![vec![receipt.clone()]] };
        assert_eq!(encode_receipts70_response(expected.clone()).len(), MAX_MESSAGE_SIZE + excess);

        let provider = MockEthProvider::default();
        let mut receipts = vec![receipt_with_data(16); first_receipt_index as usize];
        receipts.push(receipt);
        let hash = insert_receipts_block(&provider, 1, receipts);

        let resp = get_receipts70(provider, first_receipt_index, vec![hash]).await;

        if excess == 0 {
            assert_eq!(resp, expected);
        } else {
            assert_eq!(resp, Receipts70 { last_block_incomplete: false, receipts: Vec::new() });
        }
        let encoded = encode_receipts70_response(resp);
        assert!(encoded.len() <= MAX_MESSAGE_SIZE);
        // The requesting side only decodes responses to requests it sent.
        let mut requester = EthStreamInner::<EthNetworkPrimitives>::new(EthVersion::Eth70);
        requester
            .encode_message(EthMessage::GetReceipts70(RequestPair {
                request_id: u64::MAX,
                message: GetReceipts70 {
                    first_block_receipt_index: first_receipt_index,
                    block_hashes: vec![hash],
                },
            }))
            .unwrap();
        requester.decode_message(encoded.as_ref().into()).unwrap();
    }

    /// Encodes a receipts response with the largest possible request ID.
    fn encode_receipts70_response(response: Receipts70) -> Bytes {
        EthStreamInner::<EthNetworkPrimitives>::new(EthVersion::Eth70)
            .encode_message(EthMessage::Receipts70(RequestPair {
                request_id: u64::MAX,
                message: response,
            }))
            .unwrap()
            .into()
    }
}
