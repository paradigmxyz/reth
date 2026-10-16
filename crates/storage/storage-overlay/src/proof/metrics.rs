//! Cursor counters owned by cached proof calculators and recorded on drop.

use reth_trie::{
    hashed_cursor::{HashedCursorMetrics, HashedCursorMetricsCache, InstrumentedHashedCursor},
    trie_cursor::{InstrumentedTrieCursor, TrieCursorMetrics, TrieCursorMetricsCache},
    TrieType,
};
use std::{
    ops::{Deref, DerefMut},
    sync::OnceLock,
};

pub(super) type TrieCursor<C> = InstrumentedTrieCursor<C, TrieMetrics>;
pub(super) type HashedCursor<C> = InstrumentedHashedCursor<C, HashedMetrics>;

pub(super) fn instrument_trie<C>(cursor: C, trie_type: TrieType) -> TrieCursor<C> {
    static STATE: OnceLock<TrieCursorMetrics> = OnceLock::new();
    static STORAGE: OnceLock<TrieCursorMetrics> = OnceLock::new();
    let metrics = match trie_type {
        TrieType::State => STATE.get_or_init(|| TrieCursorMetrics::new(TrieType::State)),
        TrieType::Storage | TrieType::Custom(_) => {
            STORAGE.get_or_init(|| TrieCursorMetrics::new(TrieType::Storage))
        }
    };
    InstrumentedTrieCursor::new(
        cursor,
        TrieMetrics { cache: Default::default(), metrics: metrics.clone() },
    )
}

pub(super) fn instrument_hashed<C>(cursor: C, trie_type: TrieType) -> HashedCursor<C> {
    static STATE: OnceLock<HashedCursorMetrics> = OnceLock::new();
    static STORAGE: OnceLock<HashedCursorMetrics> = OnceLock::new();
    let metrics = match trie_type {
        TrieType::State => STATE.get_or_init(|| HashedCursorMetrics::new(TrieType::State)),
        TrieType::Storage | TrieType::Custom(_) => {
            STORAGE.get_or_init(|| HashedCursorMetrics::new(TrieType::Storage))
        }
    };
    InstrumentedHashedCursor::new(
        cursor,
        HashedMetrics { cache: Default::default(), metrics: metrics.clone() },
    )
}

#[derive(Debug)]
pub(super) struct TrieMetrics {
    cache: TrieCursorMetricsCache,
    metrics: TrieCursorMetrics,
}

impl Deref for TrieMetrics {
    type Target = TrieCursorMetricsCache;
    fn deref(&self) -> &Self::Target {
        &self.cache
    }
}

impl DerefMut for TrieMetrics {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.cache
    }
}

impl Drop for TrieMetrics {
    fn drop(&mut self) {
        self.metrics.record(&mut self.cache);
    }
}

#[derive(Debug)]
pub(super) struct HashedMetrics {
    cache: HashedCursorMetricsCache,
    metrics: HashedCursorMetrics,
}

impl Deref for HashedMetrics {
    type Target = HashedCursorMetricsCache;
    fn deref(&self) -> &Self::Target {
        &self.cache
    }
}

impl DerefMut for HashedMetrics {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.cache
    }
}

impl Drop for HashedMetrics {
    fn drop(&mut self) {
        self.metrics.record(&mut self.cache);
    }
}
