//! Blocks/Headers management for the p2p network.

use crate::{
    budget::DEFAULT_BUDGET_TRY_DRAIN_DOWNLOADERS, metered_poll_nested_stream_with_budget,
    metrics::EthRequestHandlerMetrics,
};
use alloy_consensus::{BlockHeader, ReceiptWithBloom};
use alloy_eips::{eip7594::BlobCellMask, BlockHashOrNumber};
use alloy_primitives::Bytes;
use alloy_rlp::Encodable;
use futures::StreamExt;
use reth_eth_wire::{
    BlockAccessLists, BlockBodies, BlockHeaders, Cells, EthNetworkPrimitives, GetBlockAccessLists,
    GetBlockBodies, GetBlockHeaders, GetCells, GetNodeData, GetReceipts, GetReceipts70,
    HeadersDirection, NetworkPrimitives, NodeData, Receipts, Receipts69, Receipts70,
};
use reth_network_api::test_utils::PeersHandle;
use reth_network_p2p::error::RequestResult;
use reth_network_peers::PeerId;
use reth_primitives_traits::Block;
use reth_storage_api::{BalProvider, BlockReader, GetBlockAccessListLimit, HeaderProvider};
use reth_transaction_pool::{
    blobstore::{BlobStoreError, BlobTransactionSidecarVariant},
    TransactionPool,
};
use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
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

/// Maximum size of replies to data retrievals: 2MB
pub const SOFT_RESPONSE_LIMIT: usize = 2 * 1024 * 1024;

trait BlobFetcher: Clone + Send + Sync + 'static {
    fn get_blob(
        &self,
        tx_hash: alloy_primitives::TxHash,
    ) -> Result<Option<Arc<BlobTransactionSidecarVariant>>, BlobStoreError>;
}

impl BlobFetcher for () {
    fn get_blob(
        &self,
        _tx_hash: alloy_primitives::TxHash,
    ) -> Result<Option<Arc<BlobTransactionSidecarVariant>>, BlobStoreError> {
        Ok(None)
    }
}

impl<P> BlobFetcher for P
where
    P: TransactionPool,
{
    fn get_blob(
        &self,
        tx_hash: alloy_primitives::TxHash,
    ) -> Result<Option<Arc<BlobTransactionSidecarVariant>>, BlobStoreError> {
        TransactionPool::get_blob(self, tx_hash)
    }
}

/// Manages eth related requests on top of the p2p network.
///
/// This can be spawned to another task and is supposed to be run as background service.
#[derive(Debug)]
#[must_use = "Manager does nothing unless polled."]
pub struct EthRequestHandler<C, B = (), N: NetworkPrimitives = EthNetworkPrimitives> {
    /// The client type that can interact with the chain.
    client: C,
    /// Used to serve blob cells for `eth/72` requests.
    blob_fetcher: B,
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
impl<C, N: NetworkPrimitives> EthRequestHandler<C, (), N> {
    /// Create a new instance
    pub fn new(client: C, peers: PeersHandle, incoming: Receiver<IncomingEthRequest<N>>) -> Self {
        Self::with_blob_fetcher(client, (), peers, incoming)
    }
}

impl<C, B, N: NetworkPrimitives> EthRequestHandler<C, B, N> {
    /// Create a new instance with blob-backed cell serving support.
    pub fn with_blob_fetcher(
        client: C,
        blob_fetcher: B,
        peers: PeersHandle,
        incoming: Receiver<IncomingEthRequest<N>>,
    ) -> Self {
        Self {
            client,
            blob_fetcher,
            peers,
            incoming_requests: ReceiverStream::new(incoming),
            metrics: Default::default(),
        }
    }
}

impl<C, B, N> EthRequestHandler<C, B, N>
where
    N: NetworkPrimitives,
    C: BlockReader,
    B: BlobFetcher,
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
        let GetCells { hashes, cell_mask } = request;
        let request_mask = BlobCellMask::new(cell_mask);
        let mut response_hashes = Vec::new();
        let mut response_cells = Vec::new();
        let mut total_bytes = 0usize;

        for tx_hash in hashes {
            let Ok(Some(sidecar)) = self.blob_fetcher.get_blob(tx_hash) else {
                continue;
            };
            let Some(sidecar) = sidecar.as_eip7594() else {
                continue;
            };

            let versioned_hashes = sidecar.versioned_hashes().collect::<Vec<_>>();
            let Ok(mut matches) = sidecar.match_versioned_hashes_cells(&versioned_hashes, request_mask)
            else {
                continue;
            };

            matches.sort_by_key(|(idx, _)| *idx);

            let mut tx_cells = Vec::new();
            let mut complete = matches.len() == versioned_hashes.len();
            for (_, cells_and_proofs) in matches {
                for cell in cells_and_proofs.blob_cells {
                    let Some(cell) = cell else {
                        complete = false;
                        break;
                    };
                    tx_cells.push(cell);
                }

                if !complete {
                    break;
                }
            }

            if !complete {
                continue;
            }

            let tx_cells_len = tx_cells.length();
            if total_bytes + tx_cells_len > SOFT_RESPONSE_LIMIT {
                break;
            }

            total_bytes += tx_cells_len;
            response_hashes.push(tx_hash);
            response_cells.push(tx_cells);
        }

        let _ = response.send(Ok(Cells {
            hashes: response_hashes,
            cells: response_cells,
            cell_mask,
        }));
    }
}

impl<C, B, N> EthRequestHandler<C, B, N>
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
        request.0.truncate(MAX_BLOCK_ACCESS_LISTS_SERVE);

        let limit = GetBlockAccessListLimit::ResponseSizeSoftLimit(SOFT_RESPONSE_LIMIT);
        let access_lists = self
            .client
            .bal_store()
            .get_by_hashes_with_limit(&request.0, limit)
            .unwrap_or_else(|_| empty_block_access_lists_with_limit(request.0.len(), limit));
        let _ = response.send(Ok(BlockAccessLists(access_lists)));
    }
}

/// Builds the error fallback response while still enforcing the BAL response soft limit.
fn empty_block_access_lists_with_limit(count: usize, limit: GetBlockAccessListLimit) -> Vec<Bytes> {
    let mut out = Vec::with_capacity(count);
    let mut size = 0;
    for _ in 0..count {
        let bal = Bytes::from_static(&[0xc0]);
        size += bal.len();
        out.push(bal);

        if limit.exceeds(size) {
            break
        }
    }
    out
}

/// An endless future.
///
/// This should be spawned or used as part of `tokio::select!`.
impl<C, B, N> Future for EthRequestHandler<C, B, N>
where
    N: NetworkPrimitives,
    C: BalProvider
        + BlockReader<Block = N::Block, Receipt = N::Receipt>
        + HeaderProvider<Header = N::BlockHeader>
        + Unpin,
    B: BlobFetcher + Unpin,
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eips::{
        eip4844::{kzg_to_versioned_hash, Blob, Bytes48},
        eip7594::{BlobTransactionSidecarEip7594, BlobTransactionSidecarVariant, CELLS_PER_EXT_BLOB},
    };
    use alloy_primitives::B256;
    use reth_storage_api::noop::NoopProvider;
    use reth_transaction_pool::{
        blobstore::BlobStore,
        test_utils::testing_pool,
    };

    fn eip7594_single_blob_sidecar() -> (BlobTransactionSidecarVariant, B256) {
        let blob = Blob::default();
        let commitment = Bytes48::default();
        let cell_proofs = vec![Bytes48::default(); CELLS_PER_EXT_BLOB];
        let versioned_hash = kzg_to_versioned_hash(commitment.as_slice());
        let sidecar = BlobTransactionSidecarEip7594::new(vec![blob], vec![commitment], cell_proofs);

        (BlobTransactionSidecarVariant::Eip7594(sidecar), versioned_hash)
    }

    #[test]
    fn serves_requested_cells_from_blob_pool() {
        let client = NoopProvider::default();
        let pool = testing_pool();
        let (sidecar, versioned_hash) = eip7594_single_blob_sidecar();
        let tx_hash = B256::random();
        pool.blob_store().insert(tx_hash, sidecar).unwrap();

        let handler = EthRequestHandler::with_blob_fetcher(
            client,
            pool,
            PeersHandle::default(),
            tokio::sync::mpsc::channel(1).1,
        );
        let (tx, rx) = oneshot::channel();
        let cell_mask = ((1u128 << 0) | (1u128 << 7)).into();

        handler.on_cells_request(
            PeerId::random(),
            GetCells { hashes: vec![tx_hash, B256::ZERO], cell_mask },
            tx,
        );

        let response = rx.blocking_recv().unwrap().unwrap();
        assert_eq!(response.hashes, vec![tx_hash]);
        assert_eq!(response.cell_mask, cell_mask);
        assert_eq!(response.cells.len(), 1);
        assert_eq!(response.cells[0].len(), 2);
        assert_ne!(versioned_hash, B256::ZERO);
    }
}
