//! Sharded storage of metric series keyed by [`Key`].

use metrics::Key;
use parking_lot::RwLock;
use std::{
    collections::HashMap,
    hash::{BuildHasherDefault, Hasher},
    sync::Arc,
};

/// Concurrent map from [`Key`] to a shared series entry.
///
/// Lookups of existing keys only take a shard read lock and never allocate. Shards are selected
/// by the hash [`Key`] computes on construction, so the map never rehashes key contents.
#[derive(Debug)]
pub(super) struct Registry<T> {
    shards: Box<[RwLock<Shard<T>>]>,
    shard_mask: usize,
}

impl<T> Registry<T> {
    /// Creates a registry with one shard per available core, rounded up to a power of two.
    pub(super) fn new() -> Self {
        let shard_count =
            std::thread::available_parallelism().map_or(1, |n| n.get()).next_power_of_two();
        Self {
            shards: (0..shard_count).map(|_| RwLock::default()).collect(),
            shard_mask: shard_count - 1,
        }
    }

    /// Returns the entry for `key`, inserting the result of `create` if it doesn't exist yet.
    pub(super) fn get_or_create(&self, key: &Key, create: impl FnOnce() -> T) -> Arc<T> {
        // The mask keeps the index in bounds because the shard count is a power of two.
        let shard = &self.shards[key.get_hash() as usize & self.shard_mask];
        if let Some(entry) = shard.read().get(key) {
            return entry.clone();
        }
        shard.write().entry(key.to_retained()).or_insert_with(|| Arc::new(create())).clone()
    }

    /// Returns all entries, holding each shard's read lock only while copying its handles.
    pub(super) fn entries(&self) -> Vec<Arc<T>> {
        let mut entries = Vec::new();
        for shard in &self.shards {
            entries.extend(shard.read().values().cloned());
        }
        entries
    }
}

type Shard<T> = HashMap<Key, Arc<T>, BuildHasherDefault<KeyHasher>>;

/// Pass-through hasher for [`Key`], whose `Hash` impl only writes its precomputed hash.
#[derive(Debug, Default)]
struct KeyHasher(u64);

impl Hasher for KeyHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        // `Key` never writes raw bytes, but stay correct for any other input.
        for byte in bytes {
            self.0 = self.0.rotate_left(8) ^ u64::from(*byte);
        }
    }

    fn write_u64(&mut self, i: u64) {
        self.0 = i;
    }
}
