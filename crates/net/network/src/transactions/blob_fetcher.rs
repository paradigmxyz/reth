//! Fetches the cells of blob transactions announced by eth/72 peers.
//!
//! eth/72 peers send blob transaction bodies without their blobs. The body is still fetched by
//! the [`TransactionFetcher`](super::TransactionFetcher), while this fetcher requests enough
//! cells with `GetCells` to rebuild every blob, checks them against the body and hands the
//! transaction back for the pool import.

use super::{
    blob_buffer::{BlobBuffer, BufferedBlob},
    PeerMetadata,
};
use alloy_eips::eip7594::{BlobCellMask, CELLS_PER_EXT_BLOB};
use alloy_primitives::{
    map::{B256Map, FbBuildHasher, HashMap},
    B256,
};
use futures::{stream::FuturesUnordered, Future, StreamExt};
use reth_eth_wire::{encode_cell_mask, Cells, GetCells, NetworkPrimitives};
use reth_metrics::{
    metrics::{Counter, Gauge},
    Metrics,
};
use reth_network_api::PeerRequest;
use reth_network_peers::PeerId;
use reth_transaction_pool::PoolTransaction;
use std::{
    pin::Pin,
    task::{Context, Poll},
    time::{Duration, Instant},
};
use tokio::sync::oneshot;

/// Most transactions waiting for their cells.
const MAX_PENDING_TRANSACTIONS: usize = 4096;

/// Most `GetCells` requests in flight.
const MAX_INFLIGHT_REQUESTS: usize = 16;

/// Most cell checks running at once, independent of the requests in flight.
const MAX_CONCURRENT_VERIFICATIONS: usize = 4;

/// Most announcing peers kept per transaction to fetch its cells from.
const MAX_PROVIDERS_PER_TRANSACTION: usize = 16;

/// How long a peer has to answer a `GetCells` request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a transaction waits for its cells before it's dropped.
const PENDING_TTL: Duration = Duration::from_secs(120);

/// How often expired transactions are removed.
const EXPIRY_INTERVAL: Duration = Duration::from_millis(100);

/// Most columns requested from a peer in a burst.
const PEER_COLUMN_BUDGET: f64 = 256.0;

/// Columns per second a peer's budget refills by.
const PEER_COLUMN_REFILL_PER_SEC: f64 = 9.0;

/// Fetches the cells of eth/72 blob transactions and checks them against their bodies.
pub(super) struct BlobFetcher<T> {
    // Bodies and cells waiting for each other.
    buffer: BlobBuffer<T>,
    // Transactions whose cells are being fetched.
    pending: B256Map<Pending>,
    // `GetCells` requests in flight.
    requests: FuturesUnordered<RequestFuture>,
    // Cell checks running off the async threads.
    verifications: FuturesUnordered<VerifyFuture<T>>,
    // Columns each peer may still be asked for.
    budgets: HashMap<PeerId, Budget, FbBuildHasher<64>>,
    // When expired transactions were last removed.
    last_expiry: Instant,
    metrics: BlobFetcherMetrics,
}

// `T` is only stored by value in maps and moved into boxed futures, never pinned in place.
impl<T> Unpin for BlobFetcher<T> {}

impl<T> std::fmt::Debug for BlobFetcher<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlobFetcher")
            .field("pending", &self.pending.len())
            .field("requests", &self.requests.len())
            .field("verifications", &self.verifications.len())
            .finish_non_exhaustive()
    }
}

type RequestFuture = Pin<Box<dyn Future<Output = (B256, PeerId, Option<Cells>)> + Send>>;

type VerifyFuture<T> = Pin<Box<dyn Future<Output = (B256, Option<(PeerId, T)>)> + Send>>;

impl<T: PoolTransaction + 'static> BlobFetcher<T> {
    /// Creates an empty fetcher.
    pub(super) fn new() -> Self {
        Self {
            buffer: Default::default(),
            pending: Default::default(),
            requests: Default::default(),
            verifications: Default::default(),
            budgets: Default::default(),
            last_expiry: Instant::now(),
            metrics: Default::default(),
        }
    }

    /// Records that `peer` announced the blob transaction `hash` with the cells in `mask`.
    pub(super) fn announce(&mut self, hash: B256, peer: PeerId, mask: BlobCellMask) {
        if mask.count() == 0 ||
            (!self.pending.contains_key(&hash) && self.pending.len() >= MAX_PENDING_TRANSACTIONS)
        {
            return
        }
        let pending = self.pending.entry(hash).or_insert_with(|| Pending::new(Instant::now()));
        if let Some((_, announced)) = pending.providers.iter_mut().find(|(id, _)| *id == peer) {
            *announced = mask;
        } else if pending.providers.len() < MAX_PROVIDERS_PER_TRANSACTION {
            pending.providers.push((peer, mask));
        }
    }

    /// Buffers the body of a blob transaction that `peer` sent without blobs.
    ///
    /// Returns `false` if the body is malformed, see [`BlobBuffer::body`].
    pub(super) fn body(&mut self, peer: PeerId, tx: T) -> bool {
        self.buffer.body(peer, tx, Instant::now())
    }

    /// Stops fetching cells from a disconnected `peer`.
    pub(super) fn drop_peer(&mut self, peer: &PeerId) {
        self.budgets.remove(peer);
        for pending in self.pending.values_mut() {
            pending.providers.retain(|(id, _)| id != peer);
        }
    }

    /// Advances requests and checks, and sends new requests to `peers`.
    ///
    /// Returns a transaction whose cells passed the check, with the peer that sent its body.
    pub(super) fn poll<N: NetworkPrimitives>(
        &mut self,
        cx: &mut Context<'_>,
        peers: &HashMap<PeerId, PeerMetadata<N>, FbBuildHasher<64>>,
    ) -> Poll<(PeerId, T)> {
        if let Poll::Ready(Some((hash, result))) = self.verifications.poll_next_unpin(cx) {
            self.pending.remove(&hash);
            if let Some(verified) = result {
                self.metrics.verified.increment(1);
                return Poll::Ready(verified)
            }
            // Either the cells or the body's proofs are invalid, and only the proofs aren't
            // bound by the transaction hash, so nobody is certainly at fault.
            self.metrics.invalid.increment(1);
        }

        while let Poll::Ready(Some((hash, peer, response))) = self.requests.poll_next_unpin(cx) {
            self.on_response(hash, peer, response);
        }

        let now = Instant::now();
        if now.duration_since(self.last_expiry) >= EXPIRY_INTERVAL {
            self.last_expiry = now;
            self.metrics.expired.increment(self.buffer.expire(now) as u64);
            self.pending.retain(|_, pending| now.duration_since(pending.created) < PENDING_TTL);
        }

        let mut queued = self.queue_verifications();
        queued |= self.send_requests(peers, now);
        self.update_gauges();
        // New futures need a first poll to register their wakers.
        if queued {
            cx.waker().wake_by_ref();
        }
        Poll::Pending
    }

    // Buffers the cells of a reply, keeping `peer` as a provider of the columns it didn't send.
    fn on_response(&mut self, hash: B256, peer: PeerId, response: Option<Cells>) {
        let Some(pending) = self.pending.get_mut(&hash) else { return };
        pending.inflight = false;
        let provider = pending
            .providers
            .iter()
            .position(|(id, _)| *id == peer)
            .map(|index| pending.providers.remove(index));
        let Some(response) = response else {
            self.metrics.failed_requests.increment(1);
            pending.target = None;
            return
        };
        // The session checked the reply against the request, so it only holds requested columns
        // with whole cells per blob. EIP-8070 lets a peer leave columns out under load.
        let mask = response.cell_mask.mask_at(0).map(BlobCellMask::new).unwrap_or_default();
        match response.cells.into_iter().next() {
            Some(cells) if mask.count() > 0 => {
                if self.buffer.cells(hash, mask, cells, Instant::now()) {
                    pending.received |= mask.bits();
                    // A short reply still made progress, so keep the peer for the columns it
                    // left out, behind the peers that haven't been asked yet.
                    pending.providers.extend(provider);
                }
            }
            // The columns this peer announced are no longer available from it.
            _ => pending.target = None,
        }
    }

    // Starts checking the transactions whose body and target columns arrived.
    fn queue_verifications(&mut self) -> bool {
        let mut queued = false;
        for (&hash, pending) in &mut self.pending {
            if self.verifications.len() >= MAX_CONCURRENT_VERIFICATIONS {
                break
            }
            if pending.verifying {
                continue
            }
            if let Some(target) = pending.target &&
                let Some(job) = self.buffer.take_ready(hash, target)
            {
                pending.verifying = true;
                queued = true;
                self.verifications.push(Box::pin(verify(hash, job)));
            }
        }
        queued
    }

    // Requests missing columns of pending transactions from the peers that announced them.
    fn send_requests<N: NetworkPrimitives>(
        &mut self,
        peers: &HashMap<PeerId, PeerMetadata<N>, FbBuildHasher<64>>,
        now: Instant,
    ) -> bool {
        let mut queued = false;
        for (&hash, pending) in &mut self.pending {
            if self.requests.len() >= MAX_INFLIGHT_REQUESTS {
                break
            }
            if pending.inflight || pending.verifying {
                continue
            }
            if pending.target.is_none() {
                pending.target = pending.recoverable_target();
            }
            let Some(target) = pending.target else { continue };
            let missing = target.bits() & !pending.received;
            if missing == 0 {
                continue
            }
            for &(peer, announced) in &pending.providers {
                let Some(metadata) = peers.get(&peer) else { continue };
                let requested = BlobCellMask::from_bits(missing & announced.bits());
                if requested.count() == 0 {
                    continue
                }
                let budget = self.budgets.entry(peer).or_insert_with(|| Budget::new(now));
                budget.refill(now);
                let columns = requested.count() as f64;
                if budget.columns < columns {
                    continue
                }
                let (tx, rx) = oneshot::channel();
                let request = PeerRequest::GetCells {
                    request: GetCells {
                        hashes: vec![hash],
                        cell_mask: encode_cell_mask(requested).into(),
                    },
                    response: tx,
                };
                if metadata.request_tx.try_send(request).is_err() {
                    continue
                }
                self.metrics.requests.increment(1);
                budget.columns -= columns;
                pending.inflight = true;
                queued = true;
                self.requests.push(Box::pin(async move {
                    let response = tokio::time::timeout(REQUEST_TIMEOUT, rx)
                        .await
                        .ok()
                        .and_then(Result::ok)
                        .and_then(Result::ok);
                    (hash, peer, response)
                }));
                break
            }
        }
        queued
    }

    fn update_gauges(&self) {
        let (buffered, bytes) = self.buffer.stats();
        self.metrics.buffered.set(buffered as f64);
        self.metrics.buffered_bytes.set(bytes as f64);
        self.metrics.pending.set(self.pending.len() as f64);
        self.metrics.inflight_requests.set(self.requests.len() as f64);
    }
}

// Checks the cells of `job` on a blocking thread.
async fn verify<T: PoolTransaction + 'static>(
    hash: B256,
    job: BufferedBlob<T>,
) -> (B256, Option<(PeerId, T)>) {
    let result = tokio::task::spawn_blocking(move || job.verify()).await.ok().flatten();
    (hash, result)
}

// A transaction whose cells are being fetched.
struct Pending {
    // When it was first announced.
    created: Instant,
    // Peers that announced it, with the cells they announced.
    providers: Vec<(PeerId, BlobCellMask)>,
    // Columns to fetch, chosen once the providers announce enough of them.
    target: Option<BlobCellMask>,
    // Columns received.
    received: u128,
    // Whether a request is in flight.
    inflight: bool,
    // Whether its cells are being checked.
    verifying: bool,
}

impl Pending {
    const fn new(created: Instant) -> Self {
        Self {
            created,
            providers: Vec::new(),
            target: None,
            received: 0,
            inflight: false,
            verifying: false,
        }
    }

    // Returns the received columns plus announced ones, up to the half needed to rebuild every
    // blob, or `None` if the providers don't announce enough of them yet.
    fn recoverable_target(&self) -> Option<BlobCellMask> {
        let needed = CELLS_PER_EXT_BLOB / 2;
        let announced = self
            .providers
            .iter()
            .fold(self.received, |mask, (_, announced)| mask | announced.bits());
        let mut target = self.received;
        for index in BlobCellMask::from_bits(announced & !self.received)
            .selected_indices()
            .take(needed.saturating_sub(self.received.count_ones() as usize))
        {
            target |= 1 << index;
        }
        (target.count_ones() as usize == needed).then_some(BlobCellMask::from_bits(target))
    }
}

// Columns a peer may be asked for, refilled over time.
struct Budget {
    columns: f64,
    updated: Instant,
}

impl Budget {
    const fn new(now: Instant) -> Self {
        Self { columns: PEER_COLUMN_BUDGET, updated: now }
    }

    fn refill(&mut self, now: Instant) {
        let elapsed = now.duration_since(self.updated).as_secs_f64();
        self.columns =
            elapsed.mul_add(PEER_COLUMN_REFILL_PER_SEC, self.columns).min(PEER_COLUMN_BUDGET);
        self.updated = now;
    }
}

/// Metrics of the eth/72 cell fetcher.
#[derive(Metrics)]
#[metrics(scope = "network.blob_fetcher")]
struct BlobFetcherMetrics {
    /// Transactions waiting for their cells.
    pending: Gauge,
    /// Transactions buffered while their body or cells are missing.
    buffered: Gauge,
    /// Bytes of buffered bodies and cells.
    buffered_bytes: Gauge,
    /// `GetCells` requests in flight.
    inflight_requests: Gauge,
    /// `GetCells` requests sent.
    requests: Counter,
    /// `GetCells` requests that timed out or whose peer disconnected.
    failed_requests: Counter,
    /// Transactions whose cells passed the check.
    verified: Counter,
    /// Transactions whose cells failed the check.
    invalid: Counter,
    /// Buffered transactions dropped before their body or cells arrived.
    expired: Counter,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transactions::blob_buffer::tests::fixture;
    use futures::task::noop_waker_ref;
    use reth_eth_wire::{EthNetworkPrimitives, EthVersion};
    use reth_network_api::{PeerKind, PeerRequestSender};
    use reth_network_p2p::error::RequestResult;
    use reth_transaction_pool::EthPooledTransaction;
    use tokio::sync::mpsc;

    fn peer(id: PeerId) -> (PeerMetadata<EthNetworkPrimitives>, mpsc::Receiver<PeerRequest>) {
        let (tx, rx) = mpsc::channel(16);
        let metadata = PeerMetadata::new(
            PeerRequestSender::new(id, tx),
            EthVersion::Eth72,
            "test".into(),
            100,
            PeerKind::Basic,
        );
        (metadata, rx)
    }

    // Returns the next `GetCells` request sent to a peer.
    fn next_request(
        rx: &mut mpsc::Receiver<PeerRequest>,
    ) -> (GetCells, oneshot::Sender<RequestResult<Cells>>) {
        let Ok(PeerRequest::GetCells { request, response }) = rx.try_recv() else {
            panic!("expected a GetCells request")
        };
        (request, response)
    }

    async fn next_transaction(
        fetcher: &mut BlobFetcher<EthPooledTransaction>,
        peers: &HashMap<PeerId, PeerMetadata<EthNetworkPrimitives>, FbBuildHasher<64>>,
    ) -> (PeerId, EthPooledTransaction) {
        tokio::time::timeout(
            Duration::from_secs(10),
            std::future::poll_fn(|cx| fetcher.poll(cx, peers)),
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn fetches_the_cells_needed_to_rebuild_the_blobs() {
        let (tx, cells) = fixture();
        let hash = *tx.hash();
        let id = PeerId::random();
        let (metadata, mut rx) = peer(id);
        let peers = HashMap::from_iter([(id, metadata)]);
        let mut fetcher = BlobFetcher::new();
        let mut cx = Context::from_waker(noop_waker_ref());

        // Fewer than half the columns announced can't rebuild the blob, so nothing is requested.
        fetcher.announce(hash, id, BlobCellMask::from_bits(u32::MAX.into()));
        assert!(fetcher.poll(&mut cx, &peers).is_pending());
        assert!(rx.try_recv().is_err());

        fetcher.announce(hash, id, BlobCellMask::from_bits(u128::MAX));
        assert!(fetcher.body(id, tx));
        assert!(fetcher.poll(&mut cx, &peers).is_pending());
        let (request, response) = next_request(&mut rx);
        let mask = request.cell_mask(0).unwrap();
        assert_eq!(mask, BlobCellMask::from_bits(u64::MAX.into()));
        response
            .send(Ok(Cells {
                hashes: vec![hash],
                cells: vec![cells.get_cells(mask).unwrap()],
                cell_mask: request.cell_mask,
            }))
            .unwrap();
        let (sender, tx) = next_transaction(&mut fetcher, &peers).await;
        assert_eq!(sender, id);
        assert_eq!(tx.blob_cell_availability().unwrap().get(), mask);
        assert!(tx.blob_cell_availability().unwrap().is_recoverable());
    }

    #[tokio::test]
    async fn empty_reply_falls_back_to_another_peer() {
        let (tx, cells) = fixture();
        let hash = *tx.hash();
        let (first, second) = (PeerId::random(), PeerId::random());
        let (p1, mut rx1) = peer(first);
        let (p2, mut rx2) = peer(second);
        let peers = HashMap::from_iter([(first, p1), (second, p2)]);
        let mut fetcher = BlobFetcher::new();
        let all = BlobCellMask::from_bits(u128::MAX);
        fetcher.announce(hash, first, all);
        fetcher.announce(hash, second, all);
        assert!(fetcher.body(first, tx));
        let mut cx = Context::from_waker(noop_waker_ref());
        assert!(fetcher.poll(&mut cx, &peers).is_pending());
        let (request, response) = next_request(&mut rx1);
        response.send(Ok(Cells { cell_mask: request.cell_mask, ..Default::default() })).unwrap();
        assert!(fetcher.poll(&mut cx, &peers).is_pending());
        let (request, response) = next_request(&mut rx2);
        let mask = request.cell_mask(0).unwrap();
        response
            .send(Ok(Cells {
                hashes: vec![hash],
                cells: vec![cells.get_cells(mask).unwrap()],
                cell_mask: request.cell_mask,
            }))
            .unwrap();
        let (_, tx) = next_transaction(&mut fetcher, &peers).await;
        assert_eq!(tx.blob_cell_availability().unwrap().get().count(), CELLS_PER_EXT_BLOB / 2);
    }

    #[tokio::test]
    async fn short_reply_requests_the_rest() {
        let (tx, cells) = fixture();
        let hash = *tx.hash();
        let id = PeerId::random();
        let (metadata, mut rx) = peer(id);
        let peers = HashMap::from_iter([(id, metadata)]);
        let mut fetcher = BlobFetcher::new();
        fetcher.announce(hash, id, BlobCellMask::from_bits(u128::MAX));
        assert!(fetcher.body(id, tx));
        let mut cx = Context::from_waker(noop_waker_ref());
        assert!(fetcher.poll(&mut cx, &peers).is_pending());
        let (_, response) = next_request(&mut rx);
        let sent = BlobCellMask::from_bits(1);
        response
            .send(Ok(Cells {
                hashes: vec![hash],
                cells: vec![cells.get_cells(sent).unwrap()],
                cell_mask: encode_cell_mask(sent).into(),
            }))
            .unwrap();
        assert!(fetcher.poll(&mut cx, &peers).is_pending());
        let (request, response) = next_request(&mut rx);
        let rest = request.cell_mask(0).unwrap();
        assert_eq!(rest, BlobCellMask::from_bits(u128::from(u64::MAX) & !1));
        response
            .send(Ok(Cells {
                hashes: vec![hash],
                cells: vec![cells.get_cells(rest).unwrap()],
                cell_mask: request.cell_mask,
            }))
            .unwrap();
        let (_, tx) = next_transaction(&mut fetcher, &peers).await;
        assert_eq!(tx.blob_cell_availability().unwrap().get().count(), CELLS_PER_EXT_BLOB / 2);
    }

    #[tokio::test]
    async fn invalid_cells_drop_the_transaction() {
        let (tx, cells) = fixture();
        let hash = *tx.hash();
        let id = PeerId::random();
        let (metadata, mut rx) = peer(id);
        let peers = HashMap::from_iter([(id, metadata)]);
        let mut fetcher = BlobFetcher::new();
        fetcher.announce(hash, id, BlobCellMask::from_bits(u128::MAX));
        assert!(fetcher.body(id, tx));
        let mut cx = Context::from_waker(noop_waker_ref());
        assert!(fetcher.poll(&mut cx, &peers).is_pending());
        let (request, response) = next_request(&mut rx);
        let mut corrupted = cells.get_cells(request.cell_mask(0).unwrap()).unwrap();
        corrupted[0][31] ^= 1;
        response
            .send(Ok(Cells {
                hashes: vec![hash],
                cells: vec![corrupted],
                cell_mask: request.cell_mask,
            }))
            .unwrap();
        // Polls until the check finishes and the transaction is dropped.
        tokio::time::timeout(Duration::from_secs(10), async {
            while !fetcher.pending.is_empty() {
                assert!(std::future::poll_fn(|cx| Poll::Ready(fetcher.poll(cx, &peers)))
                    .await
                    .is_pending());
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(fetcher.buffer.stats(), (0, 0));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn peer_budget_refills_up_to_its_cap() {
        let now = Instant::now();
        let mut budget = Budget { columns: 0.0, updated: now };
        budget.refill(now + Duration::from_secs(1));
        assert_eq!(budget.columns, PEER_COLUMN_REFILL_PER_SEC);
        budget.refill(now + Duration::from_secs(100));
        assert_eq!(budget.columns, PEER_COLUMN_BUDGET);
    }
}
