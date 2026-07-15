//! Request scheduling for the second-generation transaction fetcher.

use super::announcement::{CandidatePeers, CandidateUpdate, PeerAnnouncementIndex};
use crate::transactions::{
    config::TransactionFetcherConfig,
    constants::{
        tx_fetcher::DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS_PER_PEER,
        SOFT_LIMIT_COUNT_HASHES_IN_GET_POOLED_TRANSACTIONS_REQUEST,
    },
};
use alloy_primitives::{
    map::{B256Map, HashMap},
    TxHash,
};
use reth_network_peers::PeerId;
use smallvec::SmallVec;

/// Selects transaction request batches and tracks their capacity reservations.
///
/// Dispatch is supplied by the caller, keeping peer channels and response futures outside this
/// state machine. A failed dispatch restores the selected hashes at the front of their peer FIFO;
/// only a successful dispatch moves hashes into the assigned stage.
#[derive(Debug)]
pub(super) struct RequestScheduler {
    /// Hashes ready for assignment and the peers that announced them.
    announced: B256Map<CandidatePeers>,
    /// Hash to the request that currently reserves it.
    fetching: B256Map<RequestId>,
    /// Candidate peers retained while a hash is assigned.
    alternates: B256Map<CandidatePeers>,
    /// Requests successfully handed to the dispatch boundary.
    requests: HashMap<RequestId, ScheduledRequest>,
    /// Active request IDs grouped by peer.
    requests_by_peer: HashMap<PeerId, PeerRequestIds>,
    /// Per-peer FIFO metadata shared by announced and assigned hashes.
    announcements: PeerAnnouncementIndex,
    /// Monotonic request identifier.
    request_seq: u64,
    /// Fetcher limits used by admission and request packing.
    config: TransactionFetcherConfig,
}

impl RequestScheduler {
    /// Creates a scheduler using the transaction fetcher configuration.
    pub(super) fn new(config: TransactionFetcherConfig) -> Self {
        Self {
            announced: B256Map::default(),
            fetching: B256Map::default(),
            alternates: B256Map::default(),
            requests: HashMap::default(),
            requests_by_peer: HashMap::default(),
            announcements: PeerAnnouncementIndex::default(),
            request_seq: 0,
            config,
        }
    }

    /// Adds announcements to the scheduler and returns the number of new hashes rejected at the
    /// global tracking limit.
    pub(super) fn on_announcement(
        &mut self,
        peer_id: PeerId,
        announcement: impl IntoIterator<Item = (TxHash, Option<usize>)>,
    ) -> usize {
        let mut dropped = 0;
        for (hash, size) in announcement {
            if let Some(request_id) = self.fetching.get(&hash).copied() {
                let serving_peer = self.requests.get(&request_id).map(|request| request.peer_id);
                let update = self.alternates.entry(hash).or_default().track(peer_id, serving_peer);
                if matches!(update, CandidateUpdate::Rejected) {
                    continue
                }
                if let CandidateUpdate::Replaced(replaced) = update {
                    self.announcements.remove(replaced, &hash);
                }
                self.announcements.insert(peer_id, hash, size);
                continue
            }

            if let Some(candidates) = self.announced.get_mut(&hash) {
                let update = candidates.track(peer_id, None);
                if matches!(update, CandidateUpdate::Rejected) {
                    continue
                }
                if let CandidateUpdate::Replaced(replaced) = update {
                    self.announcements.remove(replaced, &hash);
                }
                self.announcements.insert(peer_id, hash, size);
                continue
            }

            if self.tracked_hashes() >= self.config.max_capacity_cache_txns_pending_fetch as usize {
                dropped += 1;
                continue
            }

            let mut candidates = CandidatePeers::default();
            let update = candidates.track(peer_id, None);
            debug_assert!(matches!(update, CandidateUpdate::Tracked));
            self.announced.insert(hash, candidates);
            self.announcements.insert(peer_id, hash, size);
        }
        dropped
    }

    /// Selects and dispatches one request for `peer_id`.
    ///
    /// `fetch_capacity` is the caller's total capacity for fetched transactions, including hashes
    /// already reserved by this scheduler. The dispatch closure may return an artifact such as a
    /// response receiver; it is returned alongside the assigned request ID without being stored or
    /// interpreted here. The caller owns fairness and iteration order across [`Self::ready_peers`];
    /// this method deliberately attempts only the selected peer. A dispatch error must mean that
    /// no request was accepted, because its hashes are immediately restored for another attempt.
    pub(super) fn try_schedule_for_peer<R, E>(
        &mut self,
        peer_id: PeerId,
        fetch_capacity: usize,
        dispatch: impl FnOnce(&[TxHash]) -> Result<R, E>,
    ) -> Result<Option<(RequestId, R)>, E> {
        let remaining_hashes = fetch_capacity.saturating_sub(self.reserved_hashes());
        if remaining_hashes == 0 ||
            self.requests.len() >= self.config.max_inflight_requests as usize ||
            self.requests_by_peer.get(&peer_id).map_or(0, SmallVec::len) >=
                self.config.max_inflight_requests_per_peer as usize
        {
            return Ok(None)
        }

        let max_hashes =
            remaining_hashes.min(SOFT_LIMIT_COUNT_HASHES_IN_GET_POOLED_TRANSACTIONS_REQUEST);
        let size_limit =
            self.config.soft_limit_byte_size_pooled_transactions_response_on_pack_request;
        let announced = &self.announced;
        let hashes = self.announcements.pack(peer_id, max_hashes, size_limit, |hash| {
            announced.get(hash).is_some_and(|candidates| candidates.as_slice().contains(&peer_id))
        });
        if hashes.is_empty() {
            return Ok(None)
        }

        let output = match dispatch(&hashes) {
            Ok(output) => output,
            Err(error) => {
                self.announcements.requeue_front(peer_id, &hashes);
                return Err(error)
            }
        };

        let request_id = self.next_request_id();
        for hash in &hashes {
            let candidates = self
                .announced
                .remove(hash)
                .expect("packed hashes remain announced until dispatch succeeds");
            self.alternates.insert(*hash, candidates);
            self.fetching.insert(*hash, request_id);
        }
        self.requests.insert(request_id, ScheduledRequest { peer_id, hashes });
        self.requests_by_peer.entry(peer_id).or_default().push(request_id);

        Ok(Some((request_id, output)))
    }

    /// Returns peers that may have eligible hashes ready for assignment.
    pub(super) fn ready_peers(&self) -> impl Iterator<Item = &PeerId> {
        self.announcements.ready_peers()
    }

    /// Returns the number of hashes that currently reserve fetch/import capacity.
    pub(super) fn reserved_hashes(&self) -> usize {
        self.fetching.len()
    }

    /// Returns whether the hash currently reserves fetch/import capacity.
    pub(super) fn is_hash_reserved(&self, hash: &TxHash) -> bool {
        self.fetching.contains_key(hash)
    }

    /// Returns the total number of hashes tracked across both stages.
    pub(super) fn tracked_hashes(&self) -> usize {
        self.announced.len().saturating_add(self.fetching.len())
    }

    /// Returns the next request identifier.
    const fn next_request_id(&mut self) -> RequestId {
        let request_id = RequestId(self.request_seq);
        self.request_seq = self.request_seq.wrapping_add(1);
        request_id
    }
}

impl Default for RequestScheduler {
    fn default() -> Self {
        Self::new(TransactionFetcherConfig::default())
    }
}

/// Identifies one request assignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct RequestId(u64);

type PeerRequestIds =
    SmallVec<[RequestId; DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS_PER_PEER as usize]>;

/// Metadata retained for a successfully dispatched request.
#[derive(Debug)]
struct ScheduledRequest {
    peer_id: PeerId,
    hashes: Vec<TxHash>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{B256, U256};
    use std::convert::Infallible;

    fn peer(value: u8) -> PeerId {
        PeerId::new([value; 64])
    }

    fn hash(value: u8) -> TxHash {
        B256::repeat_byte(value)
    }

    fn announcement(
        entries: impl IntoIterator<Item = (TxHash, usize)>,
    ) -> Vec<(TxHash, Option<usize>)> {
        entries.into_iter().map(|(hash, size)| (hash, Some(size))).collect()
    }

    fn dispatch_hashes(hashes: &[TxHash]) -> Result<Vec<TxHash>, Infallible> {
        Ok(hashes.to_vec())
    }

    #[test]
    fn admission_limit_is_shared_across_announcements() {
        let config = TransactionFetcherConfig {
            max_capacity_cache_txns_pending_fetch: 3,
            ..Default::default()
        };
        let mut scheduler = RequestScheduler::new(config);
        let peer_id = peer(1);

        assert_eq!(
            scheduler.on_announcement(peer_id, announcement([(hash(1), 10), (hash(2), 10)])),
            0
        );
        assert_eq!(
            scheduler.on_announcement(peer_id, announcement([(hash(3), 10), (hash(4), 10)])),
            1
        );
        assert_eq!(scheduler.tracked_hashes(), 3);
        assert!(!scheduler.announced.contains_key(&hash(4)));
    }

    #[test]
    fn candidate_replacement_removes_evicted_peer_announcement() {
        let mut scheduler = RequestScheduler::default();
        let tx_hash = hash(1);
        for value in 1..=4 {
            scheduler.on_announcement(peer(value), announcement([(tx_hash, 10)]));
        }

        scheduler.on_announcement(peer(5), announcement([(tx_hash, 10)]));

        assert_eq!(scheduler.announced[&tx_hash].as_slice(), &[peer(2), peer(3), peer(4), peer(5)]);
        assert!(!scheduler.announcements.has_queued(&peer(1)));
        assert!(!scheduler.ready_peers().any(|peer_id| peer_id == &peer(1)));
    }

    #[test]
    fn serving_peer_is_protected_during_candidate_replacement() {
        let mut scheduler = RequestScheduler::default();
        let tx_hash = hash(1);
        for value in 1..=4 {
            scheduler.on_announcement(peer(value), announcement([(tx_hash, 10)]));
        }

        let (request_id, requested) =
            scheduler.try_schedule_for_peer(peer(1), usize::MAX, dispatch_hashes).unwrap().unwrap();
        assert_eq!(requested, vec![tx_hash]);
        assert_eq!(scheduler.requests[&request_id].peer_id, peer(1));
        assert_eq!(scheduler.requests[&request_id].hashes, [tx_hash]);

        scheduler.on_announcement(peer(5), announcement([(tx_hash, 10)]));

        assert_eq!(
            scheduler.alternates[&tx_hash].as_slice(),
            &[peer(1), peer(3), peer(4), peer(5)]
        );
        assert!(!scheduler.announcements.has_queued(&peer(2)));
    }

    #[test]
    fn fetch_capacity_bounds_and_reserves_hashes() {
        let peer_id = peer(1);
        let mut scheduler = RequestScheduler::default();
        scheduler.on_announcement(peer_id, announcement((1..=5).map(|value| (hash(value), 10))));

        let (_, requested) =
            scheduler.try_schedule_for_peer(peer_id, 2, dispatch_hashes).unwrap().unwrap();

        assert_eq!(requested, vec![hash(1), hash(2)]);
        assert_eq!(scheduler.reserved_hashes(), 2);
        assert!(requested.iter().all(|hash| scheduler.is_hash_reserved(hash)));
        assert_eq!(scheduler.announced.len(), 3);
    }

    #[test]
    fn repeated_scheduling_accounts_for_existing_reservations() {
        let config = TransactionFetcherConfig {
            max_inflight_requests: 3,
            max_inflight_requests_per_peer: 3,
            soft_limit_byte_size_pooled_transactions_response_on_pack_request: 100,
            ..Default::default()
        };
        let mut scheduler = RequestScheduler::new(config);
        let peer_id = peer(1);
        scheduler.on_announcement(peer_id, announcement((1..=3).map(|value| (hash(value), 80))));

        let (_, first) =
            scheduler.try_schedule_for_peer(peer_id, 2, dispatch_hashes).unwrap().unwrap();
        let (_, second) =
            scheduler.try_schedule_for_peer(peer_id, 2, dispatch_hashes).unwrap().unwrap();

        assert_eq!(first, vec![hash(1)]);
        assert_eq!(second, vec![hash(2)]);
        assert!(scheduler.try_schedule_for_peer(peer_id, 2, dispatch_hashes).unwrap().is_none());
        assert_eq!(scheduler.reserved_hashes(), 2);
        assert_eq!(scheduler.announced.len(), 1);
        assert_eq!(scheduler.requests_by_peer[&peer_id].len(), 2);
        assert!(scheduler.announcements.has_queued(&peer_id));
    }

    #[test]
    fn global_and_per_peer_request_limits_are_independent() {
        let config = TransactionFetcherConfig {
            max_inflight_requests: 2,
            max_inflight_requests_per_peer: 1,
            soft_limit_byte_size_pooled_transactions_response_on_pack_request: 10,
            ..Default::default()
        };
        let mut scheduler = RequestScheduler::new(config);
        let first_peer = peer(1);
        let second_peer = peer(2);
        let third_peer = peer(3);
        scheduler.on_announcement(first_peer, announcement([(hash(1), 10), (hash(2), 10)]));
        scheduler.on_announcement(second_peer, announcement([(hash(3), 10)]));
        scheduler.on_announcement(third_peer, announcement([(hash(4), 10)]));

        assert!(scheduler
            .try_schedule_for_peer(first_peer, usize::MAX, dispatch_hashes)
            .unwrap()
            .is_some());
        assert!(scheduler
            .try_schedule_for_peer(first_peer, usize::MAX, dispatch_hashes)
            .unwrap()
            .is_none());
        assert!(scheduler.announcements.has_queued(&first_peer));
        assert!(scheduler
            .try_schedule_for_peer(second_peer, usize::MAX, dispatch_hashes)
            .unwrap()
            .is_some());
        assert_eq!(scheduler.requests.len(), 2);
        assert!(scheduler
            .try_schedule_for_peer(third_peer, usize::MAX, dispatch_hashes)
            .unwrap()
            .is_none());
        assert!(scheduler.announcements.has_queued(&third_peer));
    }

    #[test]
    fn failed_dispatch_restores_fifo_without_reserving_capacity() {
        let peer_id = peer(1);
        let mut scheduler = RequestScheduler::default();
        scheduler.on_announcement(peer_id, announcement((1..=3).map(|value| (hash(value), 10))));

        assert_eq!(
            scheduler.try_schedule_for_peer(peer_id, 2, |_| Err::<(), _>("full")),
            Err("full")
        );
        assert_eq!(scheduler.reserved_hashes(), 0);
        assert_eq!(scheduler.requests.len(), 0);
        assert_eq!(scheduler.ready_peers().copied().collect::<Vec<_>>(), vec![peer_id]);
        assert!(scheduler.announcements.has_queued(&peer_id));

        let (_, requested) =
            scheduler.try_schedule_for_peer(peer_id, 2, dispatch_hashes).unwrap().unwrap();
        assert_eq!(requested, vec![hash(1), hash(2)]);
    }

    #[test]
    fn assigned_hashes_continue_to_count_toward_admission_capacity() {
        let config = TransactionFetcherConfig {
            max_capacity_cache_txns_pending_fetch: 1,
            ..Default::default()
        };
        let mut scheduler = RequestScheduler::new(config);
        let peer_id = peer(1);

        scheduler.on_announcement(peer_id, announcement([(hash(1), 10)]));
        scheduler.try_schedule_for_peer(peer_id, usize::MAX, dispatch_hashes).unwrap().unwrap();

        assert_eq!(scheduler.on_announcement(peer_id, announcement([(hash(2), 10)])), 1);
        assert_eq!(scheduler.tracked_hashes(), 1);
        assert!(scheduler.is_hash_reserved(&hash(1)));
        assert!(!scheduler.announced.contains_key(&hash(2)));
    }

    #[test]
    fn ready_peers_are_exposed_without_scanning_metadata() {
        let mut scheduler = RequestScheduler::default();
        let peer_id = peer(1);
        scheduler.on_announcement(peer_id, announcement([(hash(1), 10)]));

        assert_eq!(scheduler.ready_peers().copied().collect::<Vec<_>>(), vec![peer_id]);
    }

    #[test]
    fn shared_hash_is_assigned_only_once_across_peers() {
        let mut scheduler = RequestScheduler::default();
        let first_peer = peer(1);
        let second_peer = peer(2);
        let tx_hash = hash(1);
        scheduler.on_announcement(first_peer, announcement([(tx_hash, 10)]));
        scheduler.on_announcement(second_peer, announcement([(tx_hash, 10)]));

        let (_, requested) = scheduler
            .try_schedule_for_peer(first_peer, usize::MAX, dispatch_hashes)
            .unwrap()
            .unwrap();
        assert_eq!(requested, vec![tx_hash]);
        assert!(scheduler
            .try_schedule_for_peer(second_peer, usize::MAX, |_| -> Result<(), Infallible> {
                panic!("shared hash dispatched twice")
            })
            .unwrap()
            .is_none());
        assert_eq!(scheduler.reserved_hashes(), 1);
        assert_eq!(scheduler.requests.len(), 1);
    }

    #[test]
    fn zero_capacities_leave_the_queue_untouched() {
        for blocked_capacity in 0..3 {
            let mut config = TransactionFetcherConfig::default();
            let mut fetch_capacity = usize::MAX;
            match blocked_capacity {
                0 => fetch_capacity = 0,
                1 => config.max_inflight_requests = 0,
                2 => config.max_inflight_requests_per_peer = 0,
                _ => unreachable!(),
            }
            let defaults = TransactionFetcherConfig::default();
            let peer_id = peer(1);
            let mut scheduler = RequestScheduler::new(config);
            scheduler.on_announcement(peer_id, announcement([(hash(1), 10), (hash(2), 10)]));

            assert!(scheduler
                .try_schedule_for_peer(peer_id, fetch_capacity, dispatch_hashes)
                .unwrap()
                .is_none());
            assert!(scheduler.announcements.has_queued(&peer_id));
            assert_eq!(scheduler.ready_peers().copied().collect::<Vec<_>>(), vec![peer_id]);

            scheduler.config.max_inflight_requests = defaults.max_inflight_requests;
            scheduler.config.max_inflight_requests_per_peer =
                defaults.max_inflight_requests_per_peer;
            let (_, requested) = scheduler
                .try_schedule_for_peer(peer_id, usize::MAX, dispatch_hashes)
                .unwrap()
                .unwrap();
            assert_eq!(requested, vec![hash(1), hash(2)]);
        }
    }

    #[test]
    fn request_packing_honors_protocol_and_byte_limits() {
        let peer_id = peer(1);
        let config = TransactionFetcherConfig {
            soft_limit_byte_size_pooled_transactions_response_on_pack_request: usize::MAX,
            ..Default::default()
        };
        let mut scheduler = RequestScheduler::new(config);
        scheduler.on_announcement(
            peer_id,
            announcement((0..300).map(|value| (B256::from(U256::from(value)), 1))),
        );

        let (_, requested) =
            scheduler.try_schedule_for_peer(peer_id, usize::MAX, dispatch_hashes).unwrap().unwrap();
        assert_eq!(requested.len(), SOFT_LIMIT_COUNT_HASHES_IN_GET_POOLED_TRANSACTIONS_REQUEST);
        assert_eq!(scheduler.announced.len(), 44);

        let config = TransactionFetcherConfig {
            soft_limit_byte_size_pooled_transactions_response_on_pack_request: 100,
            ..Default::default()
        };
        let mut scheduler = RequestScheduler::new(config);
        scheduler
            .on_announcement(peer_id, announcement([(hash(1), 80), (hash(2), 30), (hash(3), 20)]));
        let (_, requested) =
            scheduler.try_schedule_for_peer(peer_id, usize::MAX, dispatch_hashes).unwrap().unwrap();
        assert_eq!(requested, vec![hash(1)]);
        assert_eq!(scheduler.announced.len(), 2);
    }
}
