//! ETH/72 cell scheduling, provider failover, and sparse custody selection.

use super::{blob_buffer::BlobBuffer, PeerMetadata};
use alloy_eips::eip7594::BlobCellMask;
use alloy_primitives::{
    map::{B256Map, FbBuildHasher, HashMap},
    B128, B256,
};
use futures::{stream::FuturesUnordered, Future, StreamExt};
use reth_eth_wire::{Cells, GetCells, NetworkPrimitives};
use reth_network_api::{CellCustody, PeerRequest};
use reth_network_peers::PeerId;
use reth_transaction_pool::PoolTransaction;
use std::{
    pin::Pin,
    task::{Context, Poll},
    time::{Duration, Instant},
};
use tokio::sync::oneshot;

/// Independent cell fetcher. Transaction bodies still use the transaction fetcher.
pub(super) struct BlobFetcher<T> {
    buffer: BlobBuffer<T>,
    pending: B256Map<Pending>,
    requests: FuturesUnordered<RequestFuture>,
    verification: FuturesUnordered<VerifyFuture<T>>,
    budgets: HashMap<PeerId, Budget, FbBuildHasher<64>>,
    custody: CellCustody,
    probability: u8,
    tick: tokio::time::Interval,
    metrics: BlobFetcherMetrics,
}

// No field exposes a pinned transaction or relies on its address.
impl<T> Unpin for BlobFetcher<T> {}
impl<T> std::fmt::Debug for BlobFetcher<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlobFetcher")
            .field("pending", &self.pending.len())
            .field("requests", &self.requests.len())
            .finish_non_exhaustive()
    }
}

type RequestFuture =
    Pin<Box<dyn Future<Output = (B256, PeerId, BlobCellMask, Option<Cells>)> + Send>>;
type VerifyFuture<T> =
    Pin<Box<dyn Future<Output = (B256, Result<(PeerId, T), Vec<PeerId>>)> + Send>>;
const MAX_PENDING: usize = 4096;
const MAX_REQUESTS: usize = 16;
const WAIT: Duration = Duration::from_secs(2);
const TIMEOUT: Duration = Duration::from_secs(5);
const TTL: Duration = Duration::from_secs(120);

impl<T: PoolTransaction + 'static> BlobFetcher<T> {
    pub(super) fn new(custody: CellCustody, probability: u8) -> Self {
        Self {
            buffer: Default::default(),
            pending: Default::default(),
            requests: Default::default(),
            verification: Default::default(),
            budgets: Default::default(),
            custody,
            probability: probability.clamp(15, 100),
            tick: tokio::time::interval(Duration::from_millis(100)),
            metrics: Default::default(),
        }
    }

    pub(super) fn announce(&mut self, hash: B256, peer: PeerId, mask: BlobCellMask) {
        if mask.count() == 0 ||
            (!self.pending.contains_key(&hash) && self.pending.len() >= MAX_PENDING)
        {
            return
        }
        let pending = self.pending.entry(hash).or_insert_with(|| Pending::new(Instant::now()));
        if let Some(provider) = pending.providers.iter_mut().find(|(id, _)| *id == peer) {
            provider.1 = mask;
        } else if pending.providers.len() < 16 {
            pending.providers.push((peer, mask));
        }
    }

    pub(super) fn body(&mut self, peer: PeerId, tx: T) -> Result<(), ()> {
        self.buffer.body(peer, tx, Instant::now())
    }

    pub(super) fn drop_peer(&mut self, peer: PeerId) {
        self.budgets.remove(&peer);
        for pending in self.pending.values_mut() {
            pending.providers.retain(|(id, _)| *id != peer);
        }
    }

    pub(super) fn poll<N: NetworkPrimitives>(
        &mut self,
        cx: &mut Context<'_>,
        peers: &HashMap<PeerId, PeerMetadata<N>, FbBuildHasher<64>>,
    ) -> Poll<BlobFetchEvent<T>> {
        let (buffered, bytes) = self.buffer.stats();
        self.metrics.buffered.set(buffered as f64);
        self.metrics.buffer_bytes.set(bytes as f64);
        self.metrics.pending.set(self.pending.len() as f64);
        self.metrics.inflight.set(self.requests.len() as f64);
        if let Poll::Ready(Some((hash, result))) = self.verification.poll_next_unpin(cx) {
            if result.is_ok() {
                self.metrics.completed.increment(1);
            } else {
                self.metrics.invalid.increment(1);
            }
            self.pending.remove(&hash);
            return Poll::Ready(match result {
                Ok((peer, tx)) => BlobFetchEvent::Transaction(peer, tx),
                Err(peers) => BlobFetchEvent::BadPeers(peers),
            })
        }
        while let Poll::Ready(Some((hash, peer, requested, response))) =
            self.requests.poll_next_unpin(cx)
        {
            let Some(pending) = self.pending.get_mut(&hash) else { continue };
            pending.inflight = false;
            pending.providers.retain(|(id, _)| *id != peer);
            let Some(response) = response else {
                self.metrics.failed_requests.increment(1);
                if pending.full == Some(true) {
                    pending.target = None;
                }
                continue
            };
            if u128::from_le_bytes(response.cell_mask.into()) != requested.bits() ||
                response.hashes.len() != response.cells.len() ||
                response.hashes.len() > 1 ||
                response.hashes.first().is_some_and(|h| *h != hash)
            {
                if pending.full == Some(true) {
                    pending.target = None;
                }
                return Poll::Ready(BlobFetchEvent::BadPeers(vec![peer]))
            }
            if response.cells.is_empty() && pending.full == Some(true) {
                pending.target = None;
            }
            if let Some(cells) = response.cells.into_iter().next() {
                if cells.is_empty() ||
                    !cells.len().is_multiple_of(requested.count()) ||
                    cells.len() / requested.count() > 128
                {
                    if pending.full == Some(true) {
                        pending.target = None;
                    }
                    return Poll::Ready(BlobFetchEvent::BadPeers(vec![peer]))
                }
                if self.buffer.cells(hash, peer, requested, cells, Instant::now()) {
                    pending.received |= requested.bits();
                }
            }
        }
        let now = Instant::now();
        while self.tick.poll_tick(cx).is_ready() {
            self.metrics.expired.increment(self.buffer.expire(now).len() as u64);
            self.pending.retain(|_, p| now.duration_since(p.created) < TTL);
        }
        let mut queued = false;
        // Bound proof verification jobs independently of network request concurrency.
        for (&hash, pending) in &mut self.pending {
            if self.verification.len() >= 4 {
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
                self.verification.push(Box::pin(async move {
                    let result = tokio::task::spawn_blocking(move || job.verify())
                        .await
                        .unwrap_or_else(|_| Err(Vec::new()));
                    (hash, result)
                }));
            }
        }
        for (&hash, pending) in &mut self.pending {
            if self.requests.len() >= MAX_REQUESTS {
                break
            }
            if pending.inflight || pending.verifying {
                continue
            }
            if pending.target.is_none() {
                let full_providers =
                    pending.providers.iter().filter(|(_, m)| m.count() >= 64).count();
                if pending.full.is_none() &&
                    full_providers < 2 &&
                    now.duration_since(pending.created) < WAIT
                {
                    continue
                }
                let custody = BlobCellMask::new(self.custody.get());
                let first_decision = pending.full.is_none();
                let full = *pending.full.get_or_insert_with(|| {
                    custody.count() == 0 ||
                        custody.count() >= 64 ||
                        full_providers < 2 ||
                        rand::random_range(0..100u8) < self.probability
                });
                if first_decision {
                    if full {
                        self.metrics.full.increment(1);
                    } else {
                        self.metrics.sampled.increment(1);
                    }
                }
                pending.target = if full {
                    let available = pending
                        .providers
                        .iter()
                        .fold(pending.received, |mask, (_, provider)| mask | provider.bits());
                    let mut selected = pending.received;
                    for index in BlobCellMask::from_bits(available & !pending.received)
                        .selected_indices()
                        .take(64usize.saturating_sub(pending.received.count_ones() as usize))
                    {
                        selected |= 1 << index;
                    }
                    (selected.count_ones() == 64).then_some(BlobCellMask::from_bits(selected))
                } else {
                    Some(custody)
                };
            }
            let Some(target) = pending.target else { continue };
            let missing = target.bits() & !pending.received;
            if missing == 0 {
                continue
            }
            for &(peer, available) in &pending.providers {
                let Some(metadata) = peers.get(&peer) else { continue };
                let requested = BlobCellMask::from_bits(missing & available.bits());
                if requested.count() == 0 {
                    continue
                }
                let budget = self
                    .budgets
                    .entry(peer)
                    .or_insert_with(|| Budget { tokens: 256.0, updated: now });
                budget.refill(now);
                if budget.tokens < requested.count() as f64 {
                    continue
                }
                let (tx, rx) = oneshot::channel();
                let request = PeerRequest::GetCells {
                    request: GetCells {
                        hashes: vec![hash],
                        cell_mask: B128::from(requested.bits().to_le_bytes()),
                    },
                    response: tx,
                };
                if metadata.request_tx.try_send(request).is_err() {
                    continue
                }
                self.metrics.requests.increment(1);
                budget.tokens -= requested.count() as f64;
                pending.inflight = true;
                queued = true;
                self.requests.push(Box::pin(async move {
                    let response = tokio::time::timeout(TIMEOUT, rx)
                        .await
                        .ok()
                        .and_then(Result::ok)
                        .and_then(Result::ok);
                    (hash, peer, requested, response)
                }));
                break
            }
        }
        // Newly queued futures need a first poll to register their request/worker wakers.
        if queued {
            cx.waker().wake_by_ref();
        }
        Poll::Pending
    }
}

pub(super) enum BlobFetchEvent<T> {
    Transaction(PeerId, T),
    BadPeers(Vec<PeerId>),
}
struct Pending {
    created: Instant,
    providers: Vec<(PeerId, BlobCellMask)>,
    target: Option<BlobCellMask>,
    full: Option<bool>,
    received: u128,
    inflight: bool,
    verifying: bool,
}
impl Pending {
    const fn new(created: Instant) -> Self {
        Self {
            created,
            providers: Vec::new(),
            target: None,
            full: None,
            received: 0,
            inflight: false,
            verifying: false,
        }
    }
}
struct Budget {
    tokens: f64,
    updated: Instant,
}
impl Budget {
    fn refill(&mut self, now: Instant) {
        self.tokens =
            now.duration_since(self.updated).as_secs_f64().mul_add(9.0, self.tokens).min(256.0);
        self.updated = now;
    }
}

#[derive(reth_metrics::Metrics)]
#[metrics(scope = "network.blob_fetcher")]
struct BlobFetcherMetrics {
    /// Transactions awaiting cell acquisition.
    pending: reth_metrics::metrics::Gauge,
    /// Buffered body/cell rendezvous entries.
    buffered: reth_metrics::metrics::Gauge,
    /// Bytes retained by incomplete body/cell deliveries.
    buffer_bytes: reth_metrics::metrics::Gauge,
    /// Active cell requests.
    inflight: reth_metrics::metrics::Gauge,
    /// Requests sent to peers.
    requests: reth_metrics::metrics::Counter,
    /// Timed out or disconnected requests.
    failed_requests: reth_metrics::metrics::Counter,
    /// Transactions selected for recoverable storage.
    full: reth_metrics::metrics::Counter,
    /// Transactions selected for custody-only storage.
    sampled: reth_metrics::metrics::Counter,
    /// Successfully verified transactions passed to the pool.
    completed: reth_metrics::metrics::Counter,
    /// Failed cell verification jobs.
    invalid: reth_metrics::metrics::Counter,
    /// Incomplete buffers removed at their deadline.
    expired: reth_metrics::metrics::Counter,
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::task::noop_waker_ref;
    use reth_eth_wire::{EthNetworkPrimitives, EthVersion};
    use reth_network_api::{PeerKind, PeerRequestSender};
    use reth_transaction_pool::EthPooledTransaction;

    fn peer(
        id: PeerId,
    ) -> (PeerMetadata<EthNetworkPrimitives>, tokio::sync::mpsc::Receiver<PeerRequest>) {
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        (
            PeerMetadata::new(
                PeerRequestSender::new(id, tx),
                EthVersion::Eth72,
                "test".into(),
                100,
                PeerKind::Basic,
            ),
            rx,
        )
    }

    #[tokio::test]
    async fn full_fetch_falls_back_after_empty_response() {
        let (tx, cells) = super::super::blob_buffer::tests::fixture();
        let hash = *tx.hash();
        let first = PeerId::random();
        let second = PeerId::random();
        let (p1, mut rx1) = peer(first);
        let (p2, mut rx2) = peer(second);
        let peers = HashMap::from_iter([(first, p1), (second, p2)]);
        let mut fetcher = BlobFetcher::new(CellCustody::default(), 100);
        let all = BlobCellMask::from_bits(u128::MAX);
        fetcher.announce(hash, first, all);
        fetcher.announce(hash, second, all);
        fetcher.body(first, tx).unwrap();
        let mut cx = Context::from_waker(noop_waker_ref());
        assert!(fetcher.poll(&mut cx, &peers).is_pending());
        let PeerRequest::GetCells { request, response } = rx1.try_recv().unwrap() else {
            panic!("expected cells")
        };
        assert_eq!(request.cell_mask, B128::from((u64::MAX as u128).to_le_bytes()));
        response.send(Ok(Cells { cell_mask: request.cell_mask, ..Default::default() })).unwrap();
        assert!(fetcher.poll(&mut cx, &peers).is_pending());
        let PeerRequest::GetCells { request, response } = rx2.try_recv().unwrap() else {
            panic!("expected fallback")
        };
        let mask = BlobCellMask::from_bits(u128::from_le_bytes(request.cell_mask.into()));
        response
            .send(Ok(Cells {
                hashes: vec![hash],
                cells: vec![cells.get_cells(mask).unwrap()],
                cell_mask: request.cell_mask,
            }))
            .unwrap();
        let event = tokio::time::timeout(
            Duration::from_secs(10),
            std::future::poll_fn(|cx| fetcher.poll(cx, &peers)),
        )
        .await
        .unwrap();
        let BlobFetchEvent::Transaction(_, tx) = event else { panic!("valid cells must import") };
        assert_eq!(tx.blob_cell_availability().unwrap().get().count(), 64);
    }

    #[tokio::test]
    async fn custody_sampling_requires_two_full_providers_and_preserves_wire_order() {
        let first = PeerId::random();
        let second = PeerId::random();
        let (p1, mut rx1) = peer(first);
        let (p2, _rx2) = peer(second);
        let peers = HashMap::from_iter([(first, p1), (second, p2)]);
        let custody = CellCustody::default();
        let bits: u128 = 1 | (1 << 8) | (1 << 127);
        custody.set(B128::from(bits));
        let mut fetcher = BlobFetcher::<EthPooledTransaction>::new(custody, 15);
        fetcher.probability = 0;
        let hash = B256::random();
        let all = BlobCellMask::from_bits(u128::MAX);
        fetcher.announce(hash, first, all);
        let mut cx = Context::from_waker(noop_waker_ref());
        assert!(fetcher.poll(&mut cx, &peers).is_pending());
        assert!(rx1.try_recv().is_err());
        fetcher.announce(hash, second, all);
        assert!(fetcher.poll(&mut cx, &peers).is_pending());
        let PeerRequest::GetCells { request, .. } = rx1.try_recv().unwrap() else {
            panic!("expected sample")
        };
        assert_eq!(request.cell_mask, B128::from(bits.to_le_bytes()));
    }

    #[test]
    fn peer_budget_refills_and_is_capped() {
        let now = Instant::now();
        let mut budget = Budget { tokens: 0.0, updated: now };
        budget.refill(now + Duration::from_secs(1));
        assert_eq!(budget.tokens, 9.0);
        budget.refill(now + Duration::from_secs(100));
        assert_eq!(budget.tokens, 256.0);
    }
}
