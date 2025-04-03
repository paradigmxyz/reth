use std::{fmt::Debug, time::Instant};

use alloy_primitives::B256;
use alloy_trie::Nibbles;

/// The key visit.
#[derive(Clone)]
pub struct KeyVisit<T> {
    /// The type of key visit.
    pub visit_type: KeyVisitType<T>,
    /// The key that was visited, if found.
    pub visited_key: Option<T>,
    /// The time at which the key was visited.
    pub at: Instant,
}

impl<T> KeyVisit<T> {
    /// Creates a new [`KeyVisit`].
    pub fn new(visit_type: KeyVisitType<T>, visited_key: Option<T>) -> Self {
        Self { visit_type, visited_key, at: Instant::now() }
    }

    /// Creates a new [`KeyVisit`] with [`KeyVisitType::SeekExact`].
    pub fn seek_exact(key: T, visited_key: Option<T>) -> Self {
        Self::new(KeyVisitType::SeekExact(key), visited_key)
    }

    /// Creates a new [`KeyVisit`] with [`KeyVisitType::SeekNonExact`].
    pub fn seek_non_exact(key: T, visited_key: Option<T>) -> Self {
        Self::new(KeyVisitType::SeekNonExact(key), visited_key)
    }

    /// Creates a new [`KeyVisit`] with [`KeyVisitType::Next`].
    pub fn next(key: Option<T>) -> Self {
        Self::new(KeyVisitType::Next, key)
    }
}

impl<T: Debug> Debug for KeyVisit<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyVisit")
            .field("visit_type", &self.visit_type)
            .field("visited_key", &self.visited_key)
            .finish()
    }
}

impl<T: PartialEq> PartialEq for KeyVisit<T> {
    fn eq(&self, other: &Self) -> bool {
        self.visit_type == other.visit_type && self.visited_key == other.visited_key
    }
}

impl<T: Eq> Eq for KeyVisit<T> {}

/// The type of key visit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyVisitType<T> {
    /// Seeked exact key.
    SeekExact(T),
    /// Seeked non-exact key, returning the next key if no exact match found.
    SeekNonExact(T),
    /// Next key.
    Next,
}

/// The cursor type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorType {
    /// The trie cursor type.
    Trie(KeyVisit<Nibbles>),
    /// The hashed cursor type.
    Hashed(KeyVisit<B256>),
}

impl CursorType {
    fn at(&self) -> Instant {
        match self {
            Self::Trie(visit) => visit.at,
            Self::Hashed(visit) => visit.at,
        }
    }
}

impl PartialOrd for CursorType {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for CursorType {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.at().cmp(&other.at())
    }
}
