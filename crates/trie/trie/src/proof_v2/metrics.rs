//! Cursor metrics of multiproof calculators.

use crate::{
    hashed_cursor::InstrumentedHashedCursorFactory, trie_cursor::InstrumentedTrieCursorFactory,
};

#[cfg(feature = "metrics")]
use crate::{hashed_cursor::HashedCursorMetrics, trie_cursor::TrieCursorMetrics, TrieType};

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
    /// Instruments all account and storage cursors opened by `factory`.
    pub fn instrument_trie_factory<F>(&self, factory: F) -> InstrumentedTrieCursorFactory<F> {
        InstrumentedTrieCursorFactory::new(
            factory,
            self.account_trie_cursor.clone(),
            self.storage_trie_cursor.clone(),
        )
    }

    /// Instruments all hashed account and storage cursors opened by `factory`.
    pub fn instrument_hashed_factory<F>(&self, factory: F) -> InstrumentedHashedCursorFactory<F> {
        InstrumentedHashedCursorFactory::new(
            factory,
            self.account_hashed_cursor.clone(),
            self.storage_hashed_cursor.clone(),
        )
    }
}

/// No-op cursor metric handles used when the `metrics` feature is disabled.
#[cfg(not(feature = "metrics"))]
#[derive(Clone, Debug, Default)]
pub struct ProofCursorMetrics;

#[cfg(not(feature = "metrics"))]
impl ProofCursorMetrics {
    /// Returns the factory unchanged when cursor metrics are disabled.
    pub const fn instrument_trie_factory<F>(&self, factory: F) -> InstrumentedTrieCursorFactory<F> {
        factory
    }

    /// Returns the factory unchanged when cursor metrics are disabled.
    pub const fn instrument_hashed_factory<F>(
        &self,
        factory: F,
    ) -> InstrumentedHashedCursorFactory<F> {
        factory
    }
}
