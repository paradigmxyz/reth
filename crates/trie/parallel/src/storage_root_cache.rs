use alloy_primitives::{map::B256Map, B256};
use reth_primitives_traits::dashmap::DashMap;
use std::sync::Arc;

/// Number of past blocks whose storage roots stay available to the proof workers.
const GENERATIONS: usize = 8;

/// Storage roots the proof workers can use instead of walking an account's storage trie.
///
/// Roots a worker computes itself are only shared with the other workers of the same block. What
/// survives the block are the roots the state root task computed for the account leaves it
/// rewrote: those are post-state roots of the block that produced them, and they stay correct
/// until that account's storage changes again, at which point the block that changes it publishes
/// the new root in a newer generation. Reads take the newest generation holding the account, so a
/// republished root shadows every older copy of it.
///
/// A worker's own roots cannot be carried over. When the sparse trie is reused the proof providers
/// skip the in-memory overlay (see `with_skip_overlay_for_reused_sparse_trie`), so a worker reads
/// the durable state rather than the parent state; that is sound for the proof it is building,
/// because the sparse trie already holds every leaf the two states disagree on, but it is not a
/// root the next block may believe.
#[derive(Clone, Debug)]
pub struct StorageRootCache {
    /// Roots computed while validating the current block. Dropped with the block.
    live: Arc<DashMap<B256, B256>>,
    /// Roots published by the state root tasks of earlier blocks, newest first.
    generations: Arc<[Arc<B256Map<B256>>]>,
}

impl Default for StorageRootCache {
    fn default() -> Self {
        Self { live: Default::default(), generations: Arc::new([]) }
    }
}

impl StorageRootCache {
    /// Returns the cached storage root for the account, if any.
    pub fn get(&self, hashed_address: &B256) -> Option<CachedStorageRoot> {
        if let Some(root) = self.live.get(hashed_address) {
            return Some(CachedStorageRoot { root: *root, carried: false })
        }

        self.generations
            .iter()
            .find_map(|generation| generation.get(hashed_address))
            .map(|root| CachedStorageRoot { root: *root, carried: true })
    }

    /// Shares a storage root a worker computed with the other workers of this block.
    pub fn insert(&self, hashed_address: B256, root: B256) {
        self.live.insert(hashed_address, root);
    }

    /// Returns the cache the next block's proofs should use.
    ///
    /// `updated_storage_roots` must hold the post-state storage root of every account whose
    /// storage this block changed, so that no generation is left answering with a root this block
    /// moved.
    pub fn advance(&self, updated_storage_roots: B256Map<B256>) -> Self {
        let mut generations = Vec::with_capacity(GENERATIONS);
        generations.push(Arc::new(updated_storage_roots));
        generations.extend(self.generations.iter().take(GENERATIONS - 1).cloned());

        Self { live: Default::default(), generations: generations.into() }
    }

    /// Returns the number of carried entries, counting an account held by several generations
    /// once per generation.
    pub fn carried_len(&self) -> usize {
        self.generations.iter().map(|generation| generation.len()).sum()
    }
}

/// A storage root served by [`StorageRootCache`].
#[derive(Clone, Copy, Debug)]
pub struct CachedStorageRoot {
    /// The storage root.
    pub root: B256,
    /// Whether the root was published by an earlier block rather than computed for this one.
    pub carried: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address(byte: u8) -> B256 {
        B256::repeat_byte(byte)
    }

    fn root(byte: u8) -> B256 {
        B256::with_last_byte(byte)
    }

    #[test]
    fn only_the_published_roots_survive_the_block() {
        let cache = StorageRootCache::default();
        cache.insert(address(1), root(1));

        let hit = cache.get(&address(1)).unwrap();
        assert_eq!(hit.root, root(1));
        assert!(!hit.carried);

        let next = cache.advance(B256Map::from_iter([(address(2), root(2))]));
        assert!(
            next.get(&address(1)).is_none(),
            "a root a worker read from the durable state must not outlive its block"
        );
        assert!(next.get(&address(2)).unwrap().carried);
    }

    #[test]
    fn a_changed_root_shadows_every_older_copy() {
        let mut cache = StorageRootCache::default();

        // Carry the same account through several generations, then change it.
        for _ in 0..3 {
            cache = cache.advance(B256Map::from_iter([(address(1), root(1))]));
        }
        cache = cache.advance(B256Map::from_iter([(address(1), root(9))]));

        assert_eq!(cache.get(&address(1)).unwrap().root, root(9));
    }

    #[test]
    fn a_worker_finishing_after_advance_cannot_publish_into_the_next_block() {
        let cache = StorageRootCache::default();
        let next = cache.advance(B256Map::from_iter([(address(1), root(9))]));

        // A storage worker of the finished block still holds the old cache.
        cache.insert(address(1), root(1));

        assert_eq!(next.get(&address(1)).unwrap().root, root(9));
    }

    #[test]
    fn entries_age_out_after_the_generation_window() {
        let mut cache =
            StorageRootCache::default().advance(B256Map::from_iter([(address(1), root(1))]));

        for _ in 1..GENERATIONS {
            cache = cache.advance(B256Map::default());
            assert!(cache.get(&address(1)).is_some());
        }
        cache = cache.advance(B256Map::default());

        assert!(cache.get(&address(1)).is_none());
    }
}
