use alloy_primitives::{map::B256Map, B256};
use reth_primitives_traits::dashmap::DashMap;
use std::sync::Arc;

/// Number of past blocks whose computed storage roots stay available to the proof workers.
const GENERATIONS: usize = 8;

/// Storage roots of accounts that the proof workers did not have to walk for themselves.
///
/// A proof is computed against the parent state of the block being validated, so a cached root is
/// the root of that account's storage trie in the parent state. It stays correct for every later
/// block until the account's storage changes, which is why [`Self::advance`] takes the roots the
/// state root task just computed for the accounts this block wrote to: after that the whole cache
/// describes the block's post state, which is the next block's parent state.
///
/// Roots computed while validating the current block go into `live`. [`Self::advance`] copies
/// `live` into a new generation rather than handing the map itself over, so a worker that outlives
/// its block cannot publish a root of an already-superseded state.
#[derive(Clone, Debug)]
pub struct StorageRootCache {
    /// Roots computed while validating the current block.
    live: Arc<DashMap<B256, B256>>,
    /// Roots computed for earlier blocks, newest first.
    ///
    /// Reads take the first match, so a root re-published by [`Self::advance`] shadows every
    /// older copy of it and stale entries never have to be hunted down in older generations.
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

    /// Records a storage root computed against the current block's parent state.
    pub fn insert(&self, hashed_address: B256, root: B256) {
        self.live.insert(hashed_address, root);
    }

    /// Returns the cache the next block's proofs should use.
    ///
    /// `updated_storage_roots` must hold the post-state storage root of every account whose
    /// storage this block changed; those roots replace what the workers cached for the parent
    /// state. Any account missing from it keeps the root it was cached with, which is only
    /// correct if the block left its storage alone.
    pub fn advance(&self, updated_storage_roots: &B256Map<B256>) -> Self {
        let mut generation = B256Map::with_capacity_and_hasher(
            self.live.len() + updated_storage_roots.len(),
            Default::default(),
        );
        for entry in self.live.iter() {
            generation.insert(*entry.key(), *entry.value());
        }
        generation.extend(updated_storage_roots.iter().map(|(address, root)| (*address, *root)));

        let mut generations = Vec::with_capacity(GENERATIONS);
        generations.push(Arc::new(generation));
        generations.extend(self.generations.iter().take(GENERATIONS - 1).cloned());

        Self { live: Default::default(), generations: generations.into() }
    }

    /// Returns the number of carried entries, counting an account cached in several generations
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
    /// Whether the root was computed while validating an earlier block.
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
    fn live_roots_become_carried_on_advance() {
        let cache = StorageRootCache::default();
        cache.insert(address(1), root(1));

        let hit = cache.get(&address(1)).unwrap();
        assert_eq!(hit.root, root(1));
        assert!(!hit.carried);

        let next = cache.advance(&B256Map::default());
        let hit = next.get(&address(1)).unwrap();
        assert_eq!(hit.root, root(1));
        assert!(hit.carried);
        assert!(cache.get(&address(2)).is_none());
    }

    #[test]
    fn a_changed_root_shadows_every_older_copy() {
        let mut cache = StorageRootCache::default();
        cache.insert(address(1), root(1));

        // Carry the same account through enough generations that several of them hold the stale
        // root, then change it.
        for _ in 0..3 {
            cache = cache.advance(&B256Map::default());
            cache.insert(address(1), root(1));
        }
        cache = cache.advance(&B256Map::from_iter([(address(1), root(9))]));

        assert_eq!(cache.get(&address(1)).unwrap().root, root(9));
    }

    #[test]
    fn a_worker_finishing_after_advance_cannot_publish_into_the_next_block() {
        let cache = StorageRootCache::default();
        let next = cache.advance(&B256Map::from_iter([(address(1), root(9))]));

        // A storage worker of the finished block still holds the old cache.
        cache.insert(address(1), root(1));

        assert_eq!(next.get(&address(1)).unwrap().root, root(9));
    }

    #[test]
    fn entries_age_out_after_the_generation_window() {
        let mut cache = StorageRootCache::default();
        cache.insert(address(1), root(1));

        for _ in 0..GENERATIONS {
            cache = cache.advance(&B256Map::default());
            assert!(cache.get(&address(1)).is_some());
        }
        cache = cache.advance(&B256Map::default());

        assert!(cache.get(&address(1)).is_none());
    }
}
