use crate::{
    cache::{LruCache, LruMap},
    message::PeerRequest,
};
use futures::{stream::FuturesUnordered, Future, FutureExt, Stream, StreamExt};
use pin_project::pin_project;
use reth_eth_wire::{EthVersion, GetPooledTransactions};
use reth_interfaces::p2p::error::{RequestError, RequestResult};
use reth_primitives::{PeerId, PooledTransactionsElement, TxHash};
use schnellru::{ByLength, Unlimited};
use std::{
    num::NonZeroUsize,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::sync::{mpsc::error::TrySendError, oneshot, oneshot::error::RecvError};
use tracing::{debug, trace};

use super::{Peer, PooledTransactions, MAX_FULL_TRANSACTIONS_PACKET_SIZE};

/// Maximum concurrent [`GetPooledTxRequest`]s to allow per peer.
pub(super) const MAX_CONCURRENT_TX_REQUESTS_PER_PEER: u8 = 1;

/// How many peers we keep track of for each missing transaction.
pub(super) const MAX_ALTERNATIVE_PEERS_PER_TX: u8 =
    MAX_REQUEST_RETRIES_PER_TX_HASH + MARGINAL_FALLBACK_PEERS_PER_TX;

/// Marginal on fallback peers. If all fallback peers are idle, at most
/// [`MAX_REQUEST_RETRIES_PER_TX_HASH`] of them can ever be needed.
const MARGINAL_FALLBACK_PEERS_PER_TX: u8 = 1;

/// Maximum request retires per [`TxHash`]. Note, this is reset should the [`TxHash`] re-appear in
/// an announcement after it has been ejected from the hash buffer.
const MAX_REQUEST_RETRIES_PER_TX_HASH: u8 = 2;

/// Maximum concurrent [`GetPooledTxRequest`]s.
const MAX_CONCURRENT_TX_REQUESTS: u32 = 10000;

/// Cache limit of transactions waiting for idle peer to be fetched.
const MAX_CAPACITY_BUFFERED_HASHES: usize = 100 * GET_POOLED_TRANSACTION_SOFT_LIMIT_NUM_HASHES;

/// Recommended soft limit for the number of hashes in a GetPooledTransactions message (8kb)
///
/// <https://github.com/ethereum/devp2p/blob/master/caps/eth.md#newpooledtransactionhashes-0x08>
const GET_POOLED_TRANSACTION_SOFT_LIMIT_NUM_HASHES: usize = 256;

/// The type responsible for fetching missing transactions from peers.
///
/// This will keep track of unique transaction hashes that are currently being fetched and submits
/// new requests on announced hashes.
#[derive(Debug)]
#[pin_project]
pub(super) struct TransactionFetcher {
    /// All peers to which a request for pooled transactions is currently active. Maps 1-1 to
    /// `inflight_requests`.
    pub(super) active_peers: LruMap<PeerId, u8, ByLength>,
    /// All currently active requests for pooled transactions.
    #[pin]
    pub(super) inflight_requests: FuturesUnordered<GetPooledTxRequestFut>,
    /// Hashes that are awaiting an idle peer so they can be fetched.
    // todo: store buffered eth68 and eth66 hashes separately
    pub(super) buffered_hashes: LruCache<TxHash>,
    /// Tracks all hashes that are currently being fetched or are buffered, mapping them to
    /// request retries and last recently seen fallback peers (max one request try for any peer).
    pub(super) unknown_hashes: LruMap<TxHash, (u8, LruCache<PeerId>), Unlimited>,
    /// Size metadata for unknown eth68 hashes.
    pub(super) eth68_meta: LruMap<TxHash, usize, Unlimited>,
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
            self.unknown_hashes.remove(&hash);
            self.eth68_meta.remove(&hash);
        }
    }

    /// Updates peer's activity status upon a resolved [`GetPooledTxRequest`].
    fn decrement_inflight_request_count_for(&mut self, peer_id: PeerId) {
        let remove = || -> bool {
            if let Some(inflight_count) = self.active_peers.get(&peer_id) {
                if *inflight_count <= MAX_CONCURRENT_TX_REQUESTS_PER_PEER {
                    return true
                }
                *inflight_count -= 1;
            }
            false
        }();

        if remove {
            self.active_peers.remove(&peer_id);
        }
    }

    /// Returns `true` if peer is idle.
    pub(super) fn is_idle(&self, peer_id: PeerId) -> bool {
        let Some(inflight_count) = self.active_peers.peek(&peer_id) else { return true };
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
        ended_sessions_buf: &mut Vec<PeerId>,
        is_session_active: impl Fn(PeerId) -> bool,
    ) -> Option<PeerId> {
        let (_, peers) = self.unknown_hashes.peek(&hash)?;

        for &peer_id in peers.iter() {
            if self.is_idle(peer_id) {
                if is_session_active(peer_id) {
                    return Some(peer_id)
                } else {
                    ended_sessions_buf.push(peer_id);
                }
            }
        }

        None
    }

    /// Packages hashes for [`GetPooledTxRequest`] up to limit. Returns left over hashes.
    pub(super) fn pack_hashes(&mut self, hashes: &mut Vec<TxHash>, peer_id: PeerId) -> Vec<TxHash> {
        let Some(hash) = hashes.first() else { return vec![] };

        if self.eth68_meta.get(hash).is_some() {
            return self.pack_hashes_eth68(hashes, peer_id)
        }
        self.pack_hashes_eth66(hashes, peer_id)
    }

    /// Packages hashes for [`GetPooledTxRequest`] up to limit as defined by protocol version 66.
    /// If necessary, takes hashes from buffer for which peer is listed as fallback peer.
    ///
    /// Returns left over hashes.
    pub(super) fn pack_hashes_eth66(
        &mut self,
        hashes: &mut Vec<TxHash>,
        peer_id: PeerId,
    ) -> Vec<TxHash> {
        if hashes.len() < GET_POOLED_TRANSACTION_SOFT_LIMIT_NUM_HASHES {
            self.fill_eth66_request_for_peer(hashes, peer_id);
            return vec![]
        }
        hashes.split_off(GET_POOLED_TRANSACTION_SOFT_LIMIT_NUM_HASHES)
    }

    /// Evaluates wether or not to include a hash in a `GetPooledTransactions` version eth68
    /// request, based on the size of the transaction and the accumulated size of the
    /// corresponding `PooledTransactions` response.
    ///
    /// Returns `true` if hash is included in request. If there is still space in the respective
    /// response but not enough for the transaction of given hash, `false` is returned.
    fn include_eth68_hash(&self, acc_size_response: &mut usize, hash: TxHash) -> bool {
        debug_assert!(
            self.eth68_meta.peek(&hash).is_some(),
            "can't find eth68 metadata for `%hash` that should be of version eth68, broken invariant `@eth68_meta` and `@self`,
`%hash`: {},
`@self`: {:?}",
            hash, self
        );

        if let Some(size) = self.eth68_meta.peek(&hash) {
            let next_acc_size = *acc_size_response + size;

            if next_acc_size <= MAX_FULL_TRANSACTIONS_PACKET_SIZE {
                // only update accumulated size of tx response if tx will fit in
                *acc_size_response = next_acc_size;
                return true
            }
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
        hashes: &mut Vec<TxHash>,
        peer_id: PeerId,
    ) -> Vec<TxHash> {
        let mut acc_size_response = 0;
        let mut surplus_hashes = vec![];

        hashes.retain(|&hash| match self.include_eth68_hash(&mut acc_size_response, hash) {
            true => true,
            false => {
                trace!(target: "net::tx",
                    peer_id=format!("{peer_id:#}"),
                    hash=%hash,
                    size=self.eth68_meta.peek(&hash).expect("should find size in `eth68-meta`"),
                    acc_size_response=acc_size_response,
                    MAX_FULL_TRANSACTIONS_PACKET_SIZE=MAX_FULL_TRANSACTIONS_PACKET_SIZE,
                    "no space for hash in `GetPooledTransactions` request to peer"
                );

                surplus_hashes.push(hash);
                false
            }
        });

        // all hashes included in request and there is still space
        // todo: compare free space with min tx size
        if acc_size_response < MAX_FULL_TRANSACTIONS_PACKET_SIZE {
            self.fill_eth68_request_for_peer(hashes, peer_id, &mut acc_size_response);
        }

        surplus_hashes
    }

    pub(super) fn buffer_hashes_for_retry(&mut self, hashes: impl IntoIterator<Item = TxHash>) {
        self.buffer_hashes(hashes, None)
    }

    /// Buffers hashes. Note: Only peers that haven't yet tried to request the hashes should be
    /// passed as `fallback_peer` parameter! Hashes that have been re-requested
    /// [`MAX_REQUEST_RETRIES_PER_TX_HASH`], are dropped.
    pub(super) fn buffer_hashes(
        &mut self,
        hashes: impl IntoIterator<Item = TxHash>,
        fallback_peer: Option<PeerId>,
    ) {
        let mut max_retried_hashes = vec![];

        for hash in hashes {
            // todo: enforce by adding new types UnknownTxHash66 and UnknownTxHash68
            debug_assert!(
                self.unknown_hashes.peek(&hash).is_some(),
                "`%hash` in `@buffered_hashes` that's not in `@unknown_hashes`, `@buffered_hashes` should be a subset of keys in `@unknown_hashes`, broken invariant `@buffered_hashes` and `@unknown_hashes`,
`%hash`: {},
`@self`: {:?}",
                hash, self
            );

            let Some((retries, peers)) = self.unknown_hashes.get(&hash) else { return };

            if let Some(peer_id) = fallback_peer {
                // peer has not yet requested hash
                peers.insert(peer_id);
            } else {
                // peer in caller's context has requested hash and is hence not eligible as
                // fallback peer.
                if *retries >= MAX_REQUEST_RETRIES_PER_TX_HASH {
                    let msg_version = || {
                        self.eth68_meta
                            .peek(&hash)
                            .map(|_| EthVersion::Eth68)
                            .unwrap_or(EthVersion::Eth66)
                    };

                    debug!(target: "net::tx",
                        hash=%hash,
                        retries=retries,
                        msg_version=%msg_version(),
                        "retry limit for `GetPooledTransactions` requests reached for hash, dropping hash"
                    );

                    max_retried_hashes.push(hash);
                    continue;
                }
                *retries += 1;
            }
            if let (_, Some(evicted_hash)) = self.buffered_hashes.insert_and_get_evicted(hash) {
                _ = self.unknown_hashes.remove(&evicted_hash);
                _ = self.eth68_meta.remove(&evicted_hash);
            }
        }

        self.remove_from_unknown_hashes(max_retried_hashes);
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
        new_announced_hashes: &mut Vec<TxHash>,
        peer_id: PeerId,
        is_session_active: impl Fn(PeerId) -> bool,
    ) {
        // filter out inflight hashes, and register the peer as fallback for all inflight hashes
        new_announced_hashes.retain(|hash| {
            // occupied entry
            if let Some((_retries, ref mut backups)) = self.unknown_hashes.peek_mut(hash) {
                // hash has been seen but is not inflight
                if self.buffered_hashes.remove(hash) {
                    return true
                }
                // hash has been seen and is in flight. store peer as fallback peer.
                //
                // remove any ended sessions, so that in case of a full cache, alive peers aren't 
                // removed in favour of lru dead peers
                let mut ended_sessions = vec!();
                for &peer_id in backups.iter() {
                    if is_session_active(peer_id) {
                        ended_sessions.push(peer_id);
                    }
                }
                for peer_id in ended_sessions {
                    backups.remove(&peer_id);
                }
                backups.insert(peer_id);
                return false
            }

            let msg_version = || self.eth68_meta.peek(hash).map(|_| EthVersion::Eth68).unwrap_or(EthVersion::Eth66);

            // vacant entry
            trace!(target: "net::tx",
                peer_id=format!("{peer_id:#}"),
                hash=%hash,
                msg_version=%msg_version(),
                "new hash seen in announcement by peer"
            );

            // todo: allow `MAX_ALTERNATIVE_PEERS_PER_TX` to be zero
            let limit = NonZeroUsize::new(MAX_ALTERNATIVE_PEERS_PER_TX.into()).expect("MAX_ALTERNATIVE_PEERS_PER_TX should be non-zero");

            if self.unknown_hashes.get_or_insert(*hash, ||
                (0, LruCache::new(limit))
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
        let msg_version = || {
            new_announced_hashes
                .first()
                .map(|hash| {
                    self.eth68_meta
                        .peek(hash)
                        .map(|_| EthVersion::Eth68)
                        .unwrap_or(EthVersion::Eth66)
                })
                .expect("`new_announced_hashes` shouldn't be empty")
        };

        if self.active_peers.len() as u32 >= MAX_CONCURRENT_TX_REQUESTS {
            debug!(target: "net::tx",
                peer_id=format!("{peer_id:#}"),
                new_announced_hashes=?new_announced_hashes,
                msg_version=%msg_version(),
                limit=MAX_CONCURRENT_TX_REQUESTS,
                "limit for concurrent `GetPooledTransactions` requests reached, dropping request for hashes to peer"
            );
            return Some(new_announced_hashes)
        }

        let Some(inflight_count) = self.active_peers.get_or_insert(peer_id, || 0) else {
            debug!(target: "net::tx",
                peer_id=format!("{peer_id:#}"),
                new_announced_hashes=?new_announced_hashes,
                msg_version=%msg_version(),
                "failed to cache active peer in schnellru::LruMap, dropping request to peer"
            );
            return Some(new_announced_hashes)
        };

        if *inflight_count >= MAX_CONCURRENT_TX_REQUESTS_PER_PEER {
            debug!(target: "net::tx",
                peer_id=format!("{peer_id:#}"),
                new_announced_hashes=?new_announced_hashes,
                msg_version=%msg_version(),
                limit=MAX_CONCURRENT_TX_REQUESTS_PER_PEER,
                "limit for concurrent `GetPooledTransactions` requests per peer reached"
            );
            return Some(new_announced_hashes)
        }

        *inflight_count += 1;

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

                    // we know that the peer is the only entry in the map, so we can remove all
                    self.remove_from_unknown_hashes(req.0);
                }
            }
            metrics_increment_egress_peer_channel_full();
            return Some(new_announced_hashes)
        } else {
            debug_assert!(
                || -> bool {
                    for hash in &new_announced_hashes {
                        if self.buffered_hashes.contains(hash) {
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

            // stores a new request future for the request
            self.inflight_requests.push(GetPooledTxRequestFut::new(
                peer_id,
                new_announced_hashes,
                rx,
            ))
        }

        None
    }

    /// Tries to fill request with eth68 hashes so that the respective tx response is at its size
    /// limit. It does so by taking buffered eth68 hashes for which peer is listed as fallback
    /// peer. A mutable reference to a list of hashes to request is passed as parameter.
    ///
    /// Loops through buffered hashes and does:
    ///
    /// 1. Check acc size against limit, if so stop looping.
    /// 2. Check if this buffered hash is an eth68 hash, else skip to next iteration.
    /// 3. Check if hash can be included with respect to size metadata and acc size copy.
    /// 4. Check if peer is fallback peer for hash and remove, else skip to next iteration.
    /// 4. Add hash to hashes list parameter.
    /// 5. Overwrite eth68 acc size with copy.
    pub(super) fn fill_eth68_request_for_peer(
        &mut self,
        hashes: &mut Vec<TxHash>,
        peer_id: PeerId,
        acc_size_response: &mut usize,
    ) {
        debug_assert!(
            {
                let mut acc_size = 0;
                for &hash in hashes.iter() {
                    _ = self.include_eth68_hash(&mut acc_size, hash);
                }
                acc_size == *acc_size_response
            },
            "an eth68 request is being assembled and caller has miscalculated accumulated size of corresponding transactions response, broken invariant `%acc_size_response` and `%hashes`,
`%acc_size_response`: {:?},
`%hashes`: {:?},
`@self`: {:?}",
            acc_size_response, hashes, self
        );

        for hash in self.buffered_hashes.iter() {
            // copy acc size
            let mut next_acc_size = *acc_size_response;

            // 1. Check acc size against limit, if so stop looping.
            if next_acc_size >= MAX_FULL_TRANSACTIONS_PACKET_SIZE {
                trace!(target: "net::tx",
                    peer_id=format!("{peer_id:#}"),
                    acc_size_eth68_response=acc_size_response, // no change acc size
                    MAX_FULL_TRANSACTIONS_PACKET_SIZE=MAX_FULL_TRANSACTIONS_PACKET_SIZE,
                    "request to peer full"
                );

                break
            }
            // 2. Check if this buffered hash is an eth68 hash, else skip to next iteration.
            if self.eth68_meta.get(hash).is_none() {
                continue
            }
            // 3. Check if hash can be included with respect to size metadata and acc size copy.
            //
            // mutates acc size copy
            if !self.include_eth68_hash(&mut next_acc_size, *hash) {
                continue
            }

            debug_assert!(
                self.unknown_hashes.get(hash).is_some(),
                "can't find buffered `%hash` in `@unknown_hashes`, `@buffered_hashes` should be a subset of keys in `@unknown_hashes`, broken invariant `@buffered_hashes` and `@unknown_hashes`,
`%hash`: {},
`@self`: {:?}",
                hash, self
            );

            if let Some((_, fallback_peers)) = self.unknown_hashes.get(hash) {
                // 4. Check if peer is fallback peer for hash and remove, else skip to next
                // iteration.
                //
                // upgrade this peer from fallback peer, soon to be active peer with inflight
                // request. since 1 retry per peer per tx hash on this tx fetcher layer, remove
                // peer.
                if fallback_peers.remove(&peer_id) {
                    // 4. Add hash to hashes list parameter.
                    hashes.push(*hash);
                    // 5. Overwrite eth68 acc size with copy.
                    *acc_size_response = next_acc_size;

                    trace!(target: "net::tx",
                        peer_id=format!("{peer_id:#}"),
                        hash=%hash,
                        acc_size_eth68_response=acc_size_response,
                        MAX_FULL_TRANSACTIONS_PACKET_SIZE=MAX_FULL_TRANSACTIONS_PACKET_SIZE,
                        "found buffered hash for request to peer"
                    );
                }
            }
        }

        // remove hashes that will be included in request from buffer
        for hash in hashes {
            self.buffered_hashes.remove(hash);
        }
    }

    /// Tries to fill request with eth66 hashes so that the respective tx response is at its size
    /// limit. It does so by taking buffered hashes for which peer is listed as fallback peer. A
    /// mutable reference to a list of hashes to request is passed as parameter.
    ///
    /// Loops through buffered hashes and does:
    ///
    /// 1. Check if this buffered hash is an eth66 hash, else skip to next iteration.
    /// 2. Check hashes count in request, if max reached stop looping.
    /// 3. Check if peer is fallback peer for hash and remove, else skip to next iteration.
    /// 4. Add hash to hashes list parameter. This increases length i.e. hashes count.
    ///
    /// Removes hashes included in request from buffer.
    pub(super) fn fill_eth66_request_for_peer(
        &mut self,
        hashes: &mut Vec<TxHash>,
        peer_id: PeerId,
    ) {
        for hash in self.buffered_hashes.iter() {
            // 1. Check hashes count in request.
            if hashes.len() >= GET_POOLED_TRANSACTION_SOFT_LIMIT_NUM_HASHES {
                break
            }
            // 2. Check if this buffered hash is an eth66 hash.
            if self.eth68_meta.get(hash).is_some() {
                continue
            }

            debug_assert!(
                self.unknown_hashes.get(hash).is_some(),
                "can't find buffered `%hash` in `@unknown_hashes`, `@buffered_hashes` should be a subset of keys in `@unknown_hashes`, broken invariant `@buffered_hashes` and `@unknown_hashes`,
`%hash`: {},
`@self`: {:?}",
                hash, self
            );

            if let Some((_, fallback_peers)) = self.unknown_hashes.get(hash) {
                // 3. Check if peer is fallback peer for hash and remove.
                //
                // upgrade this peer from fallback peer, soon to be active peer with inflight
                // request. since 1 retry per peer per tx hash on this tx fetcher layer, remove
                // peer.
                if fallback_peers.remove(&peer_id) {
                    // 4. Add hash to hashes list parameter.
                    hashes.push(*hash);

                    trace!(target: "net::tx",
                        peer_id=format!("{peer_id:#}"),
                        hash=%hash,
                        MAX_FULL_TRANSACTIONS_PACKET_SIZE=MAX_FULL_TRANSACTIONS_PACKET_SIZE,
                        "found buffered hash for request to peer"
                    );
                }
            }
        }

        // remove hashes that will be included in request from buffer
        for hash in hashes {
            self.buffered_hashes.remove(hash);
        }
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
            let GetPooledTxResponse { peer_id, .. } = response;

            debug_assert!(
                self.active_peers.get(&peer_id).is_some(),
                "`%peer_id` has been removed from `@active_peers` before inflight request(s) resolved, broken invariant `@active_peers` and `@inflight_requests`,
`%peer_id`: {},
`@self`: {:?}",
                peer_id, self
            );

            self.decrement_inflight_request_count_for(peer_id);

            let GetPooledTxResponse { peer_id, mut requested_hashes, result } = response;

            return match result {
                Ok(Ok(transactions)) => {
                    // clear received hashes
                    requested_hashes.retain(|requested_hash| {
                        if transactions.hashes().any(|hash| hash == requested_hash) {
                            // hash is now known, stop tracking
                            self.unknown_hashes.remove(requested_hash);
                            self.eth68_meta.remove(requested_hash);
                            return false
                        }
                        true
                    });
                    // buffer left over hashes
                    self.buffer_hashes_for_retry(requested_hashes);

                    Poll::Ready(Some(FetchEvent::TransactionsFetched {
                        peer_id,
                        transactions: transactions.0,
                    }))
                }
                Ok(Err(req_err)) => {
                    self.buffer_hashes_for_retry(requested_hashes);
                    Poll::Ready(Some(FetchEvent::FetchError { peer_id, error: req_err }))
                }
                Err(_) => {
                    self.buffer_hashes_for_retry(requested_hashes);
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
            active_peers: LruMap::new(MAX_CONCURRENT_TX_REQUESTS),
            inflight_requests: Default::default(),
            buffered_hashes: LruCache::new(
                NonZeroUsize::new(MAX_CAPACITY_BUFFERED_HASHES)
                    .expect("buffered cache limit should be non-zero"),
            ),
            unknown_hashes: LruMap::new_unlimited(),
            eth68_meta: LruMap::new_unlimited(),
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

#[cfg(test)]
mod test {
    use reth_primitives::B256;

    use crate::transactions::tests::default_cache;

    use super::*;

    #[test]
    fn pack_eth68_request_surplus_hashes() {
        reth_tracing::init_test_tracing();

        let tx_fetcher = &mut TransactionFetcher::default();

        let peer_id = PeerId::new([1; 64]);

        let eth68_hashes = [
            B256::from_slice(&[1; 32]),
            B256::from_slice(&[2; 32]),
            B256::from_slice(&[3; 32]),
            B256::from_slice(&[4; 32]),
            B256::from_slice(&[5; 32]),
            B256::from_slice(&[6; 32]),
        ];
        let eth68_hashes_sizes = [
            MAX_FULL_TRANSACTIONS_PACKET_SIZE - 4,
            MAX_FULL_TRANSACTIONS_PACKET_SIZE, // this one will not fit
            2,                                 // this one will fit
            3,                                 // but now this one won't
            2,                                 /* this one will, no more txns will fit
                                                * after this */
            1,
        ];

        // load unseen hashes in reverse order so index 0 in seen_eth68_hashes and
        // seen_eth68_hashes_sizes is lru!

        for i in (0..6).rev() {
            tx_fetcher.unknown_hashes.insert(eth68_hashes[i], (0, default_cache()));
            tx_fetcher.eth68_meta.insert(eth68_hashes[i], eth68_hashes_sizes[i]);
        }

        let mut eth68_hashes_to_request = eth68_hashes.clone().to_vec();
        let surplus_eth68_hashes =
            tx_fetcher.pack_hashes_eth68(&mut eth68_hashes_to_request, peer_id);

        assert_eq!(surplus_eth68_hashes, vec!(eth68_hashes[1], eth68_hashes[3], eth68_hashes[5]));
        assert_eq!(
            eth68_hashes_to_request,
            vec!(eth68_hashes[0], eth68_hashes[2], eth68_hashes[4])
        );
    }
}
