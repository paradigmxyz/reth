//! Preserved sparse trie for reuse across payload validations.

use alloy_primitives::B256;
use reth_trie_sparse::SparseStateTrie;
use tracing::debug;

/// Type alias for the sparse trie type used in preservation.
pub type SparseTrie = SparseStateTrie;

/// Guard that holds the lock on the preserved trie.
/// While held, the next trie take will block. Call `store()` to save the trie before dropping.
#[derive(Debug)]
pub struct PreservedTrieGuard<'a>(parking_lot::MutexGuard<'a, Option<PreservedSparseTrie>>);

impl<'a> PreservedTrieGuard<'a> {
    /// Creates a new guard from the preserved trie lock.
    pub(crate) const fn new(
        guard: parking_lot::MutexGuard<'a, Option<PreservedSparseTrie>>,
    ) -> Self {
        PreservedTrieGuard(guard)
    }

    /// Stores a preserved trie for later reuse.
    pub fn store(&mut self, trie: PreservedSparseTrie) {
        self.0.replace(trie);
    }
}

/// A preserved sparse trie that can be reused across payload validations.
///
/// The trie exists in one of two states:
/// - **Anchored**: Has a computed state root and can be reused for payloads whose parent state root
///   matches the anchor.
/// - **Cleared**: Trie data has been cleared but allocations are preserved for reuse.
#[derive(Debug)]
pub enum PreservedSparseTrie {
    /// Trie with a computed state root that can be reused for continuation payloads.
    Anchored {
        /// The sparse state trie anchored to the computed state root.
        trie: SparseTrie,
        /// The state root this trie represents (computed from the previous block).
        /// Used to verify continuity: new payload's `parent_state_root` must match this.
        state_root: B256,
    },
    /// Cleared trie with preserved allocations, ready for fresh use.
    Cleared {
        /// The sparse state trie with cleared data but preserved allocations.
        trie: SparseTrie,
    },
}

impl PreservedSparseTrie {
    /// Creates a new anchored preserved trie.
    ///
    /// The `state_root` is the computed state root from the trie, which becomes the
    /// anchor for determining if subsequent payloads can reuse this trie.
    pub const fn anchored(trie: SparseTrie, state_root: B256) -> Self {
        Self::Anchored { trie, state_root }
    }

    /// Creates a cleared preserved trie (allocations preserved, data cleared).
    pub const fn cleared(trie: SparseTrie) -> Self {
        Self::Cleared { trie }
    }

    /// Consumes self and returns the trie for reuse.
    ///
    /// If the preserved trie is anchored and the parent state root matches, the preserved
    /// trie structure is reused directly. Otherwise, the trie is cleared but allocations
    /// are preserved to reduce memory overhead.
    pub fn into_trie_for(self, parent_state_root: B256) -> SparseTrie {
        match self {
            Self::Anchored { trie, state_root } if state_root == parent_state_root => {
                debug!(
                    target: "engine::tree::payload_processor",
                    %state_root,
                    "Reusing anchored sparse trie for continuation payload"
                );
                trie
            }
            Self::Anchored { mut trie, state_root } => {
                debug!(
                    target: "engine::tree::payload_processor",
                    anchor_root = %state_root,
                    %parent_state_root,
                    "Clearing anchored sparse trie - parent state root mismatch"
                );
                trie.clear();
                trie
            }
            Self::Cleared { trie } => {
                debug!(
                    target: "engine::tree::payload_processor",
                    %parent_state_root,
                    "Using cleared sparse trie with preserved allocations"
                );
                trie
            }
        }
    }
}
