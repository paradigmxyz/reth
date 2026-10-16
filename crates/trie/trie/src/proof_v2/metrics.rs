//! Cursor metrics of multiproof calculators.

use crate::TrieType;

#[cfg(feature = "metrics")]
use crate::{
    hashed_cursor::{HashedCursorMetrics, HashedCursorMetricsRecorder, InstrumentedHashedCursor},
    trie_cursor::{InstrumentedTrieCursor, TrieCursorMetrics, TrieCursorMetricsRecorder},
};

/// Cursor metric handles shared by multiproof calculators.
#[cfg(feature = "metrics")]
#[derive(Clone, Debug)]
pub struct ProofCursorMetrics {
    account_trie_cursor: TrieCursorMetrics,
    storage_trie_cursor: TrieCursorMetrics,
    account_hashed_cursor: HashedCursorMetrics,
    storage_hashed_cursor: HashedCursorMetrics,
}

#[cfg(feature = "metrics")]
impl Default for ProofCursorMetrics {
    /// Registers cursor metric handles with the installed metrics recorder.
    fn default() -> Self {
        Self {
            account_trie_cursor: TrieCursorMetrics::new(TrieType::State),
            storage_trie_cursor: TrieCursorMetrics::new(TrieType::Storage),
            account_hashed_cursor: HashedCursorMetrics::new(TrieType::State),
            storage_hashed_cursor: HashedCursorMetrics::new(TrieType::Storage),
        }
    }
}

#[cfg(feature = "metrics")]
impl ProofCursorMetrics {
    /// Wraps a trie cursor so its operations are recorded into the handles for `trie_type`.
    pub(super) fn instrument_trie_cursor<C>(
        &self,
        cursor: C,
        trie_type: TrieType,
    ) -> MeteredTrieCursor<C> {
        match trie_type {
            TrieType::State => &self.account_trie_cursor,
            TrieType::Storage | TrieType::Custom(_) => &self.storage_trie_cursor,
        }
        .instrument(cursor)
    }

    /// Wraps a hashed cursor so its operations are recorded into the handles for `trie_type`.
    pub(super) fn instrument_hashed_cursor<C>(
        &self,
        cursor: C,
        trie_type: TrieType,
    ) -> MeteredHashedCursor<C> {
        match trie_type {
            TrieType::State => &self.account_hashed_cursor,
            TrieType::Storage | TrieType::Custom(_) => &self.storage_hashed_cursor,
        }
        .instrument(cursor)
    }
}

/// No-op cursor metric handles used when the `metrics` feature is disabled.
#[cfg(not(feature = "metrics"))]
#[derive(Clone, Debug, Default)]
pub struct ProofCursorMetrics;

#[cfg(not(feature = "metrics"))]
impl ProofCursorMetrics {
    /// Returns the trie cursor unchanged.
    pub(super) const fn instrument_trie_cursor<C>(
        &self,
        cursor: C,
        _trie_type: TrieType,
    ) -> MeteredTrieCursor<C> {
        cursor
    }

    /// Returns the hashed cursor unchanged.
    pub(super) const fn instrument_hashed_cursor<C>(
        &self,
        cursor: C,
        _trie_type: TrieType,
    ) -> MeteredHashedCursor<C> {
        cursor
    }
}

/// Trie cursor counted into [`ProofCursorMetrics`].
#[cfg(feature = "metrics")]
pub(super) type MeteredTrieCursor<C> = InstrumentedTrieCursor<C, TrieCursorMetricsRecorder>;

/// Trie cursor used as is when the `metrics` feature is disabled.
#[cfg(not(feature = "metrics"))]
pub(super) type MeteredTrieCursor<C> = C;

/// Hashed cursor counted into [`ProofCursorMetrics`].
#[cfg(feature = "metrics")]
pub(super) type MeteredHashedCursor<C> = InstrumentedHashedCursor<C, HashedCursorMetricsRecorder>;

/// Hashed cursor used as is when the `metrics` feature is disabled.
#[cfg(not(feature = "metrics"))]
pub(super) type MeteredHashedCursor<C> = C;
