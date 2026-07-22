//! Blocks/Headers management for the p2p network.

use crate::{
    budget::DEFAULT_BUDGET_TRY_DRAIN_DOWNLOADERS, metered_poll_nested_stream_with_budget,
    metrics::EthRequestHandlerMetrics,
};
use alloy_consensus::{
    constants::{EMPTY_ROOT_HASH, KECCAK_EMPTY},
    BlockHeader, ReceiptWithBloom,
};
use alloy_eips::BlockHashOrNumber;
use alloy_primitives::{Bytes, B256, U256};
use alloy_rlp::{Encodable, RlpEncodable};
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
use reth_primitives_traits::{Account, Block};
use reth_storage_api::{
    errors::provider::ProviderResult, BalProvider, BlockReader, BytecodeReader,
    GetBlockAccessListLimit, HeaderProvider, RangeEnd, RangeResponse, StateProviderFactory,
    StateRangeProviderFactory,
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

/// Maximum number of storage range account lookups to serve.
pub const MAX_STORAGE_RANGE_ACCOUNTS_SERVE: usize = 1024;

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
    C: BalProvider + StateProviderFactory + StateRangeProviderFactory,
{
    /// Handles `snap/2` (EIP-8189) requests.
    ///
    /// `GetAccountRange`/`GetStorageRanges` are hash-native throughout and served from retained
    /// canonical roots via [`StateRangeProviderFactory`]. `GetByteCodes` is content-addressed and
    /// independent of any particular state root, so it's served directly.
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
                let request_id = req.request_id;
                let response = self.get_account_range_response(req).unwrap_or_else(|error| {
                    tracing::debug!(target: "net::snap", %error, "failed to serve account range");
                    AccountRangeMessage { request_id, accounts: Vec::new(), proof: Vec::new() }
                });
                Ok(SnapResponse::AccountRange(response))
            }
            SnapProtocolMessage::GetStorageRanges(req) => {
                let request_id = req.request_id;
                let response = self.get_storage_ranges_response(req).unwrap_or_else(|error| {
                    tracing::debug!(target: "net::snap", %error, "failed to serve storage ranges");
                    StorageRangesMessage { request_id, slots: Vec::new(), proof: Vec::new() }
                });
                Ok(SnapResponse::StorageRanges(response))
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

    /// Serves a `GetAccountRange` request via [`StateRangeProviderFactory`].
    ///
    /// Fails the request if a storage root or proof becomes unavailable after the range lookup.
    /// Proves the boundary between `starting_hash` and the last returned account, per snap/2's
    /// boundary-proof requirement, unless the range is a zero-origin range that exhausted the
    /// trie, in which case no proof is needed.
    fn get_account_range_response(
        &self,
        req: GetAccountRangeMessage,
    ) -> ProviderResult<AccountRangeMessage> {
        let empty = AccountRangeMessage {
            request_id: req.request_id,
            accounts: Vec::new(),
            proof: Vec::new(),
        };

        let response_bytes = (req.response_bytes as usize).min(SOFT_RESPONSE_LIMIT);
        let Some(state) = self.client.state_range_provider(req.root_hash)? else {
            return Ok(empty)
        };
        let RangeResponse { items: accounts, end } =
            state.account_range(req.starting_hash, req.limit_hash, response_bytes)?;

        let proof = if req.starting_hash == B256::ZERO && end == RangeEnd::Exhausted {
            Vec::new()
        } else {
            let boundary_keys = boundary_proof_keys(req.starting_hash, accounts.last());
            state.account_range_proof(&boundary_keys)?
        };

        let mut account_data = Vec::with_capacity(accounts.len());
        for (hash, account) in accounts {
            let storage_root = state.storage_root_by_hash(hash)?;
            account_data
                .push(AccountData { hash, body: slim_account_body(&account, storage_root) });
        }

        Ok(AccountRangeMessage { request_id: req.request_id, accounts: account_data, proof })
    }

    /// Serves a `GetStorageRanges` request via [`StateRangeProviderFactory`].
    ///
    /// `starting_hash`/`limit_hash` apply only to the first account. An account unavailable at
    /// this root makes the whole response empty, rather than skipping it and shifting later
    /// accounts' positions. A proof stops the response at the first account that isn't a
    /// complete, zero-origin range.
    fn get_storage_ranges_response(
        &self,
        req: GetStorageRangesMessage,
    ) -> ProviderResult<StorageRangesMessage> {
        let empty = StorageRangesMessage {
            request_id: req.request_id,
            slots: Vec::new(),
            proof: Vec::new(),
        };
        let Some(state) = self.client.state_range_provider(req.root_hash)? else {
            return Ok(empty)
        };
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
            let needs_proof = origin != B256::ZERO || end != RangeEnd::Exhausted;
            slots.push(
                account_slots
                    .into_iter()
                    .map(|(hash, value)| StorageData {
                        hash,
                        // snap clients verify proofs against RLP-encoded storage trie leaves.
                        data: alloy_rlp::encode(value).into(),
                    })
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

/// Boundary-proof keys for a range reply: `origin`, plus the last returned item's key if any.
fn boundary_proof_keys<T>(origin: B256, last: Option<&(B256, T)>) -> Vec<B256> {
    match last {
        Some((last, _)) => vec![origin, *last],
        None => vec![origin],
    }
}

/// Like the consensus trie account, but the code hash and storage root are empty byte strings
/// rather than [`KECCAK_EMPTY`]/[`EMPTY_ROOT_HASH`] when the account has no code/storage, to
/// avoid transferring the same 32 bytes for every EOA. Borrowed to encode without allocating.
#[derive(RlpEncodable)]
struct SlimAccountBody<'a> {
    /// The account's nonce.
    nonce: u64,
    /// The account's balance.
    balance: U256,
    /// Empty when the account has no storage.
    storage_root: &'a [u8],
    /// Empty when the account has no code.
    code_hash: &'a [u8],
}

/// RLP-encodes `account` in snap/2's slim format; see [`SlimAccountBody`].
fn slim_account_body(account: &Account, storage_root: B256) -> Bytes {
    let storage_root: &[u8] =
        if storage_root == EMPTY_ROOT_HASH { &[] } else { storage_root.as_slice() };
    let code_hash: &[u8] = match &account.bytecode_hash {
        Some(hash) if *hash != KECCAK_EMPTY => hash.as_slice(),
        _ => &[],
    };

    alloy_rlp::encode(SlimAccountBody {
        nonce: account.nonce,
        balance: account.balance,
        storage_root,
        code_hash,
    })
    .into()
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
    use alloy_primitives::{keccak256, Address, TxHash, B128};
    use reth_network_api::test_utils::PeersHandle;
    use reth_provider::test_utils::{ExtendedAccount, MockEthProvider};
    use reth_storage_api::noop::NoopProvider;
    use reth_transaction_pool::blobstore::{
        BlobCellAvailability, BlobSidecar, BlobStoreCleanupStat, BlobStoreError,
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
    }

    impl BlobStore for CountingBlobStore {
        fn insert(&self, _tx: B256, data: BlobSidecar) -> Result<(), BlobStoreError> {
            let _ = data.availability().set(BlobCellAvailability::full());
            Ok(())
        }

        fn insert_all(&self, txs: Vec<(B256, BlobSidecar)>) -> Result<(), BlobStoreError> {
            for (_, data) in txs {
                let _ = data.availability().set(BlobCellAvailability::full());
            }
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

    /// Creates a request handler backed by the mock provider for snap response tests.
    fn snap_handler(
        provider: MockEthProvider,
    ) -> EthRequestHandler<MockEthProvider, EthNetworkPrimitives> {
        let (peers_tx, _) = mpsc::unbounded_channel();
        let (_incoming_tx, incoming_rx) = mpsc::channel(1);
        EthRequestHandler::new(provider, PeersHandle::new(peers_tx), incoming_rx)
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
        let handler = snap_handler(provider);
        let (response, rx) = oneshot::channel();

        handler.on_snap_request(PeerId::default(), request, response);

        assert_eq!(rx.await.unwrap(), Ok(expected));
    }

    #[tokio::test]
    async fn unavailable_snap_state_returns_empty_response() {
        let provider = MockEthProvider::default();
        let handler = snap_handler(provider.clone());
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

    #[tokio::test]
    async fn snap_requests_return_empty_responses_on_inconsistent_provider_results() {
        let missing_storage_root = MockEthProvider::default();
        missing_storage_root
            .set_snap_account_range(vec![(B256::ZERO, Account::default())], RangeEnd::Exhausted);

        let missing_account_proof = MockEthProvider::default();
        missing_account_proof.set_snap_account_range(Vec::new(), RangeEnd::Exhausted);

        let missing_storage_proof = MockEthProvider::default();
        missing_storage_proof
            .push_snap_storage_range(vec![(B256::ZERO, U256::from(1))], RangeEnd::ByteLimit);

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
            let handler = snap_handler(provider.clone());
            let (response, rx) = oneshot::channel();
            handler.on_snap_request(PeerId::default(), request, response);
            assert_eq!(rx.await.unwrap(), Ok(expected));
            assert_eq!(provider.snap_state_range_resolutions(), 1);
        }
    }

    #[tokio::test]
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
                    Account { nonce: 1, balance: U256::from(2), bytecode_hash: Some(code_hash) },
                ),
                (second_hash, Account { nonce: 3, balance: U256::from(4), bytecode_hash: None }),
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

        let handler = snap_handler(provider.clone());
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
                    AccountData { hash: first_hash, body: full_body.into() },
                    AccountData { hash: second_hash, body: empty_body },
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
            vec![(hash, Account { nonce: 1, balance: U256::from(2), bytecode_hash: None })],
            RangeEnd::Exhausted,
        );
        provider.set_snap_storage_root(hash, EMPTY_ROOT_HASH);
        // A non-empty mocked proof: if the skip-proof optimization regressed to "never skip",
        // this wrongly-included proof would surface in the response and fail the assertion below.
        provider.set_snap_account_proof(Some(vec![Bytes::from_static(&[0xaa])]));

        let handler = snap_handler(provider.clone());
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

        let handler = snap_handler(provider.clone());
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

        let handler = snap_handler(provider.clone());
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

        let handler = snap_handler(provider);
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
        let handler = snap_handler(provider.clone());
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
        provider.push_snap_storage_range(vec![(hash, U256::from(1))], RangeEnd::HashLimit);
        provider.set_snap_storage_proof(Some(proof.clone()));

        let handler = snap_handler(provider.clone());
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
            vec![(B256::repeat_byte(0x01), U256::from(1))],
            RangeEnd::Exhausted,
        );
        let missing_account = B256::repeat_byte(0x01);
        let valid_account = B256::repeat_byte(0x02);

        let handler = snap_handler(provider.clone());
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
}
