//! Two-stage transaction fetcher.
//!
//! Stage 1 (QUEUE): Hash is ready to be fetched, waiting for an idle peer.
//!
//! Stage 2 (FETCH): Hash is being actively fetched from a specific peer.

use super::{
    config::TransactionFetcherConfig,
    constants::{tx_fetcher::*, SOFT_LIMIT_COUNT_HASHES_IN_GET_POOLED_TRANSACTIONS_REQUEST},
    PeerMetadata, PooledTransactions,
};
use crate::metrics::TransactionFetcherMetrics;
use alloy_consensus::transaction::PooledTransaction;
use alloy_primitives::{
    map::{B256Map, B256Set, HashMap, HashSet},
    TxHash,
};
use derive_more::{Constructor, Deref};
use futures::{
    future::{AbortHandle, Abortable},
    stream::FuturesUnordered,
    Future, StreamExt,
};
use metrics::Histogram;
use rand::seq::SliceRandom;
use reth_eth_wire::{
    DedupPayload, GetPooledTransactions, PartiallyValidData, RequestTxHashes, ValidAnnouncementData,
};
use reth_eth_wire_types::{EthNetworkPrimitives, NetworkPrimitives};
use reth_metrics::{metrics::Counter, Metrics};
use reth_network_api::PeerRequest;
use reth_network_p2p::error::{RequestError, RequestResult};
use reth_network_peers::PeerId;
use reth_primitives_traits::SignedTransaction;
use smallvec::SmallVec;
use std::{
    collections::VecDeque,
    hash::BuildHasher,
    pin::Pin,
    task::{Context, Poll, Waker},
};
use tokio::{
    sync::{mpsc::error::TrySendError, oneshot, oneshot::error::RecvError},
    time::Instant,
};

/// The type responsible for fetching missing transactions from peers.
///
/// Uses a two-stage pipeline:
/// - Stage 1 (announced): hashes ready to fetch, indexed by peer
/// - Stage 2 (fetching): hashes actively being fetched
#[derive(Debug)]
pub(super) struct TransactionFetcher<N: NetworkPrimitives = EthNetworkPrimitives> {
    /// Stage 1: hashes ready to fetch, indexed by hash -> set of peers that can serve it.
    announced: B256Map<CandidatePeers>,

    /// Stage 2: hash -> request currently fetching it.
    fetching: B256Map<u64>,
    /// Stage 2: hash -> alternate peers if current fetch fails.
    alternates: B256Map<CandidatePeers>,
    /// Stage 2: request ID -> in-flight request details.
    inflight_requests: HashMap<u64, InflightRequest>,
    /// Response futures, polled only when their oneshot receiver wakes.
    inflight_responses: FuturesUnordered<Abortable<InflightResponse<N::PooledTransaction>>>,
    /// Waiters that wake the manager after a full peer channel regains capacity.
    peer_capacity_waiters: FuturesUnordered<Abortable<PeerCapacityWaiter>>,
    /// Peers with a registered channel-capacity waiter.
    peers_waiting_for_capacity: HashMap<PeerId, CapacityWaiterState>,
    /// Active requests grouped by peer for enforcing the configured per-peer limit.
    inflight_by_peer: HashMap<PeerId, PeerRequestIds>,

    /// Per-peer FIFO queues and metadata retained across both stages.
    announces: HashMap<PeerId, PeerAnnouncements>,
    /// Peers that may currently have hashes ready to fetch.
    ready_peers: HashSet<PeerId>,
    /// Monotonic request identifier.
    request_seq: u64,
    /// Monotonic peer-capacity waiter identifier.
    capacity_waiter_seq: u64,

    /// Stored waker so `schedule_fetches` (called outside `poll_next`) can wake the task
    /// to register oneshot wakers for newly created inflight requests.
    waker: Option<Waker>,

    /// Config.
    info: TransactionFetcherInfo,
    /// Metrics shared with the legacy fetcher so existing dashboards remain valid.
    metrics: TransactionFetcherMetrics,
    /// Metrics specific to the two-stage implementation.
    additional_metrics: TransactionFetcher2Metrics,
}

/// Bound all tracked sources for a hash to the peer currently serving the request plus the
/// established fallback-peer limit.
const MAX_CANDIDATE_PEERS_PER_HASH: usize = 1 + DEFAULT_MAX_COUNT_FALLBACK_PEERS as usize;

type CandidatePeers = SmallVec<[PeerId; MAX_CANDIDATE_PEERS_PER_HASH]>;
type PeerRequestIds = SmallVec<[u64; DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS_PER_PEER as usize]>;
type PeerCapacityWaiter = Pin<Box<dyn Future<Output = (PeerId, u64, bool)> + Send>>;

/// Additional metrics owned by the two-stage fetcher implementation.
#[derive(Metrics)]
#[metrics(scope = "network")]
struct TransactionFetcher2Metrics {
    /// Announcements dropped at the global hash-tracking bound.
    dropped_announces_capacity_limit: Counter,
    /// Latency until a transaction request completes.
    fetch_response_latency_seconds: Histogram,
    /// Broadcasts that satisfied hashes waiting in the queue.
    late_broadcast_announced: Counter,
    /// Broadcasts that satisfied hashes assigned to a request.
    late_broadcast_inflight: Counter,
}

/// Result of updating a hash's bounded candidate set.
enum CandidateUpdate {
    /// The candidate was added or refreshed.
    Tracked,
    /// The candidate replaced the least-recently announced unprotected peer.
    Replaced(PeerId),
    /// Every retained candidate was protected from replacement.
    Rejected,
}

/// A completed inflight request ready for processing.
#[derive(Debug)]
struct InflightCompletion<T> {
    request_id: u64,
    result: Result<RequestResult<PooledTransactions<T>>, RecvError>,
}

/// Metadata from an announcement.
#[derive(Debug, Clone)]
struct TxAnnounceMetadata {
    size: usize,
    queued: bool,
    generation: u64,
}

/// Announcement order and metadata for one peer.
#[derive(Debug, Default)]
struct PeerAnnouncements {
    order: VecDeque<(TxHash, u64)>,
    metadata: B256Map<TxAnnounceMetadata>,
    next_generation: u64,
    stale_entries: usize,
}

impl PeerAnnouncements {
    /// Returns a generation that distinguishes a new queue entry from stale entries for the same
    /// hash.
    const fn next_generation(&mut self) -> u64 {
        let generation = self.next_generation;
        self.next_generation = self.next_generation.wrapping_add(1);
        generation
    }

    /// Queues an existing metadata entry at the back.
    fn queue_back(&mut self, hash: TxHash) -> bool {
        if self.metadata.get(&hash).is_none_or(|metadata| metadata.queued) {
            return false
        }
        let generation = self.next_generation();
        let metadata = self.metadata.get_mut(&hash).expect("metadata checked above");
        metadata.queued = true;
        metadata.generation = generation;
        self.order.push_back((hash, generation));
        true
    }

    /// Restores an existing metadata entry at the front after a failed send.
    fn queue_front(&mut self, hash: TxHash) -> bool {
        if self.metadata.get(&hash).is_none_or(|metadata| metadata.queued) {
            return false
        }
        let generation = self.next_generation();
        let metadata = self.metadata.get_mut(&hash).expect("metadata checked above");
        metadata.queued = true;
        metadata.generation = generation;
        self.order.push_front((hash, generation));
        true
    }

    /// Removes metadata in O(1), compacting the FIFO only when stale entries outnumber live ones.
    fn remove(&mut self, hash: &TxHash) {
        if self.metadata.remove(hash).is_some_and(|metadata| metadata.queued) {
            self.stale_entries += 1;
        }
        if self.stale_entries > self.metadata.len() {
            let metadata = &self.metadata;
            self.order.retain(|(hash, generation)| {
                metadata
                    .get(hash)
                    .is_some_and(|entry| entry.queued && entry.generation == *generation)
            });
            self.stale_entries = 0;
        }
    }
}

/// An in-flight request to a peer.
#[derive(Debug)]
struct InflightRequest {
    peer_id: PeerId,
    hashes: Vec<TxHash>,
    stolen: B256Set,
    sent_at: Instant,
    abort_handle: AbortHandle,
}

/// An abortable peer-capacity waiter currently associated with a session.
#[derive(Debug)]
struct CapacityWaiterState {
    id: u64,
    abort_handle: AbortHandle,
}

/// Response future associated with an in-flight request ID.
#[derive(Debug)]
struct InflightResponse<T> {
    request_id: u64,
    response: oneshot::Receiver<RequestResult<PooledTransactions<T>>>,
}

impl<T> Future for InflightResponse<T> {
    type Output = InflightCompletion<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        Pin::new(&mut this.response)
            .poll(cx)
            .map(|result| InflightCompletion { request_id: this.request_id, result })
    }
}

/// A completed request with its captured result, ready to be processed.
#[derive(Debug)]
struct CompletedRequest<T> {
    request_id: u64,
    peer_id: PeerId,
    hashes: Vec<TxHash>,
    stolen: B256Set,
    result: Result<RequestResult<PooledTransactions<T>>, RecvError>,
    sent_at: Instant,
}

/// Pushes `val` into `vec` if it's not already present. O(n) scan, suitable for small vecs.
fn push_if_absent<T: PartialEq, const N: usize>(vec: &mut SmallVec<[T; N]>, val: T) {
    if !vec.contains(&val) {
        vec.push(val);
    }
}

/// Tracks the most recent serving peers while preserving the legacy fetcher's bound.
fn track_candidate_peer(
    peers: &mut CandidatePeers,
    peer_id: PeerId,
    protected: Option<PeerId>,
) -> CandidateUpdate {
    if let Some(position) = peers.iter().position(|candidate| candidate == &peer_id) {
        let peer_id = peers.remove(position);
        peers.push(peer_id);
        return CandidateUpdate::Tracked;
    }

    if peers.len() == MAX_CANDIDATE_PEERS_PER_HASH {
        let Some(position) = peers.iter().position(|candidate| Some(*candidate) != protected)
        else {
            return CandidateUpdate::Rejected
        };
        let replaced = peers.remove(position);
        peers.push(peer_id);
        return CandidateUpdate::Replaced(replaced)
    }

    peers.push(peer_id);
    CandidateUpdate::Tracked
}

impl<N: NetworkPrimitives> TransactionFetcher<N> {
    /// Sets up transaction fetcher with config.
    pub(super) fn with_transaction_fetcher_config(config: &TransactionFetcherConfig) -> Self {
        let info = TransactionFetcherInfo::from(config.clone());
        let metrics = TransactionFetcherMetrics::default();
        metrics.capacity_inflight_requests.increment(config.max_inflight_requests as u64);

        Self {
            announced: B256Map::default(),
            fetching: B256Map::default(),
            alternates: B256Map::default(),
            inflight_requests: HashMap::default(),
            inflight_responses: FuturesUnordered::new(),
            peer_capacity_waiters: FuturesUnordered::new(),
            peers_waiting_for_capacity: HashMap::default(),
            inflight_by_peer: HashMap::default(),
            announces: HashMap::default(),
            ready_peers: HashSet::default(),
            request_seq: 0,
            capacity_waiter_seq: 0,
            waker: None,
            info,
            metrics,
            additional_metrics: TransactionFetcher2Metrics::default(),
        }
    }

    /// Returns the next request identifier.
    #[inline]
    const fn next_request_id(&mut self) -> u64 {
        let request_id = self.request_seq;
        self.request_seq = self.request_seq.wrapping_add(1);
        request_id
    }

    /// Returns the next peer-capacity waiter identifier.
    #[inline]
    const fn next_capacity_waiter_id(&mut self) -> u64 {
        let waiter_id = self.capacity_waiter_seq;
        self.capacity_waiter_seq = self.capacity_waiter_seq.wrapping_add(1);
        waiter_id
    }

    /// Removes an active request and its per-peer index entry.
    fn take_inflight_request(&mut self, request_id: u64) -> Option<InflightRequest> {
        let request = self.inflight_requests.remove(&request_id)?;
        let remove_peer_entry =
            if let Some(requests) = self.inflight_by_peer.get_mut(&request.peer_id) {
                requests.retain(|id| *id != request_id);
                requests.is_empty()
            } else {
                false
            };
        if remove_peer_entry {
            self.inflight_by_peer.remove(&request.peer_id);
        }
        Some(request)
    }

    /// Moves a set of peers back into `announced` (Stage 1) for the given hash.
    ///
    /// The `announces` index is not modified because it is kept as a superset across
    /// stages — entries are only removed when a hash leaves the system entirely.
    fn restore_to_announced(&mut self, hash: TxHash, peers: CandidatePeers) {
        for peer_id in &peers {
            self.queue_peer_hash(*peer_id, hash);
        }
        self.announced.insert(hash, peers);
    }

    /// Ensures a tracked hash is present in a peer's FIFO queue.
    fn queue_peer_hash(&mut self, peer_id: PeerId, hash: TxHash) {
        let Some(peer_announces) = self.announces.get_mut(&peer_id) else { return };
        if !peer_announces.metadata.contains_key(&hash) {
            return
        }
        peer_announces.queue_back(hash);
        self.ready_peers.insert(peer_id);
    }

    /// Handles a new announcement from a peer.
    pub(super) fn on_new_announcement(
        &mut self,
        peer_id: PeerId,
        announcement: ValidAnnouncementData,
    ) {
        let max_announced = self.info.max_capacity_cache_txns_pending_fetch as usize;
        for (hash, metadata) in announcement {
            // Already in Stage 2 (fetching)?
            if let Some(request_id) = self.fetching.get(&hash) {
                let fetching_peer = self.inflight_requests.get(request_id).map(|req| req.peer_id);
                let update = track_candidate_peer(
                    self.alternates.entry(hash).or_default(),
                    peer_id,
                    fetching_peer,
                );
                if matches!(update, CandidateUpdate::Rejected) {
                    continue
                }
                if let CandidateUpdate::Replaced(replaced) = update {
                    self.remove_from_announces(&replaced, &hash);
                }
                self.insert_peer_announce(&peer_id, &hash, &metadata);
                continue;
            }

            // Already in Stage 1 (announced)?
            if self.announced.contains_key(&hash) {
                let update =
                    track_candidate_peer(self.announced.entry(hash).or_default(), peer_id, None);
                if matches!(update, CandidateUpdate::Rejected) {
                    continue
                }
                if let CandidateUpdate::Replaced(replaced) = update {
                    self.remove_from_announces(&replaced, &hash);
                }
                self.insert_peer_announce(&peer_id, &hash, &metadata);
                self.ready_peers.insert(peer_id);
                continue;
            }

            // New hash
            if self.announced.len().saturating_add(self.fetching.len()) >= max_announced {
                // Keep already-admitted work stable at capacity. Fresh hashes can be announced
                // again after the queue drains instead of evicting hashes that already have
                // candidate metadata and FIFO position.
                self.additional_metrics.dropped_announces_capacity_limit.increment(1);
                continue;
            }
            let mut sources = SmallVec::new();
            sources.push(peer_id);
            self.announced.insert(hash, sources);
            self.insert_peer_announce(&peer_id, &hash, &metadata);
            self.ready_peers.insert(peer_id);
        }
    }

    /// Inserts a peer announcement into the announces index if not already present.
    /// If the entry already exists with unknown size (0), updates to the known size
    /// without changing its FIFO position.
    #[inline]
    fn insert_peer_announce(
        &mut self,
        peer_id: &PeerId,
        hash: &TxHash,
        metadata: &Option<(u8, usize)>,
    ) {
        let size = metadata.map(|(_, sz)| sz).unwrap_or(0);
        let peer_announces = self.announces.entry(*peer_id).or_default();
        if let Some(entry) = peer_announces.metadata.get_mut(hash) {
            if entry.size == 0 && size > 0 {
                entry.size = size;
            }
        } else {
            let generation = peer_announces.next_generation();
            peer_announces
                .metadata
                .insert(*hash, TxAnnounceMetadata { size, queued: true, generation });
            peer_announces.order.push_back((*hash, generation));
        }
    }

    /// Pops a FIFO request for `peer_id`, rotating hashes that are currently owned by another
    /// request. No fetch-stage state is changed until the network send succeeds.
    fn take_request_hashes_for_peer(
        &mut self,
        peer_id: PeerId,
        max_hashes: usize,
        size_limit: usize,
    ) -> Vec<TxHash> {
        let Some(peer_announces) = self.announces.get_mut(&peer_id) else {
            self.ready_peers.remove(&peer_id);
            return Vec::new();
        };

        let scan_len = peer_announces.order.len();
        let mut request = Vec::with_capacity(
            max_hashes.min(SOFT_LIMIT_COUNT_HASHES_IN_GET_POOLED_TRANSACTIONS_REQUEST),
        );
        let mut expected_size = 0usize;
        let mut exhausted = true;

        for _ in 0..scan_len {
            if request.len() == max_hashes {
                exhausted = false;
                break;
            }

            let Some((hash, generation)) = peer_announces.order.pop_front() else { break };
            let Some(metadata) = peer_announces.metadata.get_mut(&hash) else {
                peer_announces.stale_entries = peer_announces.stale_entries.saturating_sub(1);
                continue
            };
            if !metadata.queued || metadata.generation != generation {
                peer_announces.stale_entries = peer_announces.stale_entries.saturating_sub(1);
                continue;
            }
            if !self.announced.get(&hash).is_some_and(|sources| sources.contains(&peer_id)) {
                peer_announces.order.push_back((hash, generation));
                continue;
            }

            let size = if metadata.size > 0 { metadata.size } else { AVERAGE_BYTE_SIZE_TX_ENCODED };
            if !request.is_empty() && expected_size.saturating_add(size) > size_limit {
                peer_announces.order.push_front((hash, generation));
                exhausted = false;
                break;
            }

            metadata.queued = false;
            expected_size = expected_size.saturating_add(size);
            request.push(hash);
        }

        if exhausted {
            self.ready_peers.remove(&peer_id);
        }
        request
    }

    /// Restores FIFO entries after a peer channel rejects a request.
    fn requeue_request(&mut self, peer_id: PeerId, hashes: &[TxHash]) {
        let Some(peer_announces) = self.announces.get_mut(&peer_id) else { return };
        let mut restored = false;
        for hash in hashes.iter().rev() {
            restored |= peer_announces.queue_front(*hash);
        }
        if restored {
            self.ready_peers.insert(peer_id);
        }
    }

    /// Iterates peers with announced hashes and sends requests to idle ones.
    pub(super) fn schedule_fetches<S: BuildHasher>(
        &mut self,
        peers: &HashMap<PeerId, PeerMetadata<N>, S>,
        fetch_capacity: usize,
    ) -> bool {
        if self.announced.is_empty() || fetch_capacity == 0 {
            return false;
        }

        let mut remaining_slots =
            self.info.max_inflight_requests.saturating_sub(self.inflight_requests.len());
        // The manager passes its remaining pool-import capacity. Hashes already assigned to
        // requests reserve that capacity until their response is processed, so only schedule the
        // unreserved remainder on subsequent calls.
        let mut remaining_hashes = fetch_capacity.saturating_sub(self.reserved_hashes());
        let max_per_peer = self.info.max_inflight_requests_per_peer;
        if remaining_slots == 0 || remaining_hashes == 0 || max_per_peer == 0 {
            return false;
        }

        let size_limit =
            self.info.soft_limit_byte_size_pooled_transactions_response_on_pack_request;
        let mut candidate_peers: Vec<PeerId> = self
            .ready_peers
            .iter()
            .filter(|peer_id| {
                !self.peers_waiting_for_capacity.contains_key(*peer_id) &&
                    self.inflight_by_peer.get(*peer_id).map_or(0, SmallVec::len) < max_per_peer
            })
            .copied()
            .collect();
        candidate_peers.shuffle(&mut rand::rng());

        let mut sent_request = false;
        let mut needs_wake = false;
        while remaining_slots > 0 && remaining_hashes > 0 && !candidate_peers.is_empty() {
            let mut next_round = Vec::new();

            for peer_id in candidate_peers {
                if remaining_slots == 0 || remaining_hashes == 0 {
                    break;
                }
                let peer_inflight = self.inflight_by_peer.get(&peer_id).map_or(0, SmallVec::len);
                if peer_inflight >= max_per_peer {
                    continue;
                }
                let Some(peer) = peers.get(&peer_id) else {
                    self.ready_peers.remove(&peer_id);
                    continue;
                };
                let request_hashes = self.take_request_hashes_for_peer(
                    peer_id,
                    remaining_hashes
                        .min(SOFT_LIMIT_COUNT_HASHES_IN_GET_POOLED_TRANSACTIONS_REQUEST),
                    size_limit,
                );

                if request_hashes.is_empty() {
                    continue;
                }

                // Send before mutating any stage maps so a full peer channel leaves the queue
                // unchanged.
                let (response_tx, response_rx) = oneshot::channel();
                let request = PeerRequest::GetPooledTransactions {
                    request: GetPooledTransactions(request_hashes.clone()),
                    response: response_tx,
                };
                match peer.request_tx.try_send(request) {
                    Ok(()) => {}
                    Err(TrySendError::Full(_)) => {
                        self.metrics.egress_peer_channel_full.increment(1);
                        self.requeue_request(peer_id, &request_hashes);
                        if !self.peers_waiting_for_capacity.contains_key(&peer_id) {
                            let waiter_id = self.next_capacity_waiter_id();
                            let (abort_handle, abort_registration) = AbortHandle::new_pair();
                            let sender = peer.request_tx.to_session_tx.clone();
                            self.peer_capacity_waiters.push(Abortable::new(
                                Box::pin(async move {
                                    let channel_open = sender.reserve_owned().await.is_ok();
                                    (peer_id, waiter_id, channel_open)
                                }),
                                abort_registration,
                            ));
                            self.peers_waiting_for_capacity.insert(
                                peer_id,
                                CapacityWaiterState { id: waiter_id, abort_handle },
                            );
                            needs_wake = true;
                        }
                        continue;
                    }
                    Err(TrySendError::Closed(_)) => {
                        self.on_peer_disconnected(&peer_id);
                        needs_wake = true;
                        continue;
                    }
                }

                let requested = request_hashes.len();
                let request_id = self.next_request_id();
                let (abort_handle, abort_registration) = AbortHandle::new_pair();
                for hash in &request_hashes {
                    if let Some(peer_set) = self.announced.remove(hash) {
                        let alternates = self.alternates.entry(*hash).or_default();
                        for peer_id in peer_set {
                            push_if_absent(alternates, peer_id);
                        }
                    }
                    self.fetching.insert(*hash, request_id);
                }

                self.inflight_requests.insert(
                    request_id,
                    InflightRequest {
                        peer_id,
                        hashes: request_hashes,
                        stolen: B256Set::default(),
                        sent_at: Instant::now(),
                        abort_handle,
                    },
                );
                self.inflight_responses.push(Abortable::new(
                    InflightResponse { request_id, response: response_rx },
                    abort_registration,
                ));
                self.inflight_by_peer.entry(peer_id).or_default().push(request_id);
                remaining_slots -= 1;
                remaining_hashes -= requested;
                sent_request = true;

                if peer_inflight + 1 < max_per_peer {
                    next_round.push(peer_id);
                }
            }

            candidate_peers = next_round;
            candidate_peers.shuffle(&mut rand::rng());
        }

        if (sent_request || needs_wake) &&
            let Some(waker) = &self.waker
        {
            waker.wake_by_ref();
        }
        sent_request
    }

    /// Called when transactions are received (via broadcast or fetch response).
    /// Removes hashes from whichever stage they are in.
    ///
    /// `is_broadcast` indicates whether these transactions arrived via an unsolicited
    /// `Transactions` broadcast (true) or as a `GetPooledTransactions` response (false).
    pub(super) fn on_transactions_received(
        &mut self,
        hashes: impl Iterator<Item = TxHash>,
        is_broadcast: bool,
    ) {
        for hash in hashes {
            // Stage 1 (announced)
            if let Some(peer_set) = self.announced.remove(&hash) {
                if is_broadcast {
                    self.additional_metrics.late_broadcast_announced.increment(1);
                }
                for peer in &peer_set {
                    self.remove_from_announces(peer, &hash);
                }
                continue;
            }

            // Stage 2 (fetching)
            if let Some(request_id) = self.fetching.remove(&hash) {
                if is_broadcast {
                    self.additional_metrics.late_broadcast_inflight.increment(1);
                }
                let fetching_peer = self.inflight_requests.get(&request_id).map(|req| req.peer_id);
                if let Some(req) = self.inflight_requests.get_mut(&request_id) {
                    req.stolen.insert(hash);
                }
                let mut peers_to_clean = self.alternates.remove(&hash).unwrap_or_default();
                if let Some(fetching_peer) = fetching_peer {
                    push_if_absent(&mut peers_to_clean, fetching_peer);
                }
                for peer in &peers_to_clean {
                    self.remove_from_announces(peer, &hash);
                }
            }
        }
    }

    /// Called when a peer disconnects. Full cleanup across all stages.
    pub(super) fn on_peer_disconnected(&mut self, peer_id: &PeerId) {
        // Clean Stage 2: redistribute hashes from every request assigned to this peer.
        let request_ids = self.inflight_by_peer.get(peer_id).cloned().unwrap_or_default();
        for request_id in request_ids {
            let Some(req) = self.take_inflight_request(request_id) else { continue };
            req.abort_handle.abort();
            for hash in &req.hashes {
                if req.stolen.contains(hash) {
                    continue;
                }
                if self.fetching.get(hash) == Some(&request_id) {
                    self.fetching.remove(hash);
                }
                if let Some(mut alts) = self.alternates.remove(hash) {
                    alts.retain(|p| p != peer_id);
                    if !alts.is_empty() {
                        self.restore_to_announced(*hash, alts);
                    }
                }
            }
        }

        // Clean announces index
        if let Some(peer_hashes) = self.announces.remove(peer_id) {
            for hash in peer_hashes.metadata.keys() {
                if let Some(inner) = self.announced.get_mut(hash) {
                    inner.retain(|p| p != peer_id);
                    if inner.is_empty() {
                        self.announced.remove(hash);
                    }
                }
                if let Some(inner) = self.alternates.get_mut(hash) {
                    inner.retain(|p| p != peer_id);
                    if inner.is_empty() {
                        self.alternates.remove(hash);
                    }
                }
            }
        }
        self.ready_peers.remove(peer_id);
        if let Some(waiter) = self.peers_waiting_for_capacity.remove(peer_id) {
            waiter.abort_handle.abort();
        }
    }

    /// Moves non-stolen hashes from Stage 2 (fetching) back to Stage 1 (announced) using
    /// alternates.
    ///
    /// The `announces` index already contains entries for alternate peers (it is kept
    /// as a superset), so only `announced` needs to be re-populated. The failed peer's
    /// announces entries are cleaned since it should not be re-selected.
    fn return_hashes_to_announced(
        &mut self,
        request_id: u64,
        hashes: &[TxHash],
        stolen: &B256Set,
        failed_peer: &PeerId,
    ) {
        for hash in hashes {
            if self.fetching.get(hash) != Some(&request_id) {
                continue
            }
            self.fetching.remove(hash);
            if stolen.contains(hash) {
                continue;
            }
            if let Some(mut alts) = self.alternates.remove(hash) {
                alts.retain(|p| p != failed_peer);
                if !alts.is_empty() {
                    self.restore_to_announced(*hash, alts);
                }
            }
            self.remove_from_announces(failed_peer, hash);
        }
    }

    /// Processes a single completed request and returns the corresponding event.
    fn process_completed_request(
        &mut self,
        completed: CompletedRequest<N::PooledTransaction>,
    ) -> FetchEvent<N::PooledTransaction> {
        let CompletedRequest { request_id, peer_id, hashes, stolen, result, sent_at } = completed;

        self.additional_metrics
            .fetch_response_latency_seconds
            .record(sent_at.elapsed().as_secs_f64());

        match result {
            Ok(Ok(transactions)) => {
                if transactions.is_empty() {
                    self.return_hashes_to_announced(request_id, &hashes, &stolen, &peer_id);
                    return FetchEvent::EmptyResponse { peer_id };
                }

                let request_hashes: B256Set = hashes.iter().copied().collect();
                let payload = UnverifiedPooledTransactions::new(transactions);
                let payload_len = payload.len();
                let (verification_outcome, verified_payload) =
                    payload.verify(&RequestTxHashes::new(request_hashes));

                let report_peer = verification_outcome == VerificationOutcome::ReportPeer;

                let unsolicited = payload_len.saturating_sub(verified_payload.len());
                if unsolicited > 0 {
                    self.metrics.unsolicited_transactions.increment(unsolicited as u64);
                }

                if verified_payload.is_empty() {
                    self.return_hashes_to_announced(request_id, &hashes, &stolen, &peer_id);
                    return FetchEvent::FetchError { peer_id, error: RequestError::BadResponse };
                }

                let valid_payload = verified_payload.dedup();
                self.metrics.fetched_transactions.increment(valid_payload.len() as u64);

                for hash in valid_payload.keys() {
                    if self.fetching.get(hash) != Some(&request_id) {
                        continue
                    }
                    let mut peers_to_clean = self.alternates.remove(hash).unwrap_or_default();
                    push_if_absent(&mut peers_to_clean, peer_id);
                    for peer in &peers_to_clean {
                        self.remove_from_announces(peer, hash);
                    }
                    self.fetching.remove(hash);
                }

                // find the position of the last delivered hash in request order. Hashes before the
                // cutoff were intentionally skipped by the peer — remove it from
                // their alternates. Hashes at/after the cutoff are likely benign
                // truncation (size limit) — keep the peer as a candidate.
                let cutoff = hashes
                    .iter()
                    .enumerate()
                    .filter(|(_, hash)| valid_payload.contains_key(*hash))
                    .map(|(i, _)| i + 1)
                    .next_back()
                    .unwrap_or(hashes.len());

                // Move undelivered, non-stolen hashes back to Stage 1
                for (i, hash) in hashes.iter().enumerate() {
                    if valid_payload.contains_key(hash) || stolen.contains(hash) {
                        continue;
                    }
                    if self.fetching.get(hash) != Some(&request_id) {
                        continue
                    }
                    self.fetching.remove(hash);
                    let mut candidates = self.alternates.remove(hash).unwrap_or_default();
                    if i < cutoff {
                        // Peer intentionally skipped this hash
                        candidates.retain(|p| p != &peer_id);
                        self.remove_from_announces(&peer_id, hash);
                    }
                    if candidates.is_empty() {
                        // No candidates remain — hash leaves the system
                        self.remove_from_announces(&peer_id, hash);
                    } else {
                        self.restore_to_announced(*hash, candidates);
                    }
                }

                // Transactions already satisfied by a broadcast must not consume manager import
                // capacity again when the original peer's response arrives later.
                let transactions_out = PooledTransactions(
                    valid_payload
                        .into_data()
                        .into_iter()
                        .filter_map(|(hash, transaction)| {
                            (!stolen.contains(&hash)).then_some(transaction)
                        })
                        .collect(),
                );

                FetchEvent::TransactionsFetched {
                    peer_id,
                    transactions: transactions_out,
                    report_peer,
                }
            }
            Ok(Err(req_err)) => {
                self.return_hashes_to_announced(request_id, &hashes, &stolen, &peer_id);
                FetchEvent::FetchError { peer_id, error: req_err }
            }
            Err(_) => {
                self.return_hashes_to_announced(request_id, &hashes, &stolen, &peer_id);
                FetchEvent::FetchError { peer_id, error: RequestError::ChannelClosed }
            }
        }
    }

    /// Updates metrics.
    #[inline]
    pub(super) fn update_metrics(&self) {
        let metrics = &self.metrics;
        metrics.inflight_transaction_requests.set(self.inflight_requests.len() as f64);
        metrics.hashes_pending_fetch.set(self.announced.len() as f64);
        metrics.hashes_inflight_transaction_requests.set(self.fetching.len() as f64);
    }

    /// Returns the number of hashes that currently reserve pool-import capacity.
    #[inline]
    pub(super) fn reserved_hashes(&self) -> usize {
        self.fetching.len()
    }

    /// Returns whether `hash` currently reserves pool-import capacity.
    #[inline]
    pub(super) fn is_hash_reserved(&self, hash: &TxHash) -> bool {
        self.fetching.contains_key(hash)
    }

    /// Returns true if the peer has no inflight request.
    #[cfg(test)]
    pub(super) fn is_idle(&self, peer_id: &PeerId) -> bool {
        self.inflight_by_peer.get(peer_id).map_or(0, SmallVec::len) <
            self.info.max_inflight_requests_per_peer
    }

    /// Returns the number of hashes in Stage 1 (announced).
    #[cfg(test)]
    pub(super) fn num_announced(&self) -> usize {
        self.announced.len()
    }

    /// Returns the number of inflight requests.
    #[cfg(test)]
    pub(super) fn num_inflight(&self) -> usize {
        self.inflight_requests.len()
    }

    /// Returns the number of hashes in Stage 2 (fetching).
    #[cfg(test)]
    pub(super) fn num_fetching(&self) -> usize {
        self.reserved_hashes()
    }

    /// Removes `hash` from `announces[peer]`. Removes the outer entry if empty.
    fn remove_from_announces(&mut self, peer: &PeerId, hash: &TxHash) {
        if let Some(inner) = self.announces.get_mut(peer) {
            inner.remove(hash);
            if inner.metadata.is_empty() {
                self.announces.remove(peer);
                self.ready_peers.remove(peer);
            }
        }
    }
}

impl<N: NetworkPrimitives> Default for TransactionFetcher<N> {
    fn default() -> Self {
        Self {
            announced: B256Map::default(),
            fetching: B256Map::default(),
            alternates: B256Map::default(),
            inflight_requests: HashMap::default(),
            inflight_responses: FuturesUnordered::new(),
            peer_capacity_waiters: FuturesUnordered::new(),
            peers_waiting_for_capacity: HashMap::default(),
            inflight_by_peer: HashMap::default(),
            announces: HashMap::default(),
            ready_peers: HashSet::default(),
            request_seq: 0,
            capacity_waiter_seq: 0,
            waker: None,
            info: TransactionFetcherInfo::default(),
            metrics: TransactionFetcherMetrics::default(),
            additional_metrics: TransactionFetcher2Metrics::default(),
        }
    }
}

impl<N: NetworkPrimitives> futures::Stream for TransactionFetcher<N> {
    type Item = FetchEvent<N::PooledTransaction>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        // Store waker so schedule_fetches can wake the task after dispatching requests
        if !this.waker.as_ref().is_some_and(|w| w.will_wake(cx.waker())) {
            this.waker = Some(cx.waker().clone());
        }

        while let Poll::Ready(Some(result)) = this.peer_capacity_waiters.poll_next_unpin(cx) {
            let Ok((peer_id, waiter_id, channel_open)) = result else { continue };
            if !this
                .peers_waiting_for_capacity
                .get(&peer_id)
                .is_some_and(|waiter| waiter.id == waiter_id)
            {
                continue
            }
            this.peers_waiting_for_capacity.remove(&peer_id);
            if !channel_open {
                this.ready_peers.remove(&peer_id);
            }
        }

        // FuturesUnordered only polls response receivers that were woken. Process one live
        // completion per stream poll so all other requests retain ownership between manager
        // events.
        loop {
            match this.inflight_responses.poll_next_unpin(cx) {
                Poll::Ready(Some(Ok(InflightCompletion { request_id, result }))) => {
                    let Some(req) = this.take_inflight_request(request_id) else {
                        // The request was removed when its peer disconnected.
                        continue
                    };
                    let event = this.process_completed_request(CompletedRequest {
                        request_id,
                        peer_id: req.peer_id,
                        hashes: req.hashes,
                        stolen: req.stolen,
                        result,
                        sent_at: req.sent_at,
                    });
                    return Poll::Ready(Some(event))
                }
                Poll::Ready(Some(Err(_))) => {
                    // Aborted when an in-flight peer disconnected.
                }
                Poll::Ready(None) | Poll::Pending => return Poll::Pending,
            }
        }
    }
}

/// Represents possible events from fetching transactions.
#[derive(Debug)]
pub(super) enum FetchEvent<T = PooledTransaction> {
    /// Triggered when transactions are successfully fetched.
    TransactionsFetched {
        /// The ID of the peer from which transactions were fetched.
        peer_id: PeerId,
        /// The transactions that were fetched, if available.
        transactions: PooledTransactions<T>,
        /// Whether the peer should be penalized for sending unsolicited transactions or for
        /// misbehavior.
        report_peer: bool,
    },
    /// Triggered when there is an error in fetching transactions.
    FetchError {
        /// The ID of the peer from which an attempt to fetch transactions resulted in an error.
        peer_id: PeerId,
        /// The specific error that occurred while fetching.
        error: RequestError,
    },
    /// An empty response was received.
    EmptyResponse {
        /// The ID of the sender.
        peer_id: PeerId,
    },
}

/// Tracks stats about the [`TransactionFetcher`].
#[derive(Debug, Constructor)]
struct TransactionFetcherInfo {
    /// Max inflight [`GetPooledTransactions`] requests.
    max_inflight_requests: usize,
    /// Max inflight [`GetPooledTransactions`] requests per peer.
    max_inflight_requests_per_peer: usize,
    /// Soft limit for the byte size of the expected [`PooledTransactions`] response, upon packing
    /// a [`GetPooledTransactions`] request with hashes.
    soft_limit_byte_size_pooled_transactions_response_on_pack_request: usize,
    /// Max capacity of the hash tracking caches.
    max_capacity_cache_txns_pending_fetch: u32,
}

impl Default for TransactionFetcherInfo {
    fn default() -> Self {
        Self::new(
            DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS as usize,
            DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS_PER_PEER as usize,
            DEFAULT_SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESP_ON_PACK_GET_POOLED_TRANSACTIONS_REQ,
            DEFAULT_MAX_CAPACITY_CACHE_PENDING_FETCH,
        )
    }
}

impl From<TransactionFetcherConfig> for TransactionFetcherInfo {
    fn from(config: TransactionFetcherConfig) -> Self {
        let TransactionFetcherConfig {
            max_inflight_requests,
            max_inflight_requests_per_peer,
            soft_limit_byte_size_pooled_transactions_response: _,
            soft_limit_byte_size_pooled_transactions_response_on_pack_request,
            max_capacity_cache_txns_pending_fetch,
        } = config;

        Self::new(
            max_inflight_requests as usize,
            max_inflight_requests_per_peer as usize,
            soft_limit_byte_size_pooled_transactions_response_on_pack_request,
            max_capacity_cache_txns_pending_fetch,
        )
    }
}

/// Wrapper of unverified [`PooledTransactions`].
#[derive(Debug, Constructor, Deref)]
struct UnverifiedPooledTransactions<T> {
    txns: PooledTransactions<T>,
}

/// [`PooledTransactions`] that have been successfully verified.
#[derive(Debug, Constructor, Deref)]
struct VerifiedPooledTransactions<T> {
    txns: PooledTransactions<T>,
}

impl<T: SignedTransaction> DedupPayload for VerifiedPooledTransactions<T> {
    type Value = T;

    fn is_empty(&self) -> bool {
        self.txns.is_empty()
    }

    fn len(&self) -> usize {
        self.txns.len()
    }

    fn dedup(self) -> PartiallyValidData<Self::Value> {
        PartiallyValidData::from_raw_data(
            self.txns.into_iter().map(|tx| (*tx.tx_hash(), tx)).collect(),
            None,
        )
    }
}

trait VerifyPooledTransactionsResponse {
    type Transaction: SignedTransaction;

    fn verify(
        self,
        requested_hashes: &RequestTxHashes,
    ) -> (VerificationOutcome, VerifiedPooledTransactions<Self::Transaction>);
}

impl<T: SignedTransaction> VerifyPooledTransactionsResponse for UnverifiedPooledTransactions<T> {
    type Transaction = T;

    fn verify(
        self,
        requested_hashes: &RequestTxHashes,
    ) -> (VerificationOutcome, VerifiedPooledTransactions<T>) {
        let Self { mut txns } = self;
        let mut outcome = VerificationOutcome::Ok;
        txns.0.retain(|tx| {
            if requested_hashes.contains(tx.tx_hash()) {
                true
            } else {
                outcome = VerificationOutcome::ReportPeer;
                false
            }
        });
        (outcome, VerifiedPooledTransactions::new(txns))
    }
}

/// Outcome from verifying a pooled-transactions response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VerificationOutcome {
    /// The response contained only requested transactions.
    Ok,
    /// The response included transactions that were not requested.
    ReportPeer,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::transactions::new_mock_session;
    use alloy_primitives::{hex, B256};
    use alloy_rlp::Decodable;
    use futures::StreamExt;
    use reth_eth_wire::EthVersion;
    use reth_ethereum_primitives::{PooledTransactionVariant, TransactionSigned};
    use std::{future::poll_fn, iter::once};

    type TestFetcher = TransactionFetcher<EthNetworkPrimitives>;
    type TestPeers = HashMap<PeerId, PeerMetadata<EthNetworkPrimitives>>;

    fn peer(n: u8) -> PeerId {
        PeerId::new([n; 64])
    }

    fn hash(n: u8) -> TxHash {
        B256::from([n; 32])
    }

    fn eth68_announcement(
        entries: impl IntoIterator<Item = (TxHash, usize)>,
    ) -> ValidAnnouncementData {
        let data: B256Map<Option<(u8, usize)>> =
            entries.into_iter().map(|(hash, size)| (hash, Some((2, size)))).collect();
        ValidAnnouncementData::from_partially_valid_data(PartiallyValidData::from_raw_data_eth68(
            data,
        ))
    }

    fn eth66_announcement(hashes: impl IntoIterator<Item = TxHash>) -> ValidAnnouncementData {
        let data: B256Map<Option<(u8, usize)>> =
            hashes.into_iter().map(|hash| (hash, None)).collect();
        ValidAnnouncementData::from_partially_valid_data(PartiallyValidData::from_raw_data_eth66(
            data,
        ))
    }

    fn test_transactions() -> (PooledTransactionVariant, PooledTransactionVariant) {
        let first = hex!(
            "02f871018302a90f808504890aef60826b6c94ddf4c5025d1a5742cf12f74eec246d4432c295e487e09c3bbcc12b2b80c080a0f21a4eacd0bf8fea9c5105c543be5a1d8c796516875710fafafdf16d16d8ee23a001280915021bb446d1973501a67f93d2b38894a514b976e7b46dc2fe54598daa"
        );
        let second = hex!(
            "02f871018302a90f808504890aef60826b6c94ddf4c5025d1a5742cf12f74eec246d4432c295e487e09c3bbcc12b2b80c080a0f21a4eacd0bf8fea9c5105c543be5a1d8c796516875710fafafdf16d16d8ee23a001280915021bb446d1973501a67f93d2b38894a514b976e7b46dc2fe54598d76"
        );
        let first = TransactionSigned::decode(&mut &first[..]).unwrap().try_into().unwrap();
        let second = TransactionSigned::decode(&mut &second[..]).unwrap().try_into().unwrap();
        (first, second)
    }

    async fn poll_fetcher_pending(fetcher: &mut TestFetcher) {
        poll_fn(|cx| {
            assert!(fetcher.poll_next_unpin(cx).is_pending());
            Poll::Ready(())
        })
        .await;
    }

    #[test]
    fn announcement_ingestion_is_manager_driven() {
        let mut fetcher = TestFetcher::default();
        let peer = peer(1);
        let hash = hash(1);

        fetcher.on_new_announcement(peer, eth68_announcement([(hash, 100)]));

        assert_eq!(fetcher.num_announced(), 1);
        assert_eq!(fetcher.num_inflight(), 0);
        assert!(fetcher.ready_peers.contains(&peer));
    }

    #[test]
    fn candidate_peers_are_bounded() {
        let mut fetcher = TestFetcher::default();
        let hash = hash(1);

        for id in 1..=8 {
            fetcher.on_new_announcement(peer(id), eth68_announcement([(hash, 100)]));
        }

        let sources = &fetcher.announced[&hash];
        assert_eq!(sources.as_slice(), &[peer(5), peer(6), peer(7), peer(8)]);
        assert_eq!(fetcher.announces.len(), MAX_CANDIDATE_PEERS_PER_HASH);
        assert!((1..=4).all(|id| !fetcher.announces.contains_key(&peer(id))));
    }

    #[tokio::test]
    async fn candidate_replacement_keeps_the_fetching_peer() {
        let mut fetcher = TestFetcher::default();
        let origin = peer(1);
        let hash = hash(1);
        fetcher.on_new_announcement(origin, eth68_announcement([(hash, 100)]));
        fetcher.on_new_announcement(peer(2), eth68_announcement([(hash, 100)]));
        fetcher.on_new_announcement(peer(3), eth68_announcement([(hash, 100)]));
        fetcher.on_new_announcement(peer(4), eth68_announcement([(hash, 100)]));

        let (origin_session, mut requests) = new_mock_session(origin, EthVersion::Eth68);
        let mut peers = TestPeers::default();
        peers.insert(origin, origin_session);
        assert!(fetcher.schedule_fetches(&peers, usize::MAX));
        let request = requests.recv().await.unwrap();

        fetcher.on_new_announcement(peer(5), eth68_announcement([(hash, 100)]));

        assert_eq!(fetcher.alternates[&hash].as_slice(), &[origin, peer(3), peer(4), peer(5)]);
        assert!(!fetcher.announces.contains_key(&peer(2)));
        drop(request);
    }

    #[test]
    fn consecutive_announcements_share_the_global_capacity() {
        let mut fetcher = TestFetcher::default();
        fetcher.info.max_capacity_cache_txns_pending_fetch = 3;
        let peer = peer(1);

        fetcher.on_new_announcement(peer, eth68_announcement([(hash(1), 100), (hash(2), 100)]));
        fetcher.on_new_announcement(peer, eth68_announcement([(hash(3), 100)]));
        fetcher.on_new_announcement(peer, eth68_announcement([(hash(4), 100)]));

        assert_eq!(fetcher.announces[&peer].metadata.len(), 3);
        assert_eq!(fetcher.num_announced(), 3);
        assert!(!fetcher.announced.contains_key(&hash(4)));
    }

    #[tokio::test]
    async fn fifo_packing_checks_prospective_size() {
        let mut fetcher = TestFetcher::default();
        fetcher.info.soft_limit_byte_size_pooled_transactions_response_on_pack_request = 100;
        let peer_id = peer(1);
        let first = hash(1);
        let second = hash(2);
        let oversized = hash(3);

        fetcher.on_new_announcement(peer_id, eth68_announcement([(first, 80)]));
        fetcher.on_new_announcement(peer_id, eth68_announcement([(second, 30)]));
        fetcher.on_new_announcement(peer_id, eth68_announcement([(oversized, 200)]));

        let (peer, mut requests) = new_mock_session(peer_id, EthVersion::Eth68);
        let mut peers = TestPeers::default();
        peers.insert(peer_id, peer);

        assert!(fetcher.schedule_fetches(&peers, usize::MAX));
        let PeerRequest::GetPooledTransactions { request, .. } = requests.recv().await.unwrap()
        else {
            panic!("unexpected request")
        };
        assert_eq!(request.0, vec![first]);
        assert!(fetcher.announced.contains_key(&second));
        assert!(fetcher.announced.contains_key(&oversized));
    }

    #[tokio::test]
    async fn first_oversized_hash_is_still_requested() {
        let mut fetcher = TestFetcher::default();
        fetcher.info.soft_limit_byte_size_pooled_transactions_response_on_pack_request = 100;
        let peer_id = peer(1);
        let oversized = hash(1);
        fetcher.on_new_announcement(peer_id, eth68_announcement([(oversized, 200)]));

        let (peer, mut requests) = new_mock_session(peer_id, EthVersion::Eth68);
        let mut peers = TestPeers::default();
        peers.insert(peer_id, peer);

        assert!(fetcher.schedule_fetches(&peers, usize::MAX));
        let PeerRequest::GetPooledTransactions { request, .. } = requests.recv().await.unwrap()
        else {
            panic!("unexpected request")
        };
        assert_eq!(request.0, vec![oversized]);
    }

    #[tokio::test]
    async fn fetch_capacity_bounds_requested_hashes() {
        let mut fetcher = TestFetcher::default();
        let peer_id = peer(1);
        for id in 1..=5 {
            fetcher.on_new_announcement(peer_id, eth68_announcement([(hash(id), 10)]));
        }

        let (peer, mut requests) = new_mock_session(peer_id, EthVersion::Eth68);
        let mut peers = TestPeers::default();
        peers.insert(peer_id, peer);

        assert!(fetcher.schedule_fetches(&peers, 2));
        let PeerRequest::GetPooledTransactions { request, .. } = requests.recv().await.unwrap()
        else {
            panic!("unexpected request")
        };
        assert_eq!(request.0.len(), 2);
        assert_eq!(fetcher.num_fetching(), 2);
        assert_eq!(fetcher.num_announced(), 3);
    }

    #[tokio::test]
    async fn repeated_scheduling_reserves_capacity_for_inflight_hashes() {
        let mut fetcher = TestFetcher::default();
        fetcher.info.max_inflight_requests = 3;
        fetcher.info.max_inflight_requests_per_peer = 3;
        fetcher.info.soft_limit_byte_size_pooled_transactions_response_on_pack_request = 100;
        let peer_id = peer(1);
        for id in 1..=3 {
            fetcher.on_new_announcement(peer_id, eth68_announcement([(hash(id), 80)]));
        }

        let (peer, mut requests) = new_mock_session(peer_id, EthVersion::Eth68);
        let mut peers = TestPeers::default();
        peers.insert(peer_id, peer);

        // The mock session channel holds one request, so drain it before scheduling the next.
        assert!(fetcher.schedule_fetches(&peers, 2));
        let first_request = requests.recv().await.unwrap();
        poll_fetcher_pending(&mut fetcher).await;
        assert!(fetcher.schedule_fetches(&peers, 2));
        let second_request = requests.recv().await.unwrap();
        assert!(!fetcher.schedule_fetches(&peers, 2));

        assert_eq!(fetcher.num_fetching(), 2);
        assert_eq!(fetcher.num_announced(), 1);
        drop((first_request, second_request));
    }

    #[tokio::test]
    async fn fetching_hashes_keep_their_tracking_capacity() {
        let mut fetcher = TestFetcher::default();
        fetcher.info.max_capacity_cache_txns_pending_fetch = 1;
        let origin = peer(1);
        let alternate = peer(2);
        let first = hash(1);
        let second = hash(2);
        fetcher.on_new_announcement(origin, eth68_announcement([(first, 100)]));
        fetcher.on_new_announcement(alternate, eth68_announcement([(first, 100)]));

        let (peer, mut requests) = new_mock_session(origin, EthVersion::Eth68);
        let mut peers = TestPeers::default();
        peers.insert(origin, peer);
        assert!(fetcher.schedule_fetches(&peers, usize::MAX));
        let PeerRequest::GetPooledTransactions { response, .. } = requests.recv().await.unwrap()
        else {
            panic!("unexpected request")
        };

        fetcher.on_new_announcement(origin, eth68_announcement([(second, 100)]));
        assert!(!fetcher.announced.contains_key(&second));

        response.send(Err(RequestError::Timeout)).unwrap();
        assert!(matches!(fetcher.next().await, Some(FetchEvent::FetchError { .. })));
        assert_eq!(fetcher.announced.len(), 1);
        assert!(fetcher.announced.contains_key(&first));
    }

    #[tokio::test]
    async fn full_peer_channel_requeues_and_wakes() {
        let mut fetcher = TestFetcher::default();
        let peer_id = peer(1);
        let first = hash(1);
        let second = hash(2);
        fetcher.on_new_announcement(peer_id, eth68_announcement([(first, 10)]));
        fetcher.on_new_announcement(peer_id, eth68_announcement([(second, 10)]));

        let (peer, mut requests) = new_mock_session(peer_id, EthVersion::Eth68);
        let (dummy_response, _dummy_receiver) = oneshot::channel();
        peer.request_tx
            .try_send(PeerRequest::GetPooledTransactions {
                request: GetPooledTransactions(Vec::new()),
                response: dummy_response,
            })
            .unwrap();
        let mut peers = TestPeers::default();
        peers.insert(peer_id, peer);
        assert!(!fetcher.schedule_fetches(&peers, usize::MAX));
        assert_eq!(fetcher.num_announced(), 2);

        // Register the channel-capacity waiter, then free the occupied slot and observe its wake.
        poll_fn(|cx| {
            assert!(fetcher.poll_next_unpin(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        drop(requests.recv().await.unwrap());
        poll_fn(|cx| {
            assert!(fetcher.poll_next_unpin(cx).is_pending());
            Poll::Ready(())
        })
        .await;

        assert!(fetcher.schedule_fetches(&peers, usize::MAX));
        let PeerRequest::GetPooledTransactions { request, .. } = requests.recv().await.unwrap()
        else {
            panic!("unexpected request")
        };
        assert_eq!(request.0, vec![first, second]);
    }

    #[tokio::test]
    async fn stale_capacity_waiter_cannot_strand_reconnected_peer() {
        let mut fetcher = TestFetcher::default();
        let peer_id = peer(1);
        let tx_hash = hash(1);
        fetcher.on_new_announcement(peer_id, eth68_announcement([(tx_hash, 10)]));

        let (old_session, old_requests) = new_mock_session(peer_id, EthVersion::Eth68);
        let (dummy_response, _dummy_receiver) = oneshot::channel();
        old_session
            .request_tx
            .try_send(PeerRequest::GetPooledTransactions {
                request: GetPooledTransactions(Vec::new()),
                response: dummy_response,
            })
            .unwrap();
        let mut peers = TestPeers::default();
        peers.insert(peer_id, old_session);
        assert!(!fetcher.schedule_fetches(&peers, usize::MAX));
        poll_fetcher_pending(&mut fetcher).await;

        fetcher.on_peer_disconnected(&peer_id);
        let (replacement, mut replacement_requests) = new_mock_session(peer_id, EthVersion::Eth68);
        peers.insert(peer_id, replacement);
        fetcher.on_new_announcement(peer_id, eth68_announcement([(tx_hash, 10)]));

        // Closing the old session completes its aborted waiter. That stale completion must not
        // clear readiness belonging to the replacement session.
        drop(old_requests);
        poll_fetcher_pending(&mut fetcher).await;
        assert!(fetcher.ready_peers.contains(&peer_id));
        assert!(fetcher.schedule_fetches(&peers, usize::MAX));

        let PeerRequest::GetPooledTransactions { request, .. } =
            replacement_requests.recv().await.unwrap()
        else {
            panic!("unexpected request")
        };
        assert_eq!(request.0, vec![tx_hash]);
    }

    #[tokio::test]
    async fn configured_per_peer_concurrency_is_honored() {
        let mut fetcher = TestFetcher::default();
        fetcher.info.max_inflight_requests = 3;
        fetcher.info.max_inflight_requests_per_peer = 2;
        fetcher.info.soft_limit_byte_size_pooled_transactions_response_on_pack_request = 100;
        let peer_id = peer(1);
        for id in 1..=3 {
            fetcher.on_new_announcement(peer_id, eth68_announcement([(hash(id), 80)]));
        }

        let (peer, mut requests) = new_mock_session(peer_id, EthVersion::Eth68);
        let mut peers = TestPeers::default();
        peers.insert(peer_id, peer);

        // The mock session channel holds one request, so drain it before scheduling the next.
        assert!(fetcher.schedule_fetches(&peers, usize::MAX));
        let PeerRequest::GetPooledTransactions { response: first_response, .. } =
            requests.recv().await.unwrap()
        else {
            panic!("unexpected request")
        };
        poll_fetcher_pending(&mut fetcher).await;
        assert!(fetcher.schedule_fetches(&peers, usize::MAX));
        let second_request = requests.recv().await.unwrap();
        assert!(!fetcher.schedule_fetches(&peers, usize::MAX));

        assert_eq!(fetcher.num_inflight(), 2);
        assert_eq!(fetcher.inflight_by_peer[&peer_id].len(), 2);
        assert!(!fetcher.is_idle(&peer_id));

        first_response.send(Ok(PooledTransactions(Vec::new()))).unwrap();
        assert!(matches!(fetcher.next().await, Some(FetchEvent::EmptyResponse { .. })));
        assert!(fetcher.schedule_fetches(&peers, usize::MAX));
        let third_request = requests.recv().await.unwrap();
        assert_eq!(fetcher.inflight_by_peer[&peer_id].len(), 2);

        drop((second_request, third_request));
    }

    #[tokio::test]
    async fn request_error_retries_from_alternate_peer() {
        let mut fetcher = TestFetcher::default();
        let first_peer = peer(1);
        let alternate = peer(2);
        let hash = hash(1);
        fetcher.on_new_announcement(first_peer, eth68_announcement([(hash, 100)]));
        fetcher.on_new_announcement(alternate, eth68_announcement([(hash, 100)]));

        let (peer, mut first_requests) = new_mock_session(first_peer, EthVersion::Eth68);
        let mut peers = TestPeers::default();
        peers.insert(first_peer, peer);
        assert!(fetcher.schedule_fetches(&peers, usize::MAX));

        let PeerRequest::GetPooledTransactions { response, .. } =
            first_requests.recv().await.unwrap()
        else {
            panic!("unexpected request")
        };
        response.send(Err(RequestError::Timeout)).unwrap();

        assert!(matches!(
            fetcher.next().await,
            Some(FetchEvent::FetchError { peer_id, error: RequestError::Timeout })
                if peer_id == first_peer
        ));
        assert_eq!(fetcher.announced[&hash].as_slice(), &[alternate]);

        let (peer, mut alternate_requests) = new_mock_session(alternate, EthVersion::Eth68);
        peers.clear();
        peers.insert(alternate, peer);
        assert!(fetcher.schedule_fetches(&peers, usize::MAX));
        let PeerRequest::GetPooledTransactions { request, .. } =
            alternate_requests.recv().await.unwrap()
        else {
            panic!("unexpected request")
        };
        assert_eq!(request.0, vec![hash]);
    }

    #[tokio::test]
    async fn exhausting_candidates_clears_state_and_allows_reannouncement() {
        let mut fetcher = TestFetcher::default();
        let first_peer = peer(1);
        let second_peer = peer(2);
        let fresh_peer = peer(3);
        let hash = hash(1);
        fetcher.on_new_announcement(first_peer, eth68_announcement([(hash, 100)]));
        fetcher.on_new_announcement(second_peer, eth68_announcement([(hash, 100)]));

        let (first_session, mut first_requests) = new_mock_session(first_peer, EthVersion::Eth68);
        let mut peers = TestPeers::default();
        peers.insert(first_peer, first_session);
        assert!(fetcher.schedule_fetches(&peers, usize::MAX));
        let PeerRequest::GetPooledTransactions { response, .. } =
            first_requests.recv().await.unwrap()
        else {
            panic!("unexpected request")
        };
        response.send(Err(RequestError::Timeout)).unwrap();
        assert!(matches!(fetcher.next().await, Some(FetchEvent::FetchError { .. })));

        let (second_session, mut second_requests) =
            new_mock_session(second_peer, EthVersion::Eth68);
        peers.clear();
        peers.insert(second_peer, second_session);
        assert!(fetcher.schedule_fetches(&peers, usize::MAX));
        let PeerRequest::GetPooledTransactions { response, .. } =
            second_requests.recv().await.unwrap()
        else {
            panic!("unexpected request")
        };
        response.send(Err(RequestError::Timeout)).unwrap();
        assert!(matches!(fetcher.next().await, Some(FetchEvent::FetchError { .. })));

        assert!(!fetcher.announced.contains_key(&hash));
        assert!(!fetcher.fetching.contains_key(&hash));
        assert!(!fetcher.alternates.contains_key(&hash));
        assert!(fetcher.announces.is_empty());

        let (fresh_session, mut fresh_requests) = new_mock_session(fresh_peer, EthVersion::Eth68);
        peers.clear();
        peers.insert(fresh_peer, fresh_session);
        fetcher.on_new_announcement(fresh_peer, eth68_announcement([(hash, 100)]));
        assert!(fetcher.schedule_fetches(&peers, usize::MAX));
        let PeerRequest::GetPooledTransactions { request, .. } =
            fresh_requests.recv().await.unwrap()
        else {
            panic!("unexpected request")
        };
        assert_eq!(request.0, vec![hash]);
    }

    #[tokio::test]
    async fn partial_response_keeps_origin_after_cutoff_only() {
        let mut fetcher = TestFetcher::default();
        let origin = peer(1);
        let alternate = peer(2);
        let (first_tx, second_tx) = test_transactions();
        let first = *first_tx.tx_hash();
        let delivered = *second_tx.tx_hash();
        let trailing = hash(3);

        fetcher.on_new_announcement(origin, eth68_announcement([(first, 100)]));
        fetcher.on_new_announcement(origin, eth68_announcement([(delivered, 100)]));
        fetcher.on_new_announcement(origin, eth68_announcement([(trailing, 100)]));
        fetcher.on_new_announcement(alternate, eth68_announcement([(first, 100)]));

        let (peer, mut requests) = new_mock_session(origin, EthVersion::Eth68);
        let mut peers = TestPeers::default();
        peers.insert(origin, peer);
        assert!(fetcher.schedule_fetches(&peers, usize::MAX));

        let PeerRequest::GetPooledTransactions { request, response } =
            requests.recv().await.unwrap()
        else {
            panic!("unexpected request")
        };
        assert_eq!(request.0, vec![first, delivered, trailing]);
        response.send(Ok(PooledTransactions(vec![second_tx]))).unwrap();

        assert!(matches!(
            fetcher.next().await,
            Some(FetchEvent::TransactionsFetched { peer_id, .. }) if peer_id == origin
        ));
        assert_eq!(fetcher.announced[&first].as_slice(), &[alternate]);
        assert_eq!(fetcher.announced[&trailing].as_slice(), &[origin]);
        assert!(!fetcher.announced.contains_key(&delivered));
    }

    #[tokio::test]
    async fn broadcast_steals_hash_from_inflight_request() {
        let mut fetcher = TestFetcher::default();
        let peer_id = peer(1);
        let hash = hash(1);
        fetcher.on_new_announcement(peer_id, eth68_announcement([(hash, 100)]));

        let (peer, mut requests) = new_mock_session(peer_id, EthVersion::Eth68);
        let mut peers = TestPeers::default();
        peers.insert(peer_id, peer);
        assert!(fetcher.schedule_fetches(&peers, usize::MAX));
        let request_id = fetcher.fetching[&hash];

        fetcher.on_transactions_received(once(hash), true);

        assert!(!fetcher.fetching.contains_key(&hash));
        assert!(fetcher.inflight_requests[&request_id].stolen.contains(&hash));
        assert!(!fetcher.announces.contains_key(&peer_id));
        drop(requests.recv().await);
    }

    #[tokio::test]
    async fn fetched_event_excludes_transactions_already_received_by_broadcast() {
        let mut fetcher = TestFetcher::default();
        let peer_id = peer(1);
        let (broadcasted, fetched) = test_transactions();
        let broadcasted_hash = *broadcasted.tx_hash();
        let fetched_hash = *fetched.tx_hash();
        fetcher.on_new_announcement(
            peer_id,
            eth68_announcement([
                (broadcasted_hash, broadcasted.eip2718_encoded_length()),
                (fetched_hash, fetched.eip2718_encoded_length()),
            ]),
        );

        let (peer, mut requests) = new_mock_session(peer_id, EthVersion::Eth68);
        let mut peers = TestPeers::default();
        peers.insert(peer_id, peer);
        assert!(fetcher.schedule_fetches(&peers, usize::MAX));
        let PeerRequest::GetPooledTransactions { response, .. } = requests.recv().await.unwrap()
        else {
            panic!("unexpected request")
        };

        fetcher.on_transactions_received(once(broadcasted_hash), true);
        response.send(Ok(PooledTransactions(vec![broadcasted, fetched]))).unwrap();

        let Some(FetchEvent::TransactionsFetched { transactions, .. }) = fetcher.next().await
        else {
            panic!("expected fetched transactions")
        };
        assert_eq!(transactions.len(), 1);
        assert_eq!(transactions[0].tx_hash(), &fetched_hash);
    }

    #[tokio::test]
    async fn stale_completion_cannot_remove_new_request_ownership() {
        let mut fetcher = TestFetcher::default();
        let origin = peer(1);
        let replacement = peer(2);
        let hash = hash(1);
        fetcher.on_new_announcement(origin, eth68_announcement([(hash, 100)]));

        let (origin_session, mut origin_requests) = new_mock_session(origin, EthVersion::Eth68);
        let mut peers = TestPeers::default();
        peers.insert(origin, origin_session);
        assert!(fetcher.schedule_fetches(&peers, usize::MAX));
        let PeerRequest::GetPooledTransactions { response: old_response, .. } =
            origin_requests.recv().await.unwrap()
        else {
            panic!("unexpected request")
        };

        fetcher.on_transactions_received(once(hash), true);
        fetcher.on_new_announcement(replacement, eth68_announcement([(hash, 100)]));

        let (replacement_session, mut replacement_requests) =
            new_mock_session(replacement, EthVersion::Eth68);
        peers.clear();
        peers.insert(replacement, replacement_session);
        assert!(fetcher.schedule_fetches(&peers, usize::MAX));
        let replacement_request = replacement_requests.recv().await.unwrap();
        let replacement_request_id = fetcher.fetching[&hash];

        old_response.send(Err(RequestError::Timeout)).unwrap();
        assert!(matches!(fetcher.next().await, Some(FetchEvent::FetchError { .. })));
        assert_eq!(fetcher.fetching[&hash], replacement_request_id);
        assert!(fetcher.inflight_requests.contains_key(&replacement_request_id));
        drop(replacement_request);
    }

    #[tokio::test]
    async fn disconnect_redistributes_all_peer_requests() {
        let mut fetcher = TestFetcher::default();
        fetcher.info.max_inflight_requests = 2;
        fetcher.info.max_inflight_requests_per_peer = 2;
        fetcher.info.soft_limit_byte_size_pooled_transactions_response_on_pack_request = 100;
        let origin = peer(1);
        let alternate = peer(2);
        let hashes = [hash(1), hash(2)];

        for hash in hashes {
            fetcher.on_new_announcement(origin, eth68_announcement([(hash, 80)]));
            fetcher.on_new_announcement(alternate, eth68_announcement([(hash, 80)]));
        }

        let (peer, mut requests) = new_mock_session(origin, EthVersion::Eth68);
        let mut peers = TestPeers::default();
        peers.insert(origin, peer);
        assert!(fetcher.schedule_fetches(&peers, usize::MAX));
        let first = requests.recv().await.unwrap();
        poll_fetcher_pending(&mut fetcher).await;
        assert!(fetcher.schedule_fetches(&peers, usize::MAX));
        let second = requests.recv().await.unwrap();
        assert_eq!(fetcher.num_inflight(), 2);

        fetcher.on_peer_disconnected(&origin);

        assert_eq!(fetcher.num_inflight(), 0);
        assert_eq!(fetcher.num_fetching(), 0);
        for hash in hashes {
            assert_eq!(fetcher.announced[&hash].as_slice(), &[alternate]);
        }
        drop((first, second));
    }

    #[tokio::test]
    async fn successful_response_filters_unsolicited_transactions() {
        let mut fetcher = TestFetcher::default();
        let peer_id = peer(1);
        let (requested, unsolicited) = test_transactions();
        let requested_hash = *requested.tx_hash();
        fetcher.on_new_announcement(
            peer_id,
            eth68_announcement([(requested_hash, requested.eip2718_encoded_length())]),
        );

        let (peer, mut requests) = new_mock_session(peer_id, EthVersion::Eth68);
        let mut peers = TestPeers::default();
        peers.insert(peer_id, peer);
        assert!(fetcher.schedule_fetches(&peers, usize::MAX));

        let PeerRequest::GetPooledTransactions { response, .. } = requests.recv().await.unwrap()
        else {
            panic!("unexpected request")
        };
        response.send(Ok(PooledTransactions(vec![requested, unsolicited]))).unwrap();

        let Some(FetchEvent::TransactionsFetched { transactions, report_peer, .. }) =
            fetcher.next().await
        else {
            panic!("expected fetched transactions")
        };
        assert_eq!(transactions.len(), 1);
        assert!(report_peer);
    }

    #[tokio::test]
    async fn ready_responses_remain_owned_between_stream_polls() {
        let mut fetcher = TestFetcher::default();
        let first_peer = peer(1);
        let second_peer = peer(2);
        fetcher.on_new_announcement(first_peer, eth66_announcement([hash(1)]));
        fetcher.on_new_announcement(second_peer, eth66_announcement([hash(2)]));

        let (first, mut first_requests) = new_mock_session(first_peer, EthVersion::Eth68);
        let (second, mut second_requests) = new_mock_session(second_peer, EthVersion::Eth68);
        let mut peers = TestPeers::default();
        peers.insert(first_peer, first);
        peers.insert(second_peer, second);
        assert!(fetcher.schedule_fetches(&peers, usize::MAX));

        let PeerRequest::GetPooledTransactions { response: first_response, .. } =
            first_requests.recv().await.unwrap()
        else {
            panic!("unexpected request")
        };
        let PeerRequest::GetPooledTransactions { response: second_response, .. } =
            second_requests.recv().await.unwrap()
        else {
            panic!("unexpected request")
        };
        first_response.send(Ok(PooledTransactions(Vec::new()))).unwrap();
        second_response.send(Ok(PooledTransactions(Vec::new()))).unwrap();

        assert!(matches!(fetcher.next().await, Some(FetchEvent::EmptyResponse { .. })));
        assert_eq!(fetcher.num_inflight(), 1);

        // The other ready response remains in the inflight map, so a broadcast arriving between
        // stream polls can still mark its hash as satisfied by gossip.
        let remaining_hash = *fetcher.fetching.keys().next().unwrap();
        fetcher.on_transactions_received(once(remaining_hash), true);
        assert!(fetcher.fetching.is_empty());
        assert!(fetcher
            .inflight_requests
            .values()
            .next()
            .unwrap()
            .stolen
            .contains(&remaining_hash));

        assert!(matches!(fetcher.next().await, Some(FetchEvent::EmptyResponse { .. })));
        assert_eq!(fetcher.num_inflight(), 0);
    }

    #[test]
    fn metadata_backfill_preserves_fifo_position() {
        let mut fetcher = TestFetcher::default();
        let peer_id = peer(1);
        let first = hash(1);
        let second = hash(2);

        fetcher.on_new_announcement(peer_id, eth66_announcement([first]));
        fetcher.on_new_announcement(peer_id, eth68_announcement([(second, 100)]));
        fetcher.on_new_announcement(peer_id, eth68_announcement([(first, 500)]));

        let peer_announces = &fetcher.announces[&peer_id];
        let order = peer_announces.order.iter().map(|(hash, _)| *hash).collect::<Vec<_>>();
        assert_eq!(order, vec![first, second]);
        assert_eq!(peer_announces.metadata[&first].size, 500);
    }

    #[tokio::test]
    async fn stale_fifo_entries_do_not_change_reannouncement_order() {
        let mut fetcher = TestFetcher::default();
        let peer_id = peer(1);
        let first = hash(1);
        let completed = hash(2);

        fetcher.on_new_announcement(peer_id, eth68_announcement([(first, 100)]));
        fetcher.on_new_announcement(peer_id, eth68_announcement([(completed, 100)]));
        fetcher.on_transactions_received(once(completed), true);
        fetcher.on_new_announcement(peer_id, eth68_announcement([(completed, 100)]));

        let (peer, mut requests) = new_mock_session(peer_id, EthVersion::Eth68);
        let mut peers = TestPeers::default();
        peers.insert(peer_id, peer);
        assert!(fetcher.schedule_fetches(&peers, usize::MAX));
        let PeerRequest::GetPooledTransactions { request, .. } = requests.recv().await.unwrap()
        else {
            panic!("unexpected request")
        };
        assert_eq!(request.0, vec![first, completed]);
        assert_eq!(fetcher.announces[&peer_id].stale_entries, 0);
    }
}
