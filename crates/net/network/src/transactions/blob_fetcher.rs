//! ETH/72 cell scheduling, provider failover, and sparse custody selection.

use super::{
    blob_buffer::{BlobBuffer, Rejected},
    PeerMetadata,
};
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

type RequestFuture = Pin<Box<dyn Future<Output = (B256, PeerId, Option<Cells>)> + Send>>;
type VerifyFuture<T> =
    Pin<Box<dyn Future<Output = (B256, Result<(PeerId, T), Option<Rejected>>)> + Send>>;
const MAX_PENDING: usize = 4096;
const MAX_REQUESTS: usize = 16;
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
            let event = match result {
                Ok((peer, tx)) => {
                    self.metrics.completed.increment(1);
                    self.pending.remove(&hash);
                    BlobFetchEvent::Transaction(peer, tx)
                }
                Err(Some(rejected)) => {
                    self.metrics.invalid.increment(1);
                    // A failed check means either the cells or the body's proofs are bad. Only
                    // a peer that sent both is certainly at fault; other failing providers are
                    // dropped for this transaction without a reputation change.
                    let reported = if rejected.peers.contains(&rejected.body_peer) {
                        vec![rejected.body_peer]
                    } else {
                        Vec::new()
                    };
                    if let Some(pending) = self.pending.get_mut(&hash) {
                        // Drop the body, which the buffer would otherwise keep over any later
                        // copy: its proofs are not bound by the transaction hash. A later
                        // announcement fetches a body again, while the verified cells are kept
                        // and the rejected columns are fetched from the remaining providers.
                        pending.received =
                            self.buffer.restore_cells(hash, rejected.verified, Instant::now());
                        pending.providers.retain(|(id, _)| !rejected.peers.contains(id));
                        pending.target = None;
                        pending.verifying = false;
                    }
                    BlobFetchEvent::BadPeers(reported)
                }
                Err(None) => {
                    self.metrics.invalid.increment(1);
                    self.pending.remove(&hash);
                    BlobFetchEvent::BadPeers(Vec::new())
                }
            };
            return Poll::Ready(event)
        }
        while let Poll::Ready(Some((hash, peer, response))) = self.requests.poll_next_unpin(cx) {
            let Some(pending) = self.pending.get_mut(&hash) else { continue };
            pending.inflight = false;
            let provider = pending
                .providers
                .iter()
                .position(|(id, _)| *id == peer)
                .map(|index| pending.providers.remove(index));
            let Some(response) = response else {
                self.metrics.failed_requests.increment(1);
                if pending.full == Some(true) {
                    pending.target = None;
                }
                continue
            };
            // The session checked the response against the request: it only holds requested
            // hashes and columns, with whole cells per blob. EIP-8070 permits a provider to
            // truncate a `Cells` response under load, so the remaining columns are scheduled from
            // other providers.
            let response_mask =
                response.cell_mask.mask_at(0).map_or(0, |mask| u128::from_le_bytes(mask.into()));
            let response_mask = BlobCellMask::from_bits(response_mask);
            match response.cells.into_iter().next() {
                Some(cells) if response_mask.count() > 0 => {
                    if self.buffer.cells(hash, peer, response_mask, cells, Instant::now()) {
                        pending.received |= response_mask.bits();
                        // A truncated reply still made progress, so keep the provider for the
                        // remaining columns, behind the providers that have not been asked yet.
                        pending.providers.extend(provider);
                    }
                }
                _ if pending.full == Some(true) => pending.target = None,
                _ => {}
            }
        }
        let now = Instant::now();
        while self.tick.poll_tick(cx).is_ready() {
            self.metrics.expired.increment(self.buffer.expire(now).len() as u64);
            self.pending.retain(|_, p| now.duration_since(p.created) < TTL);
        }
        // A CL may expand its custody set after a transaction entered the sampler path. Extend
        // the target so the delta columns are fetched instead of leaving the pending transaction
        // on the old custody set (EIP-8070 ?engine_forkchoiceUpdatedV4).
        let custody = BlobCellMask::new(self.custody.get());
        for pending in self.pending.values_mut() {
            if pending.full == Some(false) &&
                let Some(target) = pending.target
            {
                let expanded = custody.bits() & !target.bits();
                if expanded != 0 {
                    pending.target = Some(BlobCellMask::from_bits(target.bits() | expanded));
                }
            }
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
                        .unwrap_or_else(|_| Err(None));
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
                // Decide independently of provider-count saturation. A sparse sampler may
                // acquire custody cells as soon as an announcement is available.
                let custody = BlobCellMask::new(self.custody.get());
                let first_decision = pending.full.is_none();
                let full = *pending.full.get_or_insert_with(|| {
                    custody.count() == 0 ||
                        custody.count() >= 64 ||
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
                    // Request the 64 data cells needed for reconstruction, rather than all 128
                    // extended cells.
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
                    // Sampling noise: request one additional unpredictable column from a
                    // provider, as required by EIP-8070, so a peer cannot advertise only the
                    // victim's custody set and selectively withhold data.
                    let available = pending
                        .providers
                        .iter()
                        .fold(0u128, |mask, (_, provider)| mask | provider.bits());
                    let extras = available & !custody.bits();
                    let sample = if extras == 0 {
                        custody.bits()
                    } else {
                        let offset = rand::random_range(0..extras.count_ones()) as usize;
                        let index = BlobCellMask::from_bits(extras)
                            .selected_indices()
                            .nth(offset)
                            .expect("extra sampling column exists");
                        custody.bits() | (1u128 << index)
                    };
                    Some(BlobCellMask::from_bits(sample))
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
                        cell_mask: B128::from(requested.bits().to_le_bytes()).into(),
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
                    (hash, peer, response)
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
    use alloy_eips::eip4844::Bytes48;
    use futures::task::noop_waker_ref;
    use reth_eth_wire::{EthNetworkPrimitives, EthVersion};
    use reth_network_api::{PeerKind, PeerRequestSender};
    use reth_transaction_pool::{blobstore::BlobTxCellSidecar, EthPooledTransaction};

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
        assert_eq!(request.cell_mask(0), Some(BlobCellMask::from_bits(u64::MAX as u128)));
        response.send(Ok(Cells { cell_mask: request.cell_mask, ..Default::default() })).unwrap();
        assert!(fetcher.poll(&mut cx, &peers).is_pending());
        let PeerRequest::GetCells { request, response } = rx2.try_recv().unwrap() else {
            panic!("expected fallback")
        };
        let mask = request.cell_mask(0).unwrap();
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
    async fn full_fetch_requests_remainder_after_truncated_response() {
        let (tx, cells) = super::super::blob_buffer::tests::fixture();
        let hash = *tx.hash();
        let provider = PeerId::random();
        let (metadata, mut rx) = peer(provider);
        let peers = HashMap::from_iter([(provider, metadata)]);
        let mut fetcher = BlobFetcher::new(CellCustody::default(), 100);
        fetcher.announce(hash, provider, BlobCellMask::from_bits(u128::MAX));
        fetcher.body(provider, tx).unwrap();
        let mut cx = Context::from_waker(noop_waker_ref());
        assert!(fetcher.poll(&mut cx, &peers).is_pending());
        let PeerRequest::GetCells { request, response } = rx.try_recv().unwrap() else {
            panic!("expected cells")
        };
        assert_eq!(request.cell_mask(0), Some(BlobCellMask::from_bits(u64::MAX as u128)));
        let truncated = BlobCellMask::from_bits(1);
        response
            .send(Ok(Cells {
                hashes: vec![hash],
                cells: vec![cells.get_cells(truncated).unwrap()],
                cell_mask: B128::from(truncated.bits().to_le_bytes()).into(),
            }))
            .unwrap();
        assert!(fetcher.poll(&mut cx, &peers).is_pending());
        let PeerRequest::GetCells { request, response } = rx.try_recv().unwrap() else {
            panic!("expected remainder")
        };
        let remainder = request.cell_mask(0).unwrap();
        assert_eq!(remainder.bits(), u64::MAX as u128 & !1);
        response
            .send(Ok(Cells {
                hashes: vec![hash],
                cells: vec![cells.get_cells(remainder).unwrap()],
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
    async fn full_fetch_refetches_cells_rejected_by_verification() {
        let (tx, cells) = super::super::blob_buffer::tests::fixture();
        let hash = *tx.hash();
        let honest = PeerId::random();
        let bad = PeerId::random();
        let (p1, mut rx1) = peer(honest);
        let (p2, mut rx2) = peer(bad);
        let peers = HashMap::from_iter([(honest, p1), (bad, p2)]);
        let mut fetcher = BlobFetcher::new(CellCustody::default(), 100);
        let all = BlobCellMask::from_bits(u128::MAX);
        fetcher.announce(hash, honest, all);
        fetcher.announce(hash, bad, all);
        fetcher.body(bad, tx.clone()).unwrap();
        let mut cx = Context::from_waker(noop_waker_ref());
        assert!(fetcher.poll(&mut cx, &peers).is_pending());
        let PeerRequest::GetCells { response, .. } = rx1.try_recv().unwrap() else {
            panic!("expected cells")
        };
        let low = BlobCellMask::from_bits(u32::MAX as u128);
        response
            .send(Ok(Cells {
                hashes: vec![hash],
                cells: vec![cells.get_cells(low).unwrap()],
                cell_mask: B128::from(low.bits().to_le_bytes()).into(),
            }))
            .unwrap();
        assert!(fetcher.poll(&mut cx, &peers).is_pending());
        let PeerRequest::GetCells { request, response } = rx2.try_recv().unwrap() else {
            panic!("expected remainder from the second provider")
        };
        let high = request.cell_mask(0).unwrap();
        let mut corrupted = cells.get_cells(high).unwrap();
        corrupted[0][31] ^= 1;
        response
            .send(Ok(Cells {
                hashes: vec![hash],
                cells: vec![corrupted],
                cell_mask: request.cell_mask,
            }))
            .unwrap();
        let event = tokio::time::timeout(
            Duration::from_secs(10),
            std::future::poll_fn(|cx| fetcher.poll(cx, &peers)),
        )
        .await
        .unwrap();
        let BlobFetchEvent::BadPeers(reported) = event else {
            panic!("bad cells must be rejected")
        };
        assert_eq!(reported, vec![bad]);
        assert!(fetcher.poll(&mut cx, &peers).is_pending());
        let PeerRequest::GetCells { request, response } = rx1.try_recv().unwrap() else {
            panic!("expected refetch of the rejected columns")
        };
        assert_eq!(request.cell_mask(0), Some(high));
        response
            .send(Ok(Cells {
                hashes: vec![hash],
                cells: vec![cells.get_cells(high).unwrap()],
                cell_mask: request.cell_mask,
            }))
            .unwrap();
        // The body was dropped with the rejected cells, so the cells wait for a new body.
        assert!(fetcher.poll(&mut cx, &peers).is_pending());
        assert!(fetcher.verification.is_empty());
        fetcher.body(honest, tx).unwrap();
        let event = tokio::time::timeout(
            Duration::from_secs(10),
            std::future::poll_fn(|cx| fetcher.poll(cx, &peers)),
        )
        .await
        .unwrap();
        let BlobFetchEvent::Transaction(_, tx) = event else { panic!("valid cells must import") };
        assert_eq!(tx.blob_cell_availability().unwrap().get().count(), 64);
        assert!(rx2.try_recv().is_err());
    }

    #[tokio::test]
    async fn bad_body_proofs_do_not_blame_cell_providers() {
        let (tx, cells) = super::super::blob_buffer::tests::fixture();
        let hash = *tx.hash();
        // The proofs are not bound by the transaction hash, so a body with a corrupted proof
        // keeps the hash but fails the honest cells checked against it.
        let mut proofs = cells.proofs().to_vec();
        proofs[0] = Bytes48::ZERO;
        let poisoned = BlobTxCellSidecar::new(
            cells.commitments().to_vec(),
            proofs,
            cells.cells().to_vec(),
            cells.mask(),
        )
        .unwrap();
        let poisoned = super::super::blob_buffer::tests::transaction(&poisoned);
        assert_eq!(*poisoned.hash(), hash);
        let low_provider = PeerId::random();
        let high_provider = PeerId::random();
        let (p1, mut rx1) = peer(low_provider);
        let (p2, mut rx2) = peer(high_provider);
        let peers = HashMap::from_iter([(low_provider, p1), (high_provider, p2)]);
        let mut fetcher = BlobFetcher::new(CellCustody::default(), 100);
        let all = BlobCellMask::from_bits(u128::MAX);
        fetcher.announce(hash, low_provider, all);
        fetcher.announce(hash, high_provider, all);
        fetcher.body(PeerId::random(), poisoned).unwrap();
        let mut cx = Context::from_waker(noop_waker_ref());
        assert!(fetcher.poll(&mut cx, &peers).is_pending());
        let PeerRequest::GetCells { response, .. } = rx1.try_recv().unwrap() else {
            panic!("expected cells")
        };
        let low = BlobCellMask::from_bits(u32::MAX as u128);
        response
            .send(Ok(Cells {
                hashes: vec![hash],
                cells: vec![cells.get_cells(low).unwrap()],
                cell_mask: B128::from(low.bits().to_le_bytes()).into(),
            }))
            .unwrap();
        assert!(fetcher.poll(&mut cx, &peers).is_pending());
        let PeerRequest::GetCells { request, response } = rx2.try_recv().unwrap() else {
            panic!("expected remainder from the second provider")
        };
        let high = request.cell_mask(0).unwrap();
        response
            .send(Ok(Cells {
                hashes: vec![hash],
                cells: vec![cells.get_cells(high).unwrap()],
                cell_mask: request.cell_mask,
            }))
            .unwrap();
        let event = tokio::time::timeout(
            Duration::from_secs(10),
            std::future::poll_fn(|cx| fetcher.poll(cx, &peers)),
        )
        .await
        .unwrap();
        let BlobFetchEvent::BadPeers(reported) = event else {
            panic!("cells checked against bad proofs must be rejected")
        };
        assert!(reported.is_empty());
        // The columns checked against the corrupted proof are fetched again from the provider
        // whose cells passed, and a correct body completes the transaction.
        assert!(fetcher.poll(&mut cx, &peers).is_pending());
        let PeerRequest::GetCells { request, response } = rx2.try_recv().unwrap() else {
            panic!("expected refetch of the rejected columns")
        };
        assert_eq!(request.cell_mask(0), Some(low));
        response
            .send(Ok(Cells {
                hashes: vec![hash],
                cells: vec![cells.get_cells(low).unwrap()],
                cell_mask: request.cell_mask,
            }))
            .unwrap();
        fetcher.body(high_provider, tx).unwrap();
        let event = tokio::time::timeout(
            Duration::from_secs(10),
            std::future::poll_fn(|cx| fetcher.poll(cx, &peers)),
        )
        .await
        .unwrap();
        let BlobFetchEvent::Transaction(_, tx) = event else { panic!("valid cells must import") };
        assert_eq!(tx.blob_cell_availability().unwrap().get().count(), 64);
        assert!(rx1.try_recv().is_err());
    }

    #[tokio::test]
    async fn custody_sampling_starts_with_one_provider() {
        let first = PeerId::random();
        let (p1, mut rx1) = peer(first);
        let peers = HashMap::from_iter([(first, p1)]);
        let custody = CellCustody::default();
        let bits: u128 = 1 | (1 << 8) | (1 << 127);
        custody.set(B128::from(bits.to_le_bytes()));
        let mut fetcher = BlobFetcher::<EthPooledTransaction>::new(custody, 15);
        fetcher.probability = 0;
        let hash = B256::random();
        let all = BlobCellMask::from_bits(u128::MAX);
        let mut cx = Context::from_waker(noop_waker_ref());
        fetcher.announce(hash, first, all);
        assert!(fetcher.poll(&mut cx, &peers).is_pending());
        let PeerRequest::GetCells { request, .. } = rx1.try_recv().unwrap() else {
            panic!("expected sample")
        };
        let requested = request.cell_mask(0).unwrap().bits();
        assert_eq!(requested & bits, bits);
        assert_eq!(requested.count_ones(), bits.count_ones() + 1);
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
