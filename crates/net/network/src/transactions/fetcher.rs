//! Fetches transactions that peers announced with `NewPooledTransactionHashes`.
//!
//! The [`TransactionFetcher`] tracks every announced hash that is not known yet, together with the
//! peers that announced it, and turns those announcements into `GetPooledTransactions` requests.
//!
//! # Model
//!
//! Every tracked hash is in exactly one of two states:
//!
//! - _pending_: waiting for one of the peers that announced it, its _candidates_, to become idle
//! - _fetching_: part of exactly one inflight request
//!
//! A hash remembers the peers that announced it as candidates. Each peer keeps a FIFO queue of
//! hashes to request, and the first peers to announce a hash get it queued right away. Later
//! announcers are only remembered, so that a hash is not given up on when the first announcers
//! fail to deliver it, without costing a queue entry per announcement. A request for an idle peer
//! is built by draining its queue in the order the announcements were processed, skipping hashes
//! that are being fetched from another peer or are not tracked anymore, until the request is
//! full. Packing a request is therefore proportional to the request size and independent of the
//! total number of pending hashes. FIFO refers to the fetcher's input order; callers may
//! reorder or deduplicate hashes within a wire announcement.
//!
//! When a request resolves, delivered hashes are dropped from tracking. Undelivered hashes go back
//! to pending and are queued for their remaining candidates, most recent announcers first, or are
//! dropped if none remain. The responding peer is dropped as a candidate for every undelivered
//! hash that precedes the last delivered hash of the request, treating it as skipped, and for
//! all requested hashes if the response was empty or the request failed.
//! Undelivered hashes after the last delivered hash keep the peer as a candidate only if it
//! delivered at least half of the request, treating that tail as potentially truncated. This is
//! a retry heuristic: transaction counts do not establish whether the response reached a byte
//! limit. Requiring half the request limits repeated low-progress retries.
//!
//! Request timeouts are enforced by the peer's session, which resolves the request with
//! [`RequestError::Timeout`], so the fetcher does not run any timers. A first timeout is retried
//! once when the responding peer is the only remaining source; otherwise another source is tried.
//!
//! # Bounds
//!
//! - a configurable number of inflight requests per peer, one by default, and a global inflight
//!   request limit
//! - a per peer limit on the number of tracked hashes it is a candidate for, so a single peer
//!   cannot flood the fetcher with announcements
//! - a global limit on the number of tracked hashes, at which the peer tracking the most hashes
//!   gives up its oldest pending hash, so a group of peers flooding the fetcher evicts its own
//!   hashes rather than everyone else's
//! - a fixed number of candidates per hash and a separate fetch-attempt limit, so remembering
//!   recent fallback sources does not allow unlimited retries

use super::{
    config::TransactionFetcherConfig,
    constants::{
        tx_fetcher::{
            AVERAGE_BYTE_SIZE_TX_ENCODED, MAX_COUNT_CANDIDATE_PEERS_PER_HASH,
            MAX_COUNT_EAGER_CANDIDATE_PEERS_PER_HASH, MAX_FETCH_ATTEMPTS_PER_HASH,
        },
        SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE,
        SOFT_LIMIT_COUNT_HASHES_IN_GET_POOLED_TRANSACTIONS_REQUEST,
    },
    PeerMetadata,
};
use crate::metrics::TransactionFetcherMetrics;
use alloy_consensus::transaction::PooledTransaction;
use alloy_primitives::{
    map::{B256Map, B256Set, Entry, FbBuildHasher, HashMap},
    TxHash,
};
use futures::{stream::FuturesUnordered, Future, FutureExt, Stream, StreamExt};
use reth_eth_wire::{Eth68TxMetadata, EthVersion, GetPooledTransactions, PooledTransactions};
use reth_eth_wire_types::{EthNetworkPrimitives, NetworkPrimitives};
use reth_network_api::PeerRequest;
use reth_network_p2p::error::{RequestError, RequestResult};
use reth_network_peers::PeerId;
use reth_primitives_traits::SignedTransaction;
use smallvec::SmallVec;
use std::{
    collections::{BinaryHeap, VecDeque},
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
};
use tokio::sync::{mpsc::error::TrySendError, oneshot};
use tracing::trace;

/// Maximum live entries inspected in a global fallback eviction search.
const MAX_EVICTION_ATTEMPTS: usize = 8;

/// Fetches transactions that peers announced but that are not in the pool yet.
///
/// Announcements are recorded with [`Self::on_announcement`], requests are sent with
/// [`Self::dispatch`] and resolved requests are yielded as [`FetchEvent`]s by the [`Stream`]
/// implementation. See the [module docs](self) for how requests are scheduled.
#[derive(Debug)]
pub struct TransactionFetcher<N: NetworkPrimitives = EthNetworkPrimitives> {
    /// All tracked hashes with their candidate peers and fetch state.
    hashes: B256Map<TxEntry>,
    /// Tracked hashes in the order they were added, used to evict the oldest pending hash once
    /// the fetcher is at capacity and the peer tracking the most hashes has none pending. Entries
    /// are removed lazily, so it may contain hashes that are not tracked anymore.
    order: VecDeque<(TxHash, u64)>,
    /// Fetch state of all peers that announced tracked hashes.
    peers: HashMap<PeerKey, PeerState>,
    /// Maps peer ids to their compact key.
    peer_keys: HashMap<PeerId, PeerKey, FbBuildHasher<64>>,
    /// Key assigned to the next new peer. Keys are never reused, so the candidates of a hash can
    /// safely refer to peers that disconnected in the meantime.
    next_peer_key: u32,
    /// Distinguishes overlapping requests, including repeat requests for the same hash.
    next_request_id: u64,
    /// Distinguishes a re-announcement from stale queue entries for an earlier tracked hash.
    next_generation: u64,
    /// Idle peers with queued hashes, in the order they became ready.
    ready: VecDeque<PeerKey>,
    /// All inflight `GetPooledTransactions` requests.
    inflight: FuturesUnordered<InflightRequest<N::PooledTransaction>>,
    /// Number of tracked hashes that are part of an inflight request.
    num_fetching: usize,
    /// Reused when verifying responses, so no sets are allocated per response.
    scratch_requested: B256Set,
    /// Reused when verifying responses, so no sets are allocated per response.
    scratch_delivered: B256Set,
    /// Reused when processing announcements, so no vector is allocated per announcement.
    scratch_queue: Vec<(TxHash, u64)>,
    /// Configured limits.
    config: TransactionFetcherConfig,
    metrics: TransactionFetcherMetrics,
}

impl<N: NetworkPrimitives> TransactionFetcher<N> {
    /// Creates a new fetcher with the given config.
    pub fn new(config: TransactionFetcherConfig) -> Self {
        let metrics = TransactionFetcherMetrics::default();
        metrics.capacity_inflight_requests.increment(config.max_inflight_requests as u64);

        Self {
            hashes: Default::default(),
            order: Default::default(),
            peers: Default::default(),
            peer_keys: Default::default(),
            next_peer_key: 0,
            next_request_id: 0,
            next_generation: 0,
            ready: Default::default(),
            inflight: Default::default(),
            num_fetching: 0,
            scratch_requested: Default::default(),
            scratch_delivered: Default::default(),
            scratch_queue: Default::default(),
            config,
            metrics,
        }
    }

    /// Returns the fetcher's config.
    pub const fn config(&self) -> &TransactionFetcherConfig {
        &self.config
    }

    /// Returns the number of tracked hashes, pending and fetching.
    pub fn num_hashes(&self) -> usize {
        self.hashes.len()
    }

    /// Returns the number of hashes that are waiting for an idle candidate peer.
    pub fn num_pending_hashes(&self) -> usize {
        self.hashes.len().saturating_sub(self.num_fetching)
    }

    /// Returns the number of hashes that are part of an inflight request.
    pub const fn num_fetching_hashes(&self) -> usize {
        self.num_fetching
    }

    /// Returns the number of inflight requests.
    pub fn num_inflight_requests(&self) -> usize {
        self.inflight.len()
    }

    /// Returns `true` if there is no inflight request to the peer.
    pub fn is_idle(&self, peer_id: &PeerId) -> bool {
        self.peer_keys
            .get(peer_id)
            .and_then(|key| self.peers.get(key))
            .is_none_or(|peer| peer.inflight == 0)
    }

    /// Records hashes announced by the peer and queues them for fetching.
    ///
    /// The metadata of a hash is the announced transaction type and size, if the announcement
    /// carried it. The caller is expected to have filtered out hashes that are already known.
    ///
    /// Requests are only sent by [`Self::dispatch`].
    pub fn on_announcement(
        &mut self,
        peer_id: PeerId,
        announcement: impl IntoIterator<Item = (TxHash, Eth68TxMetadata)>,
    ) {
        let key = self.peer_key(peer_id);
        let max_per_peer = self.config.max_announced_hashes_per_peer as usize;
        let max_total = self.config.max_capacity_cache_txns_pending_fetch as usize;

        let mut dropped_peer_limit = 0u64;
        let mut evicted_at_capacity = 0u64;
        let mut dropped_at_capacity = 0u64;

        // The peer's count of tracked hashes and the hashes to queue for it are kept locally and
        // applied once at the end, so every announced hash costs a single map lookup.
        let Some(peer) = self.peers.get(&key) else { return };
        let mut tracked = peer.tracked;
        let mut queue = std::mem::take(&mut self.scratch_queue);
        queue.clear();
        // Entries at the front of `queue` already evicted or moved into the peer queue.
        let mut queue_start = 0;
        // Other peers can only lose tracked hashes during this announcement. Refresh their
        // heap entries lazily instead of scanning every peer for each hash at capacity.
        let mut eviction = None;
        // Gossip commonly replaces the same fallback peer for every hash in a batch. Apply
        // that peer's accounting once, flushing before eviction needs up-to-date counts.
        let mut retired_candidates = DeferredCandidateRemovals::default();

        for (hash, metadata) in announcement {
            let size = announced_size(metadata);
            let at_capacity = self.hashes.len() >= max_total;

            // whether the hash is queued for the peer right away
            let (eager, generation) = match self.hashes.entry(hash) {
                Entry::Occupied(mut occupied) => {
                    let entry = occupied.get_mut();
                    if let Some(candidate) = entry.candidate_mut(key) {
                        // announced before by this peer, keep the latest size
                        candidate.set_size(size);
                        continue
                    }
                    if tracked >= max_per_peer {
                        dropped_peer_limit += 1;
                        continue
                    }
                    if entry.candidates.len() == MAX_COUNT_CANDIDATE_PEERS_PER_HASH {
                        // Keep the first eager sources and the most recent fallback sources.
                        // A fixed first-N list lets early co-announcers exclude every later source.
                        let replace = (MAX_COUNT_EAGER_CANDIDATE_PEERS_PER_HASH..
                            entry.candidates.len())
                            .find(|&idx| {
                                entry.fetching_by.is_none_or(|(fetching, _)| {
                                    entry.candidates[idx].peer != fetching
                                })
                            })
                            .expect("only one candidate can be fetching");
                        let removed = entry.candidates.remove(replace);
                        retired_candidates.record_removed(removed.peer, &mut self.peers);
                    }
                    // The first announcers get the hash queued right away, later ones are only
                    // remembered and asked once the earlier ones failed to deliver.
                    let eager = entry.candidates.len() < MAX_COUNT_EAGER_CANDIDATE_PEERS_PER_HASH ||
                        (entry.fetching_by.is_none() &&
                            !entry.candidates.iter().any(Candidate::is_queued));
                    let candidate = if eager {
                        Candidate::queued(key, size)
                    } else {
                        Candidate::unqueued(key, size)
                    };
                    entry.candidates.push(candidate);
                    (eager, entry.generation)
                }
                Entry::Vacant(vacant) => {
                    let generation = self.next_generation;
                    self.next_generation += 1;
                    if tracked >= max_per_peer {
                        dropped_peer_limit += 1;
                        continue
                    }
                    if at_capacity {
                        retired_candidates.flush(&mut self.peers);
                        // the evicted hash may be one of this peer's, so its count is synced
                        if let Some(peer) = self.peers.get_mut(&key) {
                            peer.tracked = tracked;
                        }
                        if self.evict_pending(key, &queue, &mut queue_start, &mut eviction) {
                            evicted_at_capacity += 1;
                            tracked = self.peers.get(&key).map_or(tracked, |peer| peer.tracked);
                        } else {
                            dropped_at_capacity += 1;
                            continue
                        }
                        self.hashes.insert(hash, TxEntry::new(key, size, generation));
                    } else {
                        vacant.insert(TxEntry::new(key, size, generation));
                    }
                    self.record_order(hash, generation);
                    (true, generation)
                }
            };

            tracked += 1;
            if eager {
                queue.push((hash, generation));
            }
        }

        retired_candidates.flush(&mut self.peers);
        let queued = queue.len() - queue_start;
        if let Some(peer) = self.peers.get_mut(&key) {
            peer.tracked = tracked;
            for &(hash, generation) in &queue[queue_start..] {
                peer.push_queue(
                    &self.hashes,
                    key,
                    (hash, generation),
                    QueuePosition::Back,
                    max_per_peer,
                );
            }
        }
        queue.clear();
        self.scratch_queue = queue;

        if dropped_peer_limit > 0 {
            self.metrics.announced_hashes_dropped_peer_limit.increment(dropped_peer_limit);
        }
        if evicted_at_capacity > 0 {
            self.metrics.hashes_evicted_at_capacity.increment(evicted_at_capacity);
        }
        if dropped_at_capacity > 0 {
            self.metrics.announced_hashes_dropped_at_capacity.increment(dropped_at_capacity);
        }

        if queued > 0 {
            trace!(target: "net::tx",
                peer_id=format!("{peer_id:#}"),
                queued,
                dropped_peer_limit,
                evicted_at_capacity,
                dropped_at_capacity,
                "queued announced hashes"
            );
        }
        if queued > 0 || queue_start > 0 {
            self.mark_ready(key, QueuePosition::Back);
        }
    }

    /// Sends `GetPooledTransactions` requests to idle peers that have queued hashes.
    ///
    /// Stops when the global inflight request limit is reached. `max_hashes_per_request` caps
    /// each request independently of hashes inflight to other peers, so stalled peers cannot
    /// shrink requests to responsive peers. Zero sends nothing. The caller must enforce its
    /// concurrent import limit when admitting responses; decoded responses remain bounded by
    /// the configured inflight request limit.
    ///
    /// Returns the number of requests sent. New requests are only polled by the next call to
    /// [`Stream::poll_next`], so the caller must poll the fetcher again if any were sent.
    pub fn dispatch(
        &mut self,
        peers: &HashMap<PeerId, PeerMetadata<N>, FbBuildHasher<64>>,
        max_hashes_per_request: usize,
    ) -> usize {
        if max_hashes_per_request == 0 {
            return 0
        }
        let max_inflight = self.config.max_inflight_requests as usize;
        let mut sent = 0;
        // peers whose session channel is full, they get another chance on the next dispatch
        let mut retry = SmallVec::<[PeerKey; 4]>::new();

        while self.inflight.len() < max_inflight {
            let Some(key) = self.ready.pop_front() else { break };
            let Some(peer) = self.peers.get_mut(&key) else { continue };
            peer.ready = false;
            if peer.inflight >= self.config.max_inflight_requests_per_peer {
                continue
            }
            let peer_id = peer.peer_id;
            // the session is gone if the manager doesn't know the peer anymore
            let Some(session) = peers.get(&peer_id) else { continue };

            let limit = max_hashes_per_request
                .min(SOFT_LIMIT_COUNT_HASHES_IN_GET_POOLED_TRANSACTIONS_REQUEST);
            let request_id = self.next_request_id;
            self.next_request_id += 1;
            let hashes = self.pack_request(key, request_id, limit);
            if hashes.is_empty() {
                continue
            }

            let (response, rx) = oneshot::channel();
            let request = PeerRequest::GetPooledTransactions {
                request: GetPooledTransactions(hashes.clone()),
                response,
            };

            match session.request_tx().try_send(request) {
                Ok(()) => {
                    trace!(target: "net::tx",
                        peer_id=format!("{peer_id:#}"),
                        hashes=hashes.len(),
                        "sending `GetPooledTransactions` request to peer's session"
                    );
                    if let Some(peer) = self.peers.get_mut(&key) {
                        peer.inflight += 1;
                    }
                    self.inflight.push(InflightRequest {
                        peer: key,
                        request_id,
                        peer_id,
                        version: session.version(),
                        client_version: session.client_version.clone(),
                        hashes,
                        response: rx,
                    });
                    sent += 1;
                    // the peer may be allowed more than one inflight request
                    self.mark_ready(key, QueuePosition::Back);
                }
                Err(err) => {
                    self.unpack_request(key, request_id, hashes);
                    match err {
                        TrySendError::Full(_) => {
                            self.metrics.egress_peer_channel_full.increment(1);
                            retry.push(key);
                        }
                        TrySendError::Closed(_) => self.on_peer_disconnected(&peer_id),
                    }
                }
            }
        }

        for key in retry {
            self.mark_ready(key, QueuePosition::Back);
        }

        sent
    }

    /// Stops tracking the given hashes because the transactions were received, e.g. over
    /// broadcast.
    ///
    /// Hashes that are part of an inflight request are simply not rescheduled when that request
    /// resolves.
    pub fn on_transactions_received<'a>(&mut self, hashes: impl IntoIterator<Item = &'a TxHash>) {
        for hash in hashes {
            self.remove_hash(hash);
        }
    }

    /// Removes the peer as a candidate for all hashes it announced and drops pending hashes that
    /// have no candidate left.
    ///
    /// An inflight request to the peer resolves with an error once its session is gone, which
    /// reschedules the requested hashes.
    pub fn on_peer_disconnected(&mut self, peer_id: &PeerId) {
        let Some(key) = self.peer_keys.remove(peer_id) else { return };
        if self.peers.remove(&key).is_none() {
            return
        }

        // Every tracked hash is visited, since the peer is a candidate of hashes that are not in
        // its queue as well. Disconnects are rare compared to announcements, so this is cheaper
        // overall than checking for gone peers whenever candidates are counted.
        let mut dropped = 0u64;
        let mut requeue = Vec::new();
        self.hashes.retain(|hash, entry| {
            let before = entry.candidates.len();
            entry.candidates.retain(|candidate| candidate.peer != key);
            if entry.candidates.len() == before || entry.fetching_by.is_some() {
                return true
            }
            if entry.candidates.is_empty() {
                dropped += 1;
                return false
            }
            // the hash must stay queued for at least one candidate
            if !entry.candidates.iter().any(|candidate| candidate.is_queued()) {
                requeue.push((entry.generation, *hash, entry.unqueued_candidates()));
            }
            true
        });

        let mut requeued = SmallVec::<[PeerKey; MAX_COUNT_CANDIDATE_PEERS_PER_HASH]>::new();
        // Requeue at the front in reverse announcement order.
        requeue.sort_unstable_by_key(|(generation, _, _)| std::cmp::Reverse(*generation));
        for (_, hash, targets) in requeue {
            self.requeue(hash, targets, &mut requeued);
        }

        if dropped > 0 {
            self.metrics.hashes_dropped_no_candidate_peers.increment(dropped);
        }
        // retried hashes go to the most recent announcers first
        for key in requeued {
            self.mark_ready(key, QueuePosition::Front);
        }
    }

    /// Updates the fetcher's gauges.
    pub fn update_metrics(&self) {
        self.metrics.inflight_transaction_requests.set(self.inflight.len() as f64);
        self.metrics.hashes_inflight_transaction_requests.set(self.num_fetching as f64);
        self.metrics.hashes_pending_fetch.set(self.num_pending_hashes() as f64);
    }

    /// Returns the key of the peer, registering the peer if it isn't known yet.
    fn peer_key(&mut self, peer_id: PeerId) -> PeerKey {
        *self.peer_keys.entry(peer_id).or_insert_with(|| {
            let key = PeerKey(self.next_peer_key);
            self.next_peer_key += 1;
            self.peers.insert(key, PeerState::new(peer_id));
            key
        })
    }

    /// Marks the peer as ready for a request if it is idle and has hashes queued, either behind
    /// the other ready peers or ahead of them.
    fn mark_ready(&mut self, key: PeerKey, position: QueuePosition) {
        if let Some(peer) = self.peers.get_mut(&key) &&
            !peer.ready &&
            !peer.queue.is_empty() &&
            peer.inflight < self.config.max_inflight_requests_per_peer
        {
            peer.ready = true;
            match position {
                QueuePosition::Front => self.ready.push_front(key),
                QueuePosition::Back => self.ready.push_back(key),
            }
        }
    }

    /// Queues the hash for the peer, at the back or the front of its queue, and marks it as
    /// queued in the hash's candidate entry.
    fn enqueue(&mut self, key: PeerKey, hash: TxHash, position: QueuePosition) {
        let max_per_peer = self.config.max_announced_hashes_per_peer as usize;
        let Some(peer) = self.peers.get_mut(&key) else { return };
        let Some(entry) = self.hashes.get_mut(&hash) else { return };
        let generation = entry.generation;
        if let Some(candidate) = entry.candidate_mut(key) {
            candidate.set_queued(true);
        }
        peer.push_queue(&self.hashes, key, (hash, generation), position, max_per_peer);
    }

    /// Records a newly tracked hash in the eviction order.
    ///
    /// Stale lifetimes are removed lazily. Once the order reaches twice the capacity it is
    /// compacted in place, retaining only the current generation of every tracked hash.
    fn record_order(&mut self, hash: TxHash, generation: u64) {
        let max_len = 2 * self.config.max_capacity_cache_txns_pending_fetch as usize;
        if self.order.len() >= max_len {
            self.order.retain(|(hash, generation)| {
                self.hashes.get(hash).is_some_and(|entry| entry.generation == *generation)
            });
        }
        self.order.push_back((hash, generation));
    }

    /// Evicts a pending hash to make room for one announced by `announcer` and returns whether
    /// one was found.
    ///
    /// Prefer the oldest exclusively announced pending hash of the peer tracking the most
    /// hashes, so a group of peers flooding the fetcher gives up its own hashes when possible. The
    /// announcer gives way when it tracks as many hashes as the busiest peer. If the
    /// chosen peer has no exclusively announced pending hash, the oldest pending hash overall
    /// is evicted. Preferring exclusive hashes protects other peers' work when possible, while
    /// the fallback prevents co-announcements from making the entire cache unevictable.
    ///
    /// `queued` are the hashes of the current announcement that are not in the announcer's queue
    /// yet; `queued_start` marks entries already evicted or moved into the peer queue.
    fn evict_pending(
        &mut self,
        announcer: PeerKey,
        queued: &[(TxHash, u64)],
        queued_start: &mut usize,
        eviction: &mut Option<EvictionState>,
    ) -> bool {
        let eviction = eviction.get_or_insert_with(|| EvictionState {
            peers: self
                .peers
                .iter()
                .filter(|(key, peer)| **key != announcer && peer.tracked > 0)
                .map(|(key, peer)| (peer.tracked, *key))
                .collect(),
            exclusive: Default::default(),
        });
        let heap = &mut eviction.peers;
        let (mut tracked, mut key) = loop {
            let Some(&(previous, key)) = heap.peek() else { break (0, announcer) };
            let current = self.peers.get(&key).map_or(0, |peer| peer.tracked);
            if current == previous {
                break (current, key)
            }
            heap.pop();
            if current > 0 {
                heap.push((current, key));
            }
        };
        if self.peers.get(&announcer).is_some_and(|peer| peer.tracked >= tracked) {
            key = announcer;
            tracked = self.peers.get(&key).map_or(0, |peer| peer.tracked);
        }
        if tracked == 0 {
            return self.evict_oldest_pending()
        }
        if self.evict_oldest_pending_of(key, &mut eviction.exclusive) {
            return true
        }
        if key == announcer {
            while *queued_start < queued.len() {
                let (hash, generation) = queued[*queued_start];
                *queued_start += 1;
                let Some(entry) =
                    self.hashes.get(&hash).filter(|entry| entry.generation == generation)
                else {
                    continue
                };
                if entry.candidates.len() == 1 && entry.fetching_by.is_none() {
                    self.remove_hash(&hash);
                    return true
                }
                // Preserve shared entries skipped by the cursor in this announcement.
                self.enqueue(key, hash, QueuePosition::Back);
            }
        }
        self.evict_oldest_pending()
    }

    /// Evicts the oldest pending hash announced only by this peer, if one exists.
    ///
    /// Scan each selected peer's queue once per announcement. A bounded prefix can hide
    /// exclusive victims behind shared hashes; caching avoids rescanning that prefix for every
    /// eviction. Newly tracked hashes from the current announcement are handled by its cursor.
    fn evict_oldest_pending_of(
        &mut self,
        key: PeerKey,
        exclusive: &mut HashMap<PeerKey, VecDeque<(TxHash, u64)>>,
    ) -> bool {
        let candidates = exclusive.entry(key).or_insert_with(|| {
            self.peers
                .get(&key)
                .into_iter()
                .flat_map(|peer| &peer.queue)
                .filter(|(hash, generation)| {
                    self.hashes.get(hash).is_some_and(|entry| {
                        entry.generation == *generation &&
                            entry.candidates.len() == 1 &&
                            entry.has_candidate(key) &&
                            entry.fetching_by.is_none()
                    })
                })
                .copied()
                .collect()
        });
        while let Some((hash, generation)) = candidates.pop_front() {
            if self.hashes.get(&hash).is_some_and(|entry| {
                entry.generation == generation &&
                    entry.candidates.len() == 1 &&
                    entry.has_candidate(key) &&
                    entry.fetching_by.is_none()
            }) {
                self.remove_hash(&hash);
                return true
            }
        }
        false
    }

    /// Evicts the oldest pending hash, including shared hashes, as a last resort.
    ///
    /// Shared hashes cannot be exempt from eviction: co-announcements could pin every slot.
    /// Returns `false` if only fetching hashes were found within the search budget.
    fn evict_oldest_pending(&mut self) -> bool {
        let mut index = 0;
        let mut attempts = 0;
        while attempts < MAX_EVICTION_ATTEMPTS {
            let Some(&(hash, generation)) = self.order.get(index) else { break };
            let Some(entry) = self.hashes.get(&hash).filter(|entry| entry.generation == generation)
            else {
                self.order.remove(index);
                continue
            };
            attempts += 1;
            if entry.fetching_by.is_some() {
                index += 1;
                continue
            }
            self.order.remove(index);
            self.remove_hash(&hash);
            return true
        }
        false
    }

    /// Drains the peer's queue into a request, in announcement order, until the request holds
    /// `max_hashes` hashes or the expected response size reaches the configured soft limit. A
    /// transaction that on its own exceeds the size limit is requested alone.
    ///
    /// The returned hashes are marked as being fetched by the peer.
    fn pack_request(&mut self, key: PeerKey, request_id: u64, max_hashes: usize) -> Vec<TxHash> {
        let max_bytes =
            self.config.soft_limit_byte_size_pooled_transactions_response_on_pack_request;
        let Some(peer) = self.peers.get_mut(&key) else { return Vec::new() };

        let mut hashes = Vec::with_capacity(peer.queue.len().min(max_hashes));
        let mut bytes = 0usize;

        while let Some((hash, generation)) = peer.queue.pop_front() {
            // skip hashes that were delivered in the meantime, or that this peer is no longer a
            // candidate for
            let Some(entry) =
                self.hashes.get_mut(&hash).filter(|entry| entry.generation == generation)
            else {
                continue
            };
            let Some(candidate) = entry.candidate_mut(key) else { continue };
            candidate.set_queued(false);
            let size = candidate.request_size();

            // hashes that are being fetched elsewhere are queued again if that fetch fails
            if entry.fetching_by.is_some() {
                continue
            }

            if !hashes.is_empty() && bytes.saturating_add(size) > max_bytes {
                if let Some(candidate) = entry.candidate_mut(key) {
                    candidate.set_queued(true);
                }
                peer.queue.push_front((hash, generation));
                break
            }

            entry.fetching_by = Some((key, request_id));
            entry.attempts += 1;
            self.num_fetching += 1;
            bytes = bytes.saturating_add(size);
            hashes.push(hash);

            if hashes.len() >= max_hashes {
                break
            }
        }

        hashes
    }

    /// Reverts [`Self::pack_request`] for a request that could not be sent: the hashes are pending
    /// again and are queued at the front of the peer's queue in their original order.
    fn unpack_request(&mut self, key: PeerKey, request_id: u64, hashes: Vec<TxHash>) {
        for hash in hashes.into_iter().rev() {
            if let Some(entry) = self.hashes.get_mut(&hash) &&
                entry.fetching_by == Some((key, request_id))
            {
                entry.fetching_by = None;
                entry.attempts -= 1;
                self.num_fetching -= 1;
                self.enqueue(key, hash, QueuePosition::Front);
            }
        }
    }

    /// Queues a pending hash for the given candidates, the ones that don't have it queued, and
    /// records those peers in `requeued`.
    fn requeue(
        &mut self,
        hash: TxHash,
        targets: SmallVec<[PeerKey; MAX_COUNT_CANDIDATE_PEERS_PER_HASH]>,
        requeued: &mut SmallVec<[PeerKey; MAX_COUNT_CANDIDATE_PEERS_PER_HASH]>,
    ) {
        for key in targets {
            self.enqueue(key, hash, QueuePosition::Front);
            if !requeued.contains(&key) {
                requeued.push(key);
            }
        }
    }

    /// Stops tracking the hash.
    fn remove_hash(&mut self, hash: &TxHash) {
        let Some(entry) = self.hashes.remove(hash) else { return };
        if entry.fetching_by.is_some() {
            self.num_fetching -= 1;
        }
        for candidate in &entry.candidates {
            if let Some(peer) = self.peers.get_mut(&candidate.peer) {
                peer.tracked = peer.tracked.saturating_sub(1);
            }
        }
    }

    /// Processes a resolved request and returns the corresponding event.
    fn on_resolved(
        &mut self,
        resolved: ResolvedRequest<N::PooledTransaction>,
    ) -> FetchEvent<N::PooledTransaction> {
        let ResolvedRequest {
            peer: key,
            request_id,
            peer_id,
            version,
            client_version,
            hashes: requested,
            result,
        } = resolved;

        if let Some(peer) = self.peers.get_mut(&key) {
            peer.inflight = peer.inflight.saturating_sub(1);
        }

        let mut delivered = std::mem::take(&mut self.scratch_delivered);
        delivered.clear();

        let outcome = match result {
            Ok(mut transactions) => {
                let mut requested_set = std::mem::take(&mut self.scratch_requested);
                requested_set.clear();
                requested_set.extend(requested.iter().copied());
                let unsolicited =
                    verify_response(&mut transactions, &requested_set, &mut delivered);
                self.scratch_requested = requested_set;
                Ok((transactions, unsolicited))
            }
            Err(error) => Err(error),
        };

        let timed_out = matches!(&outcome, Err(RequestError::Timeout));
        self.on_delivery(key, request_id, &requested, &delivered, timed_out);
        self.scratch_delivered = delivered;
        self.mark_ready(key, QueuePosition::Back);

        match outcome {
            Ok((transactions, unsolicited)) => {
                if unsolicited > 0 {
                    self.metrics.unsolicited_transactions.increment(unsolicited as u64);
                    trace!(target: "net::tx",
                        peer_id=format!("{peer_id:#}"),
                        unsolicited,
                        "received transactions in `PooledTransactions` response that weren't requested"
                    );
                }
                if !transactions.is_empty() {
                    self.metrics.fetched_transactions.increment(transactions.len() as u64);
                    FetchEvent::TransactionsFetched {
                        peer_id,
                        version,
                        client_version,
                        transactions,
                        report_peer: unsolicited > 0,
                    }
                } else if unsolicited > 0 {
                    // the peer only sent transactions we didn't ask for
                    FetchEvent::FetchError { peer_id, error: RequestError::BadResponse }
                } else {
                    trace!(target: "net::tx",
                        peer_id=format!("{peer_id:#}"),
                        requested=requested.len(),
                        "received empty `PooledTransactions` response, peer failed to serve hashes it announced"
                    );
                    FetchEvent::EmptyResponse { peer_id }
                }
            }
            Err(error) => FetchEvent::FetchError { peer_id, error },
        }
    }

    /// Settles the requested hashes of a resolved request: delivered hashes are dropped and
    /// undelivered hashes are rescheduled for their remaining candidates.
    fn on_delivery(
        &mut self,
        key: PeerKey,
        request_id: u64,
        requested: &[TxHash],
        delivered: &B256Set,
        timed_out: bool,
    ) {
        // Position right after the last delivered hash. Drop the responder as a candidate for
        // missing hashes before it. Keep it for the undelivered tail only if at least half of
        // the requested hashes were delivered, treating that tail as potentially truncated.
        // This is a retry heuristic: transaction counts do not establish whether the response
        // reached a byte limit. Requiring half the request limits repeated low-progress retries.
        let cutoff = if 2 * delivered.len() >= requested.len() {
            requested
                .iter()
                .rposition(|hash| delivered.contains(hash))
                .map_or(requested.len(), |idx| idx + 1)
        } else {
            requested.len()
        };

        let mut dropped = 0u64;
        let mut requeued = SmallVec::<[PeerKey; MAX_COUNT_CANDIDATE_PEERS_PER_HASH]>::new();

        // iterate in reverse so that queueing at the front of a queue preserves the request order
        for (idx, hash) in requested.iter().enumerate().rev() {
            if delivered.contains(hash) {
                self.remove_hash(hash);
                continue
            }

            // the hash was received elsewhere in the meantime, or was even announced and assigned
            // to another peer again
            let Some(entry) = self.hashes.get_mut(hash) else { continue };
            if entry.fetching_by != Some((key, request_id)) {
                continue
            }
            entry.fetching_by = None;
            self.num_fetching -= 1;

            // A short adaptive session timeout can expire before the first response from the
            // only source arrives. Allow one retry; prefer another source whenever available.
            let retry_only_source = timed_out && entry.attempts == 1 && entry.candidates.len() == 1;
            let peers = &mut self.peers;
            entry.candidates.retain(|candidate| {
                // disconnected peers are pruned as well
                let Some(peer) = peers.get_mut(&candidate.peer) else { return false };
                if idx < cutoff && candidate.peer == key && !retry_only_source {
                    peer.tracked = peer.tracked.saturating_sub(1);
                    return false
                }
                true
            });

            if entry.candidates.is_empty() || entry.attempts >= MAX_FETCH_ATTEMPTS_PER_HASH {
                // Rotating fallback candidates must not allow unlimited retries for one hash.
                self.remove_hash(hash);
                dropped += 1;
                continue
            }

            let targets = entry.unqueued_candidates();
            self.requeue(*hash, targets, &mut requeued);
        }

        if dropped > 0 {
            self.metrics.hashes_dropped_no_candidate_peers.increment(dropped);
            trace!(target: "net::tx", dropped, "dropped hashes with no candidates or exhausted fetch attempts");
        }

        // retried hashes go to the most recent announcers first
        for key in requeued {
            self.mark_ready(key, QueuePosition::Front);
        }
    }
}

#[cfg(test)]
impl<N: NetworkPrimitives> TransactionFetcher<N> {
    /// Returns the connected peers that are candidates for the hash, in announcement order.
    pub(super) fn candidate_peers(&self, hash: &TxHash) -> Vec<PeerId> {
        self.hashes
            .get(hash)
            .map(|entry| {
                entry
                    .candidates
                    .iter()
                    .filter_map(|candidate| self.peers.get(&candidate.peer))
                    .map(|peer| peer.peer_id)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Returns the peer the hash is currently fetched from, if any.
    fn fetching_peer(&self, hash: &TxHash) -> Option<PeerId> {
        let (key, _) = self.hashes.get(hash)?.fetching_by?;
        self.peers.get(&key).map(|peer| peer.peer_id)
    }

    /// Returns the hashes queued for the peer, oldest first. May include hashes that are not
    /// tracked anymore or are being fetched elsewhere.
    pub(super) fn queued_hashes(&self, peer_id: &PeerId) -> Vec<TxHash> {
        self.peer_keys
            .get(peer_id)
            .and_then(|key| self.peers.get(key))
            .map(|peer| peer.queue.iter().map(|(hash, _)| *hash).collect())
            .unwrap_or_default()
    }

    /// Returns the number of peers the fetcher tracks.
    fn num_peers(&self) -> usize {
        self.peers.len()
    }

    /// Panics if the internal bookkeeping is inconsistent.
    fn assert_invariants(&self) {
        let fetching = self.hashes.values().filter(|entry| entry.fetching_by.is_some()).count();
        assert_eq!(fetching, self.num_fetching, "fetching counter out of sync");

        let ordered = self
            .order
            .iter()
            .filter(|(hash, generation)| {
                self.hashes.get(hash).is_some_and(|entry| entry.generation == *generation)
            })
            .map(|(hash, _)| *hash)
            .collect::<B256Set>();
        assert!(
            self.order.len() <= 2 * self.config.max_capacity_cache_txns_pending_fetch as usize,
            "eviction order grew beyond its bound"
        );

        for (hash, entry) in &self.hashes {
            assert!(
                !entry.candidates.is_empty() || entry.fetching_by.is_some(),
                "{hash} is pending without candidates"
            );
            assert!(
                entry.candidates.len() <= MAX_COUNT_CANDIDATE_PEERS_PER_HASH,
                "{hash} has too many candidates"
            );
            let mut unique = entry.candidates.iter().map(|c| c.peer).collect::<Vec<_>>();
            unique.sort_unstable();
            unique.dedup();
            assert_eq!(unique.len(), entry.candidates.len(), "{hash} has duplicate candidates");
            assert!(ordered.contains(hash), "{hash} is missing from the eviction order");
            if let Some((peer, request_id)) = entry.fetching_by {
                assert!(
                    self.inflight.iter().any(|request| request.peer == peer &&
                        request.request_id == request_id &&
                        request.hashes.contains(hash)),
                    "fetching hash has no matching request"
                );
            }
            assert!(entry.attempts <= MAX_FETCH_ATTEMPTS_PER_HASH);
        }

        // a hash flagged as queued is in the peer's queue, and a pending hash is queued for at
        // least one connected candidate, otherwise it could starve
        let queued = self
            .peers
            .iter()
            .map(|(key, peer)| {
                assert!(
                    peer.queue.len() <= 2 * self.config.max_announced_hashes_per_peer as usize,
                    "queue of {:#} grew beyond its bound",
                    peer.peer_id
                );
                (
                    *key,
                    peer.queue
                        .iter()
                        .filter(|(hash, generation)| {
                            self.hashes
                                .get(hash)
                                .is_some_and(|entry| entry.generation == *generation)
                        })
                        .map(|(hash, _)| *hash)
                        .collect::<B256Set>(),
                )
            })
            .collect::<HashMap<_, _>>();
        for (hash, entry) in &self.hashes {
            let mut fetchable = entry.fetching_by.is_some();
            for candidate in &entry.candidates {
                let queue =
                    queued.get(&candidate.peer).expect("candidate refers to a disconnected peer");
                if candidate.is_queued() {
                    assert!(
                        queue.contains(hash),
                        "{hash} is flagged but not queued for {:?}",
                        candidate.peer
                    );
                    fetchable = true;
                }
            }
            assert!(fetchable, "pending {hash} is not queued for any connected candidate");
        }

        for (key, peer) in &self.peers {
            assert_eq!(
                peer.inflight as usize,
                self.inflight.iter().filter(|request| request.peer == *key).count()
            );
            assert!(peer.inflight <= self.config.max_inflight_requests_per_peer);
            if peer.inflight < self.config.max_inflight_requests_per_peer &&
                peer.queue.iter().any(|(hash, generation)| {
                    self.hashes.get(hash).is_some_and(|entry| {
                        entry.generation == *generation &&
                            entry.fetching_by.is_none() &&
                            entry
                                .candidates
                                .iter()
                                .any(|candidate| candidate.peer == *key && candidate.is_queued())
                    })
                })
            {
                assert!(peer.ready, "idle peer has queued work but is not ready");
            }
            let tracked = self.hashes.values().filter(|entry| entry.has_candidate(*key)).count();
            assert_eq!(tracked, peer.tracked, "tracked counter of {:#} out of sync", peer.peer_id);
            assert!(
                peer.tracked <= self.config.max_announced_hashes_per_peer as usize,
                "{:#} exceeds the per peer limit",
                peer.peer_id
            );
            assert_eq!(
                peer.ready,
                self.ready.contains(key),
                "ready flag of {:#} out of sync",
                peer.peer_id
            );
            if peer.ready {
                assert!(
                    !peer.queue.is_empty(),
                    "{:#} is ready without queued hashes",
                    peer.peer_id
                );
                assert!(
                    peer.inflight < self.config.max_inflight_requests_per_peer,
                    "{:#} is ready but busy",
                    peer.peer_id
                );
            }
            assert_eq!(
                self.peer_keys.get(&peer.peer_id),
                Some(key),
                "peer key mapping out of sync"
            );
        }

        for (peer_id, key) in &self.peer_keys {
            assert!(self.peers.contains_key(key), "{peer_id:#} maps to an unknown key");
        }

        let inflight = self.peers.values().map(|peer| peer.inflight as usize).sum::<usize>();
        assert!(inflight <= self.inflight.len(), "peers claim more inflight requests than exist");
    }
}

impl<N: NetworkPrimitives> Default for TransactionFetcher<N> {
    fn default() -> Self {
        Self::new(TransactionFetcherConfig::default())
    }
}

impl<N: NetworkPrimitives> Stream for TransactionFetcher<N> {
    type Item = FetchEvent<N::PooledTransaction>;

    /// Advances all inflight requests and yields the next resolved request as an event.
    ///
    /// Never terminates, returns [`Poll::Pending`] while no request is inflight.
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // `FuturesUnordered` yields `None` while empty but keeps working once new requests are
        // pushed, so this is mapped to pending
        match self.inflight.poll_next_unpin(cx) {
            Poll::Ready(Some(resolved)) => Poll::Ready(Some(self.on_resolved(resolved))),
            Poll::Ready(None) | Poll::Pending => Poll::Pending,
        }
    }
}

/// Represents possible events from fetching transactions.
#[derive(Debug)]
pub enum FetchEvent<T = PooledTransaction> {
    /// Triggered when transactions are successfully fetched.
    TransactionsFetched {
        /// The ID of the peer from which transactions were fetched.
        peer_id: PeerId,
        /// The negotiated protocol of the session that served the response.
        version: EthVersion,
        /// The client version of the session that served the response.
        client_version: Arc<str>,
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

/// Compact identifier of a peer within the fetcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct PeerKey(u32);

/// Cached eviction work for one announcement. Other peers can only lose candidates; newly
/// tracked hashes from this announcer are handled by the announcement's scratch cursor.
#[derive(Debug)]
struct EvictionState {
    peers: BinaryHeap<(usize, PeerKey)>,
    exclusive: HashMap<PeerKey, VecDeque<(TxHash, u64)>>,
}

/// Position for newly queued hashes or ready peers.
#[derive(Debug, Clone, Copy)]
enum QueuePosition {
    Front,
    Back,
}

/// Batches consecutive removals from the same candidate peer during an announcement.
#[derive(Debug, Default)]
struct DeferredCandidateRemovals {
    pending: Option<(PeerKey, usize)>,
}

impl DeferredCandidateRemovals {
    fn record_removed(&mut self, key: PeerKey, peers: &mut HashMap<PeerKey, PeerState>) {
        if let Some((pending_key, count)) = &mut self.pending &&
            *pending_key == key
        {
            *count += 1;
        } else {
            self.flush(peers);
            self.pending = Some((key, 1));
        }
    }

    /// Synchronizes tracked counts before capacity eviction or the end of an announcement.
    fn flush(&mut self, peers: &mut HashMap<PeerKey, PeerState>) {
        if let Some((key, count)) = self.pending.take() {
            peers.get_mut(&key).expect("candidate peer is connected").tracked -= count;
        }
    }
}

/// State of a tracked hash.
#[derive(Debug)]
struct TxEntry {
    /// Peers that announced the hash and may be asked for it, oldest first.
    candidates: SmallVec<[Candidate; MAX_COUNT_CANDIDATE_PEERS_PER_HASH]>,
    /// Identity of this tracking lifetime, also stored in lazy queue entries.
    generation: u64,
    /// The peer and request currently fetching the transaction, if any.
    fetching_by: Option<(PeerKey, u64)>,
    /// Sent requests for this entry, bounded independently of candidate replacement.
    attempts: usize,
}

impl TxEntry {
    /// A new entry for a hash that is queued for the announcing peer.
    fn new(peer: PeerKey, size: u32, generation: u64) -> Self {
        let mut candidates = SmallVec::new();
        candidates.push(Candidate::queued(peer, size));
        Self { candidates, generation, fetching_by: None, attempts: 0 }
    }

    fn has_candidate(&self, key: PeerKey) -> bool {
        self.candidates.iter().any(|candidate| candidate.peer == key)
    }

    /// Returns the candidates that don't have the hash queued, in the order they announced it.
    fn unqueued_candidates(&self) -> SmallVec<[PeerKey; MAX_COUNT_CANDIDATE_PEERS_PER_HASH]> {
        self.candidates
            .iter()
            .filter(|candidate| !candidate.is_queued())
            .map(|candidate| candidate.peer)
            .collect()
    }

    fn candidate_mut(&mut self, key: PeerKey) -> Option<&mut Candidate> {
        self.candidates.iter_mut().find(|candidate| candidate.peer == key)
    }
}

/// A peer that announced a hash.
#[derive(Debug, Clone, Copy)]
struct Candidate {
    peer: PeerKey,
    /// The size the peer announced for the transaction, 0 if unknown, with
    /// [`Self::QUEUED`] set while the hash is in the peer's queue.
    size_and_queued: u32,
}

impl Candidate {
    /// Marks the hash as queued for the peer. Announced sizes are capped well below this bit.
    const QUEUED: u32 = 1 << 31;

    /// A candidate that has the hash queued.
    const fn queued(peer: PeerKey, size: u32) -> Self {
        Self { peer, size_and_queued: size | Self::QUEUED }
    }

    /// A candidate that doesn't have the hash queued.
    const fn unqueued(peer: PeerKey, size: u32) -> Self {
        Self { peer, size_and_queued: size }
    }

    const fn is_queued(&self) -> bool {
        self.size_and_queued & Self::QUEUED != 0
    }

    const fn set_queued(&mut self, queued: bool) {
        if queued {
            self.size_and_queued |= Self::QUEUED;
        } else {
            self.size_and_queued &= !Self::QUEUED;
        }
    }

    const fn set_size(&mut self, size: u32) {
        self.size_and_queued = (self.size_and_queued & Self::QUEUED) | size;
    }

    /// Returns the size to account for when packing the hash into a request to this peer.
    const fn request_size(&self) -> usize {
        let size = self.size_and_queued & !Self::QUEUED;
        if size == 0 {
            AVERAGE_BYTE_SIZE_TX_ENCODED
        } else {
            size as usize
        }
    }
}

/// Fetch state of a peer.
#[derive(Debug)]
struct PeerState {
    peer_id: PeerId,
    /// Hashes the peer announced, oldest first. Entries are removed lazily, so the queue may
    /// contain hashes that are not tracked anymore or that the peer is no longer a candidate for.
    queue: VecDeque<(TxHash, u64)>,
    /// Number of tracked hashes that list this peer as a candidate.
    tracked: usize,
    /// Number of inflight requests to this peer.
    inflight: u8,
    /// Whether the peer is queued in the ready list.
    ready: bool,
}

impl PeerState {
    const fn new(peer_id: PeerId) -> Self {
        Self { peer_id, queue: VecDeque::new(), tracked: 0, inflight: 0, ready: false }
    }

    /// Queues the hash at the back or the front of the queue.
    ///
    /// Queues are bounded: once a queue holds twice `max_tracked` hashes, entries the peer is no
    /// longer a candidate for and duplicates are removed. A duplicate can occur when a hash is
    /// tracked again after it was delivered or evicted, while its old entry still lingers in the
    /// queue. Generations prevent those stale entries from selecting a newer tracking lifetime.
    fn push_queue(
        &mut self,
        hashes: &B256Map<TxEntry>,
        key: PeerKey,
        queued: (TxHash, u64),
        position: QueuePosition,
        max_tracked: usize,
    ) {
        if self.queue.len() >= 2 * max_tracked {
            let mut seen = B256Set::with_capacity_and_hasher(self.tracked, Default::default());
            self.queue.retain(|(hash, generation)| {
                hashes.get(hash).is_some_and(|entry| {
                    entry.generation == *generation && entry.has_candidate(key)
                }) && seen.insert(*hash)
            });
        }
        match position {
            QueuePosition::Front => self.queue.push_front(queued),
            QueuePosition::Back => self.queue.push_back(queued),
        }
    }
}

/// An inflight `GetPooledTransactions` request.
#[derive(Debug)]
struct InflightRequest<T> {
    peer: PeerKey,
    request_id: u64,
    peer_id: PeerId,
    version: EthVersion,
    client_version: Arc<str>,
    /// The requested hashes, in request order.
    hashes: Vec<TxHash>,
    response: oneshot::Receiver<RequestResult<PooledTransactions<T>>>,
}

impl<T> Future for InflightRequest<T> {
    type Output = ResolvedRequest<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let result =
            ready!(this.response.poll_unpin(cx)).unwrap_or(Err(RequestError::ChannelClosed));
        Poll::Ready(ResolvedRequest {
            peer: this.peer,
            request_id: this.request_id,
            peer_id: this.peer_id,
            version: this.version,
            client_version: this.client_version.clone(),
            hashes: std::mem::take(&mut this.hashes),
            result,
        })
    }
}

/// A resolved `GetPooledTransactions` request.
#[derive(Debug)]
struct ResolvedRequest<T> {
    peer: PeerKey,
    request_id: u64,
    peer_id: PeerId,
    version: EthVersion,
    client_version: Arc<str>,
    hashes: Vec<TxHash>,
    result: RequestResult<PooledTransactions<T>>,
}

/// Returns the announced transaction size used for request packing, capped at the response soft
/// limit, or 0 if the announcement carried no size.
fn announced_size(metadata: Eth68TxMetadata) -> u32 {
    metadata
        .map_or(0, |(_, size)| size.min(SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE) as u32)
}

/// Filters a response down to the transactions that were requested, dropping duplicates.
///
/// Records the delivered hashes in `delivered` and returns the number of unsolicited transactions
/// that were dropped.
fn verify_response<T: SignedTransaction>(
    transactions: &mut PooledTransactions<T>,
    requested: &B256Set,
    delivered: &mut B256Set,
) -> usize {
    let mut unsolicited = 0;
    transactions.0.retain(|tx| {
        let hash = *tx.tx_hash();
        if !requested.contains(&hash) {
            unsolicited += 1;
            return false
        }
        delivered.insert(hash)
    });
    unsolicited
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::transactions::new_mock_session_with_capacity;
    use alloy_consensus::transaction::Recovered;
    use alloy_primitives::B256;
    use futures::task::{noop_waker_ref, waker, ArcWake};
    use rand::{rngs::StdRng, seq::IndexedRandom, Rng, SeedableRng};
    use reth_eth_wire::EthVersion;
    use reth_ethereum_primitives::{PooledTransactionVariant, TransactionSigned};
    use reth_transaction_pool::test_utils::MockTransactionFactory;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use tokio::sync::mpsc;

    type Fetcher = TransactionFetcher<EthNetworkPrimitives>;
    type ResponseSender =
        oneshot::Sender<RequestResult<PooledTransactions<PooledTransactionVariant>>>;

    const KIB: usize = 1024;

    fn peer(n: u8) -> PeerId {
        PeerId::new([n; 64])
    }

    fn hash(n: u64) -> TxHash {
        let mut bytes = [0u8; 32];
        bytes[24..].copy_from_slice(&n.to_be_bytes());
        B256::from(bytes)
    }

    fn hashes(range: std::ops::Range<u64>) -> Vec<TxHash> {
        range.map(hash).collect()
    }

    /// Counts how often a task is woken.
    #[derive(Default)]
    struct WakeCounter(AtomicUsize);

    impl ArcWake for WakeCounter {
        fn wake_by_ref(arc_self: &Arc<Self>) {
            arc_self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl WakeCounter {
        fn wakes(&self) -> usize {
            self.0.load(Ordering::SeqCst)
        }
    }

    fn pooled_txs(count: usize) -> Vec<PooledTransactionVariant> {
        let mut factory = MockTransactionFactory::default();
        (0..count)
            .map(|_| {
                let recovered: Recovered<TransactionSigned> =
                    factory.create_eip1559().transaction.into();
                PooledTransactionVariant::try_from(recovered.into_inner())
                    .expect("eip1559 transaction converts to pooled transaction")
            })
            .collect()
    }

    fn hashes_of(txs: &[PooledTransactionVariant]) -> Vec<TxHash> {
        txs.iter().map(|tx| *tx.tx_hash()).collect()
    }

    /// A fetcher with mock peer sessions.
    struct Rig {
        fetcher: Fetcher,
        peers: HashMap<PeerId, PeerMetadata<EthNetworkPrimitives>, FbBuildHasher<64>>,
        sessions: HashMap<PeerId, mpsc::Receiver<PeerRequest>, FbBuildHasher<64>>,
    }

    impl Rig {
        fn new() -> Self {
            Self::with_config(TransactionFetcherConfig::default())
        }

        fn with_config(config: TransactionFetcherConfig) -> Self {
            Self {
                fetcher: Fetcher::new(config),
                peers: Default::default(),
                sessions: Default::default(),
            }
        }

        fn verify(&self) {
            self.fetcher.assert_invariants();
        }

        fn add_peer(&mut self, peer_id: PeerId) {
            self.add_peer_with_capacity(peer_id, 8);
        }

        fn add_peer_with_capacity(&mut self, peer_id: PeerId, capacity: usize) {
            let (peer, rx) = new_mock_session_with_capacity(peer_id, EthVersion::Eth68, capacity);
            self.peers.insert(peer_id, peer);
            self.sessions.insert(peer_id, rx);
        }

        fn announce(&mut self, peer_id: PeerId, hashes: &[TxHash]) {
            self.announce_with_sizes(peer_id, hashes.iter().map(|hash| (*hash, 512)));
        }

        fn announce_with_sizes(
            &mut self,
            peer_id: PeerId,
            entries: impl IntoIterator<Item = (TxHash, usize)>,
        ) {
            self.fetcher.on_announcement(
                peer_id,
                entries.into_iter().map(|(h, size)| (h, Some((2, size)))),
            );
            self.verify();
        }

        fn announce_unsized(&mut self, peer_id: PeerId, hashes: &[TxHash]) {
            self.fetcher.on_announcement(peer_id, hashes.iter().map(|hash| (*hash, None)));
            self.verify();
        }

        fn dispatch(&mut self) -> usize {
            self.dispatch_with_budget(usize::MAX)
        }

        fn dispatch_with_budget(&mut self, max_hashes_per_request: usize) -> usize {
            let sent = self.fetcher.dispatch(&self.peers, max_hashes_per_request);
            self.verify();
            sent
        }

        /// Takes the next request the peer's session received.
        fn take_request(&mut self, peer_id: PeerId) -> Option<(Vec<TxHash>, ResponseSender)> {
            match self.sessions.get_mut(&peer_id)?.try_recv().ok()? {
                PeerRequest::GetPooledTransactions { request, response } => {
                    Some((request.0, response))
                }
                _ => unreachable!("the fetcher only sends `GetPooledTransactions` requests"),
            }
        }

        fn next_event(&mut self) -> Option<FetchEvent<PooledTransactionVariant>> {
            let mut cx = Context::from_waker(noop_waker_ref());
            let event = match self.fetcher.poll_next_unpin(&mut cx) {
                Poll::Ready(event) => event,
                Poll::Pending => None,
            };
            self.verify();
            event
        }

        fn drain_events(&mut self) -> Vec<FetchEvent<PooledTransactionVariant>> {
            std::iter::from_fn(|| self.next_event()).collect()
        }

        fn respond(
            &mut self,
            peer_id: PeerId,
            txs: Vec<PooledTransactionVariant>,
        ) -> FetchEvent<PooledTransactionVariant> {
            let (_, response) = self.take_request(peer_id).expect("request inflight");
            response.send(Ok(PooledTransactions(txs))).unwrap();
            self.next_event().expect("response yields an event")
        }

        fn fail(
            &mut self,
            peer_id: PeerId,
            error: RequestError,
        ) -> FetchEvent<PooledTransactionVariant> {
            let (_, response) = self.take_request(peer_id).expect("request inflight");
            response.send(Err(error)).unwrap();
            self.next_event().expect("error yields an event")
        }

        fn disconnect(&mut self, peer_id: PeerId) {
            self.peers.remove(&peer_id);
            self.sessions.remove(&peer_id);
            self.fetcher.on_peer_disconnected(&peer_id);
            self.verify();
        }
    }

    #[test]
    fn announced_hashes_are_requested_from_announcing_peer() {
        let mut rig = Rig::new();
        let peer_a = peer(1);
        rig.add_peer(peer_a);
        let mut txs = pooled_txs(2);
        txs.sort_unstable_by_key(|tx| std::cmp::Reverse(*tx.tx_hash()));
        let hashes = hashes_of(&txs);

        let mut cx = Context::from_waker(noop_waker_ref());
        assert!(rig.fetcher.poll_next_unpin(&mut cx).is_pending());
        rig.announce(peer_a, &hashes[..1]);
        rig.announce(peer_a, &hashes[1..]);
        rig.announce(peer_a, &hashes);
        assert_eq!(rig.fetcher.queued_hashes(&peer_a), hashes);
        assert_eq!(rig.fetcher.candidate_peers(&hashes[0]), vec![peer_a]);
        assert_eq!(rig.fetcher.num_pending_hashes(), 2);
        assert!(rig.fetcher.is_idle(&peer_a));

        assert_eq!(rig.dispatch(), 1);
        assert!(!rig.fetcher.is_idle(&peer_a));
        assert_eq!(rig.fetcher.num_inflight_requests(), 1);
        assert_eq!(rig.fetcher.num_fetching_hashes(), 2);
        assert_eq!(rig.fetcher.num_pending_hashes(), 0);
        assert_eq!(rig.fetcher.fetching_peer(&hashes[0]), Some(peer_a));

        let (requested, response) = rig.take_request(peer_a).unwrap();
        assert_eq!(requested, hashes);
        response.send(Ok(PooledTransactions(txs))).unwrap();

        let FetchEvent::TransactionsFetched { peer_id, transactions, report_peer, .. } =
            rig.next_event().unwrap()
        else {
            panic!("expected fetched transactions")
        };
        assert_eq!(peer_id, peer_a);
        assert_eq!(transactions.len(), 2);
        assert!(!report_peer);

        assert_eq!(rig.fetcher.num_hashes(), 0);
        assert_eq!(rig.fetcher.num_inflight_requests(), 0);
        assert!(rig.fetcher.is_idle(&peer_a));
        assert!(rig.fetcher.poll_next_unpin(&mut cx).is_pending());
    }

    #[test]
    fn request_is_capped_by_hash_count() {
        let mut rig = Rig::new();
        let peer_a = peer(1);
        rig.add_peer(peer_a);
        let hashes = hashes(0..300);

        // no size metadata, so the count limit is what bounds the request
        rig.announce_unsized(peer_a, &hashes);
        assert_eq!(rig.dispatch(), 1);

        let (requested, response) = rig.take_request(peer_a).unwrap();
        assert_eq!(requested.len(), SOFT_LIMIT_COUNT_HASHES_IN_GET_POOLED_TRANSACTIONS_REQUEST);
        assert_eq!(requested, hashes[..256]);
        assert_eq!(rig.fetcher.num_pending_hashes(), 44);

        // the peer is busy until the request resolves
        assert_eq!(rig.dispatch(), 0);
        response.send(Err(RequestError::BadResponse)).unwrap();
        rig.next_event().unwrap();
        assert_eq!(rig.fetcher.num_pending_hashes(), 44);

        assert_eq!(rig.dispatch(), 1);
        let (requested, _) = rig.take_request(peer_a).unwrap();
        assert_eq!(requested, hashes[256..]);
    }

    #[test]
    fn request_is_capped_by_expected_response_size() {
        let mut rig = Rig::new();
        let peer_a = peer(1);
        rig.add_peer(peer_a);
        let hashes = hashes(0..3);

        rig.announce_with_sizes(
            peer_a,
            [(hashes[0], 100 * KIB), (hashes[1], 100 * KIB), (hashes[2], 100)],
        );

        rig.dispatch();
        let (requested, response) = rig.take_request(peer_a).unwrap();
        assert_eq!(requested, hashes[..1], "second transaction doesn't fit in 128 KiB");

        response.send(Ok(PooledTransactions(vec![]))).unwrap();
        rig.next_event().unwrap();
        // the empty response dropped the peer as candidate for the first hash
        assert_eq!(rig.fetcher.num_hashes(), 2);

        rig.dispatch();
        let (requested, _) = rig.take_request(peer_a).unwrap();
        assert_eq!(requested, hashes[1..]);
    }

    #[test]
    fn oversized_transaction_is_requested_alone() {
        let mut rig = Rig::new();
        let peer_a = peer(1);
        rig.add_peer(peer_a);
        let hashes = hashes(0..3);

        rig.announce_with_sizes(
            peer_a,
            [(hashes[0], 100), (hashes[1], 1024 * KIB), (hashes[2], 100)],
        );

        let mut requests = Vec::new();
        for _ in 0..3 {
            assert_eq!(rig.dispatch(), 1);
            let (requested, response) = rig.take_request(peer_a).unwrap();
            requests.push(requested);
            // A bad response keeps other hashes pending but drops the requested ones.
            response.send(Err(RequestError::BadResponse)).unwrap();
            rig.next_event().unwrap();
        }
        assert_eq!(
            requests,
            vec![hashes[..1].to_vec(), hashes[1..2].to_vec(), hashes[2..].to_vec()]
        );
        assert_eq!(rig.fetcher.num_hashes(), 0);
    }

    #[test]
    fn announced_size_is_per_peer() {
        let mut rig = Rig::new();
        let peer_a = peer(1);
        let peer_b = peer(2);
        rig.add_peer(peer_a);
        rig.add_peer(peer_b);
        let hashes = hashes(0..2);

        // peer_a doesn't know the sizes, peer_b announces a huge first transaction
        rig.announce_unsized(peer_a, &hashes);
        rig.announce_with_sizes(peer_b, [(hashes[0], 1024 * KIB), (hashes[1], 100)]);

        // peer_a's request is packed with the size estimate
        rig.dispatch();
        let (requested, response) = rig.take_request(peer_a).unwrap();
        assert_eq!(requested, hashes);
        response.send(Ok(PooledTransactions(vec![]))).unwrap();
        rig.next_event().unwrap();

        // peer_b's request honors the size it announced
        rig.dispatch();
        let (requested, _) = rig.take_request(peer_b).unwrap();
        assert_eq!(requested, hashes[..1]);
    }

    #[test]
    fn announced_sizes_are_capped() {
        let mut rig = Rig::new();
        let peer_a = peer(1);
        rig.add_peer(peer_a);
        let hashes = hashes(0..3);

        // absurd sizes must not break the size accounting
        rig.announce_with_sizes(
            peer_a,
            [(hashes[0], usize::MAX - 1), (hashes[1], usize::MAX), (hashes[2], 100)],
        );

        rig.dispatch();
        let (requested, response) = rig.take_request(peer_a).unwrap();
        assert_eq!(requested, hashes[..1], "an oversized transaction is requested alone");
        response.send(Err(RequestError::BadResponse)).unwrap();
        rig.next_event().unwrap();

        rig.dispatch();
        let (requested, _) = rig.take_request(peer_a).unwrap();
        assert_eq!(requested, hashes[1..2]);
    }

    #[test]
    fn hash_is_fetched_from_one_peer_at_a_time() {
        let mut rig = Rig::new();
        let peer_a = peer(1);
        let peer_b = peer(2);
        rig.add_peer(peer_a);
        rig.add_peer(peer_b);
        let txs = pooled_txs(1);
        let hashes = hashes_of(&txs);

        rig.announce(peer_a, &hashes);
        rig.announce(peer_b, &hashes);
        assert_eq!(rig.fetcher.candidate_peers(&hashes[0]), vec![peer_a, peer_b]);

        assert_eq!(rig.dispatch(), 1);
        assert_eq!(rig.fetcher.fetching_peer(&hashes[0]), Some(peer_a));
        assert!(rig.take_request(peer_b).is_none());
        // peer_b's queue entry was consumed without a request
        assert!(rig.fetcher.queued_hashes(&peer_b).is_empty());
        assert_eq!(rig.fetcher.candidate_peers(&hashes[0]), vec![peer_a, peer_b]);

        rig.respond(peer_a, txs);
        assert_eq!(rig.fetcher.num_hashes(), 0);
        assert_eq!(rig.dispatch(), 0);
    }

    #[test]
    fn failed_request_is_retried_with_alternate_peer() {
        let mut rig = Rig::new();
        let peer_a = peer(1);
        let peer_b = peer(2);
        rig.add_peer(peer_a);
        rig.add_peer(peer_b);
        let txs = pooled_txs(2);
        let hashes = hashes_of(&txs);

        rig.announce(peer_a, &hashes);
        rig.announce(peer_b, &hashes);
        rig.dispatch();
        // peer_b skipped the hashes while they were inflight to peer_a
        assert!(rig.fetcher.queued_hashes(&peer_b).is_empty());

        let FetchEvent::FetchError { peer_id, error } = rig.fail(peer_a, RequestError::Timeout)
        else {
            panic!("expected fetch error")
        };
        assert_eq!(peer_id, peer_a);
        assert!(matches!(error, RequestError::Timeout));

        // the hashes are pending again for peer_b only and queued in request order
        assert_eq!(rig.fetcher.num_pending_hashes(), 2);
        assert_eq!(rig.fetcher.candidate_peers(&hashes[0]), vec![peer_b]);
        assert_eq!(rig.fetcher.queued_hashes(&peer_b), hashes);

        assert_eq!(rig.dispatch(), 1);
        assert!(rig.take_request(peer_a).is_none());
        let event = rig.respond(peer_b, txs);
        assert!(
            matches!(event, FetchEvent::TransactionsFetched { peer_id, .. } if peer_id == peer_b)
        );
        assert_eq!(rig.fetcher.num_hashes(), 0);
    }

    #[test]
    fn empty_response_drops_peer_as_candidate() {
        let mut rig = Rig::new();
        let peer_a = peer(1);
        rig.add_peer(peer_a);
        let hashes = hashes(0..2);

        rig.announce(peer_a, &hashes);
        rig.dispatch();
        let event = rig.respond(peer_a, vec![]);
        assert!(matches!(event, FetchEvent::EmptyResponse { peer_id } if peer_id == peer_a));

        // no other peer announced the hashes, so they are dropped
        assert_eq!(rig.fetcher.num_hashes(), 0);
        assert_eq!(rig.dispatch(), 0);
    }

    #[test]
    fn partial_response_keeps_peer_for_truncated_tail() {
        let mut rig = Rig::new();
        let peer_a = peer(1);
        rig.add_peer(peer_a);
        let txs = pooled_txs(4);
        let hashes = hashes_of(&txs);

        rig.announce(peer_a, &hashes);
        rig.dispatch();

        // the first half is delivered, the rest looks like truncation
        rig.respond(peer_a, txs[..2].to_vec());
        assert_eq!(rig.fetcher.num_pending_hashes(), 2);
        assert_eq!(rig.fetcher.candidate_peers(&hashes[2]), vec![peer_a]);
        assert_eq!(rig.fetcher.queued_hashes(&peer_a), hashes[2..]);

        rig.dispatch();
        let (requested, _) = rig.take_request(peer_a).unwrap();
        assert_eq!(requested, hashes[2..]);
    }

    #[test]
    fn small_partial_response_drops_peer_for_all_undelivered_hashes() {
        let mut rig = Rig::new();
        let peer_a = peer(1);
        let peer_b = peer(2);
        rig.add_peer(peer_a);
        rig.add_peer(peer_b);
        let txs = pooled_txs(10);
        let hashes = hashes_of(&txs);

        rig.announce(peer_a, &hashes);
        rig.announce(peer_b, &hashes[..5]);
        rig.dispatch();
        assert_eq!(rig.fetcher.fetching_peer(&hashes[0]), Some(peer_a));

        // Delivering fewer than half of the requested hashes drops peer_a for the rest,
        // limiting repeated low-progress retries regardless of transaction sizes.
        rig.respond(peer_a, txs[..1].to_vec());
        assert_eq!(rig.fetcher.num_hashes(), 4, "hashes without another candidate are dropped");
        for hash in &hashes[1..5] {
            assert_eq!(rig.fetcher.candidate_peers(hash), vec![peer_b]);
        }
        assert!(rig.fetcher.queued_hashes(&peer_a).is_empty());
    }

    #[test]
    fn skipped_hashes_without_alternates_are_dropped() {
        let mut rig = Rig::new();
        let peer_a = peer(1);
        rig.add_peer(peer_a);
        let txs = pooled_txs(4);
        let hashes = hashes_of(&txs);

        rig.announce(peer_a, &hashes);
        rig.dispatch();
        rig.respond(peer_a, txs[1..3].to_vec());

        // the first hash was skipped and has no other candidate, the last one is retried
        assert_eq!(rig.fetcher.num_hashes(), 1);
        assert_eq!(rig.fetcher.candidate_peers(&hashes[3]), vec![peer_a]);
        assert!(rig.fetcher.candidate_peers(&hashes[0]).is_empty());
    }

    #[test]
    fn unsolicited_and_duplicate_transactions_are_filtered_and_reported() {
        let mut rig = Rig::new();
        let peer_a = peer(1);
        rig.add_peer(peer_a);
        let txs = pooled_txs(2);
        let hashes = hashes_of(&txs);

        rig.announce(peer_a, &hashes[..1]);
        rig.dispatch();

        let FetchEvent::TransactionsFetched { transactions, report_peer, .. } =
            rig.respond(peer_a, vec![txs[0].clone(), txs[1].clone(), txs[0].clone()])
        else {
            panic!("expected fetched transactions")
        };
        assert!(report_peer);
        assert_eq!(transactions.0, txs[..1]);
        assert_eq!(rig.fetcher.num_hashes(), 0);
    }

    #[test]
    fn response_with_only_unsolicited_transactions_is_a_bad_response() {
        let mut rig = Rig::new();
        let peer_a = peer(1);
        let peer_b = peer(2);
        rig.add_peer(peer_a);
        rig.add_peer(peer_b);
        let txs = pooled_txs(2);
        let hashes = hashes_of(&txs);

        rig.announce(peer_a, &hashes[..1]);
        rig.announce(peer_b, &hashes[..1]);
        rig.dispatch();

        let event = rig.respond(peer_a, txs[1..].to_vec());
        assert!(matches!(
            event,
            FetchEvent::FetchError { peer_id, error: RequestError::BadResponse } if peer_id == peer_a
        ));
        // treated like a failed request, peer_b gets to try
        assert_eq!(rig.fetcher.candidate_peers(&hashes[0]), vec![peer_b]);
        rig.dispatch();
        assert!(rig.take_request(peer_b).is_some());
    }

    #[test]
    fn duplicate_transactions_in_response_are_deduplicated() {
        let mut rig = Rig::new();
        let peer_a = peer(1);
        rig.add_peer(peer_a);
        let txs = pooled_txs(1);

        rig.announce(peer_a, &hashes_of(&txs));
        rig.dispatch();

        let FetchEvent::TransactionsFetched { transactions, report_peer, .. } =
            rig.respond(peer_a, vec![txs[0].clone(), txs[0].clone()])
        else {
            panic!("expected fetched transactions")
        };
        assert_eq!(transactions.len(), 1);
        assert!(!report_peer);
    }

    #[test]
    fn received_transactions_stop_tracking() {
        let mut rig = Rig::new();
        let peer_a = peer(1);
        rig.add_peer(peer_a);
        let txs = pooled_txs(3);
        let hashes = hashes_of(&txs);

        rig.announce(peer_a, &hashes[..2]);
        rig.dispatch();
        rig.announce(peer_a, &hashes[2..]);
        assert_eq!(rig.fetcher.num_fetching_hashes(), 2);
        assert_eq!(rig.fetcher.num_pending_hashes(), 1);

        // one inflight and the pending hash arrive over broadcast
        rig.fetcher.on_transactions_received([&hashes[0], &hashes[2]]);
        rig.fetcher.assert_invariants();
        assert_eq!(rig.fetcher.num_hashes(), 1);
        assert_eq!(rig.fetcher.num_fetching_hashes(), 1);

        // the response doesn't include the hash received over broadcast, which must not be
        // rescheduled, but includes the other one
        let FetchEvent::TransactionsFetched { transactions, .. } =
            rig.respond(peer_a, txs[1..2].to_vec())
        else {
            panic!("expected fetched transactions")
        };
        assert_eq!(transactions.len(), 1);
        assert_eq!(rig.fetcher.num_hashes(), 0);
        assert_eq!(rig.dispatch(), 0);
    }

    #[test]
    fn response_delivering_hash_received_over_broadcast_is_still_returned() {
        let mut rig = Rig::new();
        let peer_a = peer(1);
        rig.add_peer(peer_a);
        let txs = pooled_txs(1);
        let hashes = hashes_of(&txs);

        rig.announce(peer_a, &hashes);
        rig.dispatch();
        rig.fetcher.on_transactions_received(&hashes);

        let FetchEvent::TransactionsFetched { transactions, report_peer, .. } =
            rig.respond(peer_a, txs)
        else {
            panic!("expected fetched transactions")
        };
        assert_eq!(transactions.len(), 1);
        assert!(!report_peer, "the transaction was requested, even if it arrived elsewhere first");
        assert_eq!(rig.fetcher.num_hashes(), 0);
    }

    #[test]
    fn peer_disconnect_drops_hashes_without_alternates() {
        let mut rig = Rig::new();
        let peer_a = peer(1);
        let peer_b = peer(2);
        rig.add_peer(peer_a);
        rig.add_peer(peer_b);
        let hashes = hashes(0..2);

        rig.announce(peer_a, &hashes);
        rig.announce(peer_b, &hashes[1..]);

        rig.disconnect(peer_a);
        assert_eq!(rig.fetcher.num_hashes(), 1);
        assert_eq!(rig.fetcher.candidate_peers(&hashes[1]), vec![peer_b]);
        assert!(rig.fetcher.is_idle(&peer_a));

        rig.dispatch();
        let (requested, _) = rig.take_request(peer_b).unwrap();
        assert_eq!(requested, hashes[1..]);
    }

    #[test]
    fn inflight_request_of_disconnected_peer_is_rescheduled() {
        let mut rig = Rig::new();
        let peer_a = peer(1);
        let peer_b = peer(2);
        rig.add_peer(peer_a);
        rig.add_peer(peer_b);
        let txs = pooled_txs(2);
        let hashes = hashes_of(&txs);

        rig.announce(peer_a, &hashes);
        rig.announce(peer_b, &hashes);
        rig.dispatch();
        assert_eq!(rig.fetcher.fetching_peer(&hashes[0]), Some(peer_a));

        // dropping the session drops the pending request, which resolves it with an error
        rig.disconnect(peer_a);
        assert_eq!(
            rig.fetcher.num_fetching_hashes(),
            2,
            "hashes stay inflight until the request resolves"
        );

        let event = rig.next_event().unwrap();
        assert!(matches!(
            event,
            FetchEvent::FetchError { peer_id, error: RequestError::ChannelClosed } if peer_id == peer_a
        ));
        assert_eq!(rig.fetcher.num_pending_hashes(), 2);
        assert_eq!(rig.fetcher.candidate_peers(&hashes[0]), vec![peer_b]);

        rig.dispatch();
        let event = rig.respond(peer_b, txs);
        assert!(matches!(event, FetchEvent::TransactionsFetched { .. }));
        assert_eq!(rig.fetcher.num_hashes(), 0);
    }

    #[test]
    fn per_peer_announcement_limit_is_enforced() {
        let config =
            TransactionFetcherConfig { max_announced_hashes_per_peer: 3, ..Default::default() };
        let mut rig = Rig::with_config(config);
        let peer_a = peer(1);
        let peer_b = peer(2);
        rig.add_peer(peer_a);
        rig.add_peer(peer_b);
        let txs = pooled_txs(5);
        let hashes = hashes_of(&txs);

        rig.announce(peer_a, &hashes);
        assert_eq!(rig.fetcher.num_hashes(), 3);
        assert_eq!(rig.fetcher.queued_hashes(&peer_a), hashes[..3]);

        // the limit is per peer, another peer can still announce the dropped hashes
        rig.announce(peer_b, &hashes[3..]);
        assert_eq!(rig.fetcher.num_hashes(), 5);
        assert_eq!(rig.fetcher.candidate_peers(&hashes[4]), vec![peer_b]);

        // delivering frees up the budget of the peer
        rig.dispatch();
        rig.respond(peer_a, txs[..3].to_vec());
        rig.announce(peer_a, &hashes[3..]);
        assert_eq!(rig.fetcher.candidate_peers(&hashes[3]), vec![peer_b, peer_a]);
    }

    #[test]
    fn global_capacity_evicts_oldest_pending_hash() {
        let config = TransactionFetcherConfig {
            max_capacity_cache_txns_pending_fetch: 2,
            ..Default::default()
        };
        let mut rig = Rig::with_config(config);
        let peer_a = peer(1);
        let peer_b = peer(2);
        rig.add_peer(peer_a);
        rig.add_peer(peer_b);
        let txs = pooled_txs(5);
        let hashes = hashes_of(&txs);

        // the oldest pending hash makes room for a new one
        rig.announce(peer_a, &hashes[..3]);
        assert_eq!(rig.fetcher.num_hashes(), 2);
        assert!(rig.fetcher.candidate_peers(&hashes[0]).is_empty());
        assert_eq!(rig.fetcher.candidate_peers(&hashes[2]), vec![peer_a]);

        // hashes that are being fetched are not evicted, the announcement is dropped instead
        rig.dispatch();
        let (requested, response) = rig.take_request(peer_a).unwrap();
        assert_eq!(requested, hashes[1..3]);
        rig.announce(peer_b, &hashes[3..4]);
        assert_eq!(rig.fetcher.num_hashes(), 2);
        assert!(rig.fetcher.candidate_peers(&hashes[3]).is_empty());

        // the second hash is delivered, the third is pending again. Both peers then track one
        // hash and the announcing peer gives way, so its own older hash makes room.
        response.send(Ok(PooledTransactions(txs[1..2].to_vec()))).unwrap();
        rig.next_event().unwrap();
        assert_eq!(rig.fetcher.num_hashes(), 1);
        rig.announce(peer_b, &hashes[3..4]);
        assert_eq!(rig.fetcher.num_hashes(), 2);
        rig.announce(peer_b, &hashes[4..5]);
        assert_eq!(rig.fetcher.num_hashes(), 2);
        assert_eq!(rig.fetcher.candidate_peers(&hashes[2]), vec![peer_a]);
        assert!(rig.fetcher.candidate_peers(&hashes[3]).is_empty());
        assert_eq!(rig.fetcher.candidate_peers(&hashes[4]), vec![peer_b]);
    }

    #[test]
    fn later_announcers_are_asked_after_the_first_ones_failed() {
        let mut rig = Rig::new();
        let batch = hashes(0..3);
        let hash = batch[0];
        let peers =
            (1..=MAX_COUNT_CANDIDATE_PEERS_PER_HASH as u8 + 2).map(peer).collect::<Vec<_>>();
        for peer_id in &peers {
            rig.add_peer(*peer_id);
            rig.announce(*peer_id, &batch);
        }

        // Keep the eager sources and rotate the oldest fallback sources out for later ones.
        let candidates = peers[..MAX_COUNT_EAGER_CANDIDATE_PEERS_PER_HASH]
            .iter()
            .chain(&peers[MAX_COUNT_EAGER_CANDIDATE_PEERS_PER_HASH + 2..])
            .copied()
            .collect::<Vec<_>>();
        assert_eq!(rig.fetcher.candidate_peers(&hash), candidates);
        for (i, peer_id) in candidates.iter().enumerate() {
            let eager = i < MAX_COUNT_EAGER_CANDIDATE_PEERS_PER_HASH;
            assert_eq!(rig.fetcher.queued_hashes(peer_id).contains(&hash), eager, "peer {i}");
        }
        assert!(rig.fetcher.queued_hashes(&peers[MAX_COUNT_CANDIDATE_PEERS_PER_HASH]).is_empty());

        // the first announcer fetches the hash, retries go to the most recent announcers first
        assert_eq!(rig.dispatch(), 1);
        assert_eq!(rig.fetcher.fetching_peer(&hash), Some(peers[0]));
        rig.fail(peers[0], RequestError::Timeout);
        assert_eq!(rig.dispatch(), 1);
        assert_eq!(rig.fetcher.fetching_peer(&hash), Some(*candidates.last().unwrap()));

        // the hash is given up on once all candidates failed
        let mut order = vec![peers[0]];
        while let Some(fetching) = rig.fetcher.fetching_peer(&hash) {
            rig.fail(fetching, RequestError::Timeout);
            order.push(fetching);
            rig.dispatch();
        }
        let expected = std::iter::once(candidates[0])
            .chain(candidates[1..].iter().rev().copied())
            .collect::<Vec<_>>();
        assert_eq!(order, expected);
        assert_eq!(rig.fetcher.num_hashes(), 0);
    }

    #[test]
    fn old_request_error_does_not_settle_a_new_request_to_the_same_peer() {
        let config =
            TransactionFetcherConfig { max_inflight_requests_per_peer: 2, ..Default::default() };
        let mut rig = Rig::with_config(config);
        let p = peer(1);
        let h = hash(1);
        rig.add_peer(p);
        rig.announce(p, &[h]);
        rig.dispatch();
        let (_, first) = rig.take_request(p).unwrap();
        rig.fetcher.on_transactions_received([&h]);
        rig.announce(p, &[h]);
        rig.dispatch();
        let (_, second) = rig.take_request(p).unwrap();
        first.send(Err(RequestError::Timeout)).unwrap();
        rig.next_event().unwrap();
        assert_eq!(rig.fetcher.num_fetching_hashes(), 1);
        assert_eq!(rig.fetcher.fetching_peer(&h), Some(p));
        assert_eq!(rig.fetcher.candidate_peers(&h), vec![p]);
        second.send(Ok(PooledTransactions(vec![]))).unwrap();
        rig.next_event().unwrap();
        assert_eq!(rig.fetcher.num_hashes(), 0);
    }

    #[test]
    fn first_timeout_retries_the_only_source_once() {
        let mut rig = Rig::new();
        let p = peer(1);
        let h = hash(1);
        rig.add_peer(p);
        rig.announce(p, &[h]);
        rig.dispatch();
        rig.fail(p, RequestError::Timeout);
        assert_eq!(rig.fetcher.candidate_peers(&h), vec![p]);
        assert_eq!(rig.dispatch(), 1);
        rig.fail(p, RequestError::Timeout);
        assert_eq!(rig.fetcher.num_hashes(), 0);
        assert_eq!(rig.dispatch(), 0);
    }

    #[test]
    fn replacing_candidates_cannot_extend_the_fetch_attempt_budget() {
        let mut rig = Rig::new();
        let h = hash(1);
        for n in 1..=MAX_COUNT_CANDIDATE_PEERS_PER_HASH as u8 {
            rig.add_peer(peer(n));
            rig.announce(peer(n), &[h]);
        }
        for n in 0..MAX_FETCH_ATTEMPTS_PER_HASH {
            let newcomer = peer((n + MAX_COUNT_CANDIDATE_PEERS_PER_HASH + 1) as u8);
            rig.add_peer(newcomer);
            rig.announce(newcomer, &[h]);
            assert_eq!(rig.dispatch(), 1);
            let fetching = rig.fetcher.fetching_peer(&h).unwrap();
            rig.fail(fetching, RequestError::Timeout);
        }
        assert_eq!(rig.fetcher.num_hashes(), 0);
    }

    #[test]
    fn self_eviction_skips_shared_and_stale_announcement_entries() {
        let mut rig = Rig::with_config(TransactionFetcherConfig {
            max_capacity_cache_txns_pending_fetch: 4,
            ..Default::default()
        });
        let honest = peer(1);
        let flooder = peer(2);
        for p in [honest, flooder] {
            rig.add_peer(p);
        }
        rig.announce(honest, &hashes(0..2));
        rig.announce(flooder, &hashes(10..12));
        // The shared cursor entry must not hide newly queued exclusive victims.
        rig.announce(flooder, &[hash(0), hash(12), hash(13), hash(14), hash(15), hash(16)]);
        assert_eq!(rig.fetcher.candidate_peers(&hash(1)), vec![honest]);
        assert_eq!(rig.fetcher.candidate_peers(&hash(0)), vec![honest, flooder]);
        assert_eq!(rig.fetcher.num_hashes(), 4);
    }

    #[test]
    fn stale_queue_entries_do_not_hide_exclusive_eviction_victims() {
        let mut rig = Rig::with_config(TransactionFetcherConfig {
            max_capacity_cache_txns_pending_fetch: 16,
            ..Default::default()
        });
        let honest = peer(1);
        let flooder = peer(2);
        let newcomer = peer(3);
        for p in [honest, flooder, newcomer] {
            rig.add_peer(p);
        }
        rig.announce(honest, &[hash(0)]);
        rig.announce(flooder, &hashes(1..16));
        rig.fetcher.on_transactions_received(hashes(1..9).iter());
        rig.announce(flooder, &hashes(16..24));
        rig.announce(newcomer, &[hash(24)]);
        assert_eq!(rig.fetcher.candidate_peers(&hash(0)), vec![honest]);
        assert!(rig.fetcher.candidate_peers(&hash(9)).is_empty());
    }

    #[test]
    fn capacity_eviction_targets_the_peer_with_the_most_hashes() {
        let config = TransactionFetcherConfig {
            max_capacity_cache_txns_pending_fetch: 100,
            max_announced_hashes_per_peer: 60,
            ..Default::default()
        };
        let mut rig = Rig::with_config(config);
        let honest = peer(1);
        let flooders = [peer(2), peer(3)];
        rig.add_peer(honest);
        for peer_id in flooders {
            rig.add_peer(peer_id);
        }

        // the honest peer's hashes are the oldest when the flood fills the fetcher
        let honest_hashes = hashes(0..10);
        rig.announce(honest, &honest_hashes);
        rig.announce(flooders[0], &hashes(100..160));
        rig.announce(flooders[1], &hashes(200..260));

        assert_eq!(rig.fetcher.num_hashes(), 100);
        for hash in &honest_hashes {
            assert_eq!(rig.fetcher.candidate_peers(hash), vec![honest], "{hash} was evicted");
        }
        assert_eq!(rig.fetcher.queued_hashes(&honest), honest_hashes);

        // the flooders gave up their oldest hashes
        assert!(rig.fetcher.candidate_peers(&hashes(100..101)[0]).is_empty());
        assert_eq!(rig.fetcher.candidate_peers(&hashes(259..260)[0]), vec![flooders[1]]);
    }

    #[test]
    fn shared_queue_prefix_does_not_hide_exclusive_eviction_victim() {
        let mut rig = Rig::with_config(TransactionFetcherConfig {
            max_capacity_cache_txns_pending_fetch: 10,
            ..Default::default()
        });
        let honest = peer(1);
        let flooder = peer(2);
        let newcomer = peer(3);
        for p in [honest, flooder, newcomer] {
            rig.add_peer(p);
        }
        rig.announce(honest, &hashes(0..9));
        rig.announce(flooder, &hashes(0..8));
        rig.announce(flooder, &[hash(9)]);
        rig.announce(newcomer, &[hash(10)]);
        for h in hashes(0..8) {
            assert_eq!(rig.fetcher.candidate_peers(&h), vec![honest, flooder]);
        }
        assert!(rig.fetcher.candidate_peers(&hash(9)).is_empty());
        assert_eq!(rig.fetcher.candidate_peers(&hash(10)), vec![newcomer]);
    }

    #[test]
    fn capacity_fallback_keeps_the_announcer_ready() {
        let mut rig = Rig::with_config(TransactionFetcherConfig {
            max_capacity_cache_txns_pending_fetch: 9,
            ..Default::default()
        });
        let stalled = peer(1);
        let source = peer(2);
        let announcer = peer(3);
        for p in [stalled, source, announcer] {
            rig.add_peer(p);
        }
        rig.announce(stalled, &hashes(0..8));
        rig.dispatch();
        rig.announce(source, &[hash(8)]);
        // The global scan sees only fetching hashes. The last new hash is dropped after
        // the shared scratch entries have been moved into the announcer's real queue.
        rig.announce(announcer, &hashes(0..10));
        rig.disconnect(source);
        assert_eq!(rig.dispatch(), 1);
        assert_eq!(rig.take_request(announcer).unwrap().0, vec![hash(8)]);
    }

    #[test]
    fn capacity_eviction_preserves_coannounced_hashes() {
        let config = TransactionFetcherConfig {
            max_capacity_cache_txns_pending_fetch: 4,
            ..Default::default()
        };
        let mut rig = Rig::with_config(config);
        let honest = peer(1);
        let flooder = peer(2);
        let newcomer = peer(3);
        for peer_id in [honest, flooder, newcomer] {
            rig.add_peer(peer_id);
        }
        let shared = hash(0);
        rig.announce(honest, &[shared]);
        rig.announce(flooder, &hashes(0..4));
        rig.announce(newcomer, &[hash(4)]);
        assert_eq!(rig.fetcher.num_hashes(), 4);
        assert_eq!(rig.fetcher.candidate_peers(&shared), vec![honest, flooder]);
        assert!(rig.fetcher.candidate_peers(&hash(1)).is_empty());
        assert_eq!(rig.fetcher.candidate_peers(&hash(4)), vec![newcomer]);
        rig.dispatch();
        assert_eq!(rig.fetcher.fetching_peer(&shared), Some(honest));
        assert_eq!(rig.take_request(flooder).unwrap().0, vec![hash(2), hash(3)]);
    }

    #[test]
    fn capacity_with_only_shared_hashes_admits_new_announcements() {
        let config = TransactionFetcherConfig {
            max_capacity_cache_txns_pending_fetch: 2,
            ..Default::default()
        };
        let mut rig = Rig::with_config(config);
        for peer_id in [peer(1), peer(2), peer(3)] {
            rig.add_peer(peer_id);
        }
        for peer_id in [peer(1), peer(2)] {
            rig.announce(peer_id, &hashes(0..2));
        }
        rig.announce(peer(3), &[hash(2)]);
        assert_eq!(rig.fetcher.num_hashes(), 2);
        assert_eq!(rig.fetcher.candidate_peers(&hash(2)), vec![peer(3)]);
        assert!(rig.fetcher.candidate_peers(&hash(0)).is_empty());
        assert_eq!(rig.fetcher.candidate_peers(&hash(1)), vec![peer(1), peer(2)]);
        rig.dispatch();
        assert_eq!(rig.fetcher.num_fetching_hashes(), 2);
    }

    #[test]
    fn retracked_hash_is_not_evicted_at_its_stale_queue_position() {
        let mut rig = Rig::with_config(TransactionFetcherConfig {
            max_capacity_cache_txns_pending_fetch: 3,
            ..Default::default()
        });
        let p = peer(1);
        rig.add_peer(p);
        rig.announce(p, &hashes(0..3));
        rig.fetcher.on_transactions_received([&hash(0)]);
        rig.announce(p, &[hash(0)]);
        rig.announce(p, &[hash(3)]);
        assert_eq!(rig.fetcher.candidate_peers(&hash(0)), vec![p]);
        assert!(rig.fetcher.candidate_peers(&hash(1)).is_empty());
        rig.dispatch();
        assert_eq!(rig.take_request(p).unwrap().0, vec![hash(2), hash(0), hash(3)]);
    }

    #[test]
    fn shared_fallback_ignores_stale_tracking_generations() {
        let mut rig = Rig::with_config(TransactionFetcherConfig {
            max_capacity_cache_txns_pending_fetch: 3,
            ..Default::default()
        });
        for p in [peer(1), peer(2), peer(3)] {
            rig.add_peer(p);
        }
        for p in [peer(1), peer(2)] {
            rig.announce(p, &hashes(0..3));
        }
        rig.fetcher.on_transactions_received([&hash(0)]);
        for p in [peer(1), peer(2)] {
            rig.announce(p, &[hash(0)]);
        }
        rig.announce(peer(3), &[hash(3)]);
        assert_eq!(rig.fetcher.candidate_peers(&hash(0)), vec![peer(1), peer(2)]);
        assert!(rig.fetcher.candidate_peers(&hash(1)).is_empty());
    }

    #[test]
    fn eviction_order_stays_bounded_when_hashes_are_tracked_again() {
        let config = TransactionFetcherConfig {
            max_capacity_cache_txns_pending_fetch: 8,
            ..Default::default()
        };
        let mut rig = Rig::with_config(config);
        let peer_a = peer(1);
        rig.add_peer(peer_a);
        let batch = hashes(0..8);

        // the same hashes are given up on and announced again over and over, every round leaves
        // stale entries in the eviction order behind
        for _ in 0..10 {
            rig.announce(peer_a, &batch);
            assert_eq!(rig.fetcher.num_hashes(), 8);
            rig.dispatch();
            rig.fail(peer_a, RequestError::BadResponse);
            assert_eq!(rig.fetcher.num_hashes(), 0);
        }
        rig.announce(peer_a, &batch);
        assert_eq!(rig.fetcher.num_hashes(), 8);

        // a new hash still evicts the oldest one rather than getting dropped
        rig.announce(peer_a, &hashes(8..9));
        assert_eq!(rig.fetcher.num_hashes(), 8);
        assert!(rig.fetcher.candidate_peers(&batch[0]).is_empty());
    }

    #[test]
    fn disconnected_remembered_candidates_are_forgotten() {
        let mut rig = Rig::new();
        let hash = hash(1);
        let peers = (1..=MAX_COUNT_CANDIDATE_PEERS_PER_HASH as u8).map(peer).collect::<Vec<_>>();
        for peer_id in &peers {
            rig.add_peer(*peer_id);
            rig.announce(*peer_id, &[hash]);
        }
        let (queued, remembered) = peers.split_at(MAX_COUNT_EAGER_CANDIDATE_PEERS_PER_HASH);

        // remembered candidates have no queue entry, they are forgotten nonetheless
        for peer_id in remembered {
            rig.disconnect(*peer_id);
        }
        assert_eq!(rig.fetcher.candidate_peers(&hash), queued);

        // which frees their slots for later announcers
        let newcomer = peer(42);
        rig.add_peer(newcomer);
        rig.announce(newcomer, &[hash]);
        assert!(rig.fetcher.candidate_peers(&hash).contains(&newcomer));

        // and a hash whose last connected candidate leaves is dropped
        for peer_id in queued {
            rig.disconnect(*peer_id);
        }
        rig.disconnect(newcomer);
        assert_eq!(rig.fetcher.num_hashes(), 0);
    }

    #[test]
    fn remembered_candidates_take_over_when_queued_ones_disconnect() {
        let mut rig = Rig::new();
        let hash = hash(1);
        let peers =
            (1..=MAX_COUNT_EAGER_CANDIDATE_PEERS_PER_HASH as u8 + 1).map(peer).collect::<Vec<_>>();
        for peer_id in &peers {
            rig.add_peer(*peer_id);
            rig.announce(*peer_id, &[hash]);
        }
        let last = *peers.last().unwrap();
        assert!(rig.fetcher.queued_hashes(&last).is_empty());

        // all peers that had the hash queued disconnect before fetching it
        for peer_id in &peers[..peers.len() - 1] {
            rig.disconnect(*peer_id);
        }
        assert_eq!(rig.fetcher.candidate_peers(&hash), vec![last]);
        assert_eq!(rig.fetcher.queued_hashes(&last), vec![hash]);
        assert_eq!(rig.dispatch(), 1);
        assert_eq!(rig.fetcher.fetching_peer(&hash), Some(last));
    }

    #[test]
    fn global_inflight_limit_defers_ready_peers() {
        let config = TransactionFetcherConfig { max_inflight_requests: 1, ..Default::default() };
        let mut rig = Rig::with_config(config);
        let peer_a = peer(1);
        let peer_b = peer(2);
        rig.add_peer(peer_a);
        rig.add_peer(peer_b);

        rig.announce(peer_a, &hashes(0..2));
        rig.announce(peer_b, &hashes(2..4));

        assert_eq!(rig.dispatch(), 1);
        let (_, response) = rig.take_request(peer_a).unwrap();
        assert!(rig.take_request(peer_b).is_none());
        assert_eq!(rig.dispatch(), 0);

        // the deferred peer is served once a request slot frees up
        rig.announce(peer_a, &hashes(4..5));
        drop(response);
        rig.next_event().unwrap();
        assert_eq!(rig.dispatch(), 1);
        assert!(rig.take_request(peer_b).is_some());
        assert!(rig.take_request(peer_a).is_none(), "peer_b was ready first");
    }

    #[test]
    fn dispatch_applies_the_request_limit_independently_to_each_peer() {
        let mut rig = Rig::new();
        let peer_a = peer(1);
        let peer_b = peer(2);
        rig.add_peer(peer_a);
        rig.add_peer(peer_b);

        rig.announce(peer_a, &hashes(0..100));
        rig.announce(peer_b, &hashes(100..200));

        assert_eq!(rig.dispatch_with_budget(0), 0);

        // Each peer can use the full request limit regardless of other inflight requests.
        assert_eq!(rig.dispatch_with_budget(40), 2);
        let (requested, response_a) = rig.take_request(peer_a).unwrap();
        assert_eq!(requested, hashes(0..40));
        let (requested, _) = rig.take_request(peer_b).unwrap();
        assert_eq!(requested, hashes(100..140));
        assert_eq!(rig.fetcher.num_fetching_hashes(), 80);

        // A smaller request limit is respected too.
        response_a.send(Err(RequestError::BadResponse)).unwrap();
        rig.next_event().unwrap();
        assert_eq!(rig.dispatch_with_budget(10), 1);
        let (requested, _) = rig.take_request(peer_a).unwrap();
        assert_eq!(requested, hashes(40..50));
    }

    #[test]
    fn stalled_peers_do_not_shrink_requests_to_an_idle_peer() {
        let mut rig = Rig::new();
        for n in 0..16 {
            let p = peer(n + 1);
            rig.add_peer(p);
            rig.announce(p, &hashes(u64::from(n) * 256..u64::from(n + 1) * 256));
        }
        assert_eq!(rig.dispatch_with_budget(4096), 16);
        assert_eq!(rig.fetcher.num_fetching_hashes(), 4096);
        // All earlier peers keep their requests open. The newly ready peer still gets a
        // full request instead of being throttled by their inflight hashes.
        let honest = peer(17);
        rig.add_peer(honest);
        rig.announce(honest, &hashes(4096..4352));
        assert_eq!(rig.dispatch_with_budget(4096), 1);
        assert_eq!(rig.take_request(honest).unwrap().0, hashes(4096..4352));
    }

    #[test]
    fn full_session_channel_rolls_back_request() {
        let mut rig = Rig::new();
        let peer_a = peer(1);
        rig.add_peer_with_capacity(peer_a, 1);
        let hashes = hashes(0..3);

        // occupy the only slot of the session channel
        let (blocker, _rx) = oneshot::channel();
        rig.peers[&peer_a]
            .request_tx()
            .try_send(PeerRequest::GetPooledTransactions {
                request: GetPooledTransactions(vec![]),
                response: blocker,
            })
            .unwrap();

        rig.announce(peer_a, &hashes);
        assert_eq!(rig.dispatch(), 0);
        assert_eq!(rig.fetcher.num_pending_hashes(), 3);
        assert_eq!(rig.fetcher.num_inflight_requests(), 0);
        assert!(rig.fetcher.is_idle(&peer_a));
        assert_eq!(rig.fetcher.queued_hashes(&peer_a), hashes);

        // drain the blocker, the peer is retried on the next dispatch
        let (blocked, _) = rig.take_request(peer_a).unwrap();
        assert!(blocked.is_empty());
        assert_eq!(rig.dispatch(), 1);
        let (requested, _) = rig.take_request(peer_a).unwrap();
        assert_eq!(requested, hashes);
    }

    #[test]
    fn closed_session_channel_disconnects_peer() {
        let mut rig = Rig::new();
        let peer_a = peer(1);
        let peer_b = peer(2);
        rig.add_peer(peer_a);
        rig.add_peer(peer_b);
        let hashes = hashes(0..2);

        rig.announce(peer_a, &hashes);
        rig.announce(peer_b, &hashes[..1]);
        // the session task is gone, but the manager didn't process the disconnect yet
        rig.sessions.remove(&peer_a);

        assert_eq!(rig.dispatch(), 1);
        assert!(rig.fetcher.candidate_peers(&hashes[0]).contains(&peer_b));
        assert_eq!(rig.fetcher.candidate_peers(&hashes[0]), vec![peer_b]);
        assert_eq!(rig.fetcher.num_hashes(), 1, "hash only announced by the gone peer is dropped");
        assert_eq!(rig.fetcher.fetching_peer(&hashes[0]), Some(peer_b));
    }

    #[test]
    fn busy_peer_queue_is_compacted() {
        let config =
            TransactionFetcherConfig { max_announced_hashes_per_peer: 4, ..Default::default() };
        let mut rig = Rig::with_config(config);
        let peer_a = peer(1);
        let peer_b = peer(2);
        rig.add_peer(peer_a);
        rig.add_peer(peer_b);

        // keep peer_a busy
        rig.announce(peer_a, &hashes(0..1));
        rig.dispatch();

        // hashes announced by the busy peer are delivered by others over and over
        for round in 1..20u64 {
            let hashes = hashes(round * 10..round * 10 + 4);
            rig.announce(peer_a, &hashes);
            rig.fetcher.on_transactions_received(&hashes);
            rig.fetcher.assert_invariants();
        }
        assert!(rig.fetcher.queued_hashes(&peer_a).len() <= 8);
    }

    #[test]
    fn late_response_for_reassigned_hash_is_ignored() {
        let mut rig = Rig::new();
        let peer_a = peer(1);
        let peer_b = peer(2);
        rig.add_peer(peer_a);
        rig.add_peer(peer_b);
        let txs = pooled_txs(1);
        let hashes = hashes_of(&txs);

        rig.announce(peer_a, &hashes);
        rig.dispatch();
        // arrives over broadcast, gets announced again and is assigned to another peer
        rig.fetcher.on_transactions_received(&hashes);
        rig.announce(peer_b, &hashes);
        rig.dispatch();
        assert_eq!(rig.fetcher.fetching_peer(&hashes[0]), Some(peer_b));

        // the original request resolves without the hash, which must not touch the new fetch
        rig.respond(peer_a, vec![]);
        assert_eq!(rig.fetcher.fetching_peer(&hashes[0]), Some(peer_b));
        assert_eq!(rig.fetcher.candidate_peers(&hashes[0]), vec![peer_b]);

        rig.respond(peer_b, txs);
        assert_eq!(rig.fetcher.num_hashes(), 0);
    }

    #[test]
    fn random_operations_keep_invariants() {
        let mut rng = StdRng::seed_from_u64(0x5eed);
        let txs = pooled_txs(150);
        let all_hashes = hashes_of(&txs);
        let by_hash = txs.iter().map(|tx| (*tx.tx_hash(), tx.clone())).collect::<B256Map<_>>();
        // more peers than a hash has candidate slots, so announcers get remembered and rejected
        let peer_ids = (1..=24).map(peer).collect::<Vec<_>>();

        let config = TransactionFetcherConfig {
            max_inflight_requests: 8,
            max_inflight_requests_per_peer: 2,
            max_capacity_cache_txns_pending_fetch: 120,
            max_announced_hashes_per_peer: 40,
            ..Default::default()
        };
        let mut rig = Rig::with_config(config);
        for peer_id in &peer_ids {
            rig.add_peer_with_capacity(*peer_id, 4);
        }
        let mut connected = peer_ids.clone();
        // requests taken from the sessions that weren't answered yet
        let mut outstanding: Vec<(PeerId, Vec<TxHash>, ResponseSender)> = Vec::new();

        for _ in 0..5000 {
            match rng.random_range(0..100u32) {
                0..=39 => {
                    let Some(&peer_id) = connected.choose(&mut rng) else { continue };
                    let count = rng.random_range(1..=30);
                    let entries = all_hashes
                        .choose_multiple(&mut rng, count)
                        .map(|hash| {
                            let size = match rng.random_range(0..10) {
                                0 => 0,
                                1 => 200 * KIB,
                                _ => rng.random_range(100..1500),
                            };
                            (*hash, size)
                        })
                        .collect::<Vec<_>>();
                    rig.announce_with_sizes(peer_id, entries);
                }
                40..=59 => {
                    let budget =
                        if rng.random_bool(0.2) { rng.random_range(0..60) } else { usize::MAX };
                    rig.dispatch_with_budget(budget);
                    for peer_id in &connected {
                        while let Some((requested, response)) = rig.take_request(*peer_id) {
                            assert!(!requested.is_empty());
                            assert!(
                                requested.len() <=
                                    SOFT_LIMIT_COUNT_HASHES_IN_GET_POOLED_TRANSACTIONS_REQUEST
                            );
                            let unique = requested.iter().copied().collect::<B256Set>();
                            assert_eq!(unique.len(), requested.len(), "request has duplicates");
                            outstanding.push((*peer_id, requested, response));
                        }
                    }
                }
                60..=84 => {
                    if outstanding.is_empty() {
                        continue
                    }
                    let (_, requested, response) =
                        outstanding.swap_remove(rng.random_range(0..outstanding.len()));
                    match rng.random_range(0..10) {
                        0..=5 => {
                            let mut delivered = requested
                                .iter()
                                .filter(|_| rng.random_bool(0.7))
                                .map(|hash| by_hash[hash].clone())
                                .collect::<Vec<_>>();
                            if rng.random_bool(0.1) &&
                                let Some(duplicate) = delivered.first().cloned()
                            {
                                delivered.push(duplicate);
                            }
                            if rng.random_bool(0.1) {
                                delivered.push(txs.choose(&mut rng).unwrap().clone());
                            }
                            let _ = response.send(Ok(PooledTransactions(delivered)));
                        }
                        6..=7 => {
                            let _ = response.send(Ok(PooledTransactions(vec![])));
                        }
                        8 => {
                            let _ = response.send(Err(RequestError::Timeout));
                        }
                        _ => drop(response),
                    }
                    rig.drain_events();
                }
                85..=89 => {
                    let count = rng.random_range(1..=10);
                    let received =
                        all_hashes.choose_multiple(&mut rng, count).copied().collect::<Vec<_>>();
                    rig.fetcher.on_transactions_received(&received);
                    rig.fetcher.assert_invariants();
                }
                90..=94 => {
                    if connected.len() > 1 {
                        let peer_id = connected.swap_remove(rng.random_range(0..connected.len()));
                        rig.disconnect(peer_id);
                        // drops the response senders of the peer's outstanding requests
                        outstanding.retain(|(id, ..)| *id != peer_id);
                        rig.drain_events();
                    }
                }
                _ => {
                    if let Some(&peer_id) = peer_ids.iter().find(|id| !connected.contains(id)) {
                        rig.add_peer_with_capacity(peer_id, 4);
                        connected.push(peer_id);
                    }
                }
            }
        }

        // settle everything that is still inflight
        drop(outstanding);
        for peer_id in &connected {
            rig.disconnect(*peer_id);
        }
        rig.drain_events();
        assert_eq!(rig.fetcher.num_inflight_requests(), 0);
        assert_eq!(rig.fetcher.num_fetching_hashes(), 0);
        assert_eq!(rig.fetcher.num_hashes(), 0);
        assert_eq!(rig.fetcher.num_peers(), 0);
    }
    #[test]
    fn resolved_requests_are_yielded_one_per_poll() {
        let mut rig = Rig::new();
        let peers = (1..=3).map(peer).collect::<Vec<_>>();
        for (i, peer_id) in peers.iter().enumerate() {
            rig.add_peer(*peer_id);
            rig.announce(*peer_id, &hashes(i as u64..i as u64 + 1));
        }
        assert_eq!(rig.dispatch(), 3);
        for peer_id in &peers {
            let (_, response) = rig.take_request(*peer_id).unwrap();
            response.send(Err(RequestError::Timeout)).unwrap();
        }

        let events = rig.drain_events();
        assert_eq!(events.len(), 3);
        assert!(events.iter().all(|event| matches!(event, FetchEvent::FetchError { .. })));
        assert!(rig.next_event().is_none());
        assert_eq!(rig.fetcher.num_inflight_requests(), 0);
    }

    #[test]
    fn requests_sent_after_a_poll_register_wakers_on_the_next_poll() {
        let mut rig = Rig::new();
        let peer_a = peer(1);
        rig.add_peer(peer_a);
        let counter = Arc::new(WakeCounter::default());
        let waker = waker(counter.clone());
        let mut cx = Context::from_waker(&waker);

        assert!(rig.fetcher.poll_next_unpin(&mut cx).is_pending());
        rig.announce(peer_a, &hashes(0..1));
        assert_eq!(rig.dispatch(), 1);

        // the caller has to poll again after dispatching, only then the request is polled and
        // registers the waker that its response wakes
        assert!(rig.fetcher.poll_next_unpin(&mut cx).is_pending());
        let wakes = counter.wakes();
        let (_, response) = rig.take_request(peer_a).unwrap();
        response.send(Err(RequestError::Timeout)).unwrap();
        assert_eq!(counter.wakes(), wakes + 1);
        assert!(matches!(
            rig.fetcher.poll_next_unpin(&mut cx),
            Poll::Ready(Some(FetchEvent::FetchError { .. }))
        ));
    }

    #[test]
    fn global_capacity_bounds_tracked_hashes_across_peers() {
        let config = TransactionFetcherConfig {
            max_capacity_cache_txns_pending_fetch: 40,
            ..Default::default()
        };
        let mut rig = Rig::with_config(config);
        let peers = (1..=4).map(peer).collect::<Vec<_>>();
        for (i, peer_id) in peers.iter().enumerate() {
            rig.add_peer(*peer_id);
            rig.announce_unsized(*peer_id, &hashes(i as u64 * 16..(i as u64 + 1) * 16));
        }

        // 24 of the 64 announced hashes were evicted, spread over the peers so that every
        // peer lost its oldest hashes and kept its newest
        assert_eq!(rig.fetcher.num_hashes(), 40);
        for (i, peer_id) in peers.iter().enumerate() {
            let first = i as u64 * 16;
            assert!(rig.fetcher.candidate_peers(&hash(first)).is_empty());
            assert!(rig.fetcher.candidate_peers(&hash(first + 3)).is_empty());
            assert_eq!(rig.fetcher.candidate_peers(&hash(first + 8)), vec![*peer_id]);
            assert_eq!(rig.fetcher.candidate_peers(&hash(first + 15)), vec![*peer_id]);
        }
        rig.fetcher.assert_invariants();

        // every peer still has hashes to request, the evicted ones are skipped
        assert_eq!(rig.dispatch(), 4);
        let mut requested_total = 0;
        for peer_id in &peers {
            let (requested, _) = rig.take_request(*peer_id).unwrap();
            assert!(requested.len() <= 256);
            requested_total += requested.len();
        }
        assert_eq!(requested_total, 40, "every remaining hash is requested");
        rig.fetcher.assert_invariants();
    }

    #[test]
    fn delivery_order_does_not_matter() {
        let mut rig = Rig::new();
        let peer_a = peer(1);
        let peer_b = peer(2);
        rig.add_peer(peer_a);
        rig.add_peer(peer_b);
        let txs = pooled_txs(4);
        let hashes = hashes_of(&txs);

        rig.announce(peer_a, &hashes);
        rig.announce(peer_b, &hashes);
        rig.dispatch();

        // the last and the first hash are delivered in reverse order, the two in between were
        // skipped by peer_a
        let event = rig.respond(peer_a, vec![txs[3].clone(), txs[0].clone()]);
        let FetchEvent::TransactionsFetched { transactions, .. } = event else { panic!() };
        assert_eq!(transactions.len(), 2);
        assert_eq!(rig.fetcher.num_hashes(), 2);
        assert_eq!(rig.fetcher.candidate_peers(&hashes[1]), vec![peer_b]);
        assert_eq!(rig.fetcher.candidate_peers(&hashes[2]), vec![peer_b]);
        assert_eq!(rig.fetcher.queued_hashes(&peer_b), hashes[1..3]);
    }

    #[test]
    fn partial_delivery_with_concurrent_requests_per_peer() {
        let config =
            TransactionFetcherConfig { max_inflight_requests_per_peer: 2, ..Default::default() };
        let mut rig = Rig::with_config(config);
        let peer_a = peer(1);
        rig.add_peer(peer_a);
        let txs = pooled_txs(300);
        let hashes = hashes_of(&txs);

        rig.announce(peer_a, &hashes);
        assert_eq!(rig.dispatch(), 2);
        let (first, response_first) = rig.take_request(peer_a).unwrap();
        let (second, response_second) = rig.take_request(peer_a).unwrap();
        assert_eq!(first, hashes[..256]);
        assert_eq!(second, hashes[256..]);

        // only the last hash of the first request is delivered, the rest was skipped and has no
        // other candidate
        response_first.send(Ok(PooledTransactions(txs[255..256].to_vec()))).unwrap();
        rig.next_event().unwrap();
        assert_eq!(rig.fetcher.num_hashes(), 44);
        assert_eq!(rig.fetcher.num_fetching_hashes(), 44);
        assert!(!rig.fetcher.is_idle(&peer_a));

        response_second.send(Ok(PooledTransactions(txs[256..].to_vec()))).unwrap();
        rig.next_event().unwrap();
        assert_eq!(rig.fetcher.num_hashes(), 0);
        assert!(rig.fetcher.is_idle(&peer_a));
    }

    #[test]
    fn old_request_resolves_after_peer_reconnected_and_reannounced() {
        let mut rig = Rig::new();
        let peer_a = peer(1);
        rig.add_peer(peer_a);
        let txs = pooled_txs(2);
        let hashes = hashes_of(&txs);

        rig.announce(peer_a, &hashes);
        rig.dispatch();
        let (_, response) = rig.take_request(peer_a).unwrap();

        // the peer reconnects and announces the hashes again while the old request is pending
        rig.disconnect(peer_a);
        rig.add_peer(peer_a);
        rig.announce(peer_a, &hashes);
        assert_eq!(rig.fetcher.candidate_peers(&hashes[0]), vec![peer_a]);
        assert_eq!(rig.fetcher.num_fetching_hashes(), 2);
        assert_eq!(rig.dispatch(), 0, "the hashes are still being fetched");

        // the old request delivers one hash and skips the other, which is retried with the new
        // session
        response.send(Ok(PooledTransactions(txs[1..].to_vec()))).unwrap();
        rig.next_event().unwrap();
        assert_eq!(rig.fetcher.num_hashes(), 1);
        assert_eq!(rig.fetcher.candidate_peers(&hashes[0]), vec![peer_a]);
        assert_eq!(rig.dispatch(), 1);
        let (requested, _) = rig.take_request(peer_a).unwrap();
        assert_eq!(requested, hashes[..1]);
    }

    #[test]
    fn requeued_hashes_are_not_queued_twice() {
        let mut rig = Rig::new();
        let peer_a = peer(1);
        let peer_b = peer(2);
        rig.add_peer(peer_a);
        rig.add_peer(peer_b);
        let hashes = hashes(0..3);

        // peer_b is busy while both announce the hashes, so they stay in its queue
        rig.announce(peer_b, &hashes[..1]);
        rig.dispatch();
        rig.announce(peer_a, &hashes[1..]);
        rig.announce(peer_b, &hashes[1..]);
        rig.dispatch();
        assert_eq!(rig.fetcher.fetching_peer(&hashes[1]), Some(peer_a));
        assert_eq!(rig.fetcher.queued_hashes(&peer_b), hashes[1..]);

        rig.fail(peer_a, RequestError::Timeout);
        assert_eq!(rig.fetcher.queued_hashes(&peer_b), hashes[1..], "no duplicates");
    }
}
