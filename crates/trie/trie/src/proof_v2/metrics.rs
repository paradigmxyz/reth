//! Cursor counters owned by cached proof calculators and recorded on drop.

use crate::{
    hashed_cursor::{HashedCursorMetrics, HashedCursorMetricsCache, InstrumentedHashedCursor},
    trie_cursor::{InstrumentedTrieCursor, TrieCursorMetrics, TrieCursorMetricsCache},
    TrieType,
};
use std::ops::{Deref, DerefMut};

/// Metric handles shared by proof generators and their cached cursors.
#[derive(Clone, Debug)]
pub struct ProofV2Metrics {
    account_trie: TrieCursorMetrics,
    storage_trie: TrieCursorMetrics,
    account_hashed: HashedCursorMetrics,
    storage_hashed: HashedCursorMetrics,
}

impl Default for ProofV2Metrics {
    fn default() -> Self {
        Self {
            account_trie: TrieCursorMetrics::new(TrieType::State),
            storage_trie: TrieCursorMetrics::new(TrieType::Storage),
            account_hashed: HashedCursorMetrics::new(TrieType::State),
            storage_hashed: HashedCursorMetrics::new(TrieType::Storage),
        }
    }
}

pub(super) type TrieCursor<C> = InstrumentedTrieCursor<C, TrieMetrics>;
pub(super) type HashedCursor<C> = InstrumentedHashedCursor<C, HashedMetrics>;

pub(super) fn instrument_trie<C>(
    cursor: C,
    trie_type: TrieType,
    metrics: &ProofV2Metrics,
) -> TrieCursor<C> {
    let metrics = match trie_type {
        TrieType::State => &metrics.account_trie,
        TrieType::Storage | TrieType::Custom(_) => &metrics.storage_trie,
    };
    InstrumentedTrieCursor::new(
        cursor,
        TrieMetrics { cache: Default::default(), metrics: metrics.clone() },
    )
}

pub(super) fn instrument_hashed<C>(
    cursor: C,
    trie_type: TrieType,
    metrics: &ProofV2Metrics,
) -> HashedCursor<C> {
    let metrics = match trie_type {
        TrieType::State => &metrics.account_hashed,
        TrieType::Storage | TrieType::Custom(_) => &metrics.storage_hashed,
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
