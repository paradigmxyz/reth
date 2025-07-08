use crate::SparseSubtrie;
use reth_trie_common::Nibbles;

/// Tracks the state of the lower subtries.
///
/// When a [`crate::ParallelSparseTrie`] is initialized/cleared then its `LowerSparseSubtrie`s are
/// all blinded, meaning they have no nodes. A blinded `LowerSparseSubtrie` may hold onto a cleared
/// [`SparseSubtrie`] in order to re-use allocations.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum LowerSparseSubtrie {
    Blind(Option<Box<SparseSubtrie>>),
    Revealed(Box<SparseSubtrie>),
}

impl Default for LowerSparseSubtrie {
    /// Creates a new blinded subtrie with no allocated storage.
    fn default() -> Self {
        Self::Blind(None)
    }
}

impl LowerSparseSubtrie {
    /// Returns a reference to the underlying [`SparseSubtrie`] if this subtrie is revealed.
    ///
    /// Returns `None` if the subtrie is blinded (has no nodes).
    pub(crate) fn as_revealed_ref(&self) -> Option<&SparseSubtrie> {
        match self {
            Self::Blind(_) => None,
            Self::Revealed(subtrie) => Some(subtrie.as_ref()),
        }
    }

    /// Returns a mutable reference to the underlying [`SparseSubtrie`] if this subtrie is revealed.
    ///
    /// Returns `None` if the subtrie is blinded (has no nodes).
    pub(crate) fn as_revealed_mut(&mut self) -> Option<&mut SparseSubtrie> {
        match self {
            Self::Blind(_) => None,
            Self::Revealed(subtrie) => Some(subtrie.as_mut()),
        }
    }

    /// Reveals the lower [`SparseSubtrie`], transitioning it from the Blinded to the Revealed
    /// variant, preserving allocations if possible.
    ///
    /// The given path is the path of a node which will be set into the [`SparseSubtrie`]'s `nodes`
    /// map immediately upon being revealed. If the subtrie is blinded, or if its current root path
    /// is longer than this one, than this one becomes the new root path of the subtrie.
    pub(crate) fn reveal(&mut self, path: &Nibbles) {
        match self {
            Self::Blind(allocated) => {
                debug_assert!(allocated.as_ref().is_none_or(|subtrie| subtrie.is_empty()));
                *self = if let Some(mut subtrie) = allocated.take() {
                    subtrie.path = *path;
                    Self::Revealed(subtrie)
                } else {
                    Self::Revealed(Box::new(SparseSubtrie::new(*path)))
                }
            }
            Self::Revealed(subtrie) => {
                if path.len() < subtrie.path.len() {
                    subtrie.path = *path;
                }
            }
        };
    }

    /// Clears the subtrie and transitions it to the blinded state, preserving a cleared
    /// [`SparseSubtrie`] if possible.
    pub(crate) fn clear(&mut self) {
        *self = match core::mem::take(self) {
            Self::Blind(allocated) => {
                debug_assert!(allocated.as_ref().is_none_or(|subtrie| subtrie.is_empty()));
                Self::Blind(allocated)
            }
            Self::Revealed(mut subtrie) => {
                subtrie.clear();
                Self::Blind(Some(subtrie))
            }
        }
    }

    /// Takes ownership of the underlying [`SparseSubtrie`] if revealed and the predicate returns
    /// true.
    ///
    /// If the subtrie is revealed, and the predicate function returns `true` when called with it,
    /// then this method will take ownership of the subtrie and transition this `LowerSparseSubtrie`
    /// to the blinded state. Otherwise, returns `None`.
    pub(crate) fn take_revealed_if<P>(&mut self, predicate: P) -> Option<Box<SparseSubtrie>>
    where
        P: FnOnce(&SparseSubtrie) -> bool,
    {
        match self {
            Self::Revealed(subtrie) if predicate(subtrie) => {
                let Self::Revealed(subtrie) = core::mem::take(self) else { unreachable!() };
                Some(subtrie)
            }
            Self::Revealed(_) | Self::Blind(_) => None,
        }
    }
}
