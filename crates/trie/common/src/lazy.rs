//! Lazy initialization wrapper for trie data.
//!
//! Provides a no-std compatible [`LazyTrieData`] type for lazily initialized
//! trie-related data containing sorted hashed state and trie updates.

use crate::{updates::TrieUpdatesSorted, HashedPostStateSorted};
use alloc::sync::Arc;
use core::fmt;
use reth_primitives_traits::sync::OnceLock;

/// Container for sorted trie data: hashed state and trie updates.
///
/// This bundles both [`HashedPostStateSorted`] and [`TrieUpdatesSorted`] together
/// for convenient passing and storage.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SortedTrieData {
    /// Sorted hashed post-state produced by execution.
    pub hashed_state: Arc<HashedPostStateSorted>,
    /// Sorted trie updates produced by state root computation.
    pub trie_updates: Arc<TrieUpdatesSorted>,
}

impl SortedTrieData {
    /// Creates a new [`SortedTrieData`] with the given values.
    pub const fn new(
        hashed_state: Arc<HashedPostStateSorted>,
        trie_updates: Arc<TrieUpdatesSorted>,
    ) -> Self {
        Self { hashed_state, trie_updates }
    }
}

/// Lazily initialized trie data containing sorted hashed state and trie updates.
///
/// This is a no-std compatible wrapper that supports two modes:
/// 1. **Ready mode**: Data is available immediately (created via `ready()`)
/// 2. **Deferred mode**: Data is computed on first access (created via `deferred()`)
///
/// In deferred mode, the computation runs on the first call to `get()`, `hashed_state()`,
/// or `trie_updates()`, and results are cached for subsequent calls.
///
/// Cloning is cheap (Arc clone) and clones share the cached state.
pub struct LazyTrieData {
    /// Cached sorted trie data, computed on first access.
    data: Arc<OnceLock<SortedTrieData>>,
    /// Optional deferred computation function.
    compute: Option<Arc<dyn Fn() -> SortedTrieData + Send + Sync>>,
}

impl Clone for LazyTrieData {
    fn clone(&self) -> Self {
        Self { data: Arc::clone(&self.data), compute: self.compute.clone() }
    }
}

impl fmt::Debug for LazyTrieData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LazyTrieData")
            .field("data", &if self.data.get().is_some() { "initialized" } else { "pending" })
            .finish()
    }
}

impl PartialEq for LazyTrieData {
    fn eq(&self, other: &Self) -> bool {
        self.get() == other.get()
    }
}

impl Eq for LazyTrieData {}

impl LazyTrieData {
    /// Creates a new [`LazyTrieData`] that is already initialized with the given values.
    pub fn ready(
        hashed_state: Arc<HashedPostStateSorted>,
        trie_updates: Arc<TrieUpdatesSorted>,
    ) -> Self {
        let data = OnceLock::new();
        let _ = data.set(SortedTrieData::new(hashed_state, trie_updates));
        Self { data: Arc::new(data), compute: None }
    }

    /// Creates a new [`LazyTrieData`] from pre-computed [`SortedTrieData`].
    pub fn from_sorted(sorted: SortedTrieData) -> Self {
        let data = OnceLock::new();
        let _ = data.set(sorted);
        Self { data: Arc::new(data), compute: None }
    }

    /// Creates a new [`LazyTrieData`] with a deferred computation function.
    ///
    /// The computation will run on the first call to `get()`, `hashed_state()`,
    /// or `trie_updates()`. Results are cached for subsequent calls.
    pub fn deferred(compute: impl Fn() -> SortedTrieData + Send + Sync + 'static) -> Self {
        Self { data: Arc::new(OnceLock::new()), compute: Some(Arc::new(compute)) }
    }

    /// Returns a reference to the sorted trie data, computing if necessary.
    ///
    /// # Panics
    ///
    /// Panics if created via `deferred()` and the computation function was not provided.
    pub fn get(&self) -> &SortedTrieData {
        self.data.get_or_init(|| {
            self.compute.as_ref().expect("LazyTrieData::get called before initialization")()
        })
    }

    /// Returns a clone of the hashed state Arc.
    ///
    /// If not initialized, computes from the deferred source or panics.
    pub fn hashed_state(&self) -> Arc<HashedPostStateSorted> {
        Arc::clone(&self.get().hashed_state)
    }

    /// Returns a clone of the trie updates Arc.
    ///
    /// If not initialized, computes from the deferred source or panics.
    pub fn trie_updates(&self) -> Arc<TrieUpdatesSorted> {
        Arc::clone(&self.get().trie_updates)
    }

    /// Returns a clone of the [`SortedTrieData`].
    ///
    /// If not initialized, computes from the deferred source or panics.
    pub fn sorted_trie_data(&self) -> SortedTrieData {
        self.get().clone()
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for LazyTrieData {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.get().serialize(serializer)
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for LazyTrieData {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let data = SortedTrieData::deserialize(deserializer)?;
        Ok(Self::from_sorted(data))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lazy_ready_is_initialized() {
        let lazy = LazyTrieData::ready(
            Arc::new(HashedPostStateSorted::default()),
            Arc::new(TrieUpdatesSorted::default()),
        );
        let _ = lazy.hashed_state();
        let _ = lazy.trie_updates();
    }

    #[test]
    fn test_lazy_clone_shares_state() {
        let lazy1 = LazyTrieData::ready(
            Arc::new(HashedPostStateSorted::default()),
            Arc::new(TrieUpdatesSorted::default()),
        );
        let lazy2 = lazy1.clone();

        // Both point to the same data
        assert!(Arc::ptr_eq(&lazy1.hashed_state(), &lazy2.hashed_state()));
        assert!(Arc::ptr_eq(&lazy1.trie_updates(), &lazy2.trie_updates()));
    }

    #[test]
    fn test_lazy_deferred() {
        let lazy = LazyTrieData::deferred(SortedTrieData::default);
        assert!(lazy.hashed_state().is_empty());
        assert!(lazy.trie_updates().is_empty());
    }

    #[test]
    fn test_lazy_from_sorted() {
        let sorted = SortedTrieData::default();
        let lazy = LazyTrieData::from_sorted(sorted);
        assert!(lazy.hashed_state().is_empty());
        assert!(lazy.trie_updates().is_empty());
    }
}
