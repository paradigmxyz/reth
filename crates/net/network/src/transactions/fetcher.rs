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
//! total number of pending hashes.
//!
//! When a request resolves, delivered hashes are dropped from tracking. Undelivered hashes go back
//! to pending and are queued for their remaining candidates, most recent announcers first, or are
//! dropped if none remain. The responding peer is dropped as a candidate for every undelivered
//! hash that precedes the last delivered hash of the request, since the peer skipped it on
//! purpose, and for all requested hashes if the response was empty or the request failed.
//! Undelivered hashes after the last delivered hash keep the peer as a candidate, since the
//! response was most likely truncated because it hit the response size limit.
//!
//! Request timeouts are enforced by the peer's session, which resolves the request with
//! [`RequestError::Timeout`], so the fetcher does not run any timers.
//!
//! # Bounds
//!
//! - a configurable number of inflight requests per peer, one by default, and a global inflight
//!   request limit
//! - a per peer limit on the number of tracked hashes it is a candidate for, so a single peer
//!   cannot flood the fetcher with announcements
//! - a global limit on the number of tracked hashes, at which the oldest pending hash is evicted
//! - a fixed number of candidates per hash, which also bounds the number of fetch attempts

use super::{
    config::TransactionFetcherConfig,
    constants::{
        tx_fetcher::{
            AVERAGE_BYTE_SIZE_TX_ENCODED, MAX_COUNT_CANDIDATE_PEERS_PER_HASH,
            MAX_COUNT_EAGER_CANDIDATE_PEERS_PER_HASH,
            MIN_COUNT_HASHES_IN_GET_POOLED_TRANSACTIONS_REQUEST,
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
use reth_eth_wire::{Eth68TxMetadata, GetPooledTransactions, PooledTransactions};
use reth_eth_wire_types::{EthNetworkPrimitives, NetworkPrimitives};
use reth_network_api::PeerRequest;
use reth_network_p2p::error::{RequestError, RequestResult};
use reth_network_peers::PeerId;
use reth_primitives_traits::SignedTransaction;
use smallvec::SmallVec;
use std::{
    collections::VecDeque,
    pin::Pin,
    task::{ready, Context, Poll},
};
use tokio::sync::{mpsc::error::TrySendError, oneshot, oneshot::error::RecvError};
use tracing::trace;

/// How many hashes that are currently being fetched are skipped over when looking for the oldest
/// pending hash to evict at capacity.
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
    /// the fetcher is at capacity. Entries are removed lazily, so it may contain hashes that are
    /// not tracked anymore.
    order: VecDeque<TxHash>,
    /// Fetch state of all peers that announced tracked hashes.
    peers: HashMap<PeerKey, PeerState>,
    /// Maps peer ids to their compact key.
    peer_keys: HashMap<PeerId, PeerKey, FbBuildHasher<64>>,
    /// Key assigned to the next new peer. Keys are never reused, so the candidates of a hash can
    /// safely refer to peers that disconnected in the meantime.
    next_peer_key: u32,
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
    scratch_queue: Vec<TxHash>,
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

        for (hash, metadata) in announcement {
            let size = announced_size(metadata);
            let at_capacity = self.hashes.len() >= max_total;

            // whether the hash is queued for the peer right away
            let eager = match self.hashes.entry(hash) {
                Entry::Occupied(mut occupied) => {
                    let entry = occupied.get_mut();
                    if let Some(candidate) = entry.candidate_mut(key) {
                        // announced before by this peer, keep the latest size
                        candidate.set_size(size);
                        continue
                    }
                    if entry.candidates.len() >= MAX_COUNT_CANDIDATE_PEERS_PER_HASH {
                        continue
                    }
                    if tracked >= max_per_peer {
                        dropped_peer_limit += 1;
                        continue
                    }
                    // The first announcers get the hash queued right away, later ones are only
                    // remembered and asked once the earlier ones failed to deliver.
                    let eager = entry.candidates.len() < MAX_COUNT_EAGER_CANDIDATE_PEERS_PER_HASH;
                    let candidate = if eager {
                        Candidate::queued(key, size)
                    } else {
                        Candidate::unqueued(key, size)
                    };
                    entry.candidates.push(candidate);
                    eager
                }
                Entry::Vacant(vacant) => {
                    if tracked >= max_per_peer {
                        dropped_peer_limit += 1;
                        continue
                    }
                    if at_capacity {
                        // the evicted hash may be one of this peer's, so its count is synced
                        if let Some(peer) = self.peers.get_mut(&key) {
                            peer.tracked = tracked;
                        }
                        if self.evict_oldest_pending() {
                            evicted_at_capacity += 1;
                            tracked = self.peers.get(&key).map_or(tracked, |peer| peer.tracked);
                        } else {
                            dropped_at_capacity += 1;
                            continue
                        }
                        self.hashes.insert(hash, TxEntry::new(key, size));
                    } else {
                        vacant.insert(TxEntry::new(key, size));
                    }
                    self.record_order(hash);
                    true
                }
            };

            tracked += 1;
            if eager {
                queue.push(hash);
            }
        }

        let queued = queue.len();
        if let Some(peer) = self.peers.get_mut(&key) {
            peer.tracked = tracked;
            for hash in &queue {
                peer.push_queue(&self.hashes, key, *hash, false, max_per_peer);
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
            self.mark_ready(key, false);
        }
    }

    /// Sends `GetPooledTransactions` requests to idle peers that have queued hashes.
    ///
    /// Stops when the global inflight request limit is reached. `max_fetching_hashes` is the
    /// number of hashes the caller can handle being inflight at once: requests are cut down to
    /// what that budget leaves, but never below
    /// [`MIN_COUNT_HASHES_IN_GET_POOLED_TRANSACTIONS_REQUEST`], so that peers that don't respond
    /// to their requests can't stop fetching from the others. A budget of zero sends nothing.
    ///
    /// Returns the number of requests sent. New requests are only polled by the next call to
    /// [`Stream::poll_next`], so the caller must poll the fetcher again if any were sent.
    pub fn dispatch(
        &mut self,
        peers: &HashMap<PeerId, PeerMetadata<N>, FbBuildHasher<64>>,
        max_fetching_hashes: usize,
    ) -> usize {
        if max_fetching_hashes == 0 {
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

            let limit = max_fetching_hashes
                .saturating_sub(self.num_fetching)
                .max(MIN_COUNT_HASHES_IN_GET_POOLED_TRANSACTIONS_REQUEST)
                .min(max_fetching_hashes)
                .min(SOFT_LIMIT_COUNT_HASHES_IN_GET_POOLED_TRANSACTIONS_REQUEST);
            let hashes = self.pack_request(key, limit);
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
                        peer_id,
                        hashes,
                        response: rx,
                    });
                    sent += 1;
                    // the peer may be allowed more than one inflight request
                    self.mark_ready(key, false);
                }
                Err(err) => {
                    self.metrics.egress_peer_channel_full.increment(1);
                    self.unpack_request(key, hashes);
                    match err {
                        TrySendError::Full(_) => retry.push(key),
                        TrySendError::Closed(_) => self.on_peer_disconnected(&peer_id),
                    }
                }
            }
        }

        for key in retry {
            self.mark_ready(key, false);
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
        let Some(peer) = self.peers.remove(&key) else { return };

        let mut dropped = 0u64;
        let mut requeued = SmallVec::<[PeerKey; MAX_COUNT_CANDIDATE_PEERS_PER_HASH]>::new();
        for hash in peer.queue {
            let Some(entry) = self.hashes.get_mut(&hash) else { continue };
            entry.candidates.retain(|candidate| candidate.peer != key);
            if entry.fetching_by.is_some() {
                continue
            }
            if entry.candidates.is_empty() {
                self.hashes.remove(&hash);
                dropped += 1;
                continue
            }
            // the hash must stay queued for at least one candidate
            if !entry.candidates.iter().any(|candidate| candidate.is_queued()) {
                let targets = entry.unqueued_candidates();
                self.requeue(hash, targets, &mut requeued);
            }
        }

        if dropped > 0 {
            self.metrics.hashes_dropped_no_candidate_peers.increment(dropped);
        }
        // retried hashes go to the most recent announcers first
        for key in requeued {
            self.mark_ready(key, true);
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
    fn mark_ready(&mut self, key: PeerKey, first: bool) {
        if let Some(peer) = self.peers.get_mut(&key) &&
            !peer.ready &&
            !peer.queue.is_empty() &&
            peer.inflight < self.config.max_inflight_requests_per_peer
        {
            peer.ready = true;
            if first {
                self.ready.push_front(key);
            } else {
                self.ready.push_back(key);
            }
        }
    }

    /// Queues the hash for the peer, at the back or the front of its queue, and marks it as
    /// queued in the hash's candidate entry.
    fn enqueue(&mut self, key: PeerKey, hash: TxHash, front: bool) {
        let max_per_peer = self.config.max_announced_hashes_per_peer as usize;
        let Some(peer) = self.peers.get_mut(&key) else { return };
        if let Some(candidate) =
            self.hashes.get_mut(&hash).and_then(|entry| entry.candidate_mut(key))
        {
            candidate.set_queued(true);
        }
        peer.push_queue(&self.hashes, key, hash, front, max_per_peer);
    }

    /// Records a newly tracked hash in the eviction order.
    fn record_order(&mut self, hash: TxHash) {
        let max_len = 2 * self.config.max_capacity_cache_txns_pending_fetch as usize;
        if self.order.len() >= max_len {
            let hashes = &self.hashes;
            self.order.retain(|hash| hashes.contains_key(hash));
        }
        self.order.push_back(hash);
    }

    /// Evicts the oldest pending hash to make room for a new one.
    ///
    /// Returns `false` if only hashes that are being fetched were found among the oldest tracked
    /// hashes.
    fn evict_oldest_pending(&mut self) -> bool {
        let mut attempts = 0;
        while let Some(hash) = self.order.pop_front() {
            let Some(entry) = self.hashes.get(&hash) else { continue };
            if entry.fetching_by.is_some() {
                self.order.push_back(hash);
                attempts += 1;
                if attempts >= MAX_EVICTION_ATTEMPTS {
                    return false
                }
                continue
            }
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
    fn pack_request(&mut self, key: PeerKey, max_hashes: usize) -> Vec<TxHash> {
        let max_bytes =
            self.config.soft_limit_byte_size_pooled_transactions_response_on_pack_request;
        let Some(peer) = self.peers.get_mut(&key) else { return Vec::new() };

        let mut hashes = Vec::with_capacity(peer.queue.len().min(max_hashes));
        let mut bytes = 0usize;

        while let Some(hash) = peer.queue.pop_front() {
            // skip hashes that were delivered in the meantime, or that this peer is no longer a
            // candidate for
            let Some(entry) = self.hashes.get_mut(&hash) else { continue };
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
                peer.queue.push_front(hash);
                break
            }

            entry.fetching_by = Some(key);
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
    fn unpack_request(&mut self, key: PeerKey, hashes: Vec<TxHash>) {
        for hash in hashes.into_iter().rev() {
            if let Some(entry) = self.hashes.get_mut(&hash) &&
                entry.fetching_by == Some(key)
            {
                entry.fetching_by = None;
                self.num_fetching -= 1;
                self.enqueue(key, hash, true);
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
            self.enqueue(key, hash, true);
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
        let ResolvedRequest { peer: key, peer_id, hashes: requested, result } = resolved;

        if let Some(peer) = self.peers.get_mut(&key) {
            peer.inflight = peer.inflight.saturating_sub(1);
        }

        let mut delivered = std::mem::take(&mut self.scratch_delivered);
        delivered.clear();

        let outcome = match result {
            Ok(Ok(mut transactions)) => {
                let mut requested_set = std::mem::take(&mut self.scratch_requested);
                requested_set.clear();
                requested_set.extend(requested.iter().copied());
                let unsolicited =
                    verify_response(&mut transactions, &requested_set, &mut delivered);
                self.scratch_requested = requested_set;
                Ok((transactions, unsolicited))
            }
            Ok(Err(error)) => Err(error),
            // the session dropped the request
            Err(_) => Err(RequestError::ChannelClosed),
        };

        self.on_delivery(key, &requested, &delivered);
        self.scratch_delivered = delivered;
        self.mark_ready(key, false);

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
    fn on_delivery(&mut self, key: PeerKey, requested: &[TxHash], delivered: &B256Set) {
        // Position right after the last delivered hash. Undelivered hashes before it were skipped
        // by the peer on purpose, so the peer is dropped as a candidate for them. Undelivered
        // hashes after it were most likely truncated because the response hit the size limit, so
        // the peer stays a candidate. If nothing was delivered, the peer is dropped for all.
        let cutoff = requested
            .iter()
            .rposition(|hash| delivered.contains(hash))
            .map_or(requested.len(), |idx| idx + 1);

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
            if entry.fetching_by != Some(key) {
                continue
            }
            entry.fetching_by = None;
            self.num_fetching -= 1;

            let peers = &mut self.peers;
            entry.candidates.retain(|candidate| {
                // disconnected peers are pruned as well
                let Some(peer) = peers.get_mut(&candidate.peer) else { return false };
                if idx < cutoff && candidate.peer == key {
                    peer.tracked = peer.tracked.saturating_sub(1);
                    return false
                }
                true
            });

            if entry.candidates.is_empty() {
                self.hashes.remove(hash);
                dropped += 1;
                continue
            }

            let targets = entry.unqueued_candidates();
            self.requeue(*hash, targets, &mut requeued);
        }

        if dropped > 0 {
            self.metrics.hashes_dropped_no_candidate_peers.increment(dropped);
            trace!(target: "net::tx", dropped, "dropped hashes that no peer is left to fetch from");
        }

        // retried hashes go to the most recent announcers first
        for key in requeued {
            self.mark_ready(key, true);
        }
    }
}

#[cfg(any(test, feature = "test-utils"))]
impl<N: NetworkPrimitives> TransactionFetcher<N> {
    /// Returns the connected peers that are candidates for the hash, in announcement order.
    pub fn candidate_peers(&self, hash: &TxHash) -> Vec<PeerId> {
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
    pub fn fetching_peer(&self, hash: &TxHash) -> Option<PeerId> {
        let key = self.hashes.get(hash)?.fetching_by?;
        self.peers.get(&key).map(|peer| peer.peer_id)
    }

    /// Returns the hashes queued for the peer, oldest first. May include hashes that are not
    /// tracked anymore or are being fetched elsewhere.
    pub fn queued_hashes(&self, peer_id: &PeerId) -> Vec<TxHash> {
        self.peer_keys
            .get(peer_id)
            .and_then(|key| self.peers.get(key))
            .map(|peer| peer.queue.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Returns the number of peers the fetcher tracks.
    pub fn num_peers(&self) -> usize {
        self.peers.len()
    }

    /// Panics if the internal bookkeeping is inconsistent.
    pub fn assert_invariants(&self) {
        let fetching = self.hashes.values().filter(|entry| entry.fetching_by.is_some()).count();
        assert_eq!(fetching, self.num_fetching, "fetching counter out of sync");

        let ordered = self.order.iter().copied().collect::<B256Set>();
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
                (*key, peer.queue.iter().copied().collect::<B256Set>())
            })
            .collect::<HashMap<_, _>>();
        for (hash, entry) in &self.hashes {
            let mut fetchable = entry.fetching_by.is_some();
            for candidate in &entry.candidates {
                let Some(queue) = queued.get(&candidate.peer) else { continue };
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

/// State of a tracked hash.
#[derive(Debug)]
struct TxEntry {
    /// Peers that announced the hash and may be asked for it, oldest first.
    candidates: SmallVec<[Candidate; MAX_COUNT_CANDIDATE_PEERS_PER_HASH]>,
    /// The peer currently fetching the transaction, if any.
    fetching_by: Option<PeerKey>,
}

impl TxEntry {
    /// A new entry for a hash that is queued for the announcing peer.
    fn new(peer: PeerKey, size: u32) -> Self {
        let mut candidates = SmallVec::new();
        candidates.push(Candidate::queued(peer, size));
        Self { candidates, fetching_by: None }
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
    queue: VecDeque<TxHash>,
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
    /// queue. Duplicates are skipped when the queue is drained, so they only cost memory.
    fn push_queue(
        &mut self,
        hashes: &B256Map<TxEntry>,
        key: PeerKey,
        hash: TxHash,
        front: bool,
        max_tracked: usize,
    ) {
        if self.queue.len() >= 2 * max_tracked {
            let mut seen = B256Set::with_capacity_and_hasher(self.tracked, Default::default());
            self.queue.retain(|hash| {
                hashes.get(hash).is_some_and(|entry| entry.has_candidate(key)) && seen.insert(*hash)
            });
        }
        if front {
            self.queue.push_front(hash);
        } else {
            self.queue.push_back(hash);
        }
    }
}

/// An inflight `GetPooledTransactions` request.
#[derive(Debug)]
struct InflightRequest<T> {
    peer: PeerKey,
    peer_id: PeerId,
    /// The requested hashes, in request order.
    hashes: Vec<TxHash>,
    response: oneshot::Receiver<RequestResult<PooledTransactions<T>>>,
}

impl<T> Future for InflightRequest<T> {
    type Output = ResolvedRequest<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let result = ready!(this.response.poll_unpin(cx));
        Poll::Ready(ResolvedRequest {
            peer: this.peer,
            peer_id: this.peer_id,
            hashes: std::mem::take(&mut this.hashes),
            result,
        })
    }
}

/// A resolved `GetPooledTransactions` request.
#[derive(Debug)]
struct ResolvedRequest<T> {
    peer: PeerKey,
    peer_id: PeerId,
    hashes: Vec<TxHash>,
    result: Result<RequestResult<PooledTransactions<T>>, RecvError>,
}

/// Returns the announced size of a transaction, capped at the response soft limit since a peer
/// can't serve anything bigger, or 0 if the announcement carried no size.
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

    /// A peer id for tests that need more than 255 peers.
    fn peer_n(n: u64) -> PeerId {
        let mut bytes = [0u8; 64];
        bytes[..8].copy_from_slice(&n.to_be_bytes());
        PeerId::new(bytes)
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
        /// Whether every operation checks the fetcher's invariants, which is too slow for large
        /// workloads.
        check_invariants: bool,
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
                check_invariants: true,
            }
        }

        fn verify(&self) {
            if self.check_invariants {
                self.fetcher.assert_invariants();
            }
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

        fn dispatch_with_budget(&mut self, max_fetching_hashes: usize) -> usize {
            let sent = self.fetcher.dispatch(&self.peers, max_fetching_hashes);
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
        let txs = pooled_txs(2);
        let hashes = hashes_of(&txs);

        rig.announce(peer_a, &hashes);
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

        let FetchEvent::TransactionsFetched { peer_id, transactions, report_peer } =
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
        assert!(rig.next_event().is_none());
    }

    #[test]
    fn duplicate_announcements_from_same_peer_are_ignored() {
        let mut rig = Rig::new();
        let peer_a = peer(1);
        rig.add_peer(peer_a);
        let hashes = hashes(0..3);

        rig.announce(peer_a, &hashes);
        rig.announce(peer_a, &hashes);

        assert_eq!(rig.fetcher.num_hashes(), 3);
        assert_eq!(rig.fetcher.queued_hashes(&peer_a), hashes);
        assert_eq!(rig.fetcher.candidate_peers(&hashes[0]), vec![peer_a]);
    }

    #[test]
    fn requests_preserve_announcement_order() {
        let mut rig = Rig::new();
        let peer_a = peer(1);
        rig.add_peer(peer_a);

        rig.announce(peer_a, &hashes(10..13));
        rig.announce(peer_a, &hashes(0..3));

        rig.dispatch();
        let (requested, _) = rig.take_request(peer_a).unwrap();
        assert_eq!(requested, [hashes(10..13), hashes(0..3)].concat());
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
        response.send(Err(RequestError::Timeout)).unwrap();
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
            // a timeout keeps other hashes pending but drops the peer for the requested ones
            response.send(Err(RequestError::Timeout)).unwrap();
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
        response.send(Err(RequestError::Timeout)).unwrap();
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
        let txs = pooled_txs(3);
        let hashes = hashes_of(&txs);

        rig.announce(peer_a, &hashes);
        rig.dispatch();

        // only the first hash is delivered, the rest looks like truncation
        rig.respond(peer_a, txs[..1].to_vec());
        assert_eq!(rig.fetcher.num_pending_hashes(), 2);
        assert_eq!(rig.fetcher.candidate_peers(&hashes[1]), vec![peer_a]);
        assert_eq!(rig.fetcher.queued_hashes(&peer_a), hashes[1..]);

        rig.dispatch();
        let (requested, _) = rig.take_request(peer_a).unwrap();
        assert_eq!(requested, hashes[1..]);
    }

    #[test]
    fn partial_response_drops_peer_for_skipped_hashes() {
        let mut rig = Rig::new();
        let peer_a = peer(1);
        let peer_b = peer(2);
        rig.add_peer(peer_a);
        rig.add_peer(peer_b);
        let txs = pooled_txs(3);
        let hashes = hashes_of(&txs);

        rig.announce(peer_a, &hashes);
        rig.announce(peer_b, &hashes);
        rig.dispatch();

        // peer_a delivers the last hash only, so it deliberately skipped the first two
        rig.respond(peer_a, txs[2..].to_vec());
        assert_eq!(rig.fetcher.num_pending_hashes(), 2);
        assert_eq!(rig.fetcher.candidate_peers(&hashes[0]), vec![peer_b]);
        assert_eq!(rig.fetcher.candidate_peers(&hashes[1]), vec![peer_b]);

        rig.dispatch();
        assert!(rig.take_request(peer_a).is_none());
        let (requested, _) = rig.take_request(peer_b).unwrap();
        assert_eq!(requested, hashes[..2]);
    }

    #[test]
    fn skipped_hashes_without_alternates_are_dropped() {
        let mut rig = Rig::new();
        let peer_a = peer(1);
        rig.add_peer(peer_a);
        let txs = pooled_txs(3);
        let hashes = hashes_of(&txs);

        rig.announce(peer_a, &hashes);
        rig.dispatch();
        rig.respond(peer_a, txs[1..2].to_vec());

        // the first hash was skipped and has no other candidate, the last one is retried
        assert_eq!(rig.fetcher.num_hashes(), 1);
        assert_eq!(rig.fetcher.candidate_peers(&hashes[2]), vec![peer_a]);
        assert!(rig.fetcher.candidate_peers(&hashes[0]).is_empty());
    }

    #[test]
    fn unsolicited_transactions_are_filtered_and_reported() {
        let mut rig = Rig::new();
        let peer_a = peer(1);
        rig.add_peer(peer_a);
        let txs = pooled_txs(2);
        let hashes = hashes_of(&txs);

        rig.announce(peer_a, &hashes[..1]);
        rig.dispatch();

        let FetchEvent::TransactionsFetched { transactions, report_peer, .. } =
            rig.respond(peer_a, txs.clone())
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
    fn reconnected_peer_starts_fresh() {
        let mut rig = Rig::new();
        let peer_a = peer(1);
        rig.add_peer(peer_a);
        let hashes = hashes(0..2);

        rig.announce(peer_a, &hashes);
        rig.dispatch();
        rig.disconnect(peer_a);
        rig.add_peer(peer_a);

        // the pending request of the old session resolves after the peer reconnected
        rig.next_event().unwrap();
        assert_eq!(rig.fetcher.num_hashes(), 0);

        rig.announce(peer_a, &hashes);
        assert_eq!(rig.dispatch(), 1);
        let (requested, _) = rig.take_request(peer_a).unwrap();
        assert_eq!(requested, hashes);
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

        // the second hash is delivered, the third is pending again and gets evicted once the
        // capacity is needed
        response.send(Ok(PooledTransactions(txs[1..2].to_vec()))).unwrap();
        rig.next_event().unwrap();
        assert_eq!(rig.fetcher.num_hashes(), 1);
        rig.announce(peer_b, &hashes[3..4]);
        assert_eq!(rig.fetcher.num_hashes(), 2);
        rig.announce(peer_b, &hashes[4..5]);
        assert_eq!(rig.fetcher.num_hashes(), 2);
        assert!(rig.fetcher.candidate_peers(&hashes[2]).is_empty());
        assert_eq!(rig.fetcher.candidate_peers(&hashes[3]), vec![peer_b]);
        assert_eq!(rig.fetcher.candidate_peers(&hashes[4]), vec![peer_b]);
    }

    #[test]
    fn later_announcers_are_asked_after_the_first_ones_failed() {
        let mut rig = Rig::new();
        let hash = hash(1);
        let peers =
            (1..=MAX_COUNT_CANDIDATE_PEERS_PER_HASH as u8 + 2).map(peer).collect::<Vec<_>>();
        for peer_id in &peers {
            rig.add_peer(*peer_id);
            rig.announce(*peer_id, &[hash]);
        }

        // the first announcers get the hash queued, the following ones are only remembered and
        // announcements beyond the candidate limit are ignored
        let candidates = &peers[..MAX_COUNT_CANDIDATE_PEERS_PER_HASH];
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
        let mut attempts = 1;
        while let Some(fetching) = rig.fetcher.fetching_peer(&hash) {
            rig.fail(fetching, RequestError::Timeout);
            attempts += 1;
            rig.dispatch();
        }
        assert_eq!(attempts, MAX_COUNT_CANDIDATE_PEERS_PER_HASH);
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
    fn per_peer_inflight_limit_allows_concurrent_requests() {
        let config =
            TransactionFetcherConfig { max_inflight_requests_per_peer: 2, ..Default::default() };
        let mut rig = Rig::with_config(config);
        let peer_a = peer(1);
        rig.add_peer(peer_a);
        let hashes = hashes(0..300);

        rig.announce_unsized(peer_a, &hashes);
        assert_eq!(rig.dispatch(), 2);

        let (first, _) = rig.take_request(peer_a).unwrap();
        let (second, _) = rig.take_request(peer_a).unwrap();
        assert_eq!(first, hashes[..256]);
        assert_eq!(second, hashes[256..]);
        assert!(!rig.fetcher.is_idle(&peer_a));
    }

    #[test]
    fn dispatch_respects_hash_budget() {
        let mut rig = Rig::new();
        let peer_a = peer(1);
        let peer_b = peer(2);
        rig.add_peer(peer_a);
        rig.add_peer(peer_b);

        rig.announce(peer_a, &hashes(0..100));
        rig.announce(peer_b, &hashes(100..200));

        assert_eq!(rig.dispatch_with_budget(0), 0);

        // requests are cut down to what the budget leaves, and a used up budget doesn't stop
        // other peers, they get the minimum request instead
        assert_eq!(rig.dispatch_with_budget(40), 2);
        let (requested, response_a) = rig.take_request(peer_a).unwrap();
        assert_eq!(requested, hashes(0..40));
        let (requested, _) = rig.take_request(peer_b).unwrap();
        assert_eq!(requested.len(), MIN_COUNT_HASHES_IN_GET_POOLED_TRANSACTIONS_REQUEST);
        assert_eq!(
            rig.fetcher.num_fetching_hashes(),
            40 + MIN_COUNT_HASHES_IN_GET_POOLED_TRANSACTIONS_REQUEST
        );

        // a budget below the minimum caps the request
        response_a.send(Err(RequestError::Timeout)).unwrap();
        rig.next_event().unwrap();
        assert_eq!(rig.dispatch_with_budget(10), 1);
        let (requested, _) = rig.take_request(peer_a).unwrap();
        assert_eq!(requested, hashes(40..50));
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
    fn verify_response_filters_unsolicited_and_duplicates() {
        let txs = pooled_txs(2);
        let requested = [hash(1), *txs[0].tx_hash(), hash(2)];

        let mut response = PooledTransactions(vec![txs[0].clone(), txs[1].clone(), txs[0].clone()]);
        let mut delivered = B256Set::default();
        let unsolicited =
            verify_response(&mut response, &requested.into_iter().collect(), &mut delivered);

        assert_eq!(response.0, txs[..1]);
        assert_eq!(delivered.into_iter().collect::<Vec<_>>(), vec![*txs[0].tx_hash()]);
        assert_eq!(unsolicited, 1);
    }

    #[test]
    fn random_operations_keep_invariants() {
        let mut rng = StdRng::seed_from_u64(0x5eed);
        let txs = pooled_txs(150);
        let all_hashes = hashes_of(&txs);
        let by_hash = txs.iter().map(|tx| (*tx.tx_hash(), tx.clone())).collect::<B256Map<_>>();
        let peer_ids = (1..=6).map(peer).collect::<Vec<_>>();

        let config = TransactionFetcherConfig {
            max_inflight_requests: 5,
            max_inflight_requests_per_peer: 2,
            max_capacity_cache_txns_pending_fetch: 100,
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
            rig.sessions.remove(peer_id);
        }
        rig.drain_events();
        assert_eq!(rig.fetcher.num_inflight_requests(), 0);
        assert_eq!(rig.fetcher.num_fetching_hashes(), 0);
    }
    #[test]
    fn stream_is_pending_without_requests_and_never_terminates() {
        let mut rig = Rig::new();
        let peer_a = peer(1);
        rig.add_peer(peer_a);
        let txs = pooled_txs(1);
        let mut cx = Context::from_waker(noop_waker_ref());

        assert!(rig.fetcher.poll_next_unpin(&mut cx).is_pending());

        rig.announce(peer_a, &hashes_of(&txs));
        rig.dispatch();
        rig.respond(peer_a, txs);

        // the stream stays open once all requests resolved
        assert!(rig.fetcher.poll_next_unpin(&mut cx).is_pending());
        assert_eq!(rig.fetcher.num_inflight_requests(), 0);
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
    fn response_wakes_the_polling_task() {
        let mut rig = Rig::new();
        let peer_a = peer(1);
        rig.add_peer(peer_a);
        rig.announce(peer_a, &hashes(0..1));
        rig.dispatch();

        let counter = Arc::new(WakeCounter::default());
        let waker = waker(counter.clone());
        let mut cx = Context::from_waker(&waker);
        assert!(rig.fetcher.poll_next_unpin(&mut cx).is_pending());
        // `FuturesUnordered` wakes the task once itself after polling newly added requests, so
        // only the change matters
        let wakes = counter.wakes();

        let (_, response) = rig.take_request(peer_a).unwrap();
        response.send(Err(RequestError::Timeout)).unwrap();
        assert_eq!(
            counter.wakes(),
            wakes + 1,
            "the response wakes the task that polled the fetcher"
        );
        assert!(matches!(
            rig.fetcher.poll_next_unpin(&mut cx),
            Poll::Ready(Some(FetchEvent::FetchError { .. }))
        ));
        rig.fetcher.assert_invariants();
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
    fn announcement_flood_from_one_peer_is_bounded() {
        let mut rig = Rig::new();
        rig.check_invariants = false;
        let peer_a = peer(1);
        rig.add_peer(peer_a);
        let limit = rig.fetcher.config().max_announced_hashes_per_peer as usize;

        // 50 full announcements of unique hashes
        for batch in 0..50u64 {
            rig.announce_unsized(peer_a, &hashes(batch * 4096..(batch + 1) * 4096));
        }
        assert_eq!(rig.fetcher.num_hashes(), limit);
        assert!(rig.fetcher.queued_hashes(&peer_a).len() <= 2 * limit);
        rig.fetcher.assert_invariants();

        // settled hashes free the peer's budget again
        assert_eq!(rig.dispatch(), 1);
        let (requested, response) = rig.take_request(peer_a).unwrap();
        assert_eq!(requested.len(), 256);
        response.send(Err(RequestError::Timeout)).unwrap();
        rig.next_event().unwrap();
        assert_eq!(rig.fetcher.num_hashes(), limit - 256);

        rig.announce_unsized(peer_a, &hashes(1_000_000..1_000_300));
        assert_eq!(rig.fetcher.num_hashes(), limit);
        rig.fetcher.assert_invariants();
    }

    #[test]
    fn global_capacity_bounds_tracked_hashes_across_peers() {
        let config = TransactionFetcherConfig {
            max_capacity_cache_txns_pending_fetch: 1000,
            ..Default::default()
        };
        let mut rig = Rig::with_config(config);
        rig.check_invariants = false;
        let peers = (1..=4).map(peer).collect::<Vec<_>>();
        for (i, peer_id) in peers.iter().enumerate() {
            rig.add_peer(*peer_id);
            rig.announce_unsized(*peer_id, &hashes(i as u64 * 400..(i as u64 + 1) * 400));
        }

        // the oldest 600 of the 1600 announced hashes were evicted for the newest ones
        assert_eq!(rig.fetcher.num_hashes(), 1000);
        assert!(rig.fetcher.candidate_peers(&hash(0)).is_empty());
        assert!(rig.fetcher.candidate_peers(&hash(599)).is_empty());
        assert_eq!(rig.fetcher.candidate_peers(&hash(600)), vec![peers[1]]);
        assert_eq!(rig.fetcher.candidate_peers(&hash(1599)), vec![peers[3]]);
        rig.fetcher.assert_invariants();

        // the first peer only has stale queue entries left
        assert_eq!(rig.dispatch(), 3);
        assert!(rig.take_request(peers[0]).is_none());
        rig.fetcher.assert_invariants();
    }

    #[test]
    fn many_peers_announcing_the_same_hashes() {
        let mut rig = Rig::new();
        rig.check_invariants = false;
        let hashes = hashes(0..100);
        let peers = (0..200).map(peer_n).collect::<Vec<_>>();
        for peer_id in &peers {
            rig.add_peer_with_capacity(*peer_id, 1);
            rig.announce(*peer_id, &hashes);
        }

        // only the first announcers of a hash are its candidates
        let candidates = &peers[..MAX_COUNT_CANDIDATE_PEERS_PER_HASH];
        assert_eq!(rig.fetcher.num_hashes(), 100);
        assert_eq!(rig.fetcher.candidate_peers(&hashes[0]), candidates);
        rig.fetcher.assert_invariants();

        // one request at a time, retried with the next candidate until all are exhausted
        let mut order = Vec::new();
        for _ in candidates {
            assert_eq!(rig.dispatch(), 1);
            let fetching = rig.fetcher.fetching_peer(&hashes[0]).unwrap();
            let (requested, response) = rig.take_request(fetching).unwrap();
            assert_eq!(requested.len(), 100);
            response.send(Err(RequestError::Timeout)).unwrap();
            rig.next_event().unwrap();
            order.push(fetching);
        }
        let expected =
            [&candidates[..1], &candidates[1..].iter().rev().copied().collect::<Vec<_>>()].concat();
        assert_eq!(order, expected, "the first announcer is asked first, then the most recent");
        assert_eq!(rig.fetcher.num_hashes(), 0);
        assert_eq!(rig.dispatch(), 0);
        rig.fetcher.assert_invariants();
    }

    #[test]
    fn global_inflight_limit_serves_peers_in_announcement_order() {
        let mut rig = Rig::new();
        rig.check_invariants = false;
        let max_inflight = rig.fetcher.config().max_inflight_requests as usize;
        let peers = (0..max_inflight as u64 + 20).map(peer_n).collect::<Vec<_>>();
        for (i, peer_id) in peers.iter().enumerate() {
            rig.add_peer(*peer_id);
            rig.announce(*peer_id, &hashes(i as u64 * 10..(i as u64 + 1) * 10));
        }

        assert_eq!(rig.dispatch(), max_inflight);
        assert!(peers[..max_inflight].iter().all(|peer_id| !rig.fetcher.is_idle(peer_id)));
        assert!(peers[max_inflight..].iter().all(|peer_id| rig.fetcher.is_idle(peer_id)));
        assert_eq!(rig.fetcher.num_fetching_hashes(), max_inflight * 10);
        rig.fetcher.assert_invariants();

        // the remaining peers are served once requests resolve
        for peer_id in &peers[..max_inflight] {
            rig.fail(*peer_id, RequestError::Timeout);
        }
        assert_eq!(rig.dispatch(), 20);
        assert_eq!(rig.fetcher.num_inflight_requests(), 20);
        rig.fetcher.assert_invariants();
    }

    #[test]
    fn huge_unsolicited_response_is_filtered() {
        let mut rig = Rig::new();
        let peer_a = peer(1);
        rig.add_peer(peer_a);
        let txs = pooled_txs(4097);

        rig.announce(peer_a, &hashes_of(&txs[..1]));
        rig.dispatch();

        let FetchEvent::TransactionsFetched { transactions, report_peer, .. } =
            rig.respond(peer_a, txs.clone())
        else {
            panic!("expected fetched transactions")
        };
        assert_eq!(transactions.0, txs[..1]);
        assert!(report_peer);
        assert_eq!(rig.fetcher.num_hashes(), 0);
    }

    #[test]
    fn peer_churn_leaves_no_state_behind() {
        let mut rig = Rig::new();
        let txs = pooled_txs(40);
        let all = hashes_of(&txs);
        let by_hash = txs.iter().map(|tx| (*tx.tx_hash(), tx.clone())).collect::<B256Map<_>>();
        let peers = (1..=6).map(peer).collect::<Vec<_>>();
        for peer_id in &peers {
            rig.add_peer_with_capacity(*peer_id, 4);
        }

        for round in 0..30usize {
            // every peer announces a window of the hashes
            for (i, peer_id) in peers.iter().enumerate() {
                let start = (round + i) % 20;
                rig.announce(*peer_id, &all[start..start + 20]);
            }
            rig.dispatch();

            // a rotating peer disconnects with its request inflight and comes back
            let churned = peers[round % peers.len()];
            rig.disconnect(churned);
            rig.add_peer_with_capacity(churned, 4);
            rig.drain_events();

            // the others deliver or fail
            for (i, peer_id) in peers.iter().enumerate() {
                if *peer_id == churned {
                    continue
                }
                if let Some((requested, response)) = rig.take_request(*peer_id) {
                    let delivered = if i % 2 == 0 {
                        requested.iter().map(|hash| by_hash[hash].clone()).collect()
                    } else {
                        vec![]
                    };
                    response.send(Ok(PooledTransactions(delivered))).unwrap();
                }
            }
            rig.drain_events();
        }

        for peer_id in &peers {
            rig.disconnect(*peer_id);
        }
        rig.drain_events();
        assert_eq!(rig.fetcher.num_hashes(), 0);
        assert_eq!(rig.fetcher.num_inflight_requests(), 0);
        assert_eq!(rig.fetcher.num_peers(), 0);
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
        assert_eq!(first.len(), 256);
        assert_eq!(second.len(), 44);

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
