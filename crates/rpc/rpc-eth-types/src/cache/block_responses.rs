//! Serialized responses of recent blocks.

use alloy_primitives::B256;
use parking_lot::Mutex;
use serde_json::value::RawValue;
use std::{collections::VecDeque, sync::Arc};

/// The serialized block and receipt responses of recent blocks.
///
/// A response is kept per block hash, so it never changes. The blocks of a reorg are removed by
/// [`cache_new_blocks_task`](super::cache_new_blocks_task), and a response is only served while its
/// block is recent, which bounds how long a reorged block that was missed there is still served.
#[derive(Debug)]
pub struct BlockResponseCache {
    /// The most recently cached responses, oldest first.
    entries: Mutex<VecDeque<Entry>>,
    /// Maximum number of responses, zero if the cache is disabled.
    max_len: usize,
    /// How far below the best block a block may be for its responses to be cached.
    depth: u64,
}

impl BlockResponseCache {
    /// Creates a cache for up to `max_len` responses of blocks at most `depth` below the best
    /// block.
    pub(crate) fn new(max_len: u32, depth: u64) -> Self {
        Self { entries: Mutex::default(), max_len: max_len as usize, depth }
    }

    /// Returns `false` if the cache keeps no responses.
    pub const fn is_enabled(&self) -> bool {
        self.max_len > 0
    }

    /// Whether block `number` is close enough to the `best` block for its responses to be cached.
    ///
    /// Subtracts from `best` instead of adding to `number`, which may come from a request and be
    /// as high as `u64::MAX`.
    pub const fn is_recent(&self, number: u64, best: u64) -> bool {
        number >= best.saturating_sub(self.depth)
    }

    /// Returns the cached response of the block, or `None` if there is none or the block is no
    /// longer recent with `best` as the best block number.
    pub fn get(&self, hash: B256, kind: BlockResponseKind, best: u64) -> Option<Arc<RawValue>> {
        self.entries
            .lock()
            .iter()
            .find(|entry| {
                entry.hash == hash && entry.kind == kind && self.is_recent(entry.number, best)
            })
            .map(|entry| entry.response.clone())
    }

    /// Caches a response of the block with the given hash and number, in place of the oldest
    /// response if the cache is full.
    ///
    /// Keeps the cached response if there already is one of this kind for the block.
    pub fn insert(
        &self,
        hash: B256,
        number: u64,
        kind: BlockResponseKind,
        response: Arc<RawValue>,
    ) {
        if !self.is_enabled() {
            return
        }
        let mut entries = self.entries.lock();
        // Requests that missed at the same time computed the same response; keep the first.
        if entries.iter().any(|entry| entry.hash == hash && entry.kind == kind) {
            return
        }
        if entries.len() == self.max_len {
            entries.pop_front();
        }
        entries.push_back(Entry { hash, number, kind, response });
    }

    /// Removes all responses of the block.
    pub(crate) fn remove_block(&self, hash: B256) {
        self.entries.lock().retain(|entry| entry.hash != hash);
    }
}

/// Which response of a block is cached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockResponseKind {
    /// The block with full transaction objects.
    FullBlock,
    /// The block with transaction hashes only.
    HashesBlock,
    /// All receipts of the block.
    Receipts,
}

impl BlockResponseKind {
    /// Returns the kind of a block response with full transaction objects or with hashes only.
    pub const fn block(full: bool) -> Self {
        if full {
            Self::FullBlock
        } else {
            Self::HashesBlock
        }
    }
}

/// A cached response and the block it belongs to.
#[derive(Debug)]
struct Entry {
    hash: B256,
    number: u64,
    kind: BlockResponseKind,
    response: Arc<RawValue>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const RECEIPTS: BlockResponseKind = BlockResponseKind::Receipts;

    fn insert(cache: &BlockResponseCache, block: u8, kind: BlockResponseKind) {
        let response = RawValue::from_string("null".to_owned()).unwrap();
        cache.insert(B256::with_last_byte(block), 10, kind, response.into());
    }

    fn is_cached(cache: &BlockResponseCache, block: u8, kind: BlockResponseKind) -> bool {
        cache.get(B256::with_last_byte(block), kind, 10).is_some()
    }

    #[test]
    fn serves_recent_blocks_only() {
        let cache = BlockResponseCache::new(4, 2);
        insert(&cache, 1, RECEIPTS);

        let hash = B256::with_last_byte(1);
        assert!(cache.get(hash, RECEIPTS, 12).is_some());
        assert!(cache.get(hash, RECEIPTS, 13).is_none());
        assert!(cache.get(hash, BlockResponseKind::FullBlock, 12).is_none());
    }

    #[test]
    fn replaces_oldest_response_when_full() {
        let cache = BlockResponseCache::new(2, 2);
        for block in 1..=3 {
            insert(&cache, block, RECEIPTS);
        }

        assert!(!is_cached(&cache, 1, RECEIPTS));
        assert!(is_cached(&cache, 2, RECEIPTS));
        assert!(is_cached(&cache, 3, RECEIPTS));
    }

    #[test]
    fn keeps_nothing_when_disabled() {
        let cache = BlockResponseCache::new(0, 2);
        insert(&cache, 1, RECEIPTS);

        assert!(!cache.is_enabled());
        assert!(!is_cached(&cache, 1, RECEIPTS));
    }

    #[test]
    fn removes_all_responses_of_a_block() {
        let cache = BlockResponseCache::new(4, 2);
        insert(&cache, 1, RECEIPTS);
        insert(&cache, 1, BlockResponseKind::FullBlock);
        insert(&cache, 2, RECEIPTS);

        cache.remove_block(B256::with_last_byte(1));

        assert!(!is_cached(&cache, 1, RECEIPTS));
        assert!(!is_cached(&cache, 1, BlockResponseKind::FullBlock));
        assert!(is_cached(&cache, 2, RECEIPTS));
    }
}
