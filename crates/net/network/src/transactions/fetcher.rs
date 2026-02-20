//! Two-stage transaction fetcher.
//!
//! Stage 1 (QUEUE): Hash is ready to be fetched, waiting for an idle peer.
//!
//! Stage 2 (FETCH): Hash is being actively fetched from a specific peer.

use super::{
    config::TransactionFetcherConfig,
    constants::{tx_fetcher::*, SOFT_LIMIT_COUNT_HASHES_IN_GET_POOLED_TRANSACTIONS_REQUEST},
    PeerMetadata, PooledTransactions, SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE,
};
use crate::{cache::LruCache, metrics::TransactionFetcherMetrics};
use alloy_consensus::transaction::PooledTransaction;
use alloy_primitives::{
    map::{B256Map, B256Set, HashMap, HashSet},
    TxHash,
};
use derive_more::{Constructor, Deref};
use rand::seq::SliceRandom;
use reth_eth_wire::{
    DedupPayload, GetPooledTransactions, PartiallyValidData, RequestTxHashes, ValidAnnouncementData,
};
use reth_eth_wire_types::{EthNetworkPrimitives, NetworkPrimitives};
use reth_network_api::PeerRequest;
use reth_network_p2p::error::{RequestError, RequestResult};
use reth_network_peers::PeerId;
use reth_primitives_traits::SignedTransaction;
use smallvec::SmallVec;
use std::{
    collections::VecDeque,
    pin::Pin,
    task::{Context, Poll, Waker},
    time::Duration,
};
use tokio::{
    sync::{oneshot, oneshot::error::RecvError},
    time::Instant,
};
use tracing::trace;

/// Pushes `val` into `vec` if it's not already present. O(n) scan, suitable for small vecs.
fn push_if_absent<T: PartialEq, const N: usize>(vec: &mut SmallVec<[T; N]>, val: T) {
    if !vec.contains(&val) {
        vec.push(val);
    }
}

/// A completed inflight request ready for processing.
#[derive(Debug)]
struct InflightCompletion<T> {
    peer_id: PeerId,
    result: Result<RequestResult<PooledTransactions<T>>, RecvError>,
}

/// The type responsible for fetching missing transactions from peers.
///
/// Uses a two-stage pipeline:
/// - Stage 1 (announced): hashes ready to fetch, indexed by peer
/// - Stage 2 (fetching): hashes actively being fetched
#[derive(Debug)]
pub struct TransactionFetcher<N: NetworkPrimitives = EthNetworkPrimitives> {
    /// Stage 1: hashes ready to fetch, indexed by hash -> set of peers that can serve it.
    announced: B256Map<SmallVec<[PeerId; 8]>>,

    /// Stage 2: hash -> peer currently fetching it.
    fetching: B256Map<PeerId>,
    /// Stage 2: hash -> alternate peers if current fetch fails.
    alternates: B256Map<SmallVec<[PeerId; 8]>>,
    /// Stage 2: peer -> their in-flight request details.
    inflight_requests: HashMap<PeerId, InflightRequest<N::PooledTransaction>>,

    /// Per-peer superset index: peer -> (hash -> metadata). Entries exist for any hash
    /// tracked for a peer across Stages 1 and 2. Only removed when a hash leaves the
    /// system entirely.
    announces: HashMap<PeerId, B256Map<TxAnnounceMetadata>>,

    /// Monotonic counter for FIFO ordering.
    tx_seq: u64,

    /// Responses ready to be yielded by the stream on the next poll.
    pending_yield: VecDeque<CompletedRequest<N::PooledTransaction>>,

    /// Timeout timer: fires when earliest inflight request times out.
    timeout_timer: Option<Pin<Box<tokio::time::Sleep>>>,

    /// Stored waker so `schedule_fetches` (called outside `poll_next`) can wake the task
    /// to register oneshot wakers for newly created inflight requests.
    waker: Option<Waker>,

    /// Incremental count of dangling (timed-out but not yet swept) inflight requests.
    dangling_count: usize,

    /// Rejection cache: transactions that failed pool import due to consensus violations.
    bad_imports: LruCache<TxHash>,
    /// Rejection cache: transactions rejected as underpriced. Unlike bad imports, peers are
    /// not penalized for these since pricing is a local/dynamic condition.
    underpriced_imports: LruCache<TxHash>,

    /// Config.
    pub info: TransactionFetcherInfo,
    metrics: TransactionFetcherMetrics,
}

/// Metadata from an announcement.
#[derive(Debug, Clone)]
struct TxAnnounceMetadata {
    size: usize,
    seq: u64,
}

/// An in-flight request to a peer.
#[derive(Debug)]
struct InflightRequest<T> {
    hashes: Vec<TxHash>,
    stolen: B256Set,
    sent_at: Instant,
    response: oneshot::Receiver<RequestResult<PooledTransactions<T>>>,
    dangling: bool,
}

/// A completed request with its captured result, ready to be processed.
#[derive(Debug)]
struct CompletedRequest<T> {
    peer_id: PeerId,
    hashes: Vec<TxHash>,
    stolen: B256Set,
    dangling: bool,
    result: Result<RequestResult<PooledTransactions<T>>, RecvError>,
    sent_at: Instant,
}

impl<N: NetworkPrimitives> TransactionFetcher<N> {
    /// Sets up transaction fetcher with config.
    pub fn with_transaction_fetcher_config(config: &TransactionFetcherConfig) -> Self {
        let info = TransactionFetcherInfo::from(config.clone());
        let metrics = TransactionFetcherMetrics::default();
        metrics.capacity_inflight_requests.increment(config.max_inflight_requests as u64);

        Self {
            announced: B256Map::default(),
            fetching: B256Map::default(),
            alternates: B256Map::default(),
            inflight_requests: HashMap::default(),
            announces: HashMap::default(),
            tx_seq: 0,
            pending_yield: VecDeque::new(),
            timeout_timer: None,
            waker: None,
            dangling_count: 0,
            bad_imports: LruCache::new(DEFAULT_MAX_COUNT_BAD_IMPORTS),
            underpriced_imports: LruCache::new(DEFAULT_MAX_COUNT_UNDERPRICED_IMPORTS),
            info,
            metrics,
        }
    }

    /// Returns the next sequence number.
    #[inline]
    const fn next_seq(&mut self) -> u64 {
        let seq = self.tx_seq;
        self.tx_seq += 1;
        seq
    }

    /// Returns total number of announcements tracked for a peer across all stages.
    #[inline]
    fn peer_announce_count(&self, peer_id: &PeerId) -> usize {
        self.announces.get(peer_id).map_or(0, |entries| entries.len())
    }

    /// Moves a set of peers back into `announced` (Stage 1) for the given hash.
    ///
    /// The `announces` index is not modified because it is kept as a superset across
    /// stages — entries are only removed when a hash leaves the system entirely.
    fn restore_to_announced(&mut self, hash: TxHash, peers: SmallVec<[PeerId; 8]>) {
        self.announced.insert(hash, peers);
    }

    /// Handles a new announcement from a peer.
    pub fn on_new_announcement(
        &mut self,
        peer_id: PeerId,
        announcement: ValidAnnouncementData,
        peers: &HashMap<PeerId, PeerMetadata<N>>,
    ) {
        // Per-peer DoS fast-path: skip entirely if already at limit
        if self.peer_announce_count(&peer_id) >= self.info.max_announces_per_peer {
            self.metrics.dropped_announces_per_peer_limit.increment(1);
            trace!(target: "net::tx",
                peer_id=format!("{peer_id:#}"),
                "dropping announcement: peer at announce limit"
            );
            return;
        }

        let max_announced = self.info.max_capacity_cache_txns_pending_fetch as usize;
        let announced_before = self.announced.len();

        for (hash, metadata) in announcement {
            // Per-peer DoS check: enforced per-hash to prevent a single large
            // batch from exceeding the limit.
            if self.peer_announce_count(&peer_id) >= self.info.max_announces_per_peer {
                self.metrics.dropped_announces_per_peer_limit.increment(1);
                break;
            }

            // Already in Stage 2 (fetching)?
            if let Some(&fetching_peer) = self.fetching.get(&hash) {
                // Never add the fetching peer to its own alternates.
                if fetching_peer != peer_id {
                    push_if_absent(self.alternates.entry(hash).or_default(), peer_id);
                }
                self.insert_peer_announce(&peer_id, &hash, &metadata);
                continue;
            }

            // Already in Stage 1 (announced)?
            if self.announced.contains_key(&hash) {
                push_if_absent(self.announced.entry(hash).or_default(), peer_id);
                self.insert_peer_announce(&peer_id, &hash, &metadata);
                continue;
            }

            // New hash
            if self.is_known_bad_import(&hash) {
                continue;
            }
            let size = metadata.map(|(_, sz)| sz).unwrap_or(0);
            if self.announced.len() >= max_announced {
                self.metrics.dropped_announces_capacity_limit.increment(1);
                continue;
            }
            let seq = self.next_seq();
            push_if_absent(self.announced.entry(hash).or_default(), peer_id);
            self.announces
                .entry(peer_id)
                .or_default()
                .insert(hash, TxAnnounceMetadata { size, seq });
        }

        if self.announced.len() > announced_before {
            self.schedule_fetches(peers);
        }
    }

    /// Caches a transaction hash that failed pool import as a consensus violation.
    #[inline]
    pub fn cache_bad_import(&mut self, hash: TxHash) {
        self.bad_imports.insert(hash);
        self.metrics.bad_imports.increment(1);
    }

    /// Caches a transaction hash rejected as underpriced.
    #[inline]
    pub fn cache_underpriced_import(&mut self, hash: TxHash) {
        self.underpriced_imports.insert(hash);
        self.metrics.underpriced_imports.increment(1);
    }

    /// Returns `true` if the hash is in the bad imports or underpriced cache.
    #[inline]
    pub fn is_known_bad_import(&self, hash: &TxHash) -> bool {
        self.bad_imports.contains(hash) || self.underpriced_imports.contains(hash)
    }

    /// Inserts a peer announcement into the announces index if not already present.
    /// If the entry already exists with unknown size (0), updates to the known size
    /// without changing the FIFO ordering (seq).
    #[inline]
    fn insert_peer_announce(
        &mut self,
        peer_id: &PeerId,
        hash: &TxHash,
        metadata: &Option<(u8, usize)>,
    ) {
        let size = metadata.map(|(_, sz)| sz).unwrap_or(0);
        let seq = self.next_seq();
        let peer_entries = self.announces.entry(*peer_id).or_default();
        let entry = peer_entries.entry(*hash).or_insert(TxAnnounceMetadata { size, seq });
        if entry.size == 0 && size > 0 {
            entry.size = size;
        }
    }

    /// Iterates peers with announced hashes and sends requests to idle ones.
    pub fn schedule_fetches(&mut self, peers: &HashMap<PeerId, PeerMetadata<N>>) {
        if self.announced.is_empty() {
            return;
        }

        let inflight_before = self.inflight_requests.len();
        let max_inflight = self.info.max_inflight_requests;
        let size_limit =
            self.info.soft_limit_byte_size_pooled_transactions_response_on_pack_request;

        // Collect idle peer IDs that have announced hashes. We must collect because the
        // loop body mutates `self.announces`. Cap at remaining inflight slots since we
        // can never dispatch more requests than that in one call.
        // Shuffle to prevent peer starvation: without randomization, the same peers
        // would consistently win the claiming race due to stable iteration order.
        let mut remaining_slots = max_inflight.saturating_sub(self.inflight_requests.len());
        let mut candidate_peers: SmallVec<[PeerId; 16]> = self
            .announces
            .keys()
            .filter(|pid| !self.inflight_requests.contains_key(*pid))
            .copied()
            .collect();
        candidate_peers.shuffle(&mut rand::rng());

        for peer_id in candidate_peers {
            if remaining_slots == 0 {
                break;
            }
            let Some(peer) = peers.get(&peer_id) else { continue };
            let Some(peer_hashes) = self.announces.get(&peer_id) else { continue };

            // Build request: collect only hashes where this peer is an owner in Stage 1,
            // sort by seq (FIFO), then take up to the request limit by count and size.
            let mut candidates: SmallVec<[(TxHash, usize, u64); 32]> = peer_hashes
                .iter()
                .filter(|(hash, _)| {
                    self.announced.get(*hash).is_some_and(|peer_set| peer_set.contains(&peer_id))
                })
                .map(|(hash, meta)| (*hash, meta.size, meta.seq))
                .collect();
            candidates.sort_unstable_by_key(|&(_, _, seq)| seq);

            let mut request_hashes =
                Vec::with_capacity(SOFT_LIMIT_COUNT_HASHES_IN_GET_POOLED_TRANSACTIONS_REQUEST);
            let mut acc_size = 0usize;

            for &(hash, size, _) in &candidates {
                acc_size += if size > 0 { size } else { AVERAGE_BYTE_SIZE_TX_ENCODED };
                request_hashes.push(hash);

                if request_hashes.len() >=
                    SOFT_LIMIT_COUNT_HASHES_IN_GET_POOLED_TRANSACTIONS_REQUEST ||
                    acc_size >= size_limit
                {
                    break;
                }
            }

            if request_hashes.is_empty() {
                continue;
            }

            // Two-phase commit: attempt send BEFORE mutating stage maps.
            let (response_tx, response_rx) = oneshot::channel();
            let req = PeerRequest::GetPooledTransactions {
                request: GetPooledTransactions(request_hashes.clone()),
                response: response_tx,
            };

            if let Err(_err) = peer.request_tx.try_send(req) {
                self.metrics.egress_peer_channel_full.increment(1);
                continue;
            }

            // Move hashes from Stage 1 → Stage 2.
            // `announces` is kept as a superset index; entries are only removed
            // when a hash leaves the system entirely (fetched / received / disconnect).
            for hash in &request_hashes {
                if let Some(mut peer_set) = self.announced.remove(hash) {
                    peer_set.retain(|p| p != &peer_id);
                    if !peer_set.is_empty() {
                        let alts = self.alternates.entry(*hash).or_default();
                        for p in peer_set {
                            push_if_absent(alts, p);
                        }
                    }
                }
                self.fetching.insert(*hash, peer_id);
            }

            // Record inflight request
            self.inflight_requests.insert(
                peer_id,
                InflightRequest {
                    hashes: request_hashes,
                    stolen: B256Set::default(),
                    sent_at: Instant::now(),
                    response: response_rx,
                    dangling: false,
                },
            );
            remaining_slots -= 1;

            if self.announced.is_empty() {
                break;
            }
        }

        if self.inflight_requests.len() > inflight_before {
            self.reschedule_timeout_timer();
            if let Some(waker) = &self.waker {
                waker.wake_by_ref();
            }
        }
    }

    /// Called when transactions are received (via broadcast or fetch response).
    /// Removes hashes from whichever stage they are in.
    ///
    /// `is_broadcast` indicates whether these transactions arrived via an unsolicited
    /// `Transactions` broadcast (true) or as a `GetPooledTransactions` response (false).
    pub fn on_transactions_received<'a>(
        &mut self,
        hashes: impl Iterator<Item = &'a TxHash>,
        is_broadcast: bool,
    ) {
        for hash in hashes {
            // Stage 1 (announced)
            if let Some(peer_set) = self.announced.remove(hash) {
                if is_broadcast {
                    self.metrics.late_broadcast_announced.increment(1);
                }
                for peer in &peer_set {
                    self.remove_from_announces(peer, hash);
                }
                continue;
            }

            // Stage 2 (fetching)
            if let Some(fetching_peer) = self.fetching.remove(hash) {
                if is_broadcast {
                    self.metrics.late_broadcast_inflight.increment(1);
                }
                if let Some(req) = self.inflight_requests.get_mut(&fetching_peer) {
                    req.stolen.insert(*hash);
                }
                let mut peers_to_clean = self.alternates.remove(hash).unwrap_or_default();
                if !peers_to_clean.contains(&fetching_peer) {
                    peers_to_clean.push(fetching_peer);
                }
                for peer in &peers_to_clean {
                    self.remove_from_announces(peer, hash);
                }
            }
        }
    }

    /// Called when a peer disconnects. Full cleanup across all stages.
    pub fn on_peer_disconnected(
        &mut self,
        peer_id: &PeerId,
        peers: &HashMap<PeerId, PeerMetadata<N>>,
    ) {
        // Clean Stage 2: if peer had inflight request, redistribute hashes
        if let Some(req) = self.inflight_requests.remove(peer_id) {
            if req.dangling {
                self.dangling_count -= 1;
            }
            for hash in &req.hashes {
                if req.stolen.contains(hash) {
                    continue;
                }
                self.fetching.remove(hash);
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
            for hash in peer_hashes.keys() {
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

        self.schedule_fetches(peers);
    }

    /// Reschedule the timeout timer for the earliest inflight or dangling request.
    fn reschedule_timeout_timer(&mut self) {
        let now = Instant::now();
        let timeout = self.info.tx_fetch_timeout;
        let dangling_limit = timeout * DANGLING_TIMEOUT_MULTIPLIER;
        let slack = self.info.tx_gather_slack;
        let mut earliest: Option<Instant> = None;

        for req in self.inflight_requests.values() {
            let expiry =
                if req.dangling { req.sent_at + dangling_limit } else { req.sent_at + timeout };
            if earliest.is_none_or(|prev| expiry < prev) {
                earliest = Some(expiry);
                if expiry.saturating_duration_since(now) < slack {
                    break;
                }
            }
        }

        self.timeout_timer =
            earliest.map(|dl| Box::pin(tokio::time::sleep(dl.saturating_duration_since(now))));
    }

    /// Process timed-out inflight requests and sweep stale dangling entries.
    ///
    /// Uses gather slack to batch nearby timeouts: finds the earliest timed-out request,
    /// then also drains any requests timing out within `slack` of it. After processing
    /// fresh timeouts, sweeps dangling requests older than `DANGLING_TIMEOUT_MULTIPLIER`
    /// × `tx_fetch_timeout`.
    fn drain_timed_out_requests(&mut self) {
        let now = Instant::now();
        let timeout = self.info.tx_fetch_timeout;
        let slack = self.info.tx_gather_slack;

        // Process fresh timeouts (non-dangling requests that have expired)
        let earliest_timeout = self
            .inflight_requests
            .values()
            .filter(|req| !req.dangling)
            .map(|req| req.sent_at + timeout)
            .filter(|&expiry| expiry <= now)
            .min();

        if let Some(earliest) = earliest_timeout {
            let batch_cutoff = earliest + slack;
            let timed_out: Vec<PeerId> = self
                .inflight_requests
                .iter()
                .filter(|(_, req)| !req.dangling && req.sent_at + timeout <= batch_cutoff)
                .map(|(peer_id, _)| *peer_id)
                .collect();

            for peer_id in timed_out {
                self.metrics.request_timeouts.increment(1);

                if let Some(req) = self.inflight_requests.get_mut(&peer_id) {
                    let hashes = std::mem::take(&mut req.hashes);
                    let stolen = std::mem::take(&mut req.stolen);
                    req.dangling = true;
                    self.dangling_count += 1;

                    // Move non-stolen hashes back to Stage 1 via alternates and
                    // clean the timed-out peer's announces entries.
                    self.return_hashes_to_announced(&hashes, &stolen, &peer_id);

                    // Clean up fetching entries for stolen hashes too
                    for hash in &stolen {
                        self.fetching.remove(hash);
                    }
                }
            }
        }

        // Sweep stale dangling requests: force-remove entries that have been dangling
        // longer than the maximum dangling duration, unblocking the peer's fetch slot.
        let dangling_limit = timeout * DANGLING_TIMEOUT_MULTIPLIER;
        let stale: Vec<PeerId> = self
            .inflight_requests
            .iter()
            .filter(|(_, req)| req.dangling && req.sent_at + dangling_limit <= now)
            .map(|(peer_id, _)| *peer_id)
            .collect();

        for peer_id in &stale {
            self.inflight_requests.remove(peer_id);
            self.dangling_count -= 1;
            self.metrics.stale_dangling_requests_removed.increment(1);
            trace!(target: "net::tx",
                peer_id=format!("{peer_id:#}"),
                "force-removed stale dangling request, unblocking peer fetch slot"
            );
        }

        self.reschedule_timeout_timer();
    }

    /// Moves non-stolen hashes from Stage 2 (fetching) back to Stage 1 (announced) using
    /// alternates.
    ///
    /// The `announces` index already contains entries for alternate peers (it is kept
    /// as a superset), so only `announced` needs to be re-populated. The failed peer's
    /// announces entries are cleaned since it should not be re-selected.
    fn return_hashes_to_announced(
        &mut self,
        hashes: &[TxHash],
        stolen: &B256Set,
        failed_peer: &PeerId,
    ) {
        for hash in hashes {
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
        let CompletedRequest { peer_id, hashes, stolen, dangling, result, sent_at } = completed;

        if dangling {
            let latency = sent_at.elapsed();
            self.metrics.dangling_response_latency_seconds.record(latency.as_secs_f64());
            let useful = matches!(&result, Ok(Ok(txns)) if !txns.is_empty());
            if useful {
                self.metrics.dangling_completed_useful.increment(1);
            } else {
                self.metrics.dangling_completed_empty.increment(1);
            }
            return FetchEvent::EmptyResponse { peer_id };
        }

        self.metrics.fetch_response_latency_seconds.record(sent_at.elapsed().as_secs_f64());

        match result {
            Ok(Ok(transactions)) => {
                if transactions.is_empty() {
                    self.return_hashes_to_announced(&hashes, &stolen, &peer_id);
                    return FetchEvent::EmptyResponse { peer_id };
                }

                let request_hashes: alloy_primitives::map::HashSet<TxHash> =
                    hashes.iter().copied().collect();
                let payload = UnverifiedPooledTransactions::new(transactions);
                let payload_len = payload.len();
                let (verification_outcome, verified_payload) =
                    payload.verify(&RequestTxHashes::new(request_hashes), &peer_id);

                let report_peer = verification_outcome == VerificationOutcome::ReportPeer;

                let unsolicited = payload_len.saturating_sub(verified_payload.len());
                if unsolicited > 0 {
                    self.metrics.unsolicited_transactions.increment(unsolicited as u64);
                }

                if verified_payload.is_empty() {
                    self.return_hashes_to_announced(&hashes, &stolen, &peer_id);
                    return FetchEvent::FetchError { peer_id, error: RequestError::BadResponse };
                }

                let valid_payload = verified_payload.dedup();

                let fetched: B256Set = valid_payload.keys().copied().collect();
                self.metrics.fetched_transactions.increment(fetched.len() as u64);

                // Collect peers to clean from alternates before removing them
                let mut peers_to_clean: HashSet<PeerId> = HashSet::default();
                peers_to_clean.insert(peer_id);
                for hash in &fetched {
                    if let Some(alts) = self.alternates.remove(hash) {
                        peers_to_clean.extend(alts);
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
                    .filter(|(_, h)| fetched.contains(*h))
                    .map(|(i, _)| i + 1)
                    .next_back()
                    .unwrap_or(hashes.len());

                // Move undelivered, non-stolen hashes back to Stage 1
                for (i, hash) in hashes.iter().enumerate() {
                    if fetched.contains(hash) || stolen.contains(hash) {
                        continue;
                    }
                    self.fetching.remove(hash);
                    let mut candidates = self.alternates.remove(hash).unwrap_or_default();
                    if i < cutoff {
                        // Peer intentionally skipped this hash
                        candidates.retain(|p| p != &peer_id);
                    }
                    if candidates.is_empty() {
                        // No candidates remain — hash leaves the system
                        self.remove_from_announces(&peer_id, hash);
                    } else {
                        self.restore_to_announced(*hash, candidates);
                    }
                }

                // Clean announces for fetched hashes across all involved peers
                for peer in &peers_to_clean {
                    for hash in &fetched {
                        self.remove_from_announces(peer, hash);
                    }
                }

                // Clean origin peer's announces for all undelivered hashes.
                // The fetching peer was excluded from alternates during
                // Stage 1→2 (schedule_fetches), so its announces entry is
                // orphaned for any hash it didn't deliver.
                for hash in &hashes {
                    if !fetched.contains(hash) && !stolen.contains(hash) {
                        self.remove_from_announces(&peer_id, hash);
                    }
                }

                let transactions_out =
                    PooledTransactions(valid_payload.into_data().into_values().collect());

                FetchEvent::TransactionsFetched {
                    peer_id,
                    transactions: transactions_out,
                    report_peer,
                }
            }
            Ok(Err(req_err)) => {
                self.return_hashes_to_announced(&hashes, &stolen, &peer_id);
                FetchEvent::FetchError { peer_id, error: req_err }
            }
            Err(_) => {
                self.return_hashes_to_announced(&hashes, &stolen, &peer_id);
                FetchEvent::FetchError { peer_id, error: RequestError::ChannelClosed }
            }
        }
    }

    /// Updates metrics.
    #[inline]
    pub fn update_metrics(&self) {
        let metrics = &self.metrics;
        metrics.inflight_transaction_requests.set(self.inflight_requests.len() as f64);
        metrics.hashes_in_announced.set(self.announced.len() as f64);
        metrics.hashes_in_fetching.set(self.fetching.len() as f64);
        metrics.dangling_requests.set(self.dangling_count as f64);
    }

    /// Inserts a hash directly into Stage 1 (announced) for a given peer. Test only.
    #[cfg(any(test, feature = "test-utils"))]
    pub(crate) fn insert_announced_for_test(&mut self, hash: TxHash, peer_id: PeerId, size: usize) {
        push_if_absent(self.announced.entry(hash).or_default(), peer_id);
        let seq = self.next_seq();
        self.announces.entry(peer_id).or_default().insert(hash, TxAnnounceMetadata { size, seq });
    }

    /// Returns true if the peer has no inflight request.
    #[cfg(test)]
    pub(crate) fn is_idle(&self, peer_id: &PeerId) -> bool {
        !self.inflight_requests.contains_key(peer_id)
    }

    /// Returns the number of hashes in Stage 1 (announced).
    #[cfg(test)]
    pub(crate) fn num_announced(&self) -> usize {
        self.announced.len()
    }

    /// Returns the number of inflight requests.
    #[cfg(test)]
    pub(crate) fn num_inflight(&self) -> usize {
        self.inflight_requests.len()
    }

    /// Returns the number of hashes in Stage 2 (fetching).
    #[cfg(test)]
    pub(crate) fn num_fetching(&self) -> usize {
        self.fetching.len()
    }

    /// Removes `hash` from `announces[peer]`. Removes the outer entry if empty.
    fn remove_from_announces(&mut self, peer: &PeerId, hash: &TxHash) {
        if let Some(inner) = self.announces.get_mut(peer) {
            inner.remove(hash);
            if inner.is_empty() {
                self.announces.remove(peer);
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
            announces: HashMap::default(),
            tx_seq: 0,
            pending_yield: VecDeque::new(),
            timeout_timer: None,
            waker: None,
            dangling_count: 0,
            bad_imports: LruCache::new(DEFAULT_MAX_COUNT_BAD_IMPORTS),
            underpriced_imports: LruCache::new(DEFAULT_MAX_COUNT_UNDERPRICED_IMPORTS),
            info: TransactionFetcherInfo::default(),
            metrics: TransactionFetcherMetrics::default(),
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

        // a) Poll timeout timer
        if let Some(timer) = this.timeout_timer.as_mut() &&
            Pin::new(timer).poll(cx).is_ready()
        {
            this.drain_timed_out_requests();
        }

        // b) Yield from previously stashed completed requests first
        if let Some(completed) = this.pending_yield.pop_front() {
            let event = this.process_completed_request(completed);
            return Poll::Ready(Some(event));
        }

        // c) Poll inflight responses. Two-phase: collect completions first (can't
        // remove from HashMap while iterating), then process.
        let mut newly_completed = Vec::<InflightCompletion<N::PooledTransaction>>::new();
        for (peer_id, req) in &mut this.inflight_requests {
            if let Poll::Ready(result) = Pin::new(&mut req.response).poll(cx) {
                newly_completed.push(InflightCompletion { peer_id: *peer_id, result });
            }
        }

        // Remove completed from inflight and build CompletedRequest entries.
        for InflightCompletion { peer_id, result } in newly_completed {
            let req = this.inflight_requests.remove(&peer_id).unwrap();
            if req.dangling {
                this.dangling_count = this.dangling_count.saturating_sub(1);
            }
            this.pending_yield.push_back(CompletedRequest {
                peer_id,
                hashes: req.hashes,
                stolen: req.stolen,
                dangling: req.dangling,
                result,
                sent_at: req.sent_at,
            });
        }

        // Yield the first completed request
        if let Some(completed) = this.pending_yield.pop_front() {
            let event = this.process_completed_request(completed);
            return Poll::Ready(Some(event));
        }

        Poll::Pending
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

/// Tracks stats about the [`TransactionFetcher`].
#[derive(Debug, Constructor)]
#[allow(clippy::too_many_arguments)]
pub struct TransactionFetcherInfo {
    /// Max inflight [`GetPooledTransactions`] requests.
    pub max_inflight_requests: usize,
    /// Soft limit for the byte size of the expected [`PooledTransactions`] response, upon packing
    /// a [`GetPooledTransactions`] request with hashes.
    pub soft_limit_byte_size_pooled_transactions_response_on_pack_request: usize,
    /// Soft limit for the byte size of a [`PooledTransactions`] response.
    pub soft_limit_byte_size_pooled_transactions_response: usize,
    /// Max capacity of the hash tracking caches.
    pub max_capacity_cache_txns_pending_fetch: u32,
    /// Time to wait for a peer to respond.
    pub tx_fetch_timeout: Duration,
    /// Slack added to timer checks.
    pub tx_gather_slack: Duration,
    /// Max announcements per peer.
    pub max_announces_per_peer: usize,
}

impl Default for TransactionFetcherInfo {
    fn default() -> Self {
        Self::new(
            DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS as usize,
            DEFAULT_SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESP_ON_PACK_GET_POOLED_TRANSACTIONS_REQ,
            SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE,
            DEFAULT_MAX_CAPACITY_CACHE_PENDING_FETCH,
            DEFAULT_TX_FETCH_TIMEOUT,
            DEFAULT_TX_GATHER_SLACK,
            DEFAULT_MAX_TX_ANNOUNCES_PER_PEER,
        )
    }
}

impl From<TransactionFetcherConfig> for TransactionFetcherInfo {
    fn from(config: TransactionFetcherConfig) -> Self {
        let TransactionFetcherConfig {
            max_inflight_requests,
            soft_limit_byte_size_pooled_transactions_response,
            soft_limit_byte_size_pooled_transactions_response_on_pack_request,
            max_capacity_cache_txns_pending_fetch,
            tx_fetch_timeout,
            max_announces_per_peer,
        } = config;

        Self::new(
            max_inflight_requests as usize,
            soft_limit_byte_size_pooled_transactions_response_on_pack_request,
            soft_limit_byte_size_pooled_transactions_response,
            max_capacity_cache_txns_pending_fetch,
            tx_fetch_timeout,
            DEFAULT_TX_GATHER_SLACK,
            max_announces_per_peer,
        )
    }
}

/// Wrapper of unverified [`PooledTransactions`].
#[derive(Debug, Constructor, Deref)]
pub struct UnverifiedPooledTransactions<T> {
    txns: PooledTransactions<T>,
}

/// [`PooledTransactions`] that have been successfully verified.
#[derive(Debug, Constructor, Deref)]
pub struct VerifiedPooledTransactions<T> {
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
        peer_id: &PeerId,
    ) -> (VerificationOutcome, VerifiedPooledTransactions<Self::Transaction>);
}

impl<T: SignedTransaction> VerifyPooledTransactionsResponse for UnverifiedPooledTransactions<T> {
    type Transaction = T;

    fn verify(
        self,
        requested_hashes: &RequestTxHashes,
        _peer_id: &PeerId,
    ) -> (VerificationOutcome, VerifiedPooledTransactions<T>) {
        let mut verification_outcome = VerificationOutcome::Ok;

        let Self { mut txns } = self;

        txns.0.retain(|tx| {
            if !requested_hashes.contains(tx.tx_hash()) {
                verification_outcome = VerificationOutcome::ReportPeer;
                return false;
            }
            true
        });

        (verification_outcome, VerifiedPooledTransactions::new(txns))
    }
}

/// Outcome from verifying a [`PooledTransactions`] response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerificationOutcome {
    /// Peer behaves appropriately.
    Ok,
    /// A penalty should be flagged for the peer.
    ReportPeer,
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::test_utils::transactions::new_mock_session;
    use alloy_primitives::{hex, map::HashMap as AHashMap, B256};
    use alloy_rlp::Decodable;
    use futures::StreamExt;
    use reth_eth_wire::EthVersion;
    use reth_ethereum_primitives::{PooledTransactionVariant, TransactionSigned};
    use reth_network_api::PeerRequest;
    use std::{future::poll_fn, iter::once, str::FromStr};

    type TestFetcher = TransactionFetcher<EthNetworkPrimitives>;

    /// Helper to build a `ValidAnnouncementData` for Eth68 announcements.
    fn make_eth68_announcement(entries: Vec<(TxHash, u8, usize)>) -> ValidAnnouncementData {
        let data: AHashMap<TxHash, Option<(u8, usize)>> =
            entries.into_iter().map(|(h, ty, sz)| (h, Some((ty, sz)))).collect();
        let partial = PartiallyValidData::from_raw_data_eth68(data);
        ValidAnnouncementData::from_partially_valid_data(partial)
    }

    /// Helper to build a `ValidAnnouncementData` for Eth66 announcements.
    #[allow(dead_code)]
    fn make_eth66_announcement(hashes: Vec<TxHash>) -> ValidAnnouncementData {
        let data: AHashMap<TxHash, Option<(u8, usize)>> =
            hashes.into_iter().map(|h| (h, None)).collect();
        let partial = PartiallyValidData::from_raw_data_eth66(data);
        ValidAnnouncementData::from_partially_valid_data(partial)
    }

    /// Decodes the two test transactions used in verification tests.
    fn test_pooled_transactions() -> (PooledTransactionVariant, PooledTransactionVariant) {
        let input_1 = hex!(
            "02f871018302a90f808504890aef60826b6c94ddf4c5025d1a5742cf12f74eec246d4432c295e487e09c3bbcc12b2b80c080a0f21a4eacd0bf8fea9c5105c543be5a1d8c796516875710fafafdf16d16d8ee23a001280915021bb446d1973501a67f93d2b38894a514b976e7b46dc2fe54598daa"
        );
        let signed_1 = TransactionSigned::decode(&mut &input_1[..]).unwrap();
        let pooled_1: PooledTransactionVariant = signed_1.try_into().unwrap();

        let input_2 = hex!(
            "02f871018302a90f808504890aef60826b6c94ddf4c5025d1a5742cf12f74eec246d4432c295e487e09c3bbcc12b2b80c080a0f21a4eacd0bf8fea9c5105c543be5a1d8c796516875710fafafdf16d16d8ee23a001280915021bb446d1973501a67f93d2b38894a514b976e7b46dc2fe54598d76"
        );
        let signed_2 = TransactionSigned::decode(&mut &input_2[..]).unwrap();
        let pooled_2: PooledTransactionVariant = signed_2.try_into().unwrap();

        (pooled_1, pooled_2)
    }

    fn peer(n: u8) -> PeerId {
        PeerId::new([n; 64])
    }

    fn hash(n: u8) -> TxHash {
        B256::from_slice(&[n; 32])
    }

    /// Asserts that the given hash exists in exactly one stage (or none if `expect_present`
    /// is false). Panics with a descriptive message on invariant violation.
    #[track_caller]
    fn assert_hash_in_single_stage(fetcher: &TestFetcher, h: &TxHash, expect_present: bool) {
        let in_announced = fetcher.announced.contains_key(h);
        let in_fetching = fetcher.fetching.contains_key(h);
        let count = in_announced as u8 + in_fetching as u8;

        if expect_present {
            assert_eq!(
                count, 1,
                "hash {h} should be in exactly one stage, but found: \
                 announced={in_announced}, fetching={in_fetching}"
            );
        } else {
            assert_eq!(
                count, 0,
                "hash {h} should not be in any stage, but found: \
                 announced={in_announced}, fetching={in_fetching}"
            );
        }
    }

    #[test]
    fn verify_response_hashes() {
        let (signed_tx_1, signed_tx_2) = test_pooled_transactions();

        let request_hashes = [
            B256::from_str("0x3b9aca00f0671c9a2a1b817a0a78d3fe0c0f776cccb2a8c3c1b412a4f4e67890")
                .unwrap(),
            *signed_tx_1.tx_hash(),
            B256::from_str("0x3b9aca00f0671c9a2a1b817a0a78d3fe0c0f776cccb2a8c3c1b412a4f4e12345")
                .unwrap(),
            B256::from_str("0x3b9aca00f0671c9a2a1b817a0a78d3fe0c0f776cccb2a8c3c1b412a4f4edabe3")
                .unwrap(),
        ];

        for hash in &request_hashes {
            assert_ne!(hash, signed_tx_2.tx_hash())
        }

        let request_hashes = RequestTxHashes::new(request_hashes.into_iter().collect());
        let response_txns = PooledTransactions(vec![signed_tx_1.clone(), signed_tx_2]);
        let payload = UnverifiedPooledTransactions::new(response_txns);

        let (outcome, verified_payload) = payload.verify(&request_hashes, &PeerId::ZERO);

        assert_eq!(VerificationOutcome::ReportPeer, outcome);
        assert_eq!(1, verified_payload.len());
        assert!(verified_payload.contains(&signed_tx_1));
    }

    #[test]
    fn verify_all_unsolicited_yields_bad_response() {
        let (_tx1, tx2) = test_pooled_transactions();
        let h_requested = hash(1); // doesn't match tx2's hash

        let request_hashes = RequestTxHashes::new(std::iter::once(h_requested).collect());
        let payload = UnverifiedPooledTransactions::new(PooledTransactions(vec![tx2]));

        let (outcome, verified_payload) = payload.verify(&request_hashes, &PeerId::ZERO);

        assert_eq!(VerificationOutcome::ReportPeer, outcome);
        assert!(verified_payload.is_empty(), "all unsolicited txns should be filtered");
    }

    #[test]
    fn verify_dedup_removes_duplicate_hashes() {
        let (tx1, _tx2) = test_pooled_transactions();
        let h1 = *tx1.tx_hash();

        let request_hashes = RequestTxHashes::new(std::iter::once(h1).collect());
        let payload = UnverifiedPooledTransactions::new(PooledTransactions(vec![tx1.clone(), tx1]));

        let (outcome, verified_payload) = payload.verify(&request_hashes, &PeerId::ZERO);

        assert_eq!(VerificationOutcome::Ok, outcome);
        let deduped = verified_payload.dedup();
        assert_eq!(deduped.len(), 1, "dedup should collapse duplicate tx hashes");
    }

    #[tokio::test]
    async fn test_new_hash_enters_announced_via_announcement() {
        let mut fetcher = TestFetcher::default();
        let peer_a = peer(1);
        let h = hash(1);

        let (peer_meta, _rx) = new_mock_session(peer_a, EthVersion::Eth68);
        let mut peers = HashMap::default();
        peers.insert(peer_a, peer_meta);

        let announcement = make_eth68_announcement(vec![(h, 0x02, 100)]);
        fetcher.on_new_announcement(peer_a, announcement, &peers);

        // Hash goes directly to Stage 1 (announced), then schedule_fetches may
        // move it to Stage 2 (fetching) if the peer is idle.
        assert_hash_in_single_stage(&fetcher, &h, true);
        assert!(
            fetcher.announced.contains_key(&h) || fetcher.fetching.contains_key(&h),
            "hash should be in Stage 1 or Stage 2"
        );
        assert!(fetcher.announces[&peer_a].contains_key(&h));
    }

    #[tokio::test]
    async fn test_blob_hash_enters_announced() {
        let mut fetcher = TestFetcher::default();
        let peer_a = peer(1);
        let h = hash(1);

        let (peer_meta, _rx) = new_mock_session(peer_a, EthVersion::Eth68);
        let mut peers = HashMap::default();
        peers.insert(peer_a, peer_meta);

        // EIP4844_TX_TYPE_ID = 0x03
        let announcement = make_eth68_announcement(vec![(h, 0x03, 131072)]);
        fetcher.on_new_announcement(peer_a, announcement, &peers);

        // Blob goes directly to Stage 1 (announced), then schedule_fetches may
        // move it to Stage 2 (fetching) if the peer is idle.
        assert_hash_in_single_stage(&fetcher, &h, true);
        assert!(
            fetcher.announced.contains_key(&h) || fetcher.fetching.contains_key(&h),
            "blob hash should be in Stage 1 or Stage 2"
        );
    }

    #[tokio::test]
    async fn test_schedule_fetches_sends_request() {
        let mut fetcher = TestFetcher::default();
        let peer_a = peer(1);
        let h1 = hash(1);
        let h2 = hash(2);

        let (peer_meta, mut mock_rx) = new_mock_session(peer_a, EthVersion::Eth66);
        let mut peers = HashMap::default();
        peers.insert(peer_a, peer_meta);

        fetcher.insert_announced_for_test(h1, peer_a, 100);
        fetcher.insert_announced_for_test(h2, peer_a, 200);

        fetcher.schedule_fetches(&peers);

        assert!(fetcher.fetching.contains_key(&h1));
        assert!(fetcher.fetching.contains_key(&h2));
        assert!(fetcher.inflight_requests.contains_key(&peer_a));
        assert!(fetcher.announced.is_empty());

        let req = mock_rx.recv().await.unwrap();
        assert!(matches!(req, PeerRequest::GetPooledTransactions { .. }));
    }

    #[test]
    fn test_schedule_skips_busy_peer() {
        let mut fetcher = TestFetcher::default();
        let peer_a = peer(1);
        let h = hash(1);

        let (peer_meta, _rx) = new_mock_session(peer_a, EthVersion::Eth66);
        let mut peers = HashMap::default();
        peers.insert(peer_a, peer_meta);

        fetcher.insert_announced_for_test(h, peer_a, 100);

        // Manually make peer busy
        let (_tx, rx) = oneshot::channel();
        fetcher.inflight_requests.insert(
            peer_a,
            InflightRequest {
                hashes: vec![hash(99)],
                stolen: B256Set::default(),
                sent_at: Instant::now(),
                response: rx,
                dangling: false,
            },
        );

        fetcher.schedule_fetches(&peers);

        assert!(fetcher.announced.contains_key(&h), "hash should stay in Stage 1");
    }

    #[tokio::test]
    async fn test_schedule_respects_max_inflight() {
        let mut fetcher = TestFetcher::default();
        fetcher.info.max_inflight_requests = 1;

        let peer_a = peer(1);
        let peer_b = peer(2);
        let h1 = hash(1);
        let h2 = hash(2);

        let (meta_a, _rx_a) = new_mock_session(peer_a, EthVersion::Eth66);
        let (meta_b, _rx_b) = new_mock_session(peer_b, EthVersion::Eth66);
        let mut peers = HashMap::default();
        peers.insert(peer_a, meta_a);
        peers.insert(peer_b, meta_b);

        fetcher.insert_announced_for_test(h1, peer_a, 100);
        fetcher.insert_announced_for_test(h2, peer_b, 100);

        fetcher.schedule_fetches(&peers);

        assert_eq!(fetcher.inflight_requests.len(), 1);
        // One hash moved to fetching, the other stays in announced
        assert_eq!(fetcher.fetching.len(), 1);
        assert_eq!(fetcher.announced.len(), 1);
    }

    #[tokio::test]
    async fn test_schedule_populates_alternates() {
        let mut fetcher = TestFetcher::default();
        let peer_a = peer(1);
        let peer_b = peer(2);
        let h = hash(1);

        let (meta_a, _rx_a) = new_mock_session(peer_a, EthVersion::Eth66);
        let (meta_b, _rx_b) = new_mock_session(peer_b, EthVersion::Eth66);
        let mut peers = HashMap::default();
        peers.insert(peer_a, meta_a);
        peers.insert(peer_b, meta_b);

        // Both peers announce the same hash
        fetcher.insert_announced_for_test(h, peer_a, 100);
        push_if_absent(fetcher.announced.entry(h).or_default(), peer_b);
        let seq = fetcher.next_seq();
        fetcher
            .announces
            .entry(peer_b)
            .or_default()
            .insert(h, TxAnnounceMetadata { size: 100, seq });

        fetcher.schedule_fetches(&peers);

        // One peer fetches, the other is an alternate
        assert_eq!(fetcher.fetching.len(), 1);
        let fetching_peer = fetcher.fetching[&h];
        let other_peer = if fetching_peer == peer_a { peer_b } else { peer_a };
        assert!(fetcher.alternates.get(&h).unwrap().contains(&other_peer));
    }

    #[tokio::test]
    async fn test_schedule_channel_full_rollback() {
        let mut fetcher = TestFetcher::default();
        let peer_a = peer(1);
        let h = hash(1);

        // Create mock session with capacity 1, then fill it
        let (meta_a, mut mock_rx) = new_mock_session(peer_a, EthVersion::Eth66);
        let mut peers = HashMap::default();
        peers.insert(peer_a, meta_a);

        // Fill the channel by sending a dummy request
        let peer_ref = peers.get(&peer_a).unwrap();
        let (dummy_tx, _dummy_rx) = oneshot::channel();
        let dummy_req = PeerRequest::GetPooledTransactions {
            request: GetPooledTransactions(vec![]),
            response: dummy_tx,
        };
        peer_ref.request_tx.try_send(dummy_req).unwrap();

        fetcher.insert_announced_for_test(h, peer_a, 100);
        fetcher.schedule_fetches(&peers);

        // Hash should be rolled back to announced
        assert!(fetcher.announced.contains_key(&h));
        assert!(!fetcher.inflight_requests.contains_key(&peer_a));

        // Drain the filled channel
        let _ = mock_rx.recv().await;
    }

    #[tokio::test]
    async fn test_successful_fetch_event() {
        let mut fetcher = TestFetcher::default();
        let peer_a = peer(1);
        let (tx1, _tx2) = test_pooled_transactions();
        let h = *tx1.tx_hash();

        let (meta_a, mut mock_rx) = new_mock_session(peer_a, EthVersion::Eth66);
        let mut peers = HashMap::default();
        peers.insert(peer_a, meta_a);

        fetcher.insert_announced_for_test(h, peer_a, 100);
        fetcher.schedule_fetches(&peers);

        // Receive the request and respond
        let req = mock_rx.recv().await.unwrap();
        let PeerRequest::GetPooledTransactions { response, .. } = req else {
            panic!("expected GetPooledTransactions");
        };
        response.send(Ok(PooledTransactions(vec![tx1]))).unwrap();

        // Poll the stream
        let event = fetcher.next().await.unwrap();
        match event {
            FetchEvent::TransactionsFetched { peer_id, transactions, .. } => {
                assert_eq!(peer_id, peer_a);
                assert_eq!(transactions.len(), 1);
            }
            _ => panic!("expected TransactionsFetched"),
        }

        assert!(fetcher.inflight_requests.is_empty());
        assert!(fetcher.fetching.is_empty());
    }

    #[tokio::test]
    async fn test_empty_response() {
        let mut fetcher = TestFetcher::default();
        let peer_a = peer(1);
        let peer_b = peer(2);
        let h = hash(1);

        let (meta_a, mut mock_rx) = new_mock_session(peer_a, EthVersion::Eth66);
        let mut peers = HashMap::default();
        peers.insert(peer_a, meta_a);

        // Only peer_a announces and fetches; peer_b is set up as alternate manually
        fetcher.insert_announced_for_test(h, peer_a, 100);
        fetcher.schedule_fetches(&peers);

        // Simulate peer_b announcing the same hash while it's in-flight (Stage 2).
        // Production path: on_new_announcement adds both alternates and announces.
        push_if_absent(fetcher.alternates.entry(h).or_default(), peer_b);
        let seq = fetcher.next_seq();
        fetcher
            .announces
            .entry(peer_b)
            .or_default()
            .insert(h, TxAnnounceMetadata { size: 100, seq });

        let req = mock_rx.recv().await.unwrap();
        let PeerRequest::GetPooledTransactions { response, .. } = req else {
            panic!("expected GetPooledTransactions");
        };
        response.send(Ok(PooledTransactions(vec![]))).unwrap();

        let event = fetcher.next().await.unwrap();
        assert!(matches!(event, FetchEvent::EmptyResponse { .. }));

        // Hash should be returned to Stage 1 with the alternate peer
        assert!(fetcher.announced.contains_key(&h));
        assert!(fetcher.announced[&h].contains(&peer_b));
        assert_announces_consistent(&fetcher);
    }

    #[tokio::test]
    async fn test_error_response() {
        let mut fetcher = TestFetcher::default();
        let peer_a = peer(1);
        let peer_b = peer(2);
        let h = hash(1);

        let (meta_a, mut mock_rx) = new_mock_session(peer_a, EthVersion::Eth66);
        let mut peers = HashMap::default();
        peers.insert(peer_a, meta_a);

        fetcher.insert_announced_for_test(h, peer_a, 100);
        fetcher.schedule_fetches(&peers);

        // Simulate peer_b announcing while in-flight
        push_if_absent(fetcher.alternates.entry(h).or_default(), peer_b);
        let seq = fetcher.next_seq();
        fetcher
            .announces
            .entry(peer_b)
            .or_default()
            .insert(h, TxAnnounceMetadata { size: 100, seq });

        let req = mock_rx.recv().await.unwrap();
        let PeerRequest::GetPooledTransactions { response, .. } = req else {
            panic!("expected GetPooledTransactions");
        };
        response.send(Err(RequestError::BadResponse)).unwrap();

        let event = fetcher.next().await.unwrap();
        assert!(matches!(event, FetchEvent::FetchError { .. }));

        // Hash returned to Stage 1 via alternates
        assert!(fetcher.announced.contains_key(&h));
        assert!(fetcher.announced[&h].contains(&peer_b));
        assert_announces_consistent(&fetcher);
    }

    #[tokio::test]
    async fn test_channel_closed() {
        let mut fetcher = TestFetcher::default();
        let peer_a = peer(1);
        let h = hash(1);

        let (meta_a, mut mock_rx) = new_mock_session(peer_a, EthVersion::Eth66);
        let mut peers = HashMap::default();
        peers.insert(peer_a, meta_a);

        fetcher.insert_announced_for_test(h, peer_a, 100);
        fetcher.schedule_fetches(&peers);

        // Receive request then drop the response sender
        let req = mock_rx.recv().await.unwrap();
        let PeerRequest::GetPooledTransactions { response, .. } = req else {
            panic!("expected GetPooledTransactions");
        };
        drop(response);

        let event = fetcher.next().await.unwrap();
        match event {
            FetchEvent::FetchError { error, .. } => {
                assert!(matches!(error, RequestError::ChannelClosed));
            }
            _ => panic!("expected FetchError with ChannelClosed"),
        }
    }

    #[tokio::test]
    async fn test_unsolicited_filtered_and_reported() {
        let mut fetcher = TestFetcher::default();
        let peer_a = peer(1);
        let (tx1, tx2) = test_pooled_transactions();
        let h1 = *tx1.tx_hash();
        // tx2 hash was NOT requested

        let (meta_a, mut mock_rx) = new_mock_session(peer_a, EthVersion::Eth66);
        let mut peers = HashMap::default();
        peers.insert(peer_a, meta_a);

        // Only request h1
        fetcher.insert_announced_for_test(h1, peer_a, 100);
        fetcher.schedule_fetches(&peers);

        let req = mock_rx.recv().await.unwrap();
        let PeerRequest::GetPooledTransactions { response, .. } = req else {
            panic!("expected GetPooledTransactions");
        };
        // Respond with both tx1 (requested) and tx2 (unsolicited)
        response.send(Ok(PooledTransactions(vec![tx1, tx2]))).unwrap();

        let event = fetcher.next().await.unwrap();
        match event {
            FetchEvent::TransactionsFetched { transactions, report_peer, .. } => {
                assert_eq!(transactions.len(), 1);
                assert!(report_peer, "should report peer for unsolicited tx");
            }
            _ => panic!("expected TransactionsFetched"),
        }
    }

    #[tokio::test]
    async fn test_stolen_hash_not_retried() {
        let mut fetcher = TestFetcher::default();
        let peer_a = peer(1);
        let peer_b = peer(2);
        let (tx1, _tx2) = test_pooled_transactions();
        let h1 = *tx1.tx_hash();
        let h2 = hash(2);

        let (meta_a, mut mock_rx) = new_mock_session(peer_a, EthVersion::Eth66);
        let mut peers = HashMap::default();
        peers.insert(peer_a, meta_a);

        fetcher.insert_announced_for_test(h1, peer_a, 100);
        fetcher.insert_announced_for_test(h2, peer_a, 100);
        // Both peers announce h2, so schedule_fetches will naturally populate alternates
        // with peer_b when peer_a is selected to fetch.
        fetcher.insert_announced_for_test(h2, peer_b, 100);
        fetcher.schedule_fetches(&peers);

        // h1 gets "stolen" (received via broadcast)
        fetcher.on_transactions_received(once(&h1), true);

        // Respond with only tx1 (which was stolen)
        let req = mock_rx.recv().await.unwrap();
        let PeerRequest::GetPooledTransactions { response, .. } = req else {
            panic!("expected GetPooledTransactions");
        };
        response.send(Ok(PooledTransactions(vec![tx1]))).unwrap();

        let event = fetcher.next().await.unwrap();
        match event {
            FetchEvent::TransactionsFetched { peer_id, transactions, .. } => {
                assert_eq!(peer_id, peer_a);
                assert_eq!(transactions.len(), 1);
            }
            _ => panic!("expected TransactionsFetched"),
        }

        // h1 should NOT be in any stage (stolen and cleaned)
        assert_hash_in_single_stage(&fetcher, &h1, false);

        // h2 was not delivered and not stolen — should be returned to Stage 1
        // with the alternate peer (peer_b)
        assert_hash_in_single_stage(&fetcher, &h2, true);
        assert!(fetcher.announced.contains_key(&h2));
        assert!(fetcher.announced[&h2].contains(&peer_b));
        assert_announces_consistent(&fetcher);
    }

    #[tokio::test]
    async fn test_disconnect_redistributes_inflight() {
        let mut fetcher = TestFetcher::default();
        let peer_a = peer(1);
        let peer_b = peer(2);
        let h1 = hash(1);
        let h2 = hash(2);

        let (_tx, rx) = oneshot::channel();
        fetcher.fetching.insert(h1, peer_a);
        fetcher.fetching.insert(h2, peer_a);
        // Simulate peer_b announcing both hashes while in-flight — production path
        // adds to both alternates and announces.
        push_if_absent(fetcher.alternates.entry(h1).or_default(), peer_b);
        push_if_absent(fetcher.alternates.entry(h2).or_default(), peer_b);
        let seq = fetcher.next_seq();
        fetcher
            .announces
            .entry(peer_b)
            .or_default()
            .insert(h1, TxAnnounceMetadata { size: 100, seq });
        let seq = fetcher.next_seq();
        fetcher
            .announces
            .entry(peer_b)
            .or_default()
            .insert(h2, TxAnnounceMetadata { size: 100, seq });
        // Fetching peer also has announces entries (superset property)
        let seq = fetcher.next_seq();
        fetcher
            .announces
            .entry(peer_a)
            .or_default()
            .insert(h1, TxAnnounceMetadata { size: 100, seq });
        let seq = fetcher.next_seq();
        fetcher
            .announces
            .entry(peer_a)
            .or_default()
            .insert(h2, TxAnnounceMetadata { size: 100, seq });
        fetcher.inflight_requests.insert(
            peer_a,
            InflightRequest {
                hashes: vec![h1, h2],
                stolen: B256Set::default(),
                sent_at: Instant::now(),
                response: rx,
                dangling: false,
            },
        );

        fetcher.on_peer_disconnected(&peer_a, &HashMap::default());

        assert!(!fetcher.inflight_requests.contains_key(&peer_a));
        assert_hash_in_single_stage(&fetcher, &h1, true);
        assert!(fetcher.announced.contains_key(&h1));
        assert!(fetcher.announced[&h1].contains(&peer_b));
        assert_hash_in_single_stage(&fetcher, &h2, true);
        assert!(fetcher.announced.contains_key(&h2));
        assert!(fetcher.announced[&h2].contains(&peer_b));
        assert_announces_consistent(&fetcher);
    }

    #[tokio::test]
    async fn test_disconnect_cleans_announced_peer() {
        let mut fetcher = TestFetcher::default();
        let peer_a = peer(1);
        let peer_b = peer(2);
        let h = hash(1);

        fetcher.insert_announced_for_test(h, peer_a, 100);
        push_if_absent(fetcher.announced.entry(h).or_default(), peer_b);
        let seq = fetcher.next_seq();
        fetcher
            .announces
            .entry(peer_b)
            .or_default()
            .insert(h, TxAnnounceMetadata { size: 100, seq });

        fetcher.on_peer_disconnected(&peer_a, &HashMap::default());

        assert!(fetcher.announced.contains_key(&h));
        assert!(!fetcher.announced[&h].contains(&peer_a));
        assert!(fetcher.announced[&h].contains(&peer_b));
    }

    #[tokio::test]
    async fn test_timeout_marks_dangling() {
        let mut fetcher = TestFetcher::default();
        let peer_a = peer(1);
        let peer_b = peer(2);
        let h = hash(1);

        let (_tx, rx) = oneshot::channel();
        fetcher.fetching.insert(h, peer_a);
        push_if_absent(fetcher.alternates.entry(h).or_default(), peer_b);
        let seq = fetcher.next_seq();
        fetcher
            .announces
            .entry(peer_a)
            .or_default()
            .insert(h, TxAnnounceMetadata { size: 100, seq });
        let seq = fetcher.next_seq();
        fetcher
            .announces
            .entry(peer_b)
            .or_default()
            .insert(h, TxAnnounceMetadata { size: 100, seq });
        fetcher.inflight_requests.insert(
            peer_a,
            InflightRequest {
                hashes: vec![h],
                stolen: B256Set::default(),
                sent_at: Instant::now() - Duration::from_secs(6), // past 5s timeout
                response: rx,
                dangling: false,
            },
        );

        fetcher.drain_timed_out_requests();

        assert!(fetcher.inflight_requests[&peer_a].dangling);
        // Hash moved to announced via alternates
        assert!(fetcher.announced.contains_key(&h));
        assert!(fetcher.announced[&h].contains(&peer_b));
        assert!(!fetcher.fetching.contains_key(&h));
    }

    #[tokio::test]
    async fn test_per_peer_announce_limit() {
        let mut fetcher = TestFetcher::default();
        fetcher.info.max_announces_per_peer = 3;
        let peer_a = peer(1);

        let (meta_a, _rx) = new_mock_session(peer_a, EthVersion::Eth68);
        let mut peers = HashMap::default();
        peers.insert(peer_a, meta_a);

        // First 3 should succeed (go to announced)
        let ann1 = make_eth68_announcement(vec![
            (hash(1), 0x02, 100),
            (hash(2), 0x02, 100),
            (hash(3), 0x02, 100),
        ]);
        fetcher.on_new_announcement(peer_a, ann1, &peers);
        assert_eq!(fetcher.announces.get(&peer_a).map_or(0, |m| m.len()), 3);

        // 4th should be dropped (peer at limit: 3 in announces)
        let ann2 = make_eth68_announcement(vec![(hash(4), 0x02, 100)]);
        fetcher.on_new_announcement(peer_a, ann2, &peers);
        assert!(!fetcher.announced.contains_key(&hash(4)));
        assert!(!fetcher.fetching.contains_key(&hash(4)));
    }

    #[tokio::test]
    async fn test_global_capacity_limit_drops_announcement() {
        let mut fetcher = TestFetcher::default();
        fetcher.info.max_capacity_cache_txns_pending_fetch = 2;
        let peer_a = peer(1);

        let (meta_a, _rx) = new_mock_session(peer_a, EthVersion::Eth68);
        let mut peers = HashMap::default();
        peers.insert(peer_a, meta_a);

        // Fill announced to capacity
        fetcher.insert_announced_for_test(hash(1), peer_a, 100);
        fetcher.insert_announced_for_test(hash(2), peer_a, 100);

        // New tx should be dropped when announced is at capacity
        let ann = make_eth68_announcement(vec![(hash(3), 0x02, 100)]);
        fetcher.on_new_announcement(peer_a, ann, &peers);

        assert_eq!(fetcher.announced.len(), 2);
        assert!(!fetcher.announced.contains_key(&hash(3)));
    }

    #[tokio::test]
    async fn test_announcement_adds_alternate_for_fetching_hash() {
        let mut fetcher = TestFetcher::default();
        let peer_a = peer(1);
        let peer_b = peer(2);
        let h = hash(1);

        // h already in Stage 2 by peer_a
        fetcher.fetching.insert(h, peer_a);

        let (meta_b, _rx) = new_mock_session(peer_b, EthVersion::Eth68);
        let mut peers = HashMap::default();
        peers.insert(peer_b, meta_b);

        let ann = make_eth68_announcement(vec![(h, 0x02, 100)]);
        fetcher.on_new_announcement(peer_b, ann, &peers);

        assert!(fetcher.alternates.get(&h).unwrap().contains(&peer_b));
        assert!(fetcher.announces.get(&peer_b).unwrap().contains_key(&h));
    }

    #[tokio::test]
    async fn test_announcement_adds_peer_to_announced_hash() {
        let mut fetcher = TestFetcher::default();
        let peer_a = peer(1);
        let peer_b = peer(2);
        let h = hash(1);

        fetcher.insert_announced_for_test(h, peer_a, 100);

        let (meta_b, _rx) = new_mock_session(peer_b, EthVersion::Eth68);
        let mut peers = HashMap::default();
        peers.insert(peer_b, meta_b);

        let ann = make_eth68_announcement(vec![(h, 0x02, 100)]);
        fetcher.on_new_announcement(peer_b, ann, &peers);

        assert!(fetcher.announced[&h].contains(&peer_b));
        assert!(fetcher.announces.get(&peer_b).unwrap().contains_key(&h));
    }

    #[tokio::test]
    async fn test_full_lifecycle_fail_and_retry_on_alternate() {
        let mut fetcher = TestFetcher::default();
        let peer_a = peer(1);
        let peer_b = peer(2);
        let (tx1, _tx2) = test_pooled_transactions();
        let h = *tx1.tx_hash();

        let (meta_a, mut rx_a) = new_mock_session(peer_a, EthVersion::Eth66);
        let (meta_b, mut rx_b) = new_mock_session(peer_b, EthVersion::Eth66);
        let mut peers = HashMap::default();
        peers.insert(peer_a, meta_a);
        peers.insert(peer_b, meta_b);

        // Both peers announce the hash
        fetcher.insert_announced_for_test(h, peer_a, 100);
        push_if_absent(fetcher.announced.entry(h).or_default(), peer_b);
        let seq = fetcher.next_seq();
        fetcher
            .announces
            .entry(peer_b)
            .or_default()
            .insert(h, TxAnnounceMetadata { size: 100, seq });

        // First schedule: one peer gets it
        fetcher.schedule_fetches(&peers);
        let fetching_peer = fetcher.fetching[&h];

        // Fail the first peer
        let (mock_rx, other_rx) =
            if fetching_peer == peer_a { (&mut rx_a, &mut rx_b) } else { (&mut rx_b, &mut rx_a) };
        let req = mock_rx.recv().await.unwrap();
        let PeerRequest::GetPooledTransactions { response, .. } = req else {
            panic!("expected GetPooledTransactions");
        };
        response.send(Err(RequestError::Timeout)).unwrap();

        let event = fetcher.next().await.unwrap();
        assert!(matches!(event, FetchEvent::FetchError { .. }));

        // Hash should be back in announced with the alternate
        assert!(fetcher.announced.contains_key(&h));

        // Schedule again: alternate gets the request
        fetcher.schedule_fetches(&peers);
        assert!(fetcher.fetching.contains_key(&h));

        let req = other_rx.recv().await.unwrap();
        let PeerRequest::GetPooledTransactions { response, .. } = req else {
            panic!("expected GetPooledTransactions");
        };
        response.send(Ok(PooledTransactions(vec![tx1]))).unwrap();

        let event = fetcher.next().await.unwrap();
        assert!(matches!(event, FetchEvent::TransactionsFetched { .. }));

        // Clean
        assert!(fetcher.announced.is_empty());
        assert!(fetcher.fetching.is_empty());
        assert!(fetcher.inflight_requests.is_empty());
    }

    #[tokio::test]
    async fn test_concurrent_fetches_multiple_peers() {
        let mut fetcher = TestFetcher::default();
        let peer_a = peer(1);
        let peer_b = peer(2);
        let peer_c = peer(3);
        let h1 = hash(1);
        let h2 = hash(2);
        let h3 = hash(3);

        let (meta_a, mut rx_a) = new_mock_session(peer_a, EthVersion::Eth66);
        let (meta_b, mut rx_b) = new_mock_session(peer_b, EthVersion::Eth66);
        let (meta_c, mut rx_c) = new_mock_session(peer_c, EthVersion::Eth66);
        let mut peers = HashMap::default();
        peers.insert(peer_a, meta_a);
        peers.insert(peer_b, meta_b);
        peers.insert(peer_c, meta_c);

        fetcher.insert_announced_for_test(h1, peer_a, 100);
        fetcher.insert_announced_for_test(h2, peer_b, 100);
        fetcher.insert_announced_for_test(h3, peer_c, 100);

        fetcher.schedule_fetches(&peers);

        assert_eq!(fetcher.inflight_requests.len(), 3);
        assert_eq!(fetcher.fetching.len(), 3);
        assert!(fetcher.announced.is_empty());

        // Complete all three with empty responses (simplest way to resolve)
        for rx in [&mut rx_a, &mut rx_b, &mut rx_c] {
            let req = rx.recv().await.unwrap();
            let PeerRequest::GetPooledTransactions { response, .. } = req else {
                panic!("expected GetPooledTransactions");
            };
            response.send(Ok(PooledTransactions(vec![]))).unwrap();
        }

        // Poll three events
        for _ in 0..3 {
            let event = fetcher.next().await.unwrap();
            assert!(matches!(event, FetchEvent::EmptyResponse { .. }));
        }

        assert!(fetcher.inflight_requests.is_empty());
        assert!(fetcher.fetching.is_empty());
    }

    #[tokio::test]
    async fn test_dangling_request_yields_empty_response() {
        let mut fetcher = TestFetcher::default();
        let peer_a = peer(1);

        let (tx, rx) = oneshot::channel();
        fetcher.inflight_requests.insert(
            peer_a,
            InflightRequest {
                hashes: vec![],
                stolen: B256Set::default(),
                sent_at: Instant::now(),
                response: rx,
                dangling: true,
            },
        );

        // Send a response on the dangling request
        tx.send(Ok(PooledTransactions(vec![]))).unwrap();

        // Poll through the actual Stream implementation
        let event = fetcher.next().await.unwrap();
        assert!(matches!(event, FetchEvent::EmptyResponse { peer_id } if peer_id == peer_a));
        assert!(fetcher.inflight_requests.is_empty());
    }

    #[test]
    fn test_return_hashes_cleans_failed_peer_announces() {
        let mut fetcher = TestFetcher::default();
        let peer_a = peer(1);
        let peer_b = peer(2);
        let h = hash(1);

        // Set up: h is being fetched by peer_a, peer_b is alternate
        fetcher.fetching.insert(h, peer_a);
        push_if_absent(fetcher.alternates.entry(h).or_default(), peer_b);
        let seq = fetcher.next_seq();
        fetcher
            .announces
            .entry(peer_a)
            .or_default()
            .insert(h, TxAnnounceMetadata { size: 100, seq });
        let seq = fetcher.next_seq();
        fetcher
            .announces
            .entry(peer_b)
            .or_default()
            .insert(h, TxAnnounceMetadata { size: 100, seq });

        fetcher.return_hashes_to_announced(&[h], &B256Set::default(), &peer_a);

        // h should be in announced with peer_b only
        assert!(fetcher.announced.contains_key(&h));
        assert!(fetcher.announced[&h].contains(&peer_b));
        // peer_a's announces entry should be cleaned
        assert!(
            !fetcher.announces.contains_key(&peer_a) ||
                !fetcher.announces[&peer_a].contains_key(&h)
        );
        assert_announces_consistent(&fetcher);
    }

    #[tokio::test]
    async fn test_timeout_fires_via_poll_next() {
        tokio::time::pause();

        let mut fetcher = TestFetcher::default();
        let peer_a = peer(1);
        let peer_b = peer(2);
        let h = hash(1);

        let (meta_a, _rx_a) = new_mock_session(peer_a, EthVersion::Eth66);
        let mut peers = HashMap::default();
        peers.insert(peer_a, meta_a);

        fetcher.insert_announced_for_test(h, peer_a, 100);
        // Add peer_b as alternate so h can be retried
        push_if_absent(fetcher.announced.entry(h).or_default(), peer_b);
        let seq = fetcher.next_seq();
        fetcher
            .announces
            .entry(peer_b)
            .or_default()
            .insert(h, TxAnnounceMetadata { size: 100, seq });

        fetcher.schedule_fetches(&peers);
        assert!(fetcher.fetching.contains_key(&h));
        assert_hash_in_single_stage(&fetcher, &h, true);

        // Advance past TX_FETCH_TIMEOUT (5s) + slack (100ms)
        tokio::time::advance(Duration::from_secs(6)).await;

        // Poll should fire the timeout timer
        poll_fn(|cx| {
            let _ = fetcher.poll_next_unpin(cx);
            Poll::Ready(())
        })
        .await;

        // Hash should be moved back to announced via alternates
        assert!(!fetcher.fetching.contains_key(&h));
        assert!(fetcher.announced.contains_key(&h));
        assert!(fetcher.announced[&h].contains(&peer_b));
        assert_hash_in_single_stage(&fetcher, &h, true);

        // Request should be dangling
        assert!(fetcher.inflight_requests[&peer_a].dangling);
    }

    #[tokio::test]
    async fn test_dangling_sweep_via_poll_next() {
        tokio::time::pause();

        let mut fetcher = TestFetcher::default();
        let peer_a = peer(1);
        let h = hash(1);

        let (meta_a, _rx_a) = new_mock_session(peer_a, EthVersion::Eth66);
        let mut peers = HashMap::default();
        peers.insert(peer_a, meta_a);

        fetcher.insert_announced_for_test(h, peer_a, 100);
        fetcher.schedule_fetches(&peers);
        assert!(fetcher.fetching.contains_key(&h));

        // Advance past TX_FETCH_TIMEOUT to trigger dangling
        tokio::time::advance(Duration::from_secs(6)).await;
        poll_fn(|cx| {
            let _ = fetcher.poll_next_unpin(cx);
            Poll::Ready(())
        })
        .await;
        assert!(fetcher.inflight_requests[&peer_a].dangling);

        // Advance past DANGLING_TIMEOUT_MULTIPLIER × TX_FETCH_TIMEOUT (20s total)
        tokio::time::advance(Duration::from_secs(15)).await;
        poll_fn(|cx| {
            let _ = fetcher.poll_next_unpin(cx);
            Poll::Ready(())
        })
        .await;

        // Dangling request should be swept
        assert!(!fetcher.inflight_requests.contains_key(&peer_a));
    }

    #[tokio::test]
    async fn test_all_unsolicited_response_yields_fetch_error() {
        let mut fetcher = TestFetcher::default();
        let peer_a = peer(1);
        let peer_b = peer(2);
        let (_tx1, tx2) = test_pooled_transactions();
        let h = hash(1); // requested hash (won't match tx2)

        let (meta_a, mut mock_rx) = new_mock_session(peer_a, EthVersion::Eth66);
        let mut peers = HashMap::default();
        peers.insert(peer_a, meta_a);

        // Both peers announce h so schedule_fetches naturally populates alternates
        fetcher.insert_announced_for_test(h, peer_a, 100);
        fetcher.insert_announced_for_test(h, peer_b, 100);
        fetcher.schedule_fetches(&peers);

        let req = mock_rx.recv().await.unwrap();
        let PeerRequest::GetPooledTransactions { response, .. } = req else {
            panic!("expected GetPooledTransactions");
        };
        // Respond with only tx2, which was NOT requested
        response.send(Ok(PooledTransactions(vec![tx2]))).unwrap();

        let event = fetcher.next().await.unwrap();
        match event {
            FetchEvent::FetchError { peer_id, error } => {
                assert_eq!(peer_id, peer_a);
                assert!(matches!(error, RequestError::BadResponse));
            }
            _ => panic!("expected FetchError with BadResponse for all-unsolicited"),
        }

        // h should be returned to Stage 1 with peer_b (and peer_a via alternates)
        assert_hash_in_single_stage(&fetcher, &h, true);
        assert!(fetcher.announced.contains_key(&h));
        assert!(fetcher.announced[&h].contains(&peer_b));
        assert_announces_consistent(&fetcher);
    }

    /// Asserts that the announces index is consistent with announced + alternates.
    /// Every `announces[peer][hash]` must have a corresponding entry in announced, alternates,
    /// or fetching. And vice versa: every peer in announced/alternates must appear in announces.
    #[track_caller]
    fn assert_announces_consistent(fetcher: &TestFetcher) {
        // Forward check: every announces entry references a valid stage.
        // With the superset model, the fetching peer's entry also remains.
        for (peer, hashes) in &fetcher.announces {
            for hash in hashes.keys() {
                let in_announced = fetcher.announced.get(hash).is_some_and(|ps| ps.contains(peer));
                let in_alternates =
                    fetcher.alternates.get(hash).is_some_and(|ps| ps.contains(peer));
                let in_fetching = fetcher.fetching.get(hash).is_some_and(|p| p == peer);
                assert!(
                    in_announced || in_alternates || in_fetching,
                    "orphan announces entry: peer={peer:#}, hash={hash} not in announced/alternates/fetching"
                );
            }
        }

        // Reverse check: every peer in announced has an announces entry
        for (hash, peers) in &fetcher.announced {
            for peer in peers {
                assert!(
                    fetcher.announces.get(peer).is_some_and(|m| m.contains_key(hash)),
                    "peer {peer:#} in announced[{hash}] but missing from announces"
                );
            }
        }

        // Reverse check: every peer in alternates has an announces entry
        for (hash, peers) in &fetcher.alternates {
            for peer in peers {
                assert!(
                    fetcher.announces.get(peer).is_some_and(|m| m.contains_key(hash)),
                    "peer {peer:#} in alternates[{hash}] but missing from announces"
                );
            }
        }
    }

    #[tokio::test]
    async fn test_schedule_fetches_rejects_non_owner_peer() {
        let mut fetcher = TestFetcher::default();
        let peer_a = peer(1);
        let peer_b = peer(2);
        let h = hash(1);

        let (meta_a, _rx_a) = new_mock_session(peer_a, EthVersion::Eth66);
        let (meta_b, _rx_b) = new_mock_session(peer_b, EthVersion::Eth66);
        let mut peers = HashMap::default();
        peers.insert(peer_a, meta_a);
        peers.insert(peer_b, meta_b);

        // h is in Stage 1 but only owned by peer_a
        fetcher.insert_announced_for_test(h, peer_a, 100);

        // Manually add h to peer_b's announces (simulating stale alternate entry)
        // but do NOT add peer_b to announced[h]
        let seq = fetcher.next_seq();
        fetcher
            .announces
            .entry(peer_b)
            .or_default()
            .insert(h, TxAnnounceMetadata { size: 100, seq });

        fetcher.schedule_fetches(&peers);

        // peer_a should have fetched — peer_b should NOT have
        // claimed h because it's not in announced[h]'s peer set
        assert!(fetcher.fetching.contains_key(&h), "hash should have been moved to Stage 2");
        assert_eq!(fetcher.fetching[&h], peer_a, "only the owner peer should fetch the hash");
    }

    #[tokio::test]
    async fn test_stage1_to_stage2_preserves_alternate_announces() {
        let mut fetcher = TestFetcher::default();
        let peer_a = peer(1);
        let peer_b = peer(2);
        let h = hash(1);

        let (meta_a, _rx_a) = new_mock_session(peer_a, EthVersion::Eth66);
        let (meta_b, _rx_b) = new_mock_session(peer_b, EthVersion::Eth66);
        let mut peers = HashMap::default();
        peers.insert(peer_a, meta_a);
        peers.insert(peer_b, meta_b);

        // Both peers announce the same hash
        fetcher.insert_announced_for_test(h, peer_a, 100);
        push_if_absent(fetcher.announced.entry(h).or_default(), peer_b);
        let seq = fetcher.next_seq();
        fetcher
            .announces
            .entry(peer_b)
            .or_default()
            .insert(h, TxAnnounceMetadata { size: 100, seq });

        fetcher.schedule_fetches(&peers);

        // One peer fetches, the other becomes an alternate
        assert!(fetcher.fetching.contains_key(&h));
        let fetching_peer = fetcher.fetching[&h];
        let other_peer = if fetching_peer == peer_a { peer_b } else { peer_a };

        // The alternate peer's announces entry should be PRESERVED (superset model)
        assert!(
            fetcher.announces.get(&other_peer).is_some_and(|m| m.contains_key(&h)),
            "alternate peer's announces should be preserved during Stage 1→2"
        );
        // And the alternate should be tracked
        assert!(fetcher.alternates.get(&h).unwrap().contains(&other_peer));

        // Full consistency check
        assert_announces_consistent(&fetcher);
    }

    #[tokio::test]
    async fn test_return_to_stage1_restores_announces() {
        let mut fetcher = TestFetcher::default();
        let peer_a = peer(1);
        let peer_b = peer(2);
        let h = hash(1);

        let (meta_a, mut rx_a) = new_mock_session(peer_a, EthVersion::Eth66);
        let (meta_b, mut rx_b) = new_mock_session(peer_b, EthVersion::Eth66);
        let mut peers = HashMap::default();
        peers.insert(peer_a, meta_a);
        peers.insert(peer_b, meta_b);

        // Both peers announce
        fetcher.insert_announced_for_test(h, peer_a, 100);
        push_if_absent(fetcher.announced.entry(h).or_default(), peer_b);
        let seq = fetcher.next_seq();
        fetcher
            .announces
            .entry(peer_b)
            .or_default()
            .insert(h, TxAnnounceMetadata { size: 100, seq });

        // Schedule: one peer fetches, the other becomes an alternate
        fetcher.schedule_fetches(&peers);
        let fetching_peer = fetcher.fetching[&h];
        let other_peer = if fetching_peer == peer_a { peer_b } else { peer_a };

        // Fail the fetching peer
        let rx = if fetching_peer == peer_a { &mut rx_a } else { &mut rx_b };
        let req = rx.recv().await.unwrap();
        let PeerRequest::GetPooledTransactions { response, .. } = req else {
            panic!("expected GetPooledTransactions");
        };
        response.send(Err(RequestError::Timeout)).unwrap();

        let event = fetcher.next().await.unwrap();
        assert!(matches!(event, FetchEvent::FetchError { .. }));

        // h should be back in announced with the other peer
        assert!(fetcher.announced.contains_key(&h));
        assert!(fetcher.announced[&h].contains(&other_peer));

        // other peer's announces entry is still present (superset model — never removed)
        assert!(
            fetcher.announces.get(&other_peer).is_some_and(|m| m.contains_key(&h)),
            "announces must be preserved for alternate peer across Stage 1→2→1"
        );

        assert_announces_consistent(&fetcher);

        // Second schedule: the other peer should now pick it up
        fetcher.schedule_fetches(&peers);
        assert!(fetcher.fetching.contains_key(&h));
    }

    #[tokio::test]
    async fn test_fifo_ordering_across_peers() {
        let mut fetcher = TestFetcher::default();
        let peer_a = peer(1);
        let h1 = hash(1);
        let h2 = hash(2);
        let h3 = hash(3);

        let (meta_a, mut rx_a) = new_mock_session(peer_a, EthVersion::Eth66);
        let mut peers = HashMap::default();
        peers.insert(peer_a, meta_a);

        // Insert in reverse order — h3 gets lowest seq, h1 gets highest
        fetcher.insert_announced_for_test(h3, peer_a, 100);
        fetcher.insert_announced_for_test(h2, peer_a, 100);
        fetcher.insert_announced_for_test(h1, peer_a, 100);

        fetcher.schedule_fetches(&peers);

        let req = rx_a.recv().await.unwrap();
        let PeerRequest::GetPooledTransactions { request, .. } = req else {
            panic!("expected GetPooledTransactions");
        };

        // Verify FIFO: h3 (seq=0) should come before h2 (seq=1) before h1 (seq=2)
        let requested = request.0;
        let pos_h3 = requested.iter().position(|h| *h == h3).unwrap();
        let pos_h2 = requested.iter().position(|h| *h == h2).unwrap();
        let pos_h1 = requested.iter().position(|h| *h == h1).unwrap();
        assert!(pos_h3 < pos_h2, "h3 (older) should be requested before h2");
        assert!(pos_h2 < pos_h1, "h2 (older) should be requested before h1");
    }

    #[tokio::test]
    async fn test_many_peers_same_hash_only_one_fetch() {
        let mut fetcher = TestFetcher::default();
        let h = hash(1);
        let mut peers = HashMap::default();
        let peer_ids: Vec<PeerId> = (1..=10).map(peer).collect();
        // Hold receivers to keep mock session channels alive.
        #[allow(clippy::collection_is_never_read)]
        let mut rxs = Vec::new();

        for &pid in &peer_ids {
            let (meta, rx) = new_mock_session(pid, EthVersion::Eth66);
            peers.insert(pid, meta);
            rxs.push(rx);
            fetcher.insert_announced_for_test(h, pid, 100);
        }

        fetcher.schedule_fetches(&peers);

        // Exactly one peer should be fetching
        assert_eq!(fetcher.fetching.len(), 1);
        assert!(fetcher.fetching.contains_key(&h));
        let fetching_peer = fetcher.fetching[&h];

        // The other 9 should be alternates
        let alts = fetcher.alternates.get(&h).unwrap();
        assert_eq!(alts.len(), 9);
        assert!(!alts.contains(&fetching_peer));

        // Hash should not be in announced
        assert!(!fetcher.announced.contains_key(&h));
        assert_hash_in_single_stage(&fetcher, &h, true);
    }

    #[tokio::test]
    async fn test_disconnect_during_pending_yield() {
        let mut fetcher = TestFetcher::default();
        let peer_a = peer(1);
        let peer_b = peer(2);
        let (tx1, _) = test_pooled_transactions();
        let h1 = *tx1.tx_hash();
        let h2 = hash(2);

        let (meta_a, mut rx_a) = new_mock_session(peer_a, EthVersion::Eth66);
        let (meta_b, mut rx_b) = new_mock_session(peer_b, EthVersion::Eth66);
        let mut peers = HashMap::default();
        peers.insert(peer_a, meta_a);
        peers.insert(peer_b, meta_b);

        fetcher.insert_announced_for_test(h1, peer_a, 100);
        fetcher.insert_announced_for_test(h2, peer_b, 100);

        fetcher.schedule_fetches(&peers);

        // Both respond
        let req_a = rx_a.recv().await.unwrap();
        let PeerRequest::GetPooledTransactions { response: resp_a, .. } = req_a else {
            panic!("expected GetPooledTransactions");
        };
        resp_a.send(Ok(PooledTransactions(vec![tx1]))).unwrap();

        let req_b = rx_b.recv().await.unwrap();
        let PeerRequest::GetPooledTransactions { response: resp_b, .. } = req_b else {
            panic!("expected GetPooledTransactions");
        };
        resp_b.send(Ok(PooledTransactions(vec![]))).unwrap();

        // First poll: both complete, one yielded, one stashed in pending_yield
        let event1 = fetcher.next().await.unwrap();
        assert!(matches!(
            &event1,
            FetchEvent::TransactionsFetched { .. } | FetchEvent::EmptyResponse { .. }
        ));

        // Disconnect peer_b while pending_yield still has its result
        fetcher.on_peer_disconnected(&peer_b, &peers);

        // Second poll: should still yield the stashed result without panic
        let event2 = fetcher.next().await;
        let event2 = event2.expect("pending_yield should still produce the second result");
        assert!(
            matches!(
                &event2,
                FetchEvent::TransactionsFetched { peer_id, .. } |
                FetchEvent::EmptyResponse { peer_id, .. }
                if *peer_id == peer_a || *peer_id == peer_b
            ),
            "second event should be a valid fetch result from one of the two peers, got: {event2:?}"
        );
    }

    #[tokio::test]
    async fn test_channel_full_preserves_alternate_announces() {
        let mut fetcher = TestFetcher::default();
        let peer_a = peer(1);
        let h = hash(1);

        let (meta_a, mut mock_rx) = new_mock_session(peer_a, EthVersion::Eth66);
        let mut peers = HashMap::default();
        peers.insert(peer_a, meta_a);

        // Only peer_a announces (single peer = deterministic selection)
        fetcher.insert_announced_for_test(h, peer_a, 100);

        // Fill peer_a's channel to force try_send failure
        let peer_ref = peers.get(&peer_a).unwrap();
        let (dummy_tx, _dummy_rx) = oneshot::channel();
        let dummy_req = PeerRequest::GetPooledTransactions {
            request: GetPooledTransactions(vec![]),
            response: dummy_tx,
        };
        peer_ref.request_tx.try_send(dummy_req).unwrap();

        fetcher.schedule_fetches(&peers);

        // With two-phase commit, h stays in announced on channel failure.
        // No state was mutated, so announces must be intact.
        assert!(fetcher.announced.contains_key(&h), "hash should stay in announced");
        assert!(!fetcher.fetching.contains_key(&h), "hash should NOT be in fetching");
        assert!(
            fetcher.announces.get(&peer_a).is_some_and(|m| m.contains_key(&h)),
            "peer_a announces should be intact after channel-full skip"
        );
        assert_announces_consistent(&fetcher);

        // Drain the filled channel
        let _ = mock_rx.recv().await;
    }

    #[tokio::test]
    async fn test_cutoff_heuristic_for_partial_response() {
        let mut fetcher = TestFetcher::default();
        let peer_a = peer(1);
        let peer_b = peer(2);
        let (tx1, _tx2) = test_pooled_transactions();
        let h1 = hash(1); // will NOT be delivered
        let h2 = hash(2); // will NOT be delivered
        let h3 = *tx1.tx_hash(); // WILL be delivered

        let (meta_a, mut mock_rx) = new_mock_session(peer_a, EthVersion::Eth66);
        let mut peers = HashMap::default();
        peers.insert(peer_a, meta_a);

        // Insert in FIFO order: h1, h2, h3
        fetcher.insert_announced_for_test(h1, peer_a, 100);
        fetcher.insert_announced_for_test(h2, peer_a, 100);
        fetcher.insert_announced_for_test(h3, peer_a, 100);

        // peer_b is alternate for all three
        for h in [h1, h2, h3] {
            push_if_absent(fetcher.announced.entry(h).or_default(), peer_b);
            let seq = fetcher.next_seq();
            fetcher
                .announces
                .entry(peer_b)
                .or_default()
                .insert(h, TxAnnounceMetadata { size: 100, seq });
        }

        fetcher.schedule_fetches(&peers);

        // Respond with only tx1 (h3) — h1, h2 are "before the cutoff" (skipped)
        let req = mock_rx.recv().await.unwrap();
        let PeerRequest::GetPooledTransactions { request, response, .. } = req else {
            panic!("expected GetPooledTransactions");
        };

        // Verify FIFO ordering in request
        let pos_h1 = request.0.iter().position(|h| *h == h1);
        let pos_h3 = request.0.iter().position(|h| *h == h3);
        assert!(pos_h1.is_some() && pos_h3.is_some());

        response.send(Ok(PooledTransactions(vec![tx1]))).unwrap();

        let event = fetcher.next().await.unwrap();
        assert!(matches!(event, FetchEvent::TransactionsFetched { .. }));

        // h1, h2 are before the cutoff — peer_a should be removed from their
        // candidates (it intentionally skipped them). They should go back to
        // Stage 1 with only peer_b.
        assert!(
            fetcher.announced.contains_key(&h1),
            "h1 should be back in Stage 1 (announced) — it was before the cutoff"
        );
        assert!(
            !fetcher.announced[&h1].contains(&peer_a),
            "peer_a should NOT be in announced[h1] — it skipped h1 (before cutoff)"
        );
        assert!(
            fetcher.announced[&h1].contains(&peer_b),
            "peer_b should be in announced[h1] as alternate"
        );

        // h2 also before the cutoff — same treatment
        assert!(
            fetcher.announced.contains_key(&h2),
            "h2 should be back in Stage 1 (announced) — it was before the cutoff"
        );
        assert!(
            !fetcher.announced[&h2].contains(&peer_a),
            "peer_a should NOT be in announced[h2] — it skipped h2 (before cutoff)"
        );
        assert!(
            fetcher.announced[&h2].contains(&peer_b),
            "peer_b should be in announced[h2] as alternate"
        );

        // h3 was delivered — should be fully cleaned
        assert_hash_in_single_stage(&fetcher, &h3, false);
        assert_announces_consistent(&fetcher);
    }

    #[tokio::test]
    async fn test_truncation_no_alternates_drops_hash() {
        let mut fetcher = TestFetcher::default();
        let peer_a = peer(1);
        let (tx1, _tx2) = test_pooled_transactions();
        let h1 = *tx1.tx_hash(); // WILL be delivered (first in FIFO)
        let h2 = hash(2); // will NOT be delivered — after cutoff, no alternates

        let (meta_a, mut mock_rx) = new_mock_session(peer_a, EthVersion::Eth66);
        let mut peers = HashMap::default();
        peers.insert(peer_a, meta_a);

        // h1 gets lower seq (inserted first)
        fetcher.insert_announced_for_test(h1, peer_a, 100);
        fetcher.insert_announced_for_test(h2, peer_a, 100);
        // No alternates for h2 — only peer_a can serve it

        fetcher.schedule_fetches(&peers);
        assert!(fetcher.fetching.contains_key(&h1));
        assert!(fetcher.fetching.contains_key(&h2));

        // Respond with only tx1 (h1). h2 is after cutoff (benign truncation).
        let req = mock_rx.recv().await.unwrap();
        let PeerRequest::GetPooledTransactions { response, .. } = req else {
            panic!("expected GetPooledTransactions");
        };
        response.send(Ok(PooledTransactions(vec![tx1]))).unwrap();

        let event = fetcher.next().await.unwrap();
        assert!(matches!(event, FetchEvent::TransactionsFetched { .. }));

        // h1 delivered — fully cleaned
        assert_hash_in_single_stage(&fetcher, &h1, false);

        // h2 was after cutoff (benign truncation) — the cutoff heuristic would keep
        // peer_a as a candidate, but peer_a was already excluded from alternates during
        // the Stage 1→2 transition. With no other peers, alternates is empty,
        // so h2 is dropped.
        assert_hash_in_single_stage(&fetcher, &h2, false);
        assert!(!fetcher.announced.contains_key(&h2), "h2 should be dropped");
        assert_announces_consistent(&fetcher);
    }

    #[tokio::test]
    async fn test_fetching_peer_reannounce_not_added_to_alternates() {
        let mut fetcher = TestFetcher::default();
        let peer_a = peer(1);
        let h = hash(1);

        // h is being fetched by peer_a
        fetcher.fetching.insert(h, peer_a);
        let seq = fetcher.next_seq();
        fetcher
            .announces
            .entry(peer_a)
            .or_default()
            .insert(h, TxAnnounceMetadata { size: 100, seq });

        let (meta_a, _rx) = new_mock_session(peer_a, EthVersion::Eth68);
        let mut peers = HashMap::default();
        peers.insert(peer_a, meta_a);

        // peer_a re-announces the same hash it's already fetching
        let ann = make_eth68_announcement(vec![(h, 0x02, 100)]);
        fetcher.on_new_announcement(peer_a, ann, &peers);

        // peer_a must NOT appear in alternates for its own fetch (invariant #3)
        assert!(
            !fetcher.alternates.contains_key(&h) || !fetcher.alternates[&h].contains(&peer_a),
            "fetching peer must not be in its own alternates"
        );
        // announces entry should still exist (superset index)
        assert!(fetcher.announces.get(&peer_a).unwrap().contains_key(&h));
    }

    #[tokio::test]
    async fn test_schedule_fetches_skips_no_stage1_work_peers() {
        let mut fetcher = TestFetcher::default();
        fetcher.info.max_inflight_requests = 1;

        let peer_a = peer(1);
        let peer_b = peer(2);
        let h = hash(1);

        let (meta_a, _rx_a) = new_mock_session(peer_a, EthVersion::Eth66);
        let (meta_b, _rx_b) = new_mock_session(peer_b, EthVersion::Eth66);
        let mut peers = HashMap::default();
        peers.insert(peer_a, meta_a);
        peers.insert(peer_b, meta_b);

        // h is in Stage 1 owned by peer_b only
        fetcher.insert_announced_for_test(h, peer_b, 100);

        // peer_a has an announces entry (e.g. from a prior alternate) but does NOT
        // own h in Stage 1 (announced). It should be skipped, not consume a slot.
        let seq = fetcher.next_seq();
        fetcher
            .announces
            .entry(peer_a)
            .or_default()
            .insert(h, TxAnnounceMetadata { size: 100, seq });

        fetcher.schedule_fetches(&peers);

        // peer_b must have fetched despite max_inflight=1, because peer_a's empty
        // request should not consume the slot.
        assert!(
            fetcher.fetching.contains_key(&h),
            "hash should be in fetching — peer_b should have been reached"
        );
        assert_eq!(fetcher.fetching[&h], peer_b);
    }

    #[test]
    fn test_size_backfill_on_reannounce() {
        let mut fetcher = TestFetcher::default();
        let peer_a = peer(1);
        let h = hash(1);

        // First announcement with unknown size (ETH66-style)
        fetcher.insert_peer_announce(&peer_a, &h, &None);
        assert_eq!(fetcher.announces[&peer_a][&h].size, 0);
        let original_seq = fetcher.announces[&peer_a][&h].seq;

        // Second announcement with known size (ETH68-style)
        fetcher.insert_peer_announce(&peer_a, &h, &Some((0x02, 500)));
        assert_eq!(fetcher.announces[&peer_a][&h].size, 500, "size should be backfilled");
        assert_eq!(
            fetcher.announces[&peer_a][&h].seq, original_seq,
            "seq should be preserved (FIFO unchanged)"
        );
    }
}
