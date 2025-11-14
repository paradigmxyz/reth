//! `TransactionFetcher` is responsible for rate limiting and retry logic for fetching
//! transactions. Upon receiving an announcement, functionality of the `TransactionFetcher` is
//! used for filtering out hashes 1) for which the tx is already known and 2) unknown but the hash
//! is already seen in a previous announcement. The hashes that remain from an announcement are
//! then packed into a request with respect to the [`EthVersion`] of the announcement. Any hashes
//! that don't fit into the request, are buffered in the `TransactionFetcher`. If on the other
//! hand, space remains, hashes that the peer has previously announced are taken out of buffered
//! hashes to fill the request up. The [`GetPooledTransactions`] request is then sent to the
//! peer's session, this marks the peer as active with respect to
//! `MAX_CONCURRENT_TX_REQUESTS_PER_PEER`.
//!
//! When a peer buffers hashes in the `TransactionsManager::on_new_pooled_transaction_hashes`
//! pipeline, it is stored as fallback peer for those hashes. When [`TransactionsManager`] is
//! polled, it checks if any of fallback peer is idle. If so, it packs a request for that peer,
//! filling it from the buffered hashes. It does so until there are no more idle peers or until
//! the hashes buffer is empty.
//!
//! If a [`GetPooledTransactions`] request resolves with an error, the hashes in the request are
//! buffered with respect to `MAX_REQUEST_RETRIES_PER_TX_HASH`. So is the case if the request
//! resolves with partial success, that is some of the requested hashes are not in the response,
//! these are then buffered.
//!
//! Most healthy peers will send the same hashes in their announcements, as RLPx is a gossip
//! protocol. This means it's unlikely, that a valid hash, will be buffered for very long
//! before it's re-tried. Nonetheless, the capacity of the buffered hashes cache must be large
//! enough to buffer many hashes during network failure, to allow for recovery.

use super::{
    config::TransactionFetcherConfig,
    constants::{tx_fetcher::*, SOFT_LIMIT_COUNT_HASHES_IN_GET_POOLED_TRANSACTIONS_REQUEST},
    PeerMetadata, PooledTransactions, SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE,
};
use crate::{
    cache::{LruCache, LruMap},
    duration_metered_exec,
    metrics::TransactionFetcherMetrics,
};
use alloy_consensus::transaction::PooledTransaction;
use alloy_primitives::TxHash;
use derive_more::{Constructor, Deref};
use futures::{stream::FuturesUnordered, Future, FutureExt, Stream, StreamExt};
use pin_project::pin_project;
use reth_eth_wire::{
    DedupPayload, GetPooledTransactions, HandleMempoolData, HandleVersionedMempoolData,
    PartiallyValidData, RequestTxHashes, ValidAnnouncementData,
};
use reth_eth_wire_types::{EthNetworkPrimitives, NetworkPrimitives};
use reth_network_api::PeerRequest;
use reth_network_p2p::error::{RequestError, RequestResult};
use reth_network_peers::PeerId;
use reth_primitives_traits::SignedTransaction;
use schnellru::ByLength;
use std::{
    collections::{HashMap, VecDeque},
    pin::Pin,
    task::{ready, Context, Poll},
    time::Duration,
};
use tokio::sync::{mpsc::error::TrySendError, oneshot, oneshot::error::RecvError};
use tracing::trace;

/// The type responsible for fetching missing transactions from peers.
///
/// This will keep track of unique transaction hashes that are currently being fetched and submits
/// new requests on announced hashes.
#[derive(Debug)]
#[pin_project]
pub struct TransactionFetcher<N: NetworkPrimitives = EthNetworkPrimitives> {
    /// Pending hashes bucketed by peer for future fetch scheduling.
    pending_by_peer: PeerPendingQueues,
    /// Whether the per-peer queue scheduler is enabled (experimental).
    use_peer_queue_scheduler: bool,
    /// All peers with to which a [`GetPooledTransactions`] request is inflight.
    pub active_peers: LruMap<PeerId, u8, ByLength>,
    /// All currently active [`GetPooledTransactions`] requests.
    ///
    /// The set of hashes encompassed by these requests are a subset of all hashes in the fetcher.
    /// It's disjoint from the set of hashes which are awaiting an idle fallback peer in order to
    /// be fetched.
    #[pin]
    pub inflight_requests: FuturesUnordered<GetPooledTxRequestFut<N::PooledTransaction>>,
    /// Hashes that are awaiting an idle fallback peer so they can be fetched.
    ///
    /// This is a subset of all hashes in the fetcher, and is disjoint from the set of hashes for
    /// which a [`GetPooledTransactions`] request is inflight.
    pub hashes_pending_fetch: LruCache<TxHash>,
    /// Tracks all hashes in the transaction fetcher.
    pub hashes_fetch_inflight_and_pending_fetch: LruMap<TxHash, TxFetchMetadata, ByLength>,
    /// Info on capacity of the transaction fetcher.
    pub info: TransactionFetcherInfo,
    #[doc(hidden)]
    metrics: TransactionFetcherMetrics,
}

impl<N: NetworkPrimitives> TransactionFetcher<N> {
    /// Returns a helper capable of constructing transaction requests from announcements or
    /// pending caches.
    #[allow(clippy::missing_const_for_fn)]
    pub fn request_builder(&mut self) -> RequestBuilder<'_, N> {
        RequestBuilder { fetcher: self }
    }

    /// Removes the peer from the active set.
    pub(crate) fn remove_peer(&mut self, peer_id: &PeerId) {
        self.active_peers.remove(peer_id);
    }

    /// Updates metrics.
    #[inline]
    pub fn update_metrics(&self) {
        let metrics = &self.metrics;

        metrics.inflight_transaction_requests.set(self.inflight_requests.len() as f64);

        let hashes_pending_fetch = self.hashes_pending_fetch.len() as f64;
        let total_hashes = self.hashes_fetch_inflight_and_pending_fetch.len() as f64;

        metrics.hashes_pending_fetch.set(hashes_pending_fetch);
        metrics.peer_queue_hashes.set(self.pending_by_peer.total_hashes() as f64);
        metrics.peer_queue_size.set(self.pending_by_peer.peer_count() as f64);
        metrics.hashes_inflight_transaction_requests.set(total_hashes - hashes_pending_fetch);
    }

    #[inline]
    fn update_pending_fetch_cache_search_metrics(&self, durations: TxFetcherSearchDurations) {
        let metrics = &self.metrics;

        let TxFetcherSearchDurations { find_idle_peer, fill_request } = durations;
        metrics
            .duration_find_idle_fallback_peer_for_any_pending_hash
            .set(find_idle_peer.as_secs_f64());
        metrics.duration_fill_request_from_hashes_pending_fetch.set(fill_request.as_secs_f64());
    }

    /// Sets up transaction fetcher with config
    pub fn with_transaction_fetcher_config(config: &TransactionFetcherConfig) -> Self {
        let TransactionFetcherConfig {
            max_inflight_requests,
            max_capacity_cache_txns_pending_fetch,
            enable_peer_queue_scheduler,
            ..
        } = *config;

        let info = config.clone().into();

        let metrics = TransactionFetcherMetrics::default();
        metrics.capacity_inflight_requests.increment(max_inflight_requests as u64);

        Self {
            pending_by_peer: PeerPendingQueues::default(),
            use_peer_queue_scheduler: enable_peer_queue_scheduler,
            active_peers: LruMap::new(max_inflight_requests),
            inflight_requests: Default::default(),
            hashes_pending_fetch: LruCache::new(max_capacity_cache_txns_pending_fetch),
            hashes_fetch_inflight_and_pending_fetch: LruMap::new(
                max_inflight_requests + max_capacity_cache_txns_pending_fetch,
            ),
            info,
            metrics,
        }
    }

    /// Removes the specified hashes from inflight tracking.
    #[inline]
    pub fn remove_hashes_from_transaction_fetcher<'a, I>(&mut self, hashes: I)
    where
        I: IntoIterator<Item = &'a TxHash>,
    {
        for hash in hashes {
            self.hashes_fetch_inflight_and_pending_fetch.remove(hash);
            if self.hashes_pending_fetch.remove(hash) {
                self.pending_by_peer.remove_hash(hash);
            }
        }
    }

    /// Convenience wrapper around [`RequestBuilder::pack_from_announcement`].
    pub fn pack_request(
        &mut self,
        hashes_to_request: &mut RequestTxHashes,
        hashes_from_announcement: ValidAnnouncementData,
    ) -> RequestTxHashes {
        self.request_builder().pack_from_announcement(hashes_to_request, hashes_from_announcement)
    }

    /// Updates peer's activity status upon a resolved [`GetPooledTxRequest`].
    fn decrement_inflight_request_count_for(&mut self, peer_id: &PeerId) {
        let remove = || -> bool {
            if let Some(inflight_count) = self.active_peers.get(peer_id) {
                *inflight_count = inflight_count.saturating_sub(1);
                if *inflight_count == 0 {
                    return true
                }
            }
            false
        }();

        if remove {
            self.active_peers.remove(peer_id);
        }
    }

    /// Returns `true` if peer is idle with respect to `self.inflight_requests`.
    #[inline]
    pub fn is_idle(&self, peer_id: &PeerId) -> bool {
        let Some(inflight_count) = self.active_peers.peek(peer_id) else { return true };
        if *inflight_count < self.info.max_inflight_requests_per_peer {
            return true
        }
        false
    }

    /// Returns any idle peer for the given hash.
    pub fn get_idle_peer_for(&self, hash: TxHash) -> Option<&PeerId> {
        let TxFetchMetadata { fallback_peers, .. } =
            self.hashes_fetch_inflight_and_pending_fetch.peek(&hash)?;

        for peer_id in fallback_peers.iter() {
            if self.is_idle(peer_id) {
                return Some(peer_id)
            }
        }

        None
    }

    /// Returns any idle peer for any hash pending fetch. If one is found, the corresponding hash
    /// is returned alongside the peer id so the caller can decide how to build the request buffer.
    ///
    /// Loops through the hashes pending fetch in lru order until one is found with an idle
    /// fallback peer, or the budget passed as parameter is depleted, whatever happens first.
    pub fn find_any_idle_fallback_peer_for_any_pending_hash(
        &mut self,
        mut budget: Option<usize>, // search fallback peers for max `budget` lru pending hashes
    ) -> Option<(PeerId, TxHash)> {
        let mut hashes_pending_fetch_iter = self.hashes_pending_fetch.iter();

        let idle_peer = loop {
            let &hash = hashes_pending_fetch_iter.next()?;

            if let Some(idle_peer) = self.get_idle_peer_for(hash) {
                break Some((*idle_peer, hash))
            }

            if let Some(ref mut bud) = budget {
                *bud = bud.saturating_sub(1);
                if *bud == 0 {
                    return None
                }
            }
        }?;
        let (peer_id, hash) = idle_peer;

        // pop hash that is loaded in request buffer from cache of hashes pending fetch
        drop(hashes_pending_fetch_iter);
        if self.hashes_pending_fetch.remove(&hash) {
            self.pending_by_peer.remove_hash(&hash);
        }

        Some((peer_id, hash))
    }

    /// Tries to buffer hashes for retry.
    pub fn try_buffer_hashes_for_retry(
        &mut self,
        mut hashes: RequestTxHashes,
        peer_failed_to_serve: &PeerId,
    ) {
        // It could be that the txns have been received over broadcast in the time being. Remove
        // the peer as fallback peer so it isn't request again for these hashes.
        hashes.retain(|hash| {
            if let Some(entry) = self.hashes_fetch_inflight_and_pending_fetch.get(hash) {
                entry.fallback_peers_mut().remove(peer_failed_to_serve);
                return true
            }
            // tx has been seen over broadcast in the time it took for the request to resolve
            false
        });

        self.buffer_hashes(hashes, None)
    }

    /// Number of hashes pending fetch.
    pub fn num_pending_hashes(&self) -> usize {
        self.hashes_pending_fetch.len()
    }

    /// Number of all transaction hashes in the fetcher.
    pub fn num_all_hashes(&self) -> usize {
        self.hashes_fetch_inflight_and_pending_fetch.len()
    }

    /// Buffers hashes. Note: Only peers that haven't yet tried to request the hashes should be
    /// passed as `fallback_peer` parameter! For re-buffering hashes on failed request, use
    /// [`TransactionFetcher::try_buffer_hashes_for_retry`]. Hashes that have been re-requested
    /// [`DEFAULT_MAX_RETRIES`], are dropped.
    pub fn buffer_hashes(&mut self, hashes: RequestTxHashes, fallback_peer: Option<PeerId>) {
        for hash in hashes {
            // hash could have been evicted from bounded lru map
            if self.hashes_fetch_inflight_and_pending_fetch.peek(&hash).is_none() {
                continue
            }

            let Some(TxFetchMetadata { retries, fallback_peers, .. }) =
                self.hashes_fetch_inflight_and_pending_fetch.get(&hash)
            else {
                return
            };

            if let Some(peer_id) = fallback_peer {
                // peer has not yet requested hash
                fallback_peers.insert(peer_id);
                self.pending_by_peer.remove_hash(&hash);
                self.pending_by_peer.assign(peer_id, hash);
            } else {
                if *retries >= DEFAULT_MAX_RETRIES {
                    trace!(target: "net::tx",
                        %hash,
                        retries,
                        "retry limit for `GetPooledTransactions` requests reached for hash, dropping hash"
                    );

                    self.hashes_fetch_inflight_and_pending_fetch.remove(&hash);
                    if self.hashes_pending_fetch.remove(&hash) {
                        self.pending_by_peer.remove_hash(&hash);
                    }
                    continue
                }
                *retries += 1;

                // pick one of the fallback peers, if any, to enqueue the hash under
                self.pending_by_peer.remove_hash(&hash);
                let fallback_iter = fallback_peers.iter().copied();
                self.pending_by_peer.assign_from_candidates(hash, fallback_iter);
            }

            if let (_, Some(evicted_hash)) = self.hashes_pending_fetch.insert_and_get_evicted(hash)
            {
                self.hashes_fetch_inflight_and_pending_fetch.remove(&evicted_hash);
                self.pending_by_peer.remove_hash(&evicted_hash);
            }
        }
    }

    /// Tries to request hashes pending fetch.
    ///
    /// Finds the first buffered hash with a fallback peer that is idle, if any. Fills the rest of
    /// the request by checking the transactions seen by the peer against the buffer.
    pub fn on_fetch_pending_hashes(&mut self, peers: &HashMap<PeerId, PeerMetadata<N>>) {
        let mut search_durations = TxFetcherSearchDurations::default();

        if self.use_peer_queue_scheduler &&
            self.try_request_via_peer_queue(peers, &mut search_durations)
        {
            self.update_pending_fetch_cache_search_metrics(search_durations);
            return
        }

        let idle_peer_and_hash = duration_metered_exec!(
            self.find_any_idle_fallback_peer_for_any_pending_hash(None),
            search_durations.find_idle_peer
        );

        let Some((peer_id, first_hash)) = idle_peer_and_hash else {
            // no peers are idle or budget is depleted
            return
        };

        let mut hashes_to_request = RequestTxHashes::with_capacity(
            DEFAULT_MARGINAL_COUNT_HASHES_GET_POOLED_TRANSACTIONS_REQUEST,
        );
        hashes_to_request.insert(first_hash);

        // peer should always exist since `is_session_active` already checked
        let Some(peer) = peers.get(&peer_id) else { return };
        let conn_eth_version = peer.version;

        duration_metered_exec!(
            {
                self.request_builder().fill_from_pending_fetch(
                    &mut hashes_to_request,
                    &peer.seen_transactions,
                    None,
                )
            },
            search_durations.fill_request
        );

        self.update_pending_fetch_cache_search_metrics(search_durations);

        trace!(target: "net::tx",
            peer_id=format!("{peer_id:#}"),
            hashes=?*hashes_to_request,
            %conn_eth_version,
            "requesting hashes that were stored pending fetch from peer"
        );

        // request the buffered missing transactions
        if let Some(failed_to_request_hashes) =
            self.request_transactions_from_peer(hashes_to_request, peer)
        {
            trace!(target: "net::tx",
                peer_id=format!("{peer_id:#}"),
                ?failed_to_request_hashes,
                %conn_eth_version,
                "failed sending request to peer's session, buffering hashes"
            );

            self.buffer_hashes(failed_to_request_hashes, Some(peer_id));
        }
    }

    fn try_request_via_peer_queue(
        &mut self,
        peers: &HashMap<PeerId, PeerMetadata<N>>,
        search_durations: &mut TxFetcherSearchDurations,
    ) -> bool {
        let idle_candidate = duration_metered_exec!(
            { self.pending_by_peer.next_idle_candidate(|peer| self.is_idle(peer)) },
            search_durations.find_idle_peer
        );

        let Some((peer_id, first_hash)) = idle_candidate else { return false };

        let Some(peer) = peers.get(&peer_id) else {
            self.pending_by_peer.remove_hash(&first_hash);
            return false
        };

        if !self.hashes_pending_fetch.remove(&first_hash) {
            self.pending_by_peer.remove_hash(&first_hash);
            return false
        }
        self.pending_by_peer.remove_hash(&first_hash);

        let mut hashes_to_request = RequestTxHashes::with_capacity(
            DEFAULT_MARGINAL_COUNT_HASHES_GET_POOLED_TRANSACTIONS_REQUEST,
        );
        hashes_to_request.insert(first_hash);

        duration_metered_exec!(
            {
                self.request_builder().fill_from_pending_fetch(
                    &mut hashes_to_request,
                    &peer.seen_transactions,
                    None,
                )
            },
            search_durations.fill_request
        );

        trace!(target: "net::tx",
            peer_id=format!("{peer_id:#}"),
            hashes=?*hashes_to_request,
            %peer.version,
            "peer-queue scheduler requesting hashes pending fetch"
        );

        if let Some(failed_to_request_hashes) =
            self.request_transactions_from_peer(hashes_to_request, peer)
        {
            self.buffer_hashes(failed_to_request_hashes, Some(peer_id));
        }

        true
    }

    /// Filters out hashes that have been seen before. For hashes that have already been seen, the
    /// peer is added as fallback peer.
    pub fn filter_unseen_and_pending_hashes(
        &mut self,
        new_announced_hashes: &mut ValidAnnouncementData,
        is_tx_bad_import: impl Fn(&TxHash) -> bool,
        peer_id: &PeerId,
        client_version: &str,
    ) {
        let mut previously_unseen_hashes_count = 0;

        let msg_version = new_announced_hashes.msg_version();

        // filter out inflight hashes, and register the peer as fallback for all inflight hashes
        new_announced_hashes.retain(|hash, metadata| {

            // occupied entry
            if let Some(TxFetchMetadata{ tx_encoded_length: previously_seen_size, ..}) = self.hashes_fetch_inflight_and_pending_fetch.peek_mut(hash) {
                // update size metadata if available
                if let Some((_ty, size)) = metadata {
                    if let Some(prev_size) = previously_seen_size {
                        // check if this peer is announcing a different size than a previous peer
                        if size != prev_size {
                            trace!(target: "net::tx",
                                peer_id=format!("{peer_id:#}"),
                                %hash,
                                size,
                                previously_seen_size,
                                %client_version,
                                "peer announced a different size for tx, this is especially worrying if one size is much bigger..."
                            );
                        }
                    }
                    // believe the most recent peer to announce tx
                    *previously_seen_size = Some(*size);
                }

                // hash has been seen but is not inflight
                if self.hashes_pending_fetch.remove(hash) {
                    self.pending_by_peer.remove_hash(hash);
                    return true
                }

                return false
            }

            // vacant entry

            if is_tx_bad_import(hash) {
                return false
            }

            previously_unseen_hashes_count += 1;

            if self.hashes_fetch_inflight_and_pending_fetch.get_or_insert(*hash, ||
                TxFetchMetadata{retries: 0, fallback_peers: LruCache::new(DEFAULT_MAX_COUNT_FALLBACK_PEERS as u32), tx_encoded_length: None}
            ).is_none() {

                trace!(target: "net::tx",
                    peer_id=format!("{peer_id:#}"),
                    %hash,
                    ?msg_version,
                    %client_version,
                    "failed to cache new announced hash from peer in schnellru::LruMap, dropping hash"
                );

                return false
            }
            true
        });

        trace!(target: "net::tx",
            peer_id=format!("{peer_id:#}"),
            previously_unseen_hashes_count=previously_unseen_hashes_count,
            msg_version=?msg_version,
            client_version=%client_version,
            "received previously unseen hashes in announcement from peer"
        );
    }

    /// Requests the missing transactions from the previously unseen announced hashes of the peer.
    /// Returns the requested hashes if the request concurrency limit is reached or if the request
    /// fails to send over the channel to the peer's session task.
    ///
    /// This filters all announced hashes that are already in flight, and requests the missing,
    /// while marking the given peer as an alternative peer for the hashes that are already in
    /// flight.
    pub fn request_transactions_from_peer(
        &mut self,
        new_announced_hashes: RequestTxHashes,
        peer: &PeerMetadata<N>,
    ) -> Option<RequestTxHashes> {
        let peer_id: PeerId = peer.request_tx.peer_id;
        let conn_eth_version = peer.version;

        if self.active_peers.len() >= self.info.max_inflight_requests {
            trace!(target: "net::tx",
                peer_id=format!("{peer_id:#}"),
                hashes=?*new_announced_hashes,
                %conn_eth_version,
                max_inflight_transaction_requests=self.info.max_inflight_requests,
                "limit for concurrent `GetPooledTransactions` requests reached, dropping request for hashes to peer"
            );
            return Some(new_announced_hashes)
        }

        let Some(inflight_count) = self.active_peers.get_or_insert(peer_id, || 0) else {
            trace!(target: "net::tx",
                peer_id=format!("{peer_id:#}"),
                hashes=?*new_announced_hashes,
                conn_eth_version=%conn_eth_version,
                "failed to cache active peer in schnellru::LruMap, dropping request to peer"
            );
            return Some(new_announced_hashes)
        };

        if *inflight_count >= self.info.max_inflight_requests_per_peer {
            trace!(target: "net::tx",
                peer_id=format!("{peer_id:#}"),
                hashes=?*new_announced_hashes,
                %conn_eth_version,
                max_concurrent_tx_reqs_per_peer=self.info.max_inflight_requests_per_peer,
                "limit for concurrent `GetPooledTransactions` requests per peer reached"
            );
            return Some(new_announced_hashes)
        }

        #[cfg(debug_assertions)]
        {
            for hash in &new_announced_hashes {
                if self.hashes_pending_fetch.contains(hash) {
                    tracing::debug!(target: "net::tx", "`{}` should have been taken out of buffer before packing in a request, breaks invariant `@hashes_pending_fetch` and `@inflight_requests`, `@hashes_fetch_inflight_and_pending_fetch` for `{}`: {:?}",
                        format!("{:?}", new_announced_hashes), // Assuming new_announced_hashes can be debug-printed directly
                        format!("{:?}", new_announced_hashes),
                        new_announced_hashes.iter().map(|hash| {
                            let metadata = self.hashes_fetch_inflight_and_pending_fetch.get(hash);
                            // Assuming you only need `retries` and `tx_encoded_length` for debugging
                            (*hash, metadata.map(|m| (m.retries, m.tx_encoded_length)))
                        }).collect::<Vec<(TxHash, Option<(u8, Option<usize>)>)>>())
                }
            }
        }

        let (response, rx) = oneshot::channel();
        let req = PeerRequest::GetPooledTransactions {
            request: GetPooledTransactions(new_announced_hashes.iter().copied().collect()),
            response,
        };

        // try to send the request to the peer
        if let Err(err) = peer.request_tx.try_send(req) {
            // peer channel is full
            return match err {
                TrySendError::Full(_) | TrySendError::Closed(_) => {
                    self.metrics.egress_peer_channel_full.increment(1);
                    Some(new_announced_hashes)
                }
            }
        }

        *inflight_count += 1;
        // stores a new request future for the request
        self.inflight_requests.push(GetPooledTxRequestFut::new(peer_id, new_announced_hashes, rx));

        None
    }

    /// Processes a resolved [`GetPooledTransactions`] request. Queues the outcome as a
    /// [`FetchEvent`], which will then be streamed by
    /// [`TransactionsManager`](super::TransactionsManager).
    pub fn on_resolved_get_pooled_transactions_request_fut(
        &mut self,
        response: GetPooledTxResponse<N::PooledTransaction>,
    ) -> FetchEvent<N::PooledTransaction> {
        // update peer activity, requests for buffered hashes can only be made to idle
        // fallback peers
        let GetPooledTxResponse { peer_id, mut requested_hashes, result } = response;

        self.decrement_inflight_request_count_for(&peer_id);

        match result {
            Ok(Ok(transactions)) => {
                //
                // 1. peer has failed to serve any of the hashes it has announced to us that we,
                // as a follow, have requested
                //
                if transactions.is_empty() {
                    trace!(target: "net::tx",
                        peer_id=format!("{peer_id:#}"),
                        requested_hashes_len=requested_hashes.len(),
                        "received empty `PooledTransactions` response from peer, peer failed to serve hashes it announced"
                    );

                    return FetchEvent::EmptyResponse { peer_id }
                }

                //
                // 2. filter out hashes that we didn't request
                //
                let payload = UnverifiedPooledTransactions::new(transactions);

                let unverified_len = payload.len();
                let (verification_outcome, verified_payload) =
                    payload.verify(&requested_hashes, &peer_id);

                let unsolicited = unverified_len - verified_payload.len();
                if unsolicited > 0 {
                    self.metrics.unsolicited_transactions.increment(unsolicited as u64);
                }

                let report_peer = if verification_outcome == VerificationOutcome::ReportPeer {
                    trace!(target: "net::tx",
                        peer_id=format!("{peer_id:#}"),
                        unverified_len,
                        verified_payload_len=verified_payload.len(),
                        "received `PooledTransactions` response from peer with entries that didn't verify against request, filtered out transactions"
                    );
                    true
                } else {
                    false
                };

                // peer has only sent hashes that we didn't request
                if verified_payload.is_empty() {
                    return FetchEvent::FetchError { peer_id, error: RequestError::BadResponse }
                }

                //
                // 3. stateless validation of payload, e.g. dedup
                //
                let unvalidated_payload_len = verified_payload.len();

                let valid_payload = verified_payload.dedup();

                // todo: validate based on announced tx size/type and report peer for sending
                // invalid response <https://github.com/paradigmxyz/reth/issues/6529>. requires
                // passing the rlp encoded length down from active session along with the decoded
                // tx.

                if valid_payload.len() != unvalidated_payload_len {
                    trace!(target: "net::tx",
                    peer_id=format!("{peer_id:#}"),
                    unvalidated_payload_len,
                    valid_payload_len=valid_payload.len(),
                    "received `PooledTransactions` response from peer with duplicate entries, filtered them out"
                    );
                }
                // valid payload will have at least one transaction at this point. even if the tx
                // size/type announced by the peer is different to the actual tx size/type, pass on
                // to pending pool imports pipeline for validation.

                //
                // 4. clear received hashes
                //
                let requested_hashes_len = requested_hashes.len();
                let mut fetched = Vec::with_capacity(valid_payload.len());
                requested_hashes.retain(|requested_hash| {
                    if valid_payload.contains_key(requested_hash) {
                        // hash is now known, stop tracking
                        fetched.push(*requested_hash);
                        return false
                    }
                    true
                });
                fetched.shrink_to_fit();
                self.metrics.fetched_transactions.increment(fetched.len() as u64);

                if fetched.len() < requested_hashes_len {
                    trace!(target: "net::tx",
                        peer_id=format!("{peer_id:#}"),
                        requested_hashes_len=requested_hashes_len,
                        fetched_len=fetched.len(),
                        "peer failed to serve hashes it announced"
                    );
                }

                //
                // 5. buffer left over hashes
                //
                self.try_buffer_hashes_for_retry(requested_hashes, &peer_id);

                let transactions = valid_payload.into_data().into_values().collect();

                FetchEvent::TransactionsFetched { peer_id, transactions, report_peer }
            }
            Ok(Err(req_err)) => {
                self.try_buffer_hashes_for_retry(requested_hashes, &peer_id);
                FetchEvent::FetchError { peer_id, error: req_err }
            }
            Err(_) => {
                self.try_buffer_hashes_for_retry(requested_hashes, &peer_id);
                // request channel closed/dropped
                FetchEvent::FetchError { peer_id, error: RequestError::ChannelClosed }
            }
        }
    }
}

impl<N: NetworkPrimitives> Stream for TransactionFetcher<N> {
    type Item = FetchEvent<N::PooledTransaction>;

    /// Advances all inflight requests and returns the next event.
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // `FuturesUnordered` doesn't close when `None` is returned. so just return pending.
        // <https://play.rust-lang.org/?version=stable&mode=debug&edition=2021&gist=815be2b6c8003303757c3ced135f363e>
        if self.inflight_requests.is_empty() {
            return Poll::Pending
        }

        if let Some(resp) = ready!(self.inflight_requests.poll_next_unpin(cx)) {
            return Poll::Ready(Some(self.on_resolved_get_pooled_transactions_request_fut(resp)))
        }

        Poll::Pending
    }
}

impl<T: NetworkPrimitives> Default for TransactionFetcher<T> {
    fn default() -> Self {
        Self {
            pending_by_peer: PeerPendingQueues::default(),
            use_peer_queue_scheduler: false,
            active_peers: LruMap::new(DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS),
            inflight_requests: Default::default(),
            hashes_pending_fetch: LruCache::new(DEFAULT_MAX_CAPACITY_CACHE_PENDING_FETCH),
            hashes_fetch_inflight_and_pending_fetch: LruMap::new(
                DEFAULT_MAX_CAPACITY_CACHE_INFLIGHT_AND_PENDING_FETCH,
            ),
            info: TransactionFetcherInfo::default(),
            metrics: Default::default(),
        }
    }
}

#[cfg(test)]
impl<N: NetworkPrimitives> TransactionFetcher<N> {
    /// Test helper exposing the Eth68 packing logic.
    pub fn pack_request_eth68(
        &mut self,
        hashes_to_request: &mut RequestTxHashes,
        hashes_from_announcement: impl HandleMempoolData
            + IntoIterator<Item = (TxHash, Option<(u8, usize)>)>,
    ) -> RequestTxHashes {
        self.request_builder().pack_eth68(hashes_to_request, hashes_from_announcement)
    }
}

/// Maintains the mapping between peers and hashes awaiting fetch so future scheduling can
/// operate on per-peer queues. This is currently auxiliary and mirrors the legacy LRU-based
/// tracking to ease migration towards a geth-like architecture.
#[derive(Default, Debug)]
struct PeerPendingQueues {
    by_peer: HashMap<PeerId, VecDeque<TxHash>>,
    owner_by_hash: HashMap<TxHash, PeerId>,
}

impl PeerPendingQueues {
    fn assign(&mut self, peer: PeerId, hash: TxHash) {
        self.remove_hash(&hash);
        let entry = self.by_peer.entry(peer).or_default();
        entry.push_back(hash);
        self.owner_by_hash.insert(hash, peer);
    }

    fn remove_hash(&mut self, hash: &TxHash) {
        if let Some(peer) = self.owner_by_hash.remove(hash)
            && let Some(queue) = self.by_peer.get_mut(&peer)
        {
            if let Some(pos) = queue.iter().position(|h| h == hash) {
                queue.remove(pos);
            }
            if queue.is_empty() {
                self.by_peer.remove(&peer);
            }
        }
    }

    fn assign_from_candidates(&mut self, hash: TxHash, peers: impl Iterator<Item = PeerId>) {
        if let Some(peer) = peers.into_iter().next() {
            self.assign(peer, hash);
        }
    }

    fn next_idle_candidate<F>(&self, mut is_idle: F) -> Option<(PeerId, TxHash)>
    where
        F: FnMut(&PeerId) -> bool,
    {
        self.by_peer.iter().find_map(|(peer, queue)| {
            let hash = queue.front()?;
            is_idle(peer).then_some((*peer, *hash))
        })
    }

    fn total_hashes(&self) -> usize {
        self.owner_by_hash.len()
    }

    fn peer_count(&self) -> usize {
        self.by_peer.len()
    }
}

/// Helper responsible for assembling [`RequestTxHashes`] from announcements or cached pending
/// hashes while honoring byte/count limits.
#[derive(Debug)]
pub struct RequestBuilder<'a, N: NetworkPrimitives> {
    fetcher: &'a mut TransactionFetcher<N>,
}

impl<'a, N: NetworkPrimitives> RequestBuilder<'a, N> {
    const fn info(&self) -> &TransactionFetcherInfo {
        &self.fetcher.info
    }

    /// Packages hashes for a [`GetPooledTransactions`] request from a network announcement.
    pub fn pack_from_announcement(
        &mut self,
        hashes_to_request: &mut RequestTxHashes,
        hashes_from_announcement: ValidAnnouncementData,
    ) -> RequestTxHashes {
        if hashes_from_announcement.msg_version().is_eth68() {
            return self.pack_eth68(hashes_to_request, hashes_from_announcement)
        }
        self.pack_eth66(hashes_to_request, hashes_from_announcement)
    }

    /// Packs hashes from an Eth68 announcement while respecting the expected response byte limit.
    fn pack_eth68(
        &self,
        hashes_to_request: &mut RequestTxHashes,
        hashes_from_announcement: impl HandleMempoolData
            + IntoIterator<Item = (TxHash, Option<(u8, usize)>)>,
    ) -> RequestTxHashes {
        let mut acc_size_response = 0;
        let mut hashes_from_announcement_iter = hashes_from_announcement.into_iter();

        if let Some((hash, Some((_ty, size)))) = hashes_from_announcement_iter.next() {
            hashes_to_request.insert(hash);

            if size >= self.info().soft_limit_byte_size_pooled_transactions_response_on_pack_request
            {
                return hashes_from_announcement_iter.collect()
            }
            acc_size_response = size;
        }

        let mut surplus_hashes = RequestTxHashes::default();

        for (hash, metadata) in hashes_from_announcement_iter.by_ref() {
            let Some((_ty, size)) = metadata else {
                unreachable!("eth68 announcements always contain metadata")
            };

            let next_acc_size = acc_size_response + size;
            if next_acc_size <=
                self.info().soft_limit_byte_size_pooled_transactions_response_on_pack_request
            {
                acc_size_response = next_acc_size;
                _ = hashes_to_request.insert(hash);
            } else {
                _ = surplus_hashes.insert(hash);
            }

            let free_space =
                self.info().soft_limit_byte_size_pooled_transactions_response_on_pack_request -
                    acc_size_response;

            if free_space < MEDIAN_BYTE_SIZE_SMALL_LEGACY_TX_ENCODED {
                break
            }
        }

        surplus_hashes.extend(hashes_from_announcement_iter.map(|(hash, _)| hash));
        surplus_hashes
    }

    /// Packs hashes from an Eth66 announcement, returning any surplus hashes.
    fn pack_eth66(
        &self,
        hashes_to_request: &mut RequestTxHashes,
        hashes_from_announcement: ValidAnnouncementData,
    ) -> RequestTxHashes {
        let (mut hashes, _version) = hashes_from_announcement.into_request_hashes();
        if hashes.len() <= SOFT_LIMIT_COUNT_HASHES_IN_GET_POOLED_TRANSACTIONS_REQUEST {
            *hashes_to_request = hashes;
            hashes_to_request.shrink_to_fit();
            RequestTxHashes::default()
        } else {
            let surplus_hashes =
                hashes.retain_count(SOFT_LIMIT_COUNT_HASHES_IN_GET_POOLED_TRANSACTIONS_REQUEST);
            *hashes_to_request = hashes;
            hashes_to_request.shrink_to_fit();
            surplus_hashes
        }
    }

    /// Fills the provided buffer with pending hashes visible to the peer until byte/count limits
    /// are met. Removes selected hashes from the pending cache.
    pub fn fill_from_pending_fetch(
        &mut self,
        hashes_to_request: &mut RequestTxHashes,
        seen_hashes: &LruCache<TxHash>,
        mut budget_fill_request: Option<usize>,
    ) {
        let Some(hash) = hashes_to_request.iter().next() else { return };

        let mut acc_size_response = self
            .fetcher
            .hashes_fetch_inflight_and_pending_fetch
            .get(hash)
            .and_then(|entry| entry.tx_encoded_len())
            .unwrap_or(AVERAGE_BYTE_SIZE_TX_ENCODED);

        if acc_size_response >=
            DEFAULT_SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE_ON_FETCH_PENDING_HASHES
        {
            return
        }

        for hash in self.fetcher.hashes_pending_fetch.iter() {
            if hashes_to_request.len() >=
                DEFAULT_SOFT_LIMIT_COUNT_HASHES_IN_GET_POOLED_TRANSACTIONS_REQUEST_ON_FETCH_PENDING_HASHES
            {
                break
            }

            if !seen_hashes.contains(hash) {
                continue
            }

            let size = self
                .fetcher
                .hashes_fetch_inflight_and_pending_fetch
                .get(hash)
                .and_then(|entry| entry.tx_encoded_len())
                .unwrap_or(AVERAGE_BYTE_SIZE_TX_ENCODED);

            let next_acc = acc_size_response + size;
            if next_acc >
                DEFAULT_SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE_ON_FETCH_PENDING_HASHES
            {
                continue
            }

            acc_size_response = next_acc;
            hashes_to_request.insert(*hash);

            if let Some(ref mut bud) = budget_fill_request {
                *bud -= 1;
                if *bud == 0 {
                    break
                }
            }
        }

        for hash in hashes_to_request.iter() {
            if self.fetcher.hashes_pending_fetch.remove(hash) {
                self.fetcher.pending_by_peer.remove_hash(hash);
            }
        }
    }
}

/// Metadata of a transaction hash that is yet to be fetched.
#[derive(Debug, Constructor)]
pub struct TxFetchMetadata {
    /// The number of times a request attempt has been made for the hash.
    retries: u8,
    /// Peers that have announced the hash, but to which a request attempt has not yet been made.
    fallback_peers: LruCache<PeerId>,
    /// Size metadata of the transaction if it has been seen in an eth68 announcement.
    // todo: store all seen sizes as a `(size, peer_id)` tuple to catch peers that respond with
    // another size tx than they announced. alt enter in request (won't catch peers announcing
    // wrong size for requests assembled from hashes pending fetch if stored in request fut)
    tx_encoded_length: Option<usize>,
}

impl TxFetchMetadata {
    /// Returns a mutable reference to the fallback peers cache for this transaction hash.
    pub const fn fallback_peers_mut(&mut self) -> &mut LruCache<PeerId> {
        &mut self.fallback_peers
    }

    /// Returns the size of the transaction, if its hash has been received in any
    /// [`Eth68`](reth_eth_wire::EthVersion::Eth68) announcement. If the transaction hash has only
    /// been seen in [`Eth66`](reth_eth_wire::EthVersion::Eth66) announcements so far, this will
    /// return `None`.
    pub const fn tx_encoded_len(&self) -> Option<usize> {
        self.tx_encoded_length
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

/// An inflight request for [`PooledTransactions`] from a peer.
#[derive(Debug)]
pub struct GetPooledTxRequest<T = PooledTransaction> {
    peer_id: PeerId,
    /// Transaction hashes that were requested, for cleanup purposes
    requested_hashes: RequestTxHashes,
    response: oneshot::Receiver<RequestResult<PooledTransactions<T>>>,
}

/// Upon reception of a response, a [`GetPooledTxRequest`] is deconstructed to form a
/// [`GetPooledTxResponse`].
#[derive(Debug)]
pub struct GetPooledTxResponse<T = PooledTransaction> {
    peer_id: PeerId,
    /// Transaction hashes that were requested, for cleanup purposes, since peer may only return a
    /// subset of requested hashes.
    requested_hashes: RequestTxHashes,
    result: Result<RequestResult<PooledTransactions<T>>, RecvError>,
}

/// Stores the response receiver made by sending a [`GetPooledTransactions`] request to a peer's
/// session.
#[must_use = "futures do nothing unless polled"]
#[pin_project::pin_project]
#[derive(Debug)]
pub struct GetPooledTxRequestFut<T = PooledTransaction> {
    #[pin]
    inner: Option<GetPooledTxRequest<T>>,
}

impl<T> GetPooledTxRequestFut<T> {
    #[inline]
    const fn new(
        peer_id: PeerId,
        requested_hashes: RequestTxHashes,
        response: oneshot::Receiver<RequestResult<PooledTransactions<T>>>,
    ) -> Self {
        Self { inner: Some(GetPooledTxRequest { peer_id, requested_hashes, response }) }
    }
}

impl<T> Future for GetPooledTxRequestFut<T> {
    type Output = GetPooledTxResponse<T>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut req = self.as_mut().project().inner.take().expect("polled after completion");
        match req.response.poll_unpin(cx) {
            Poll::Ready(result) => Poll::Ready(GetPooledTxResponse {
                peer_id: req.peer_id,
                requested_hashes: req.requested_hashes,
                result,
            }),
            Poll::Pending => {
                self.project().inner.set(Some(req));
                Poll::Pending
            }
        }
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

        #[cfg(debug_assertions)]
        let mut tx_hashes_not_requested: smallvec::SmallVec<[TxHash; 16]> = smallvec::smallvec!();
        #[cfg(not(debug_assertions))]
        let mut tx_hashes_not_requested_count = 0;

        txns.0.retain(|tx| {
            if !requested_hashes.contains(tx.tx_hash()) {
                verification_outcome = VerificationOutcome::ReportPeer;

                #[cfg(debug_assertions)]
                tx_hashes_not_requested.push(*tx.tx_hash());
                #[cfg(not(debug_assertions))]
                {
                    tx_hashes_not_requested_count += 1;
                }

                return false
            }
            true
        });

        #[cfg(debug_assertions)]
        if !tx_hashes_not_requested.is_empty() {
            trace!(target: "net::tx",
                peer_id=format!("{_peer_id:#}"),
                ?tx_hashes_not_requested,
                "transactions in `PooledTransactions` response from peer were not requested"
            );
        }
        #[cfg(not(debug_assertions))]
        if tx_hashes_not_requested_count != 0 {
            trace!(target: "net::tx",
                peer_id=format!("{_peer_id:#}"),
                tx_hashes_not_requested_count,
                "transactions in `PooledTransactions` response from peer were not requested"
            );
        }

        (verification_outcome, VerifiedPooledTransactions::new(txns))
    }
}

/// Outcome from verifying a [`PooledTransactions`] response. Signals to caller whether to penalize
/// the sender of the response or not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerificationOutcome {
    /// Peer behaves appropriately.
    Ok,
    /// A penalty should be flagged for the peer. Peer sent a response with unacceptably
    /// invalid entries.
    ReportPeer,
}

/// Tracks stats about the [`TransactionFetcher`].
#[derive(Debug, Constructor)]
pub struct TransactionFetcherInfo {
    /// Max inflight [`GetPooledTransactions`] requests.
    pub max_inflight_requests: usize,
    /// Max inflight [`GetPooledTransactions`] requests per peer.
    pub max_inflight_requests_per_peer: u8,
    /// Soft limit for the byte size of the expected [`PooledTransactions`] response, upon packing
    /// a [`GetPooledTransactions`] request with hashes (by default less than 2 MiB worth of
    /// transactions is requested).
    pub soft_limit_byte_size_pooled_transactions_response_on_pack_request: usize,
    /// Soft limit for the byte size of a [`PooledTransactions`] response, upon assembling the
    /// response. Spec'd at 2 MiB, but can be adjusted for research purpose.
    pub soft_limit_byte_size_pooled_transactions_response: usize,
    /// Max capacity of the cache of transaction hashes, for transactions that weren't yet fetched.
    /// A transaction is pending fetch if its hash didn't fit into a [`GetPooledTransactions`] yet,
    /// or it wasn't returned upon request to peers.
    pub max_capacity_cache_txns_pending_fetch: u32,
}

impl Default for TransactionFetcherInfo {
    fn default() -> Self {
        Self::new(
            DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS as usize,
            DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS_PER_PEER,
            DEFAULT_SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESP_ON_PACK_GET_POOLED_TRANSACTIONS_REQ,
            SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE,
            DEFAULT_MAX_CAPACITY_CACHE_PENDING_FETCH,
        )
    }
}

impl From<TransactionFetcherConfig> for TransactionFetcherInfo {
    fn from(config: TransactionFetcherConfig) -> Self {
        let TransactionFetcherConfig {
            max_inflight_requests,
            max_inflight_requests_per_peer,
            soft_limit_byte_size_pooled_transactions_response,
            soft_limit_byte_size_pooled_transactions_response_on_pack_request,
            max_capacity_cache_txns_pending_fetch,
            enable_peer_queue_scheduler: _,
        } = config;

        Self::new(
            max_inflight_requests as usize,
            max_inflight_requests_per_peer,
            soft_limit_byte_size_pooled_transactions_response_on_pack_request,
            soft_limit_byte_size_pooled_transactions_response,
            max_capacity_cache_txns_pending_fetch,
        )
    }
}

#[derive(Debug, Default)]
struct TxFetcherSearchDurations {
    find_idle_peer: Duration,
    fill_request: Duration,
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::test_utils::transactions::{buffer_hash_to_tx_fetcher, new_mock_session};
    use alloy_primitives::{hex, B256};
    use alloy_rlp::Decodable;
    use derive_more::IntoIterator;
    use reth_eth_wire_types::EthVersion;
    use reth_ethereum_primitives::TransactionSigned;
    use std::{collections::HashSet, str::FromStr};

    #[derive(IntoIterator)]
    struct TestValidAnnouncementData(Vec<(TxHash, Option<(u8, usize)>)>);

    impl HandleMempoolData for TestValidAnnouncementData {
        fn is_empty(&self) -> bool {
            self.0.is_empty()
        }

        fn len(&self) -> usize {
            self.0.len()
        }

        fn retain_by_hash(&mut self, mut f: impl FnMut(&TxHash) -> bool) {
            self.0.retain(|(hash, _)| f(hash))
        }
    }

    impl HandleVersionedMempoolData for TestValidAnnouncementData {
        fn msg_version(&self) -> EthVersion {
            EthVersion::Eth68
        }
    }

    #[test]
    fn pack_eth68_request() {
        reth_tracing::init_test_tracing();

        // RIG TEST

        let tx_fetcher = &mut TransactionFetcher::<EthNetworkPrimitives>::default();

        let eth68_hashes = [
            B256::from_slice(&[1; 32]),
            B256::from_slice(&[2; 32]),
            B256::from_slice(&[3; 32]),
            B256::from_slice(&[4; 32]),
            B256::from_slice(&[5; 32]),
        ];
        let eth68_sizes = [
            DEFAULT_SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESP_ON_PACK_GET_POOLED_TRANSACTIONS_REQ - MEDIAN_BYTE_SIZE_SMALL_LEGACY_TX_ENCODED - 1, // first will fit
            DEFAULT_SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESP_ON_PACK_GET_POOLED_TRANSACTIONS_REQ, // second won't
            2, // free space > `MEDIAN_BYTE_SIZE_SMALL_LEGACY_TX_ENCODED`, third will fit, no more after this
            9,
            0,
        ];

        let expected_request_hashes =
            [eth68_hashes[0], eth68_hashes[2]].into_iter().collect::<HashSet<_>>();

        let expected_surplus_hashes =
            [eth68_hashes[1], eth68_hashes[3], eth68_hashes[4]].into_iter().collect::<HashSet<_>>();

        let mut eth68_hashes_to_request = RequestTxHashes::with_capacity(3);

        let valid_announcement_data = TestValidAnnouncementData(
            eth68_hashes
                .into_iter()
                .zip(eth68_sizes)
                .map(|(hash, size)| (hash, Some((0u8, size))))
                .collect::<Vec<_>>(),
        );

        // TEST

        let surplus_eth68_hashes =
            tx_fetcher.pack_request_eth68(&mut eth68_hashes_to_request, valid_announcement_data);

        let eth68_hashes_to_request = eth68_hashes_to_request.into_iter().collect::<HashSet<_>>();
        let surplus_eth68_hashes = surplus_eth68_hashes.into_iter().collect::<HashSet<_>>();

        assert_eq!(expected_request_hashes, eth68_hashes_to_request);
        assert_eq!(expected_surplus_hashes, surplus_eth68_hashes);
    }

    #[test]
    fn fill_pending_respects_byte_limit() {
        reth_tracing::init_test_tracing();

        let tx_fetcher = &mut TransactionFetcher::<EthNetworkPrimitives>::default();
        let peer = PeerId::new([5; 64]);

        let hashes =
            [B256::from_slice(&[1; 32]), B256::from_slice(&[2; 32]), B256::from_slice(&[3; 32])];

        let limit =
            DEFAULT_SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE_ON_FETCH_PENDING_HASHES;

        let sizes = [limit / 2, limit, MEDIAN_BYTE_SIZE_SMALL_LEGACY_TX_ENCODED];

        for (hash, size) in hashes.iter().zip(sizes) {
            buffer_hash_to_tx_fetcher(tx_fetcher, *hash, peer, 0, Some(size));
        }

        let mut seen = LruCache::new(8);
        for hash in &hashes {
            seen.insert(*hash);
        }

        let mut hashes_to_request = RequestTxHashes::default();
        hashes_to_request.insert(hashes[0]);

        tx_fetcher.request_builder().fill_from_pending_fetch(&mut hashes_to_request, &seen, None);

        assert!(hashes_to_request.contains(&hashes[0]));
        assert!(hashes_to_request.contains(&hashes[2]));
        assert!(!hashes_to_request.contains(&hashes[1]));
    }

    #[test]
    fn fill_pending_respects_hash_count_limit() {
        reth_tracing::init_test_tracing();

        let tx_fetcher = &mut TransactionFetcher::<EthNetworkPrimitives>::default();
        let peer = PeerId::new([7; 64]);

        let limit = DEFAULT_SOFT_LIMIT_COUNT_HASHES_IN_GET_POOLED_TRANSACTIONS_REQUEST_ON_FETCH_PENDING_HASHES;
        let total = limit + 5;

        let mut hashes = Vec::with_capacity(total);
        for i in 0..total {
            let mut bytes = [0u8; 32];
            bytes[31] = i as u8;
            hashes.push(B256::from(bytes));
        }

        for hash in &hashes {
            buffer_hash_to_tx_fetcher(tx_fetcher, *hash, peer, 0, None);
        }

        let mut seen = LruCache::new((total + 1) as u32);
        for hash in &hashes {
            seen.insert(*hash);
        }

        let first = hashes[0];
        let mut hashes_to_request = RequestTxHashes::default();
        hashes_to_request.insert(first);

        tx_fetcher.request_builder().fill_from_pending_fetch(&mut hashes_to_request, &seen, None);

        assert_eq!(
            limit,
            hashes_to_request.len(),
            "should cap number of hashes even if more are pending"
        );
    }

    #[tokio::test]
    async fn test_on_fetch_pending_hashes() {
        reth_tracing::init_test_tracing();

        let tx_fetcher = &mut TransactionFetcher::default();

        // RIG TEST

        // hashes that will be fetched because they are stored as pending fetch
        let seen_hashes = [
            B256::from_slice(&[1; 32]),
            B256::from_slice(&[2; 32]),
            B256::from_slice(&[3; 32]),
            B256::from_slice(&[4; 32]),
        ];
        //
        // txns 1-3 are small, all will fit in request. no metadata has been made available for
        // hash 4, it has only been seen over eth66 conn, so average tx size will be assumed in
        // filling request.
        let seen_eth68_hashes_sizes = [120, 158, 116];

        // peer that will fetch seen hashes because they are pending fetch
        let peer_1 = PeerId::new([1; 64]);
        // second peer, won't do anything in this test
        let peer_2 = PeerId::new([2; 64]);

        // add seen hashes to peers seen transactions
        //
        // get handle for peer_1's session to receive request for pending hashes
        let (mut peer_1_data, mut peer_1_mock_session_rx) =
            new_mock_session(peer_1, EthVersion::Eth66);
        for hash in &seen_hashes {
            peer_1_data.seen_transactions.insert(*hash);
        }
        let (mut peer_2_data, _) = new_mock_session(peer_2, EthVersion::Eth66);
        for hash in &seen_hashes {
            peer_2_data.seen_transactions.insert(*hash);
        }
        let mut peers = HashMap::default();
        peers.insert(peer_1, peer_1_data);
        peers.insert(peer_2, peer_2_data);

        // insert seen_hashes into tx fetcher
        for i in 0..3 {
            // insert peer_2 as fallback peer for seen_hashes
            buffer_hash_to_tx_fetcher(
                tx_fetcher,
                seen_hashes[i],
                peer_2,
                0,
                Some(seen_eth68_hashes_sizes[i]),
            );
        }
        buffer_hash_to_tx_fetcher(tx_fetcher, seen_hashes[3], peer_2, 0, None);

        // insert pending hash without peer_1 as fallback peer, only with peer_2 as fallback peer
        let hash_other = B256::from_slice(&[5; 32]);
        buffer_hash_to_tx_fetcher(tx_fetcher, hash_other, peer_2, 0, None);

        // add peer_1 as lru fallback peer for seen hashes
        for hash in &seen_hashes {
            buffer_hash_to_tx_fetcher(tx_fetcher, *hash, peer_1, 0, None);
        }

        // seen hashes and the random hash from peer_2 are pending fetch
        assert_eq!(tx_fetcher.num_pending_hashes(), 5);

        // TEST

        tx_fetcher.on_fetch_pending_hashes(&peers, |_| true);

        // mock session of peer_1 receives request
        let req = peer_1_mock_session_rx
            .recv()
            .await
            .expect("peer session should receive request with buffered hashes");
        let PeerRequest::GetPooledTransactions { request, .. } = req else { unreachable!() };
        let GetPooledTransactions(requested_hashes) = request;

        assert_eq!(
            requested_hashes.into_iter().collect::<HashSet<_>>(),
            seen_hashes.into_iter().collect::<HashSet<_>>()
        )
    }

    #[test]
    fn verify_response_hashes() {
        let input = hex!(
            "02f871018302a90f808504890aef60826b6c94ddf4c5025d1a5742cf12f74eec246d4432c295e487e09c3bbcc12b2b80c080a0f21a4eacd0bf8fea9c5105c543be5a1d8c796516875710fafafdf16d16d8ee23a001280915021bb446d1973501a67f93d2b38894a514b976e7b46dc2fe54598daa"
        );
        let signed_tx_1: PooledTransaction =
            TransactionSigned::decode(&mut &input[..]).unwrap().try_into().unwrap();
        let input = hex!(
            "02f871018302a90f808504890aef60826b6c94ddf4c5025d1a5742cf12f74eec246d4432c295e487e09c3bbcc12b2b80c080a0f21a4eacd0bf8fea9c5105c543be5a1d8c796516875710fafafdf16d16d8ee23a001280915021bb446d1973501a67f93d2b38894a514b976e7b46dc2fe54598d76"
        );
        let signed_tx_2: PooledTransaction =
            TransactionSigned::decode(&mut &input[..]).unwrap().try_into().unwrap();

        // only tx 1 is requested
        let request_hashes = [
            B256::from_str("0x3b9aca00f0671c9a2a1b817a0a78d3fe0c0f776cccb2a8c3c1b412a4f4e67890")
                .unwrap(),
            *signed_tx_1.hash(),
            B256::from_str("0x3b9aca00f0671c9a2a1b817a0a78d3fe0c0f776cccb2a8c3c1b412a4f4e12345")
                .unwrap(),
            B256::from_str("0x3b9aca00f0671c9a2a1b817a0a78d3fe0c0f776cccb2a8c3c1b412a4f4edabe3")
                .unwrap(),
        ];

        for hash in &request_hashes {
            assert_ne!(hash, signed_tx_2.hash())
        }

        let request_hashes = RequestTxHashes::new(request_hashes.into_iter().collect());

        // but response contains tx 1 + another tx
        let response_txns = PooledTransactions(vec![signed_tx_1.clone(), signed_tx_2]);
        let payload = UnverifiedPooledTransactions::new(response_txns);

        let (outcome, verified_payload) = payload.verify(&request_hashes, &PeerId::ZERO);

        assert_eq!(VerificationOutcome::ReportPeer, outcome);
        assert_eq!(1, verified_payload.len());
        assert!(verified_payload.contains(&signed_tx_1));
    }
}
