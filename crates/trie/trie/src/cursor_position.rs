//! Logical positions shared by trie and hashed overlay cursors.

use std::{fmt::Debug, ops::Bound};

/// The latest logical result and the lower bound of requests that can reuse the cursor positions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CursorPosition<K> {
    /// No seek has been made since construction or reset.
    Unseeked,
    /// A seek or advance returned an entry, possibly beyond the requested lower bound.
    Positioned { key: K, lower_bound: Bound<K> },
    /// A seek missed or iteration ended, but the scanned range remains reusable.
    NoCurrent { lower_bound: Bound<K> },
}

impl<K: Copy + Ord + Debug> CursorPosition<K> {
    /// Returns the key of the latest logical result, if any.
    pub(crate) const fn key(&self) -> Option<K> {
        match self {
            Self::Positioned { key, .. } => Some(*key),
            Self::Unseeked | Self::NoCurrent { .. } => None,
        }
    }

    /// Returns the reusable request bound, or unbounded before the first seek.
    const fn lower_bound(&self) -> Bound<K> {
        match self {
            Self::Unseeked => Bound::Unbounded,
            Self::Positioned { lower_bound, .. } | Self::NoCurrent { lower_bound } => *lower_bound,
        }
    }

    /// Begins a seek or advance without discarding the previous result's ordering check.
    pub(crate) const fn set_lower_bound(&mut self, lower_bound: Bound<K>) {
        *self = match self.key() {
            Some(key) => Self::Positioned { key, lower_bound },
            None => Self::NoCurrent { lower_bound },
        };
    }

    /// Records the result of a seek or advance while retaining its reusable request bound.
    pub(crate) fn set_key(&mut self, key: Option<K>) {
        debug_assert!(
            self.key().is_none_or(|last| key.is_none_or(|next| next >= last)),
            "Cannot return entry {:?} previous to the last returned entry at {:?}",
            key,
            self.key(),
        );
        let lower_bound = self.lower_bound();
        *self = match key {
            Some(key) => Self::Positioned { key, lower_bound },
            None => Self::NoCurrent { lower_bound },
        };
    }

    /// Whether a request precedes the range covered by the current cursor positions.
    pub(crate) fn must_rewind(&self, key: K) -> bool {
        match self.lower_bound() {
            Bound::Included(lower) => key < lower,
            Bound::Excluded(lower) => key <= lower,
            Bound::Unbounded => false,
        }
    }
}
