//! Announcement state for the second-generation transaction fetcher.

use super::super::constants::tx_fetcher::{
    AVERAGE_BYTE_SIZE_TX_ENCODED, DEFAULT_MAX_COUNT_FALLBACK_PEERS,
};
use alloy_primitives::{
    map::{B256Map, HashMap, HashSet},
    TxHash,
};
use reth_network_peers::PeerId;
use smallvec::SmallVec;
use std::collections::VecDeque;

/// Per-peer announcement metadata and FIFO order.
#[derive(Debug, Default)]
pub(super) struct PeerAnnouncementIndex {
    peers: HashMap<PeerId, PeerAnnouncements>,
    ready: HashSet<PeerId>,
}

impl PeerAnnouncementIndex {
    /// Inserts a hash at the back of a peer's FIFO.
    ///
    /// A later announcement can fill in an unknown size without changing the hash's position.
    pub(super) fn insert(&mut self, peer_id: PeerId, hash: TxHash, size: Option<usize>) {
        let peer = self.peers.entry(peer_id).or_default();
        if let Some(metadata) = peer.metadata.get_mut(&hash) {
            if metadata.size.is_none() {
                metadata.size = size;
            }
            return
        }

        let generation = peer.next_generation();
        peer.metadata.insert(hash, AnnouncementMetadata { size, queued: true, generation });
        peer.order.push_back((hash, generation));
        peer.queued_entries += 1;
        self.ready.insert(peer_id);
    }

    /// Packs eligible hashes in FIFO order, bounded by hash count and expected response size.
    ///
    /// The first hash is always accepted, even when it exceeds `size_limit`, so an oversized
    /// transaction cannot permanently block the queue.
    pub(super) fn pack<F>(
        &mut self,
        peer_id: PeerId,
        max_hashes: usize,
        size_limit: usize,
        mut is_eligible: F,
    ) -> Vec<TxHash>
    where
        F: FnMut(&TxHash) -> bool,
    {
        if max_hashes == 0 {
            return Vec::new()
        }

        let Some(peer) = self.peers.get_mut(&peer_id) else {
            self.ready.remove(&peer_id);
            return Vec::new()
        };
        let scan_len = peer.order.len();
        let mut hashes = Vec::with_capacity(max_hashes.min(scan_len));
        let mut expected_size = 0usize;
        let mut exhausted = true;

        for _ in 0..scan_len {
            if hashes.len() == max_hashes {
                exhausted = false;
                break
            }

            let Some((hash, generation)) = peer.order.pop_front() else { break };
            let Some(metadata) = peer.metadata.get_mut(&hash) else {
                peer.stale_entries = peer.stale_entries.saturating_sub(1);
                continue
            };
            if !metadata.queued || metadata.generation != generation {
                peer.stale_entries = peer.stale_entries.saturating_sub(1);
                continue
            }
            if !is_eligible(&hash) {
                peer.order.push_back((hash, generation));
                continue
            }

            let size = metadata.size.unwrap_or(AVERAGE_BYTE_SIZE_TX_ENCODED);
            if !hashes.is_empty() && expected_size.saturating_add(size) > size_limit {
                peer.order.push_front((hash, generation));
                exhausted = false;
                break
            }

            metadata.queued = false;
            peer.queued_entries -= 1;
            expected_size = expected_size.saturating_add(size);
            hashes.push(hash);
        }

        if exhausted {
            self.ready.remove(&peer_id);
        }

        hashes
    }

    /// Restores hashes at the front in their original order.
    pub(super) fn requeue_front(&mut self, peer_id: PeerId, hashes: &[TxHash]) {
        let Some(peer) = self.peers.get_mut(&peer_id) else { return };
        let mut restored = false;
        for hash in hashes.iter().rev() {
            restored |= peer.queue_front(*hash);
        }
        if restored {
            self.ready.insert(peer_id);
        }
    }

    /// Restores hashes at the back in their original order.
    pub(super) fn requeue_back(&mut self, peer_id: PeerId, hashes: &[TxHash]) {
        let Some(peer) = self.peers.get_mut(&peer_id) else { return };
        let mut restored = false;
        for hash in hashes {
            restored |= peer.queue_back(*hash);
        }
        if restored {
            self.ready.insert(peer_id);
        }
    }

    /// Removes a hash announced by a peer and drops empty peer state.
    pub(super) fn remove(&mut self, peer_id: PeerId, hash: &TxHash) -> bool {
        let Some(peer) = self.peers.get_mut(&peer_id) else { return false };
        let removed = peer.remove(hash);
        if peer.queued_entries == 0 {
            self.ready.remove(&peer_id);
        }
        if peer.metadata.is_empty() {
            self.peers.remove(&peer_id);
        }
        removed
    }

    /// Removes all announcement state for a peer and yields its hashes.
    pub(super) fn remove_peer(&mut self, peer_id: PeerId) -> impl Iterator<Item = TxHash> + use<> {
        self.ready.remove(&peer_id);
        self.peers.remove(&peer_id).into_iter().flat_map(|peer| peer.metadata.into_keys())
    }

    /// Returns whether a peer has queued work.
    pub(super) fn has_queued(&self, peer_id: &PeerId) -> bool {
        self.peers.get(peer_id).is_some_and(|peer| peer.queued_entries > 0)
    }

    /// Returns peers that may have eligible queued work.
    pub(super) fn ready_peers(&self) -> impl Iterator<Item = &PeerId> {
        self.ready.iter()
    }
}

/// Peers that can serve one hash, ordered from least to most recently announced.
#[derive(Debug, Default, Clone)]
pub(super) struct CandidatePeers {
    peers: SmallVec<[PeerId; MAX_CANDIDATE_PEERS_PER_HASH]>,
}

impl CandidatePeers {
    /// Tracks the peer as the most recent candidate.
    ///
    /// At capacity, the least-recent candidate is replaced unless it is `protected`.
    pub(super) fn track(&mut self, peer_id: PeerId, protected: Option<PeerId>) -> CandidateUpdate {
        if let Some(position) = self.peers.iter().position(|candidate| candidate == &peer_id) {
            self.peers.remove(position);
            self.peers.push(peer_id);
            return CandidateUpdate::Tracked
        }

        if self.peers.len() == MAX_CANDIDATE_PEERS_PER_HASH {
            let Some(position) =
                self.peers.iter().position(|candidate| Some(*candidate) != protected)
            else {
                return CandidateUpdate::Rejected
            };
            let replaced = self.peers.remove(position);
            self.peers.push(peer_id);
            return CandidateUpdate::Replaced(replaced)
        }

        self.peers.push(peer_id);
        CandidateUpdate::Tracked
    }

    /// Removes a peer from the candidate list.
    pub(super) fn remove(&mut self, peer_id: &PeerId) -> bool {
        let Some(position) = self.peers.iter().position(|candidate| candidate == peer_id) else {
            return false
        };
        self.peers.remove(position);
        true
    }

    /// Returns whether the candidate list is empty.
    pub(super) fn is_empty(&self) -> bool {
        self.peers.is_empty()
    }

    /// Returns candidates from least to most recently announced.
    pub(super) fn as_slice(&self) -> &[PeerId] {
        &self.peers
    }
}

/// A tracked candidate update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CandidateUpdate {
    /// The candidate was inserted or refreshed.
    Tracked,
    /// The candidate replaced an older peer.
    Replaced(PeerId),
    /// Every retained candidate was protected.
    Rejected,
}

/// The active serving peer plus the existing fallback-peer allowance.
const MAX_CANDIDATE_PEERS_PER_HASH: usize = 1 + DEFAULT_MAX_COUNT_FALLBACK_PEERS as usize;

#[derive(Debug, Clone)]
struct AnnouncementMetadata {
    size: Option<usize>,
    queued: bool,
    generation: u64,
}

#[derive(Debug, Default)]
struct PeerAnnouncements {
    order: VecDeque<(TxHash, u64)>,
    metadata: B256Map<AnnouncementMetadata>,
    next_generation: u64,
    stale_entries: usize,
    queued_entries: usize,
}

impl PeerAnnouncements {
    const fn next_generation(&mut self) -> u64 {
        let generation = self.next_generation;
        self.next_generation = self.next_generation.wrapping_add(1);
        generation
    }

    fn queue_front(&mut self, hash: TxHash) -> bool {
        let Some(metadata) = self.metadata.get(&hash) else { return false };
        if metadata.queued {
            return true
        }
        let generation = self.next_generation();
        let metadata = self.metadata.get_mut(&hash).expect("metadata checked above");
        metadata.queued = true;
        metadata.generation = generation;
        self.order.push_front((hash, generation));
        self.queued_entries += 1;
        true
    }

    fn queue_back(&mut self, hash: TxHash) -> bool {
        let Some(metadata) = self.metadata.get(&hash) else { return false };
        if metadata.queued {
            return true
        }
        let generation = self.next_generation();
        let metadata = self.metadata.get_mut(&hash).expect("metadata checked above");
        metadata.queued = true;
        metadata.generation = generation;
        self.order.push_back((hash, generation));
        self.queued_entries += 1;
        true
    }

    fn remove(&mut self, hash: &TxHash) -> bool {
        let Some(metadata) = self.metadata.remove(hash) else { return false };
        if metadata.queued {
            self.stale_entries += 1;
            self.queued_entries -= 1;
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
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;

    fn peer(value: u8) -> PeerId {
        PeerId::new([value; 64])
    }

    fn hash(value: u8) -> TxHash {
        B256::repeat_byte(value)
    }

    #[test]
    fn packs_fifo_by_prospective_size() {
        let peer_id = peer(1);
        let mut index = PeerAnnouncementIndex::default();
        index.insert(peer_id, hash(1), Some(80));
        index.insert(peer_id, hash(2), Some(30));
        index.insert(peer_id, hash(3), Some(20));

        assert_eq!(index.pack(peer_id, 3, 100, |_| true), vec![hash(1)]);
        assert_eq!(index.ready_peers().copied().collect::<Vec<_>>(), vec![peer_id]);
        assert_eq!(index.pack(peer_id, 3, 100, |_| true), vec![hash(2), hash(3)]);
        assert!(!index.has_queued(&peer_id));
        assert!(index.ready_peers().next().is_none());
    }

    #[test]
    fn first_oversized_hash_does_not_block_fifo() {
        let peer_id = peer(1);
        let mut index = PeerAnnouncementIndex::default();
        index.insert(peer_id, hash(1), Some(200));
        index.insert(peer_id, hash(2), Some(10));

        assert_eq!(index.pack(peer_id, 2, 100, |_| true), vec![hash(1)]);
        assert_eq!(index.pack(peer_id, 2, 100, |_| true), vec![hash(2)]);
    }

    #[test]
    fn eligibility_rotates_without_losing_fifo_entries() {
        let peer_id = peer(1);
        let mut index = PeerAnnouncementIndex::default();
        for value in 1..=3 {
            index.insert(peer_id, hash(value), Some(10));
        }

        assert_eq!(index.pack(peer_id, 1, 100, |hash| hash != &self::hash(1)), vec![hash(2)]);
        assert_eq!(index.pack(peer_id, 2, 100, |_| true), vec![hash(3), hash(1)]);
    }

    #[test]
    fn ready_index_is_restored_after_an_exhausted_scan() {
        let peer_id = peer(1);
        let tx_hash = hash(1);
        let mut index = PeerAnnouncementIndex::default();
        index.insert(peer_id, tx_hash, Some(10));

        assert!(index.pack(peer_id, 1, 100, |_| false).is_empty());
        assert!(index.has_queued(&peer_id));
        assert!(index.ready_peers().next().is_none());

        index.requeue_back(peer_id, &[tx_hash]);
        assert_eq!(index.ready_peers().copied().collect::<Vec<_>>(), vec![peer_id]);
        assert_eq!(index.pack(peer_id, 1, 100, |_| true), vec![tx_hash]);
    }

    #[test]
    fn requeues_at_front_or_back_in_original_order() {
        let peer_id = peer(1);
        let mut index = PeerAnnouncementIndex::default();
        for value in 1..=4 {
            index.insert(peer_id, hash(value), Some(10));
        }

        let first = index.pack(peer_id, 2, 100, |_| true);
        assert_eq!(first, vec![hash(1), hash(2)]);
        index.requeue_front(peer_id, &first);
        assert_eq!(index.pack(peer_id, 3, 100, |_| true), vec![hash(1), hash(2), hash(3)]);

        let second = index.pack(peer_id, 1, 100, |_| true);
        assert_eq!(second, vec![hash(4)]);
        index.requeue_back(peer_id, &second);
        assert_eq!(index.pack(peer_id, 1, 100, |_| true), vec![hash(4)]);
    }

    #[test]
    fn metadata_backfill_preserves_fifo_position() {
        let peer_id = peer(1);
        let mut index = PeerAnnouncementIndex::default();
        index.insert(peer_id, hash(1), None);
        index.insert(peer_id, hash(2), Some(100));
        index.insert(peer_id, hash(1), Some(500));

        assert_eq!(index.pack(peer_id, 2, 550, |_| true), vec![hash(1)]);
        assert_eq!(index.pack(peer_id, 2, 550, |_| true), vec![hash(2)]);
    }

    #[test]
    fn removals_compact_stale_entries_and_clean_empty_peers() {
        let peer_id = peer(1);
        let mut index = PeerAnnouncementIndex::default();
        for value in 1..=4 {
            index.insert(peer_id, hash(value), Some(10));
        }

        assert!(index.remove(peer_id, &hash(1)));
        assert!(index.remove(peer_id, &hash(2)));
        assert!(index.remove(peer_id, &hash(3)));
        assert_eq!(index.peers[&peer_id].order.len(), 1);
        assert_eq!(index.pack(peer_id, 1, 100, |_| true), vec![hash(4)]);
        assert!(index.remove(peer_id, &hash(4)));
        assert!(!index.peers.contains_key(&peer_id));
    }

    #[test]
    fn removing_peer_returns_all_tracked_hashes() {
        let peer_id = peer(1);
        let mut index = PeerAnnouncementIndex::default();
        index.insert(peer_id, hash(1), Some(10));
        index.insert(peer_id, hash(2), Some(20));

        let mut removed = index.remove_peer(peer_id).collect::<Vec<_>>();
        removed.sort_unstable();
        assert_eq!(removed, vec![hash(1), hash(2)]);
        assert!(!index.has_queued(&peer_id));
    }

    #[test]
    fn candidates_are_bounded_by_recency() {
        let mut candidates = CandidatePeers::default();
        for value in 1..=4 {
            assert_eq!(candidates.track(peer(value), None), CandidateUpdate::Tracked);
        }
        assert_eq!(candidates.track(peer(5), None), CandidateUpdate::Replaced(peer(1)));
        assert_eq!(candidates.as_slice(), &[peer(2), peer(3), peer(4), peer(5)]);

        assert_eq!(candidates.track(peer(2), None), CandidateUpdate::Tracked);
        assert_eq!(candidates.as_slice(), &[peer(3), peer(4), peer(5), peer(2)]);
    }

    #[test]
    fn candidate_replacement_preserves_protected_peer() {
        let mut candidates = CandidatePeers::default();
        for value in 1..=4 {
            candidates.track(peer(value), None);
        }

        assert_eq!(candidates.track(peer(5), Some(peer(1))), CandidateUpdate::Replaced(peer(2)));
        assert_eq!(candidates.as_slice(), &[peer(1), peer(3), peer(4), peer(5)]);
    }

    #[test]
    fn candidate_cleanup_removes_peer() {
        let mut candidates = CandidatePeers::default();
        candidates.track(peer(1), None);

        assert!(candidates.remove(&peer(1)));
        assert!(candidates.is_empty());
        assert!(!candidates.remove(&peer(1)));
    }
}
