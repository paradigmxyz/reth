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

use crate::{
    cache::{LruCache, LruMap},
    duration_metered_exec,
    message::PeerRequest,
    metrics::TransactionFetcherMetrics,
    transactions::{validation, PartiallyFilterMessage},
};
use derive_more::{Constructor, Deref};
use futures::{stream::FuturesUnordered, Future, FutureExt, Stream, StreamExt};

use pin_project::pin_project;
use reth_eth_wire::{
    DedupPayload, EthVersion, GetPooledTransactions, HandleMempoolData, HandleVersionedMempoolData,
    PartiallyValidData, RequestTxHashes, ValidAnnouncementData,
};
use reth_interfaces::p2p::error::{RequestError, RequestResult};
use reth_primitives::{PeerId, PooledTransactionsElement, TxHash};
use schnellru::ByLength;
#[cfg(debug_assertions)]
use smallvec::{smallvec, SmallVec};
use std::{
    collections::HashMap,
    num::NonZeroUsize,
    pin::Pin,
    task::{ready, Context, Poll},
    time::{Duration, Instant},
};
use tokio::sync::{mpsc::error::TrySendError, oneshot, oneshot::error::RecvError};
use tracing::{debug, trace};
use validation::FilterOutcome;

use super::{
    config::TransactionFetcherConfig,
    constants::{tx_fetcher::*, SOFT_LIMIT_COUNT_HASHES_IN_GET_POOLED_TRANSACTIONS_REQUEST},
    MessageFilter, PeerMetadata, PooledTransactions,
    SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE,
};

/// The type responsible for fetching missing transactions from peers.
///
/// This will keep track of unique transaction hashes that are currently being fetched and submits
/// new requests on announced hashes.
#[derive(Debug)]
#[pin_project]
pub struct TransactionFetcher {
    /// All peers with to which a [`GetPooledTransactions`] request is inflight.
    pub active_peers: LruMap<PeerId, u8, ByLength>,
    /// All currently active [`GetPooledTransactions`] requests.
    ///
    /// The set of hashes encompassed by these requests are a subset of all hashes in the fetcher.
    /// It's disjoint from the set of hashes which are awaiting an idle fallback peer in order to
    /// be fetched.
    #[pin]
    pub inflight_requests: FuturesUnordered<GetPooledTxRequestFut>,
    /// Hashes that are awaiting an idle fallback peer so they can be fetched.
    ///
    /// This is a subset of all hashes in the fetcher, and is disjoint from the set of hashes for
    /// which a [`GetPooledTransactions`] request is inflight.
    pub hashes_pending_fetch: LruCache<TxHash>,
    /// Tracks all hashes in the transaction fetcher.
    pub(super) hashes_fetch_inflight_and_pending_fetch: LruMap<TxHash, TxFetchMetadata, ByLength>,
    /// Filter for valid announcement and response data.
    pub(super) filter_valid_message: MessageFilter,
    /// Info on capacity of the transaction fetcher.
    pub info: TransactionFetcherInfo,
    #[doc(hidden)]
    metrics: TransactionFetcherMetrics,
}

// === impl TransactionFetcher ===

impl TransactionFetcher {
    /// Updates metrics.
    #[inline]
    pub fn update_metrics(&self) {
        let metrics = &self.metrics;

        metrics.inflight_transaction_requests.set(self.inflight_requests.len() as f64);

        let hashes_pending_fetch = self.hashes_pending_fetch.len() as f64;
        let total_hashes = self.hashes_fetch_inflight_and_pending_fetch.len() as f64;

        metrics.hashes_pending_fetch.set(hashes_pending_fetch);
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
        let mut tx_fetcher = TransactionFetcher::default();

        tx_fetcher.info.soft_limit_byte_size_pooled_transactions_response =
            config.soft_limit_byte_size_pooled_transactions_response;
        tx_fetcher.info.soft_limit_byte_size_pooled_transactions_response_on_pack_request =
            config.soft_limit_byte_size_pooled_transactions_response_on_pack_request;
        tx_fetcher
            .metrics
            .capacity_inflight_requests
            .increment(tx_fetcher.info.max_inflight_requests as u64);

        tx_fetcher
    }

    /// Removes the specified hashes from inflight tracking.
    #[inline]
    pub fn remove_hashes_from_transaction_fetcher<I>(&mut self, hashes: I)
    where
        I: IntoIterator<Item = TxHash>,
    {
        for hash in hashes {
            self.hashes_fetch_inflight_and_pending_fetch.remove(&hash);
            self.hashes_pending_fetch.remove(&hash);
        }
    }

    /// Updates peer's activity status upon a resolved [`GetPooledTxRequest`].
    fn decrement_inflight_request_count_for(&mut self, peer_id: &PeerId) {
        let remove = || -> bool {
            if let Some(inflight_count) = self.active_peers.get(peer_id) {
                *inflight_count -= 1;
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
    pub fn is_idle(&self, peer_id: &PeerId) -> bool {
        let Some(inflight_count) = self.active_peers.peek(peer_id) else { return true };
        if *inflight_count < DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS_PER_PEER {
            return true
        }
        false
    }

    /// Returns any idle peer for the given hash.
    pub fn get_idle_peer_for(
        &self,
        hash: TxHash,
        is_session_active: impl Fn(&PeerId) -> bool,
    ) -> Option<&PeerId> {
        let TxFetchMetadata { fallback_peers, .. } =
            self.hashes_fetch_inflight_and_pending_fetch.peek(&hash)?;

        for peer_id in fallback_peers.iter() {
            if self.is_idle(peer_id) && is_session_active(peer_id) {
                return Some(peer_id)
            }
        }

        None
    }

    /// Returns any idle peer for any hash pending fetch. If one is found, the corresponding
    /// hash is written to the request buffer that is passed as parameter.
    ///
    /// Loops through the hashes pending fetch in lru order until one is found with an idle
    /// fallback peer, or the budget passed as parameter is depleted, whatever happens first.
    pub fn find_any_idle_fallback_peer_for_any_pending_hash(
        &mut self,
        hashes_to_request: &mut RequestTxHashes,
        is_session_active: impl Fn(&PeerId) -> bool,
        mut budget: Option<usize>, // search fallback peers for max `budget` lru pending hashes
    ) -> Option<PeerId> {
        let mut hashes_pending_fetch_iter = self.hashes_pending_fetch.iter();

        let idle_peer = loop {
            let &hash = hashes_pending_fetch_iter.next()?;

            let idle_peer = self.get_idle_peer_for(hash, &is_session_active);

            if idle_peer.is_some() {
                hashes_to_request.insert(hash);
                break idle_peer.copied()
            }

            if let Some(ref mut bud) = budget {
                *bud = bud.saturating_sub(1);
                if *bud == 0 {
                    return None
                }
            }
        };
        let hash = hashes_to_request.iter().next()?;

        // pop hash that is loaded in request buffer from cache of hashes pending fetch
        drop(hashes_pending_fetch_iter);
        _ = self.hashes_pending_fetch.remove(hash);

        idle_peer
    }

    /// Packages hashes for a [`GetPooledTxRequest`] up to limit. Returns left over hashes. Takes
    /// a [`RequestTxHashes`] buffer as parameter for filling with hashes to request.
    ///
    /// Returns left over hashes.
    pub fn pack_request(
        &mut self,
        hashes_to_request: &mut RequestTxHashes,
        hashes_from_announcement: ValidAnnouncementData,
    ) -> RequestTxHashes {
        if hashes_from_announcement.msg_version().is_eth68() {
            return self.pack_request_eth68(hashes_to_request, hashes_from_announcement)
        }
        self.pack_request_eth66(hashes_to_request, hashes_from_announcement)
    }

    /// Packages hashes for a [`GetPooledTxRequest`] from an
    /// [`Eth68`](reth_eth_wire::EthVersion::Eth68) announcement up to limit as defined by protocol
    /// version 68. Takes a [`RequestTxHashes`] buffer as parameter for filling with hashes to
    /// request.
    ///
    /// Returns left over hashes.
    ///
    /// Loops through hashes passed as parameter and checks if a hash fits in the expected
    /// response. If no, it's added to surplus hashes. If yes, it's added to hashes to the request
    /// and expected response size is accumulated.
    pub fn pack_request_eth68(
        &mut self,
        hashes_to_request: &mut RequestTxHashes,
        hashes_from_announcement: impl HandleMempoolData
            + IntoIterator<Item = (TxHash, Option<(u8, usize)>)>,
    ) -> RequestTxHashes {
        let mut acc_size_response = 0;
        let hashes_from_announcement_len = hashes_from_announcement.len();

        let mut hashes_from_announcement_iter = hashes_from_announcement.into_iter();

        if let Some((hash, Some((_ty, size)))) = hashes_from_announcement_iter.next() {
            hashes_to_request.insert(hash);

            // tx is really big, pack request with single tx
            if size >= self.info.soft_limit_byte_size_pooled_transactions_response_on_pack_request {
                return hashes_from_announcement_iter.collect::<RequestTxHashes>()
            } else {
                acc_size_response = size;
            }
        }

        let mut surplus_hashes = RequestTxHashes::with_capacity(hashes_from_announcement_len - 1);

        // folds size based on expected response size  and adds selected hashes to the request
        // list and the other hashes to the surplus list
        loop {
            let Some((hash, metadata)) = hashes_from_announcement_iter.next() else { break };

            let Some((_ty, size)) = metadata else {
                unreachable!("this method is called upon reception of an eth68 announcement")
            };

            let next_acc_size = acc_size_response + size;

            if next_acc_size <=
                self.info.soft_limit_byte_size_pooled_transactions_response_on_pack_request
            {
                // only update accumulated size of tx response if tx will fit in without exceeding
                // soft limit
                acc_size_response = next_acc_size;
                _ = hashes_to_request.insert(hash)
            } else {
                _ = surplus_hashes.insert(hash)
            }

            let free_space =
                self.info.soft_limit_byte_size_pooled_transactions_response_on_pack_request -
                    acc_size_response;

            if free_space < MEDIAN_BYTE_SIZE_SMALL_LEGACY_TX_ENCODED {
                break
            }
        }

        surplus_hashes.extend(hashes_from_announcement_iter.map(|(hash, _metadata)| hash));
        surplus_hashes.shrink_to_fit();
        hashes_to_request.shrink_to_fit();

        surplus_hashes
    }

    /// Packages hashes for a [`GetPooledTxRequest`] from an
    /// [`Eth66`](reth_eth_wire::EthVersion::Eth66) announcement up to limit as defined by
    /// protocol version 66. Takes a [`RequestTxHashes`] buffer as parameter for filling with
    /// hashes to request.
    ///
    /// Returns left over hashes.
    pub fn pack_request_eth66(
        &mut self,
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

    /// Buffers hashes. Note: Only peers that haven't yet tried to request the hashes should be
    /// passed as `fallback_peer` parameter! For re-buffering hashes on failed request, use
    /// [`TransactionFetcher::try_buffer_hashes_for_retry`]. Hashes that have been re-requested
    /// [`DEFAULT_MAX_RETRIES`], are dropped.
    pub fn buffer_hashes(&mut self, hashes: RequestTxHashes, fallback_peer: Option<PeerId>) {
        let mut max_retried_and_evicted_hashes = vec![];

        for hash in hashes.into_iter() {
            debug_assert!(
                self.hashes_fetch_inflight_and_pending_fetch.peek(&hash).is_some(),
                "`%hash` in `@buffered_hashes` that's not in `@hashes_fetch_inflight_and_pending_fetch`, `@buffered_hashes` should be a subset of keys in `@hashes_fetch_inflight_and_pending_fetch`, broken invariant `@buffered_hashes` and `@hashes_fetch_inflight_and_pending_fetch`,
`%hash`: {hash}"
            );

            let Some(TxFetchMetadata { retries, fallback_peers, .. }) =
                self.hashes_fetch_inflight_and_pending_fetch.get(&hash)
            else {
                return
            };

            if let Some(peer_id) = fallback_peer {
                // peer has not yet requested hash
                fallback_peers.insert(peer_id);
            } else {
                if *retries >= DEFAULT_MAX_RETRIES {
                    trace!(target: "net::tx",
                        hash=%hash,
                        retries=retries,
                        "retry limit for `GetPooledTransactions` requests reached for hash, dropping hash"
                    );

                    max_retried_and_evicted_hashes.push(hash);
                    continue
                }
                *retries += 1;
            }
            if let (_, Some(evicted_hash)) = self.hashes_pending_fetch.insert_and_get_evicted(hash)
            {
                max_retried_and_evicted_hashes.push(evicted_hash);
            }
        }

        self.remove_hashes_from_transaction_fetcher(max_retried_and_evicted_hashes);
    }

    /// Tries to request hashes pending fetch.
    ///
    /// Finds the first buffered hash with a fallback peer that is idle, if any. Fills the rest of
    /// the request by checking the transactions seen by the peer against the buffer.
    pub fn on_fetch_pending_hashes(
        &mut self,
        peers: &HashMap<PeerId, PeerMetadata>,
        has_capacity_wrt_pending_pool_imports: impl Fn(usize) -> bool,
    ) {
        let init_capacity_req = approx_capacity_get_pooled_transactions_req_eth68(&self.info);
        let mut hashes_to_request = RequestTxHashes::with_capacity(init_capacity_req);
        let is_session_active = |peer_id: &PeerId| peers.contains_key(peer_id);

        let mut search_durations = TxFetcherSearchDurations::default();

        // budget to look for an idle peer before giving up
        let budget_find_idle_fallback_peer = self
            .search_breadth_budget_find_idle_fallback_peer(&has_capacity_wrt_pending_pool_imports);

        let acc = &mut search_durations.fill_request;
        let peer_id = duration_metered_exec!(
            {
                let Some(peer_id) = self.find_any_idle_fallback_peer_for_any_pending_hash(
                    &mut hashes_to_request,
                    is_session_active,
                    budget_find_idle_fallback_peer,
                ) else {
                    // no peers are idle or budget is depleted
                    return
                };

                peer_id
            },
            acc
        );

        // peer should always exist since `is_session_active` already checked
        let Some(peer) = peers.get(&peer_id) else { return };
        let conn_eth_version = peer.version;

        // fill the request with more hashes pending fetch that have been announced by the peer.
        // the search for more hashes is done with respect to the given budget, which determines
        // how many hashes to loop through before giving up. if no more hashes are found wrt to
        // the budget, the single hash that was taken out of the cache above is sent in a request.
        let budget_fill_request = self
            .search_breadth_budget_find_intersection_pending_hashes_and_hashes_seen_by_peer(
                &has_capacity_wrt_pending_pool_imports,
            );

        let acc = &mut search_durations.find_idle_peer;
        duration_metered_exec!(
            {
                self.fill_request_from_hashes_pending_fetch(
                    &mut hashes_to_request,
                    &peer.seen_transactions,
                    budget_fill_request,
                )
            },
            acc
        );

        // free unused memory
        hashes_to_request.shrink_to_fit();

        self.update_pending_fetch_cache_search_metrics(search_durations);

        trace!(target: "net::tx",
            peer_id=format!("{peer_id:#}"),
            hashes=?*hashes_to_request,
            conn_eth_version=%conn_eth_version,
            "requesting hashes that were stored pending fetch from peer"
        );

        // request the buffered missing transactions
        if let Some(failed_to_request_hashes) =
            self.request_transactions_from_peer(hashes_to_request, peer)
        {
            debug!(target: "net::tx",
                peer_id=format!("{peer_id:#}"),
                failed_to_request_hashes=?failed_to_request_hashes,
                conn_eth_version=%conn_eth_version,
                "failed sending request to peer's session, buffering hashes"
            );

            self.buffer_hashes(failed_to_request_hashes, Some(peer_id));
        }
    }

    /// Filters out hashes that have been seen before. For hashes that have already been seen, the
    /// peer is added as fallback peer.
    pub fn filter_unseen_and_pending_hashes(
        &mut self,
        new_announced_hashes: &mut ValidAnnouncementData,
        is_tx_bad_import: impl Fn(&TxHash) -> bool,
        peer_id: &PeerId,
        is_session_active: impl Fn(PeerId) -> bool,
        client_version: &str,
    ) {
        #[cfg(not(debug_assertions))]
        let mut previously_unseen_hashes_count = 0;
        #[cfg(debug_assertions)]
        let mut previously_unseen_hashes = Vec::with_capacity(new_announced_hashes.len() / 4);

        let msg_version = new_announced_hashes.msg_version();

        // filter out inflight hashes, and register the peer as fallback for all inflight hashes
        new_announced_hashes.retain(|hash, metadata| {

            // occupied entry

            if let Some(TxFetchMetadata{ref mut fallback_peers, tx_encoded_length: ref mut previously_seen_size, ..}) = self.hashes_fetch_inflight_and_pending_fetch.peek_mut(hash) {
                // update size metadata if available
                if let Some((_ty, size)) = metadata {
                    if let Some(prev_size) = previously_seen_size {
                        // check if this peer is announcing a different size than a previous peer
                        if size != prev_size {
                            trace!(target: "net::tx",
                                peer_id=format!("{peer_id:#}"),
                                hash=%hash,
                                size=size,
                                previously_seen_size=previously_seen_size,
                                client_version=%client_version,
                                "peer announced a different size for tx, this is especially worrying if one size is much bigger..."
                            );
                        }
                    }
                    // believe the most recent peer to announce tx
                    *previously_seen_size = Some(*size);
                }

                // hash has been seen but is not inflight
                if self.hashes_pending_fetch.remove(hash) {
                    return true
                }
                // hash has been seen and is in flight. store peer as fallback peer.
                //
                // remove any ended sessions, so that in case of a full cache, alive peers aren't
                // removed in favour of lru dead peers
                let mut ended_sessions = vec![];
                for &peer_id in fallback_peers.iter() {
                    if is_session_active(peer_id) {
                        ended_sessions.push(peer_id);
                    }
                }
                for peer_id in ended_sessions {
                    fallback_peers.remove(&peer_id);
                }

                return false
            }

            // vacant entry

            if is_tx_bad_import(hash) {
                return false
            }

            #[cfg(not(debug_assertions))]
            {
                previously_unseen_hashes_count += 1;
            }
            #[cfg(debug_assertions)]
            previously_unseen_hashes.push(*hash);

            // todo: allow `MAX_ALTERNATIVE_PEERS_PER_TX` to be zero
            let limit = NonZeroUsize::new(DEFAULT_MAX_COUNT_FALLBACK_PEERS.into()).expect("MAX_ALTERNATIVE_PEERS_PER_TX should be non-zero");

            if self.hashes_fetch_inflight_and_pending_fetch.get_or_insert(*hash, ||
                TxFetchMetadata{retries: 0, fallback_peers: LruCache::new(limit), tx_encoded_length: None}
            ).is_none() {

                debug!(target: "net::tx",
                    peer_id=format!("{peer_id:#}"),
                    hash=%hash,
                    msg_version=?msg_version,
                    client_version=%client_version,
                    "failed to cache new announced hash from peer in schnellru::LruMap, dropping hash"
                );

                return false
            }
            true
        });

        #[cfg(not(debug_assertions))]
        trace!(target: "net::tx",
            peer_id=format!("{peer_id:#}"),
            previously_unseen_hashes_count=previously_unseen_hashes_count,
            msg_version=?msg_version,
            client_version=%client_version,
            "received previously unseen hashes in announcement from peer"
        );

        #[cfg(debug_assertions)]
        trace!(target: "net::tx",
            peer_id=format!("{peer_id:#}"),
            msg_version=?msg_version,
            client_version=%client_version,
            previously_unseen_hashes_len=?previously_unseen_hashes.len(),
            previously_unseen_hashes=?previously_unseen_hashes,
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
        peer: &PeerMetadata,
    ) -> Option<RequestTxHashes> {
        let peer_id: PeerId = peer.request_tx.peer_id;
        let conn_eth_version = peer.version;

        if self.active_peers.len() >= self.info.max_inflight_requests {
            trace!(target: "net::tx",
                peer_id=format!("{peer_id:#}"),
                new_announced_hashes=?*new_announced_hashes,
                conn_eth_version=%conn_eth_version,
                max_inflight_transaction_requests=self.info.max_inflight_requests,
                "limit for concurrent `GetPooledTransactions` requests reached, dropping request for hashes to peer"
            );
            return Some(new_announced_hashes)
        }

        let Some(inflight_count) = self.active_peers.get_or_insert(peer_id, || 0) else {
            debug!(target: "net::tx",
                peer_id=format!("{peer_id:#}"),
                new_announced_hashes=?*new_announced_hashes,
                conn_eth_version=%conn_eth_version,
                "failed to cache active peer in schnellru::LruMap, dropping request to peer"
            );
            return Some(new_announced_hashes)
        };

        if *inflight_count >= DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS_PER_PEER {
            trace!(target: "net::tx",
                peer_id=format!("{peer_id:#}"),
                new_announced_hashes=?*new_announced_hashes,
                conn_eth_version=%conn_eth_version,
                MAX_CONCURRENT_TX_REQUESTS_PER_PEER=DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS_PER_PEER,
                "limit for concurrent `GetPooledTransactions` requests per peer reached"
            );
            return Some(new_announced_hashes)
        }

        *inflight_count += 1;

        debug_assert!(
            || -> bool {
                for hash in new_announced_hashes.iter() {
                    if self.hashes_pending_fetch.contains(hash) {
                        return false
                    }
                }
                true
            }(),
            "`%new_announced_hashes` should been taken out of buffer before packing in a request, breaks invariant `@buffered_hashes` and `@inflight_requests`,
`@hashes_fetch_inflight_and_pending_fetch` for `%new_announced_hashes`: {:?}",
            new_announced_hashes.iter().map(|hash|
                (*hash, self.hashes_fetch_inflight_and_pending_fetch.get(hash).cloned())
            ).collect::<Vec<(TxHash, Option<TxFetchMetadata>)>>());

        let (response, rx) = oneshot::channel();
        let req: PeerRequest = PeerRequest::GetPooledTransactions {
            request: GetPooledTransactions(
                new_announced_hashes.iter().copied().collect::<Vec<_>>(),
            ),
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
        } else {
            // stores a new request future for the request
            self.inflight_requests.push(GetPooledTxRequestFut::new(
                peer_id,
                new_announced_hashes,
                rx,
            ))
        }

        None
    }

    /// Tries to fill request with hashes pending fetch so that the expected [`PooledTransactions`]
    /// response is full enough. A mutable reference to a list of hashes to request is passed as
    /// parameter. A budget is passed as parameter, this ensures that the node stops searching
    /// for more hashes after the budget is depleted. Under bad network conditions, the cache of
    /// hashes pending fetch may become very full for a while. As the node recovers, the hashes
    /// pending fetch cache should get smaller. The budget should aim to be big enough to loop
    /// through all buffered hashes in good network conditions.
    ///
    /// The request hashes buffer is filled as if it's an eth68 request, i.e. smartly assemble
    /// the request based on expected response size. For any hash missing size metadata, it is
    /// guessed at [`AVERAGE_BYTE_SIZE_TX_ENCODED`].

    /// Loops through hashes pending fetch and does:
    ///
    /// 1. Check if a hash pending fetch is seen by peer.
    /// 2. Optimistically include the hash in the request.
    /// 3. Accumulate expected total response size.
    /// 4. Check if acc size and hashes count is at limit, if so stop looping.
    /// 5. Remove hashes to request from cache of hashes pending fetch.
    pub fn fill_request_from_hashes_pending_fetch(
        &mut self,
        hashes_to_request: &mut RequestTxHashes,
        seen_hashes: &LruCache<TxHash>,
        mut budget_fill_request: Option<usize>, // check max `budget` lru pending hashes
    ) {
        let Some(hash) = hashes_to_request.iter().next() else { return };

        let mut acc_size_response = self
            .hashes_fetch_inflight_and_pending_fetch
            .get(hash)
            .and_then(|entry| entry.tx_encoded_len())
            .unwrap_or(AVERAGE_BYTE_SIZE_TX_ENCODED);

        // if request full enough already, we're satisfied, send request for single tx
        if acc_size_response >=
            DEFAULT_SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE_ON_FETCH_PENDING_HASHES
        {
            return
        }

        // try to fill request by checking if any other hashes pending fetch (in lru order) are
        // also seen by peer
        for hash in self.hashes_pending_fetch.iter() {
            // 1. Check if a hash pending fetch is seen by peer.
            if !seen_hashes.contains(hash) {
                continue
            };

            // 2. Optimistically include the hash in the request.
            hashes_to_request.insert(*hash);

            // 3. Accumulate expected total response size.
            let size = self
                .hashes_fetch_inflight_and_pending_fetch
                .get(hash)
                .and_then(|entry| entry.tx_encoded_len())
                .unwrap_or(AVERAGE_BYTE_SIZE_TX_ENCODED);

            acc_size_response += size;

            // 4. Check if acc size or hashes count is at limit, if so stop looping.
            // if expected response is full enough or the number of hashes in the request is
            // enough, we're satisfied
            if acc_size_response >=
                DEFAULT_SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE_ON_FETCH_PENDING_HASHES ||
                hashes_to_request.len() >
                    DEFAULT_SOFT_LIMIT_COUNT_HASHES_IN_GET_POOLED_TRANSACTIONS_REQUEST_ON_FETCH_PENDING_HASHES
            {
                break
            }

            if let Some(ref mut bud) = budget_fill_request {
                *bud = bud.saturating_sub(1);
                if *bud == 0 {
                    return
                }
            }
        }

        // 5. Remove hashes to request from cache of hashes pending fetch.
        for hash in hashes_to_request.iter() {
            self.hashes_pending_fetch.remove(hash);
        }
    }

    /// Returns `true` if [`TransactionFetcher`] has capacity to request pending hashes. Returns
    /// `false` if [`TransactionFetcher`] is operating close to full capacity.
    pub fn has_capacity_for_fetching_pending_hashes(&self) -> bool {
        let info = &self.info;

        self.has_capacity(info.max_inflight_requests)
    }

    /// Returns `true` if the number of inflight requests are under a given tolerated max.
    fn has_capacity(&self, max_inflight_requests: usize) -> bool {
        self.inflight_requests.len() <= max_inflight_requests
    }

    /// Returns the limit to enforce when looking for any pending hash with an idle fallback peer.
    ///
    /// Returns `Some(limit)` if [`TransactionFetcher`] and the
    /// [`TransactionPool`](reth_transaction_pool::TransactionPool) are operating close to full
    /// capacity. Returns `None`, unlimited, if they are not that busy.
    pub fn search_breadth_budget_find_idle_fallback_peer(
        &self,
        has_capacity_wrt_pending_pool_imports: impl Fn(usize) -> bool,
    ) -> Option<usize> {
        let info = &self.info;

        let tx_fetcher_has_capacity = self.has_capacity(
            info.max_inflight_requests /
                DEFAULT_DIVISOR_MAX_COUNT_INFLIGHT_REQUESTS_ON_FIND_IDLE_PEER,
        );
        let tx_pool_has_capacity = has_capacity_wrt_pending_pool_imports(
            DEFAULT_DIVISOR_MAX_COUNT_PENDING_POOL_IMPORTS_ON_FIND_IDLE_PEER,
        );

        if tx_fetcher_has_capacity && tx_pool_has_capacity {
            // unlimited search breadth
            None
        } else {
            // limited breadth of search for idle peer
            let limit = DEFAULT_BUDGET_FIND_IDLE_FALLBACK_PEER;

            trace!(target: "net::tx",
                inflight_requests=self.inflight_requests.len(),
                max_inflight_transaction_requests=info.max_inflight_requests,
                hashes_pending_fetch=self.hashes_pending_fetch.len(),
                limit=limit,
                "search breadth limited in search for idle fallback peer for some hash pending fetch"
            );

            Some(limit)
        }
    }

    /// Returns the limit to enforce when looking for the intersection between hashes announced by
    /// peer and hashes pending fetch.
    ///
    /// Returns `Some(limit)` if [`TransactionFetcher`] and the
    /// [`TransactionPool`](reth_transaction_pool::TransactionPool) are operating close to full
    /// capacity. Returns `None`, unlimited, if they are not that busy.
    pub fn search_breadth_budget_find_intersection_pending_hashes_and_hashes_seen_by_peer(
        &self,
        has_capacity_wrt_pending_pool_imports: impl Fn(usize) -> bool,
    ) -> Option<usize> {
        let info = &self.info;

        let tx_fetcher_has_capacity = self.has_capacity(
            info.max_inflight_requests /
                DEFAULT_DIVISOR_MAX_COUNT_INFLIGHT_REQUESTS_ON_FIND_INTERSECTION,
        );
        let tx_pool_has_capacity = has_capacity_wrt_pending_pool_imports(
            DEFAULT_DIVISOR_MAX_COUNT_PENDING_POOL_IMPORTS_ON_FIND_INTERSECTION,
        );

        if tx_fetcher_has_capacity && tx_pool_has_capacity {
            // unlimited search breadth
            None
        } else {
            // limited breadth of search for idle peer
            let limit = DEFAULT_BUDGET_FIND_INTERSECTION_ANNOUNCED_BY_PEER_AND_PENDING_FETCH;

            trace!(target: "net::tx",
                inflight_requests=self.inflight_requests.len(),
                max_inflight_transaction_requests=self.info.max_inflight_requests,
                hashes_pending_fetch=self.hashes_pending_fetch.len(),
                limit=limit,
                "search breadth limited in search for intersection of hashes announced by peer and hashes pending fetch"
            );

            Some(limit)
        }
    }

    /// Returns the approx number of transactions that a [`GetPooledTransactions`] request will
    /// have capacity for w.r.t. the given version of the protocol.
    pub fn approx_capacity_get_pooled_transactions_req(
        &self,
        announcement_version: EthVersion,
    ) -> usize {
        if announcement_version.is_eth68() {
            approx_capacity_get_pooled_transactions_req_eth68(&self.info)
        } else {
            approx_capacity_get_pooled_transactions_req_eth66()
        }
    }

    /// Processes a resolved [`GetPooledTransactions`] request. Queues the outcome as a
    /// [`FetchEvent`], which will then be streamed by
    /// [`TransactionsManager`](super::TransactionsManager).
    pub fn on_resolved_get_pooled_transactions_request_fut(
        &mut self,
        response: GetPooledTxResponse,
    ) -> FetchEvent {
        // update peer activity, requests for buffered hashes can only be made to idle
        // fallback peers
        let GetPooledTxResponse { peer_id, mut requested_hashes, result } = response;

        debug_assert!(
            self.active_peers.get(&peer_id).is_some(),
            "`%peer_id` has been removed from `@active_peers` before inflight request(s) resolved, broken invariant `@active_peers` and `@inflight_requests`,
`%peer_id`: {},
`@hashes_fetch_inflight_and_pending_fetch` for `%requested_hashes`: {:?}",
            peer_id, requested_hashes.iter().map(|hash|
                (*hash, self.hashes_fetch_inflight_and_pending_fetch.get(hash).cloned())
            ).collect::<Vec<(TxHash, Option<TxFetchMetadata>)>>());

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

                if verification_outcome == VerificationOutcome::ReportPeer {
                    // todo: report peer for sending hashes that weren't requested
                    trace!(target: "net::tx",
                        peer_id=format!("{peer_id:#}"),
                        unverified_len=unverified_len,
                        verified_payload_len=verified_payload.len(),
                        "received `PooledTransactions` response from peer with entries that didn't verify against request, filtered out transactions"
                    );
                }
                // peer has only sent hashes that we didn't request
                if verified_payload.is_empty() {
                    return FetchEvent::FetchError { peer_id, error: RequestError::BadResponse }
                }

                //
                // 3. stateless validation of payload, e.g. dedup
                //
                let unvalidated_payload_len = verified_payload.len();

                let (validation_outcome, valid_payload) =
                    self.filter_valid_message.partially_filter_valid_entries(verified_payload);

                // todo: validate based on announced tx size/type and report peer for sending
                // invalid response <https://github.com/paradigmxyz/reth/issues/6529>. requires
                // passing the rlp encoded length down from active session along with the decoded
                // tx.

                if validation_outcome == FilterOutcome::ReportPeer {
                    trace!(target: "net::tx",
                        peer_id=format!("{peer_id:#}"),
                        unvalidated_payload_len=unvalidated_payload_len,
                        valid_payload_len=valid_payload.len(),
                        "received invalid `PooledTransactions` response from peer, filtered out duplicate entries"
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

                let transactions =
                    valid_payload.into_data().into_values().collect::<PooledTransactions>();

                FetchEvent::TransactionsFetched { peer_id, transactions }
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

impl Stream for TransactionFetcher {
    type Item = FetchEvent;

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

impl Default for TransactionFetcher {
    fn default() -> Self {
        Self {
            active_peers: LruMap::new(DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS),
            inflight_requests: Default::default(),
            hashes_pending_fetch: LruCache::new(
                NonZeroUsize::new(DEFAULT_MAX_CAPACITY_CACHE_PENDING_FETCH)
                    .expect("buffered cache limit should be non-zero"),
            ),
            hashes_fetch_inflight_and_pending_fetch: LruMap::new(
                DEFAULT_MAX_CAPACITY_CACHE_INFLIGHT_AND_PENDING_FETCH
                    .try_into()
                    .expect("proper size for inflight and pending fetch cache"),
            ),
            filter_valid_message: Default::default(),
            info: TransactionFetcherInfo::default(),
            metrics: Default::default(),
        }
    }
}

/// Metadata of a transaction hash that is yet to be fetched.
#[derive(Debug, Constructor, Clone)]
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
    pub fn fallback_peers_mut(&mut self) -> &mut LruCache<PeerId> {
        &mut self.fallback_peers
    }

    /// Returns the size of the transaction, if its hash has been received in any
    /// [`Eth68`](reth_eth_wire::EthVersion::Eth68) announcement. If the transaction hash has only
    /// been seen in [`Eth66`](reth_eth_wire::EthVersion::Eth66) announcements so far, this will
    /// return `None`.
    pub fn tx_encoded_len(&self) -> Option<usize> {
        self.tx_encoded_length
    }
}

/// Represents possible events from fetching transactions.
#[derive(Debug)]
pub enum FetchEvent {
    /// Triggered when transactions are successfully fetched.
    TransactionsFetched {
        /// The ID of the peer from which transactions were fetched.
        peer_id: PeerId,
        /// The transactions that were fetched, if available.
        transactions: PooledTransactions,
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
pub struct GetPooledTxRequest {
    peer_id: PeerId,
    /// Transaction hashes that were requested, for cleanup purposes
    requested_hashes: RequestTxHashes,
    response: oneshot::Receiver<RequestResult<PooledTransactions>>,
}

/// Upon reception of a response, a [`GetPooledTxRequest`] is deconstructed to form a
/// [`GetPooledTxResponse`].
#[derive(Debug)]
pub struct GetPooledTxResponse {
    peer_id: PeerId,
    /// Transaction hashes that were requested, for cleanup purposes, since peer may only return a
    /// subset of requested hashes.
    requested_hashes: RequestTxHashes,
    result: Result<RequestResult<PooledTransactions>, RecvError>,
}

/// Stores the response receiver made by sending a [`GetPooledTransactions`] request to a peer's
/// session.
#[must_use = "futures do nothing unless polled"]
#[pin_project::pin_project]
#[derive(Debug)]
pub struct GetPooledTxRequestFut {
    #[pin]
    inner: Option<GetPooledTxRequest>,
}

impl GetPooledTxRequestFut {
    #[inline]
    fn new(
        peer_id: PeerId,
        requested_hashes: RequestTxHashes,
        response: oneshot::Receiver<RequestResult<PooledTransactions>>,
    ) -> Self {
        Self { inner: Some(GetPooledTxRequest { peer_id, requested_hashes, response }) }
    }
}

impl Future for GetPooledTxRequestFut {
    type Output = GetPooledTxResponse;

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
pub struct UnverifiedPooledTransactions {
    txns: PooledTransactions,
}

/// [`PooledTransactions`] that have been successfully verified.
#[derive(Debug, Constructor, Deref)]
pub struct VerifiedPooledTransactions {
    txns: PooledTransactions,
}

impl DedupPayload for VerifiedPooledTransactions {
    type Value = PooledTransactionsElement;

    fn is_empty(&self) -> bool {
        self.txns.is_empty()
    }

    fn len(&self) -> usize {
        self.txns.len()
    }

    fn dedup(self) -> PartiallyValidData<Self::Value> {
        let Self { txns } = self;
        let unique_fetched = txns
            .into_iter()
            .map(|tx| (*tx.hash(), tx))
            .collect::<HashMap<TxHash, PooledTransactionsElement>>();

        PartiallyValidData::from_raw_data(unique_fetched, None)
    }
}

trait VerifyPooledTransactionsResponse {
    fn verify(
        self,
        requested_hashes: &RequestTxHashes,
        peer_id: &PeerId,
    ) -> (VerificationOutcome, VerifiedPooledTransactions);
}

impl VerifyPooledTransactionsResponse for UnverifiedPooledTransactions {
    fn verify(
        self,
        requested_hashes: &RequestTxHashes,
        _peer_id: &PeerId,
    ) -> (VerificationOutcome, VerifiedPooledTransactions) {
        let mut verification_outcome = VerificationOutcome::Ok;

        let Self { mut txns } = self;

        #[cfg(debug_assertions)]
        let mut tx_hashes_not_requested: SmallVec<[TxHash; 16]> = smallvec!();

        txns.0.retain(|tx| {
            if !requested_hashes.contains(tx.hash()) {
                verification_outcome = VerificationOutcome::ReportPeer;

                #[cfg(debug_assertions)]
                tx_hashes_not_requested.push(*tx.hash());

                return false
            }
            true
        });

        #[cfg(debug_assertions)]
        trace!(target: "net::tx",
            peer_id=format!("{_peer_id:#}"),
            tx_hashes_not_requested=?tx_hashes_not_requested,
            "transactions in `PooledTransactions` response from peer were not requested"
        );

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
#[derive(Debug)]
pub struct TransactionFetcherInfo {
    /// Currently active outgoing [`GetPooledTransactions`] requests.
    pub max_inflight_requests: usize,
    /// Soft limit for the byte size of the expected
    /// [`PooledTransactions`] response on packing a
    /// [`GetPooledTransactions`] request with hashes.
    pub(super) soft_limit_byte_size_pooled_transactions_response_on_pack_request: usize,
    /// Soft limit for the byte size of a [`PooledTransactions`]
    /// response on assembling a [`GetPooledTransactions`]
    /// request. Spec'd at 2 MiB.
    pub soft_limit_byte_size_pooled_transactions_response: usize,
}

impl TransactionFetcherInfo {
    /// Creates a new max
    pub fn new(
        max_inflight_transaction_requests: usize,
        soft_limit_byte_size_pooled_transactions_response_on_pack_request: usize,
        soft_limit_byte_size_pooled_transactions_response: usize,
    ) -> Self {
        Self {
            max_inflight_requests: max_inflight_transaction_requests,
            soft_limit_byte_size_pooled_transactions_response_on_pack_request,
            soft_limit_byte_size_pooled_transactions_response,
        }
    }
}

impl Default for TransactionFetcherInfo {
    fn default() -> Self {
        Self::new(
            DEFAULT_MAX_COUNT_INFLIGHT_REQUESTS_ON_FETCH_PENDING_HASHES,
            DEFAULT_SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESP_ON_PACK_GET_POOLED_TRANSACTIONS_REQ,
            SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE
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
    use std::{collections::HashSet, str::FromStr};

    use alloy_rlp::Decodable;
    use derive_more::IntoIterator;
    use reth_primitives::{hex, TransactionSigned, B256};

    use crate::transactions::tests::{default_cache, new_mock_session};

    use super::*;

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

        let tx_fetcher = &mut TransactionFetcher::default();

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
        let mut peers = HashMap::new();
        peers.insert(peer_1, peer_1_data);
        peers.insert(peer_2, peer_2_data);

        // insert peer_2 as fallback peer for seen_hashes
        let mut backups = default_cache();
        backups.insert(peer_2);
        // insert seen_hashes into tx fetcher
        for i in 0..3 {
            let meta = TxFetchMetadata::new(0, backups.clone(), Some(seen_eth68_hashes_sizes[i]));
            tx_fetcher.hashes_fetch_inflight_and_pending_fetch.insert(seen_hashes[i], meta);
        }
        let meta = TxFetchMetadata::new(0, backups.clone(), None);
        tx_fetcher.hashes_fetch_inflight_and_pending_fetch.insert(seen_hashes[3], meta);
        //
        // insert pending hash without peer_1 as fallback peer, only with peer_2 as fallback peer
        let hash_other = B256::from_slice(&[5; 32]);
        tx_fetcher
            .hashes_fetch_inflight_and_pending_fetch
            .insert(hash_other, TxFetchMetadata::new(0, backups, None));
        tx_fetcher.hashes_pending_fetch.insert(hash_other);

        // add peer_1 as lru fallback peer for seen hashes
        for hash in &seen_hashes {
            tx_fetcher
                .hashes_fetch_inflight_and_pending_fetch
                .get(hash)
                .unwrap()
                .fallback_peers_mut()
                .insert(peer_1);
        }

        // mark seen hashes as pending fetch
        for hash in &seen_hashes {
            tx_fetcher.hashes_pending_fetch.insert(*hash);
        }

        // seen hashes and the random hash from peer_2 are pending fetch
        assert_eq!(tx_fetcher.hashes_pending_fetch.len(), 5);

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
        let input = hex!("02f871018302a90f808504890aef60826b6c94ddf4c5025d1a5742cf12f74eec246d4432c295e487e09c3bbcc12b2b80c080a0f21a4eacd0bf8fea9c5105c543be5a1d8c796516875710fafafdf16d16d8ee23a001280915021bb446d1973501a67f93d2b38894a514b976e7b46dc2fe54598daa");
        let signed_tx_1: PooledTransactionsElement =
            TransactionSigned::decode(&mut &input[..]).unwrap().try_into().unwrap();
        let input = hex!("02f871018302a90f808504890aef60826b6c94ddf4c5025d1a5742cf12f74eec246d4432c295e487e09c3bbcc12b2b80c080a0f21a4eacd0bf8fea9c5105c543be5a1d8c796516875710fafafdf16d16d8ee23a001280915021bb446d1973501a67f93d2b38894a514b976e7b46dc2fe54598d76");
        let signed_tx_2: PooledTransactionsElement =
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

        let request_hashes =
            RequestTxHashes::new(request_hashes.into_iter().collect::<HashSet<_>>());

        // but response contains tx 1 + another tx
        let response_txns = PooledTransactions(vec![signed_tx_1.clone(), signed_tx_2]);
        let payload = UnverifiedPooledTransactions::new(response_txns);

        let (outcome, verified_payload) = payload.verify(&request_hashes, &PeerId::ZERO);

        assert_eq!(VerificationOutcome::ReportPeer, outcome);
        assert_eq!(1, verified_payload.len());
        assert!(verified_payload.contains(&signed_tx_1));
    }
}
