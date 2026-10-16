//! Metric handles for reusable account and storage proof calculators.

use super::{HashedCursor, TrieCursor};
use crate::{hashed_cursor::HashedCursorMetrics, trie_cursor::TrieCursorMetrics, TrieType};

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

impl ProofV2Metrics {
    pub(super) fn instrument_trie<C>(&self, cursor: C, trie_type: TrieType) -> TrieCursor<C> {
        match trie_type {
            TrieType::State => &self.account_trie,
            TrieType::Storage | TrieType::Custom(_) => &self.storage_trie,
        }
        .instrument(cursor)
    }

    pub(super) fn instrument_hashed<C>(&self, cursor: C, trie_type: TrieType) -> HashedCursor<C> {
        match trie_type {
            TrieType::State => &self.account_hashed,
            TrieType::Storage | TrieType::Custom(_) => &self.storage_hashed,
        }
        .instrument(cursor)
    }
}
