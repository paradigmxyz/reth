use crate::{
    cache::{LruCache, LruMap},
    message::PeerRequest,
};
use futures::{stream::FuturesUnordered, Future, FutureExt, Stream, StreamExt};
use pin_project::pin_project;
use reth_eth_wire::{EthVersion, GetPooledTransactions, ValidAnnouncementData};
use reth_interfaces::p2p::error::{RequestError, RequestResult};
use reth_primitives::{PeerId, PooledTransactionsElement, TxHash};
use schnellru::{ByLength, Unlimited};
use std::{
    collections::HashMap,
    mem,
    num::NonZeroUsize,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::sync::{mpsc::error::TrySendError, oneshot, oneshot::error::RecvError};
use tracing::{debug, trace};

use super::{
    AnnouncementFilter, Peer, PooledTransactions, TransactionsManagerMetrics,
    NEW_POOLED_TRANSACTION_HASHES_SOFT_LIMIT, POOLED_TRANSACTIONS_RESPONSE_SOFT_LIMIT_BYTE_SIZE,
};

/// How many peers we keep track of for each missing transaction.
pub(super) const MAX_ALTERNATIVE_PEERS_PER_TX: u8 =
    MAX_REQUEST_RETRIES_PER_TX_HASH + MARGINAL_FALLBACK_PEERS_PER_TX;

/// Marginal on fallback peers. If all fallback peers are idle, at most
/// [`MAX_REQUEST_RETRIES_PER_TX_HASH`] of them can ever be needed.
const MARGINAL_FALLBACK_PEERS_PER_TX: u8 = 1;

/// Maximum request retires per [`TxHash`]. Note, this is reset should the [`TxHash`] re-appear in
/// an announcement after it has been ejected from the hash buffer.
const MAX_REQUEST_RETRIES_PER_TX_HASH: u8 = 2;

/// Default maximum concurrent [`GetPooledTxRequest`]s.
pub(super) const DEFAULT_MAX_CONCURRENT_TX_REQUESTS: u32 = 10000;

/// Maximum concurrent [`GetPooledTxRequest`]s to allow per peer.
pub(super) const MAX_CONCURRENT_TX_REQUESTS_PER_PEER: u8 = 1;

/// Cache limit of transactions waiting for idle peer to be fetched.
pub(super) const MAX_CAPACITY_CACHE_FOR_HASHES_PENDING_FETCH: usize =
    100 * GET_POOLED_TRANSACTION_SOFT_LIMIT_NUM_HASHES;

/// Default maximum number of hashes pending fetch to tolerate at any time.
const DEFAULT_MAX_HASHES_PENDING_FETCH: usize = MAX_CAPACITY_CACHE_FOR_HASHES_PENDING_FETCH / 2;

/// Recommended soft limit for the number of hashes in a GetPooledTransactions message (8kb)
///
/// <https://github.com/ethereum/devp2p/blob/master/caps/eth.md#newpooledtransactionhashes-0x08>
pub(super) const GET_POOLED_TRANSACTION_SOFT_LIMIT_NUM_HASHES: usize = 256;

/// Soft limit for a [`PooledTransactions`] response if request is filled from hashes pending
/// fetch.
const SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE_ON_FETCH_PENDING: usize =
    POOLED_TRANSACTIONS_RESPONSE_SOFT_LIMIT_BYTE_SIZE / 2;

/// Soft limit for the number of hashes in a [`GetPooledTransactions`] request is filled from
/// hashes pending fetch.
const GET_POOLED_TRANSACTION_SOFT_LIMIT_NUM_HASHES_ON_FETCH_PENDING: usize =
    GET_POOLED_TRANSACTION_SOFT_LIMIT_NUM_HASHES / 2;

/// Average byte size of an encoded transaction. Estimated by the standard recommended max
/// response size divided by the recommended max count of hashes in an [`GetPooledTransactions`]
/// request.
const TX_ENCODED_LENGTH_AVERAGE: usize =
    POOLED_TRANSACTIONS_RESPONSE_SOFT_LIMIT_BYTE_SIZE / NEW_POOLED_TRANSACTION_HASHES_SOFT_LIMIT;

/// The type responsible for fetching missing transactions from peers.
///
/// This will keep track of unique transaction hashes that are currently being fetched and submits
/// new requests on announced hashes.
#[derive(Debug)]
#[pin_project]
pub(super) struct TransactionFetcher<T = TxUnknownToPool> {
    /// All peers with an `inflight_requests`.
    pub(super) active_peers: LruMap<PeerId, u8, ByLength>,
    /// All currently active requests for pooled transactions.
    #[pin]
    pub(super) inflight_requests: FuturesUnordered<GetPooledTxRequestFut>,
    /// Hashes that are awaiting an idle peer so they can be fetched.
    pub(super) hashes_pending_fetch: LruCache<TxHash>,
    /// Tracks all hashes in the transaction fetcher.
    pub(super) hashes_unknown_to_pool: LruMap<TxHash, T, Unlimited>,
    /// Filter for valid eth68 announcements.
    pub(super) filter_valid_hashes: AnnouncementFilter,
    /// Info on capacity of the transaction fetcher.
    tx_fetcher_info: TransactionFetcherInfo,
}

#[derive(Debug)]
pub(super) struct TxUnknownToPool {
    retries: u8,
    fallback_peers: LruCache<PeerId>,
    // todo: store all seen sizes as a `(size, peer_id)` tuple to catch peers that respond with
    // another size tx than they announced. alt enter in request (won't catch peers announcing
    // wrong size incase of refetch if stored in request fut)
    tx_encoded_length: Option<usize>,
}

impl TxUnknownToPool {
    pub fn fallback_peers_mut(&mut self) -> &mut LruCache<PeerId> {
        &mut self.fallback_peers
    }

    pub fn tx_encoded_len(&self) -> Option<usize> {
        self.tx_encoded_length
    }

    #[cfg(test)]
    pub fn new(
        retries: u8,
        fallback_peers: LruCache<PeerId>,
        tx_encoded_length: Option<usize>,
    ) -> Self {
        Self { retries, fallback_peers, tx_encoded_length }
    }
}
// === impl TransactionFetcher ===

impl TransactionFetcher {
    /// Removes the specified hashes from inflight tracking.
    #[inline]
    fn remove_from_unknown_hashes<I>(&mut self, hashes: I)
    where
        I: IntoIterator<Item = TxHash>,
    {
        for hash in hashes {
            self.hashes_unknown_to_pool.remove(&hash);
            self.hashes_pending_fetch.remove(&hash);
        }
    }

    /// Updates peer's activity status upon a resolved [`GetPooledTxRequest`].
    fn decrement_inflight_request_count_for(&mut self, peer_id: &PeerId) {
        let remove = || -> bool {
            if let Some(inflight_count) = self.active_peers.get(peer_id) {
                if *inflight_count <= MAX_CONCURRENT_TX_REQUESTS_PER_PEER {
                    return true
                }
                *inflight_count -= 1;
            }
            false
        }();

        if remove {
            self.active_peers.remove(peer_id);
        }
    }

    /// Returns `true` if peer is idle.
    pub(super) fn is_idle(&self, peer_id: &PeerId) -> bool {
        let Some(inflight_count) = self.active_peers.peek(peer_id) else { return true };
        if *inflight_count < MAX_CONCURRENT_TX_REQUESTS_PER_PEER {
            return true
        }
        false
    }

    /// Returns any idle peer for the given hash. Writes peer IDs of any ended sessions to buffer
    /// passed as parameter.
    pub(super) fn get_idle_peer_for(
        &self,
        hash: TxHash,
        is_session_active: impl Fn(&PeerId) -> bool,
    ) -> Option<&PeerId> {
        let TxUnknownToPool { fallback_peers, .. } = self.hashes_unknown_to_pool.peek(&hash)?;

        for peer_id in fallback_peers.iter() {
            if self.is_idle(peer_id) && is_session_active(peer_id) {
                return Some(peer_id)
            }
        }

        None
    }

    /// Returns any idle peer for any buffered unknown hash, and writes that hash to the request's
    /// hashes buffer that is passed as parameter.
    ///
    /// Loops through the fallback peers of each buffered hashes, until an idle fallback peer is
    /// found. As a side effect, dead fallback peers are filtered out for visited hashes.
    pub(super) fn pop_any_idle_peer(
        &mut self,
        hashes: &mut Vec<TxHash>,
        is_session_active: impl Fn(&PeerId) -> bool,
        mut budget: usize, // try to find `budget` idle peers before giving up
    ) -> Option<PeerId> {
        let mut hashes_pending_fetch_iter = self.hashes_pending_fetch.iter();

        let idle_peer = loop {
            budget = budget.saturating_sub(1);
            if budget == 0 {
                return None
            }

            let &hash = hashes_pending_fetch_iter.next()?;

            let idle_peer = self.get_idle_peer_for(hash, &is_session_active);

            if idle_peer.is_some() {
                hashes.push(hash);
                break idle_peer.map(|p| *p)
            }
        };
        let hash = hashes.first()?;

        // pop hash that is loaded in request buffer from cache of hashes pending fetch
        drop(hashes_pending_fetch_iter);
        _ = self.hashes_pending_fetch.remove(hash);

        idle_peer
    }

    /// Packages hashes for [`GetPooledTxRequest`] up to limit as defined by protocol version 66.
    /// If necessary, takes hashes from buffer for which peer is listed as fallback peer.
    ///
    /// Returns left over hashes.
    pub(super) fn pack_hashes_eth66(
        &mut self,
        hashes_to_request: &mut Vec<TxHash>,
        hashes_from_announcement: ValidAnnouncementData,
    ) -> Vec<TxHash> {
        let mut hashes = hashes_from_announcement.into_keys().collect::<Vec<_>>();

        let surplus_hashes = if hashes.len() <= GET_POOLED_TRANSACTION_SOFT_LIMIT_NUM_HASHES {
            vec![]
        } else {
            hashes.split_off(GET_POOLED_TRANSACTION_SOFT_LIMIT_NUM_HASHES - 1)
        };

        _ = mem::replace(hashes_to_request, hashes);

        surplus_hashes
    }

    /// Evaluates wether or not to include a hash in a `GetPooledTransactions` version eth68
    /// request, based on the size of the transaction and the accumulated size of the
    /// corresponding `PooledTransactions` response.
    ///
    /// Returns `true` if hash is included in request. If there is still space in the respective
    /// response but not enough for the transaction of given hash, `false` is returned.
    fn fold_size_of_eth68_response(&self, acc_size_response: &mut usize, size: usize) -> bool {
        let next_acc_size = *acc_size_response + size;

        if next_acc_size <= POOLED_TRANSACTIONS_RESPONSE_SOFT_LIMIT_BYTE_SIZE {
            // only update accumulated size of tx response if tx will fit in without exceeding
            // soft limit
            *acc_size_response = next_acc_size;
            return true
        }

        false
    }

    /// Packages hashes for [`GetPooledTxRequest`] up to limit as defined by protocol version 68.
    /// If necessary, takes hashes from buffer for which peer is listed as fallback peer. Returns
    /// left over hashes.
    ///
    /// 1. Loops through hashes passed as parameter, calculating the accumulated size of the
    /// response that this request would generate if filled with requested hashes.
    /// 2.a. All hashes fit in response and there is no more space. Returns empty vector.
    /// 2.b. Some hashes didn't fit in and there is no more space. Returns surplus hashes.
    /// 2.c. All hashes fit in response and there is still space. Surplus hashes = empty vector.
    /// 2.d. Some hashes didn't fit in but there is still space. Surplus hashes != empty vector.
    /// 3. Try to fill remaining space with hashes from buffer.
    /// 4. Return surplus hashes.
    pub(super) fn pack_hashes_eth68(
        &mut self,
        hashes_to_request: &mut Vec<TxHash>,
        hashes_from_announcement: ValidAnnouncementData,
    ) -> Vec<TxHash> {
        let mut acc_size_response = 0;
        let mut surplus_hashes = vec![];

        for (hash, metadata) in hashes_from_announcement {
            let Some((_ty, size)) = metadata else {
                unreachable!("this method is called upon reception of an eth68 announcement")
            };

            match self.fold_size_of_eth68_response(&mut acc_size_response, size) {
                true => hashes_to_request.push(hash),
                false => surplus_hashes.push(hash),
            }
        }

        surplus_hashes
    }

    pub(super) fn buffer_hashes_for_retry(
        &mut self,
        mut hashes: Vec<TxHash>,
        peer_failed_to_serve: &PeerId,
    ) {
        // It could be that the txns have been received over broadcast in the time being. Remove
        // the peer as fallback peer so it isn't request again for these hashes.
        hashes.retain(|hash| {
            if let Some(entry) = self.hashes_unknown_to_pool.get(hash) {
                entry.fallback_peers_mut().remove(peer_failed_to_serve);
                return true
            }
            // tx has been seen over broadcast in the time it took for the request to resolve
            false
        });

        self.buffer_hashes(hashes, None)
    }

    /// Buffers hashes. Note: Only peers that haven't yet tried to request the hashes should be
    /// passed as `fallback_peer` parameter! Hashes that have been re-requested
    /// [`MAX_REQUEST_RETRIES_PER_TX_HASH`], are dropped.
    pub(super) fn buffer_hashes(&mut self, hashes: Vec<TxHash>, fallback_peer: Option<PeerId>) {
        let mut max_retried_and_evicted_hashes = vec![];

        for hash in hashes {
            // todo: enforce by adding new types UnknownTxHash66 and UnknownTxHash68
            debug_assert!(
                self.hashes_unknown_to_pool.peek(&hash).is_some(),
                "`%hash` in `@buffered_hashes` that's not in `@unknown_hashes`, `@buffered_hashes` should be a subset of keys in `@unknown_hashes`, broken invariant `@buffered_hashes` and `@unknown_hashes`,
`%hash`: {hash},
`@self`: {self:?}",
            );

            let Some(TxUnknownToPool { retries, fallback_peers, .. }) =
                self.hashes_unknown_to_pool.get(&hash)
            else {
                return
            };

            if let Some(peer_id) = fallback_peer {
                // peer has not yet requested hash
                fallback_peers.insert(peer_id);
            } else {
                if *retries >= MAX_REQUEST_RETRIES_PER_TX_HASH {
                    debug!(target: "net::tx",
                        hash=%hash,
                        retries=retries,
                        "retry limit for `GetPooledTransactions` requests reached for hash, dropping hash"
                    );

                    max_retried_and_evicted_hashes.push(hash);
                    continue;
                }
                *retries += 1;
            }
            if let (_, Some(evicted_hash)) = self.hashes_pending_fetch.insert_and_get_evicted(hash)
            {
                max_retried_and_evicted_hashes.push(evicted_hash);
            }
        }

        self.remove_from_unknown_hashes(max_retried_and_evicted_hashes);
    }

    /// Tries to request hashes pending fetch.
    ///
    /// Finds the first buffered hash with a fallback peer that is idle, if any. Fills the rest of
    /// the request by checking the transactions seen by the peer against the buffer.
    pub(super) fn request_hashes_pending_fetch(
        &mut self,
        peers: &HashMap<PeerId, Peer>,
        metrics: &TransactionsManagerMetrics,
        budget_find_idle_peer: usize,
    ) {
        let mut hashes_to_request = vec![];
        let is_session_active = |peer_id: &PeerId| peers.contains_key(peer_id);

        // budget to look for an idle peer before giving up
        let Some(peer_id) = self.pop_any_idle_peer(
            &mut hashes_to_request,
            is_session_active,
            budget_find_idle_peer,
        ) else {
            // no peers are idle or budget is depleted
            return
        };
        let Some(peer) = peers.get(&peer_id) else { return };

        // fill the request with other buffered hashes that have been announced by the peer.
        // look up the given number of lru hashes that are pending fetch, in the hashes seen
        // by this peer, before giving up and sending a request with the single tx popped
        // above.
        let budget_lru_hashes_pending_fetch = MAX_CAPACITY_CACHE_FOR_HASHES_PENDING_FETCH / 2;

        self.fill_request_from_hashes_pending_fetch(
            &mut hashes_to_request,
            &peer.transactions,
            budget_lru_hashes_pending_fetch,
        );

        trace!(target: "net::tx",
            peer_id=format!("{peer_id:#}"),
            hashes=?hashes_to_request,
            "requesting hashes that were stored pending fetch from peer"
        );

        // request the buffered missing transactions
        if let Some(failed_to_request_hashes) =
            self.request_transactions_from_peer(hashes_to_request, peer, || {
                metrics.egress_peer_channel_full.increment(1)
            })
        {
            debug!(target: "net::tx",
                peer_id=format!("{peer_id:#}"),
                failed_to_request_hashes=?failed_to_request_hashes,
                "failed sending request to peer's session, buffering hashes"
            );

            self.buffer_hashes(failed_to_request_hashes, Some(peer_id));
        }
    }

    /// Removes the provided transaction hashes from the inflight requests set.
    ///
    /// This is called when we receive full transactions that are currently scheduled for fetching.
    #[inline]
    pub(super) fn on_received_full_transactions_broadcast(
        &mut self,
        hashes: impl IntoIterator<Item = TxHash>,
    ) {
        self.remove_from_unknown_hashes(hashes)
    }

    pub(super) fn filter_unseen_hashes(
        &mut self,
        new_announced_hashes: &mut ValidAnnouncementData,
        peer_id: &PeerId,
        is_session_active: impl Fn(PeerId) -> bool,
    ) {
        // filter out inflight hashes, and register the peer as fallback for all inflight hashes
        new_announced_hashes.retain(|hash, metadata| {
            // occupied entry
            if let Some(TxUnknownToPool{ref mut fallback_peers, tx_encoded_length: ref mut previously_seen_size, ..}) = self.hashes_unknown_to_pool.peek_mut(hash) {
                // update size metadata if available
                if let Some((_ty, size)) = metadata {
                    if let Some(prev_size) = previously_seen_size {
                        // check if this peer is announcing a different size than a previous peer
                        if size != prev_size {
                            debug!(target: "net::tx",
                                peer_id=format!("{peer_id:#}"),
                                size=size,
                                previously_seen_size=previously_seen_size,
                                "peer announced a different size for tx, this is especially worrying if either size is very big..."
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
                let mut ended_sessions = vec!();
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
            let msg_version = || if metadata.is_some() { EthVersion::Eth68 } else { EthVersion::Eth66 };

            trace!(target: "net::tx",
                peer_id=format!("{peer_id:#}"),
                hash=%hash,
                msg_version=%msg_version(),
                "new hash seen in announcement by peer"
            );

            // todo: allow `MAX_ALTERNATIVE_PEERS_PER_TX` to be zero
            let limit = NonZeroUsize::new(MAX_ALTERNATIVE_PEERS_PER_TX.into()).expect("MAX_ALTERNATIVE_PEERS_PER_TX should be non-zero");

            if self.hashes_unknown_to_pool.get_or_insert(*hash, ||
                TxUnknownToPool{retries: 0, fallback_peers: LruCache::new(limit), tx_encoded_length: None}
            ).is_none() {

                debug!(target: "net::tx",
                    peer_id=format!("{peer_id:#}"),
                    hash=%hash,
                    msg_version=%msg_version(),
                    "failed to cache new announced hash from peer in schnellru::LruMap, dropping hash"
                );

                return false
            }
            true
        });
    }

    /// Requests the missing transactions from the announced hashes of the peer. Returns the
    /// requested hashes if concurrency limit is reached or if the request fails to send over the
    /// channel to the peer's session task.
    ///
    /// This filters all announced hashes that are already in flight, and requests the missing,
    /// while marking the given peer as an alternative peer for the hashes that are already in
    /// flight.
    pub(super) fn request_transactions_from_peer(
        &mut self,
        new_announced_hashes: Vec<TxHash>,
        peer: &Peer,
        metrics_increment_egress_peer_channel_full: impl FnOnce(),
    ) -> Option<Vec<TxHash>> {
        let peer_id: PeerId = peer.request_tx.peer_id;
        let msg_version = peer.version;

        if self.active_peers.len() >= self.tx_fetcher_info.max_inflight_transaction_requests {
            debug!(target: "net::tx",
                peer_id=format!("{peer_id:#}"),
                new_announced_hashes=?new_announced_hashes,
                msg_version=%msg_version,
                max_inflight_transaction_requests=self.tx_fetcher_info.max_inflight_transaction_requests,
                "limit for concurrent `GetPooledTransactions` requests reached, dropping request for hashes to peer"
            );
            return Some(new_announced_hashes)
        }

        let Some(inflight_count) = self.active_peers.get_or_insert(peer_id, || 0) else {
            debug!(target: "net::tx",
                peer_id=format!("{peer_id:#}"),
                new_announced_hashes=?new_announced_hashes,
                msg_version=%msg_version,
                "failed to cache active peer in schnellru::LruMap, dropping request to peer"
            );
            return Some(new_announced_hashes)
        };

        if *inflight_count >= MAX_CONCURRENT_TX_REQUESTS_PER_PEER {
            debug!(target: "net::tx",
                peer_id=format!("{peer_id:#}"),
                new_announced_hashes=?new_announced_hashes,
                msg_version=%msg_version,
                MAX_CONCURRENT_TX_REQUESTS_PER_PEER=MAX_CONCURRENT_TX_REQUESTS_PER_PEER,
                "limit for concurrent `GetPooledTransactions` requests per peer reached"
            );
            return Some(new_announced_hashes)
        }

        *inflight_count += 1;

        debug_assert!(
            || -> bool {
                for hash in &new_announced_hashes {
                    if self.hashes_pending_fetch.contains(hash) {
                        return false
                    }
                }
                true
            }(),
            "`%new_announced_hashes` should been taken out of buffer before packing in a request, breaks invariant `@buffered_hashes` and `@inflight_requests`,
`%new_announced_hashes`: {:?}, 
`@self`: {:?}",
            new_announced_hashes, self
        );

        let (response, rx) = oneshot::channel();
        let req: PeerRequest = PeerRequest::GetPooledTransactions {
            request: GetPooledTransactions(new_announced_hashes.clone()),
            response,
        };

        // try to send the request to the peer
        if let Err(err) = peer.request_tx.try_send(req) {
            // peer channel is full
            match err {
                TrySendError::Full(req) | TrySendError::Closed(req) => {
                    // need to do some cleanup so
                    let req = req.into_get_pooled_transactions().expect("is get pooled tx");

                    metrics_increment_egress_peer_channel_full();
                    return Some(req.0)
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

    /// Tries to fill request with hashes pending fetch so that the expected tx response is full
    /// enough. A mutable reference to a list of hashes to request is passed as parameter. A
    /// budget is passed as parameter. This ensures that the node stops searching for more hashes
    /// after the budget is depleted. Under bad network conditions, the cache of tx hashes pending
    /// fetch may become very full for a while. As the node recovers, the hashes pending fetch
    /// cache should get smaller. The budget aims to be big enough to loop through all buffered
    /// hashes in good network conditions.
    ///
    /// The request id filled as if it's an eth68 request, i.e. smartly assemble the request based
    /// on expected response size. For any hash missing size metadata, it is guessed at
    /// [`TX_ENCODED_LENGTH_AVERAGE`].
    ///
    /// Loops through buffered hashes and does:
    ///
    /// 1. Check if budget is depleted, if so stop looping.
    /// 1. Check if a hash pending fetch is seen by peer.
    /// 2. Optimistically include the hash in the request.
    /// 3. Accumulate expected total response size.
    /// 4. Check if acc size and hashes count is at limit, if so stop looping.
    /// 5. Remove hashes to request from cache of hashes pending fetch.
    pub(super) fn fill_request_from_hashes_pending_fetch(
        &mut self,
        hashes_to_request: &mut Vec<TxHash>,
        seen_hashes: &LruCache<TxHash>,
        mut budget: usize, // only check `budget` lru pending hashes
    ) {
        let Some(hash) = hashes_to_request.first() else { return };

        let mut acc_size_response = self
            .hashes_unknown_to_pool
            .get(hash)
            .and_then(|entry| entry.tx_encoded_len())
            .unwrap_or(TX_ENCODED_LENGTH_AVERAGE);

        // if request full enough already, we're satisfied, send request for single tx
        if acc_size_response >= SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE_ON_FETCH_PENDING {
            return
        }

        // try to fill request by checking if any other hashes pending fetch (in lru order) are
        // also seen by peer
        for hash in self.hashes_pending_fetch.iter() {
            budget = budget.saturating_sub(1);
            if budget == 0 {
                break
            }

            // 1. Check if a hash seen by peer is pending fetch.
            if !seen_hashes.contains(hash) {
                continue;
            };

            // 2. Optimistically include the hash in the request.
            hashes_to_request.push(*hash);

            // 3. Accumulate expected total response size.
            let size = self
                .hashes_unknown_to_pool
                .get(hash)
                .and_then(|entry| entry.tx_encoded_len())
                .unwrap_or(TX_ENCODED_LENGTH_AVERAGE);

            acc_size_response += size;

            // 4. Check if acc size or hashes count is at limit, if so stop looping.
            // if request full enough or the number of hashes in the request is enough, we're
            // satisfied
            if acc_size_response >=
                SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE_ON_FETCH_PENDING ||
                hashes_to_request.len() >
                    GET_POOLED_TRANSACTION_SOFT_LIMIT_NUM_HASHES_ON_FETCH_PENDING
            {
                break
            }
        }

        // 5. Remove hashes to request from cache of hashes pending fetch.
        for hash in hashes_to_request {
            self.hashes_pending_fetch.remove(hash);
        }
    }

    pub(super) fn has_capacity_for_fetching_pending_hashes(&mut self) -> bool {
        let info = &mut self.tx_fetcher_info;

        self.inflight_requests.len() < info.max_inflight_transaction_requests ||
            self.hashes_pending_fetch.len() > info.max_hashes_pending_fetch
    }
}

impl Stream for TransactionFetcher {
    type Item = FetchEvent;

    /// Advances all inflight requests and returns the next event.
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.as_mut().project();
        let res = this.inflight_requests.poll_next_unpin(cx);

        if let Poll::Ready(Some(response)) = res {
            // update peer activity, requests for buffered hashes can only be made to idle
            // fallback peers
            let GetPooledTxResponse { peer_id, mut requested_hashes, result } = response;

            debug_assert!(
                self.active_peers.get(&peer_id).is_some(),
                "`%peer_id` has been removed from `@active_peers` before inflight request(s) resolved, broken invariant `@active_peers` and `@inflight_requests`,
`%peer_id`: {},
`@self`: {:?}",
                peer_id, self
            );

            self.decrement_inflight_request_count_for(&peer_id);

            return match result {
                Ok(Ok(transactions)) => {
                    // clear received hashes
                    let mut fetched = Vec::with_capacity(transactions.hashes().count());
                    requested_hashes.retain(|requested_hash| {
                        if transactions.hashes().any(|hash| hash == requested_hash) {
                            // hash is now known, stop tracking
                            fetched.push(*requested_hash);
                            return false
                        }
                        true
                    });
                    self.remove_from_unknown_hashes(fetched);
                    // buffer left over hashes
                    self.buffer_hashes_for_retry(requested_hashes, &peer_id);

                    Poll::Ready(Some(FetchEvent::TransactionsFetched {
                        peer_id,
                        transactions: transactions.0,
                    }))
                }
                Ok(Err(req_err)) => {
                    self.buffer_hashes_for_retry(requested_hashes, &peer_id);
                    Poll::Ready(Some(FetchEvent::FetchError { peer_id, error: req_err }))
                }
                Err(_) => {
                    self.buffer_hashes_for_retry(requested_hashes, &peer_id);
                    // request channel closed/dropped
                    Poll::Ready(Some(FetchEvent::FetchError {
                        peer_id,
                        error: RequestError::ChannelClosed,
                    }))
                }
            }
        }

        Poll::Pending
    }
}

impl Default for TransactionFetcher {
    fn default() -> Self {
        Self {
            active_peers: LruMap::new(DEFAULT_MAX_CONCURRENT_TX_REQUESTS),
            inflight_requests: Default::default(),
            hashes_pending_fetch: LruCache::new(
                NonZeroUsize::new(MAX_CAPACITY_CACHE_FOR_HASHES_PENDING_FETCH)
                    .expect("buffered cache limit should be non-zero"),
            ),
            hashes_unknown_to_pool: LruMap::new_unlimited(),
            filter_valid_hashes: Default::default(),
            tx_fetcher_info: TransactionFetcherInfo::new(
                DEFAULT_MAX_CONCURRENT_TX_REQUESTS as usize,
                DEFAULT_MAX_HASHES_PENDING_FETCH,
            ),
        }
    }
}

/// Represents possible events from fetching transactions.
#[derive(Debug)]
pub(super) enum FetchEvent {
    /// Triggered when transactions are successfully fetched.
    TransactionsFetched {
        /// The ID of the peer from which transactions were fetched.
        peer_id: PeerId,
        /// The transactions that were fetched, if available.
        transactions: Vec<PooledTransactionsElement>,
    },
    /// Triggered when there is an error in fetching transactions.
    FetchError {
        /// The ID of the peer from which an attempt to fetch transactions resulted in an error.
        peer_id: PeerId,
        /// The specific error that occurred while fetching.
        error: RequestError,
    },
}

/// An inflight request for `PooledTransactions` from a peer
pub(super) struct GetPooledTxRequest {
    peer_id: PeerId,
    /// Transaction hashes that were requested, for cleanup purposes
    requested_hashes: Vec<TxHash>,
    response: oneshot::Receiver<RequestResult<PooledTransactions>>,
}

pub(super) struct GetPooledTxResponse {
    peer_id: PeerId,
    /// Transaction hashes that were requested, for cleanup purposes
    requested_hashes: Vec<TxHash>,
    result: Result<RequestResult<PooledTransactions>, RecvError>,
}

#[must_use = "futures do nothing unless polled"]
#[pin_project::pin_project]
pub(super) struct GetPooledTxRequestFut {
    #[pin]
    inner: Option<GetPooledTxRequest>,
}

impl GetPooledTxRequestFut {
    #[inline]
    fn new(
        peer_id: PeerId,
        requested_hashes: Vec<TxHash>,
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

/// Tracks stats about the [`TransactionsManager`].
#[derive(Debug)]
pub struct TransactionFetcherInfo {
    /// Currently active outgoing [`GetPooledTransactions`] requests.
    max_inflight_transaction_requests: usize,
    /// Number of pending hashes.
    max_hashes_pending_fetch: usize,
}

impl TransactionFetcherInfo {
    pub fn new(max_inflight_transaction_requests: usize, max_hashes_pending_fetch: usize) -> Self {
        Self { max_inflight_transaction_requests, max_hashes_pending_fetch }
    }
}

#[cfg(test)]
mod test {
    use std::collections::{HashMap, HashSet};

    use reth_primitives::B256;

    use super::*;

    #[test]
    fn pack_eth68_request_surplus_hashes() {
        reth_tracing::init_test_tracing();

        let tx_fetcher = &mut TransactionFetcher::default();

        let eth68_hashes = [
            B256::from_slice(&[1; 32]),
            B256::from_slice(&[2; 32]),
            B256::from_slice(&[3; 32]),
            B256::from_slice(&[4; 32]),
            B256::from_slice(&[5; 32]),
            B256::from_slice(&[6; 32]),
        ];
        let eth68_hashes_sizes = [
            POOLED_TRANSACTIONS_RESPONSE_SOFT_LIMIT_BYTE_SIZE - 6,
            POOLED_TRANSACTIONS_RESPONSE_SOFT_LIMIT_BYTE_SIZE, // this one will not fit
            2,
            9, // this one won't
            2,
            2,
        ];

        let mut eth68_hashes_to_request = Vec::new();
        let mut valid_announcement_data = HashMap::new();
        for i in 0..eth68_hashes.len() {
            valid_announcement_data.insert(eth68_hashes[i], Some((0, eth68_hashes_sizes[i])));
        }
        let surplus_eth68_hashes =
            tx_fetcher.pack_hashes_eth68(&mut eth68_hashes_to_request, valid_announcement_data);

        assert_eq!(
            surplus_eth68_hashes.into_iter().collect::<HashSet<_>>(),
            vec!(eth68_hashes[1], eth68_hashes[3]).into_iter().collect::<HashSet<_>>()
        );
        assert_eq!(
            eth68_hashes_to_request.into_iter().collect::<HashSet<_>>(),
            vec!(eth68_hashes[0], eth68_hashes[2], eth68_hashes[4], eth68_hashes[5])
                .into_iter()
                .collect::<HashSet<_>>()
        );
    }
}
